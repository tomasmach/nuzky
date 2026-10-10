mod qa_support;
use nuzky_engine::{
    Renderer, Wait,
    export::{ExportOptions, export},
    media::{Transfer, VideoDecoder},
    proxy,
};
use qa_support::*;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
};

/// Size the proxy and the original are compared at, in stored orientation.
const SMALL: (u32, u32) = (480, 270);

/// A phone clip: HEVC 10 bit HLG, 2560x1440 stored and shown portrait, at variable frame rate (jittered
/// 30 fps, then 12 fps), with B-frames, and with the picture starting 101.667 ms after the sound.
fn phone_clip(d: &Path) -> Option<PathBuf> {
    let encoders = run(Command::new("ffmpeg").args(["-hide_banner", "-encoders"]));
    if !String::from_utf8_lossy(&encoders.stdout).contains("libx265") {
        eprintln!("SKIP: ffmpeg without libx265");
        return None;
    }
    let stored = d.join("stored.mov");
    let times = "setpts='(61+if(lt(N,24),N*20+eq(mod(N,4),1)*7,480+(N-24)*50))/600/TB'";
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=2560x1440:r=30:d=1.6",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:r=48000:d=3",
            "-vf",
            &format!("settb=1/600,{times},scale=out_color_matrix=bt2020nc:out_range=tv,format=yuv420p10le"),
            "-fps_mode",
            "vfr",
            "-enc_time_base",
            "1/600",
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-x265-params",
            "pools=2:frame-threads=1:log-level=error:keyint=30:bframes=3:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc:range=limited",
            "-tag:v",
            "hvc1",
            "-video_track_timescale",
            "600",
            "-c:a",
            "aac",
        ],
        &stored,
    );
    let path = d.join("phone.mov");
    ff(
        &["-display_rotation", "-90", "-i", stored.to_str().unwrap(), "-c", "copy", "-video_track_timescale", "600"],
        &path,
    );
    Some(path)
}

fn frame_times(mut decoder: VideoDecoder) -> Vec<i64> {
    std::iter::from_fn(|| decoder.next_frame().unwrap().map(|(t, _)| t)).collect()
}

fn render(project: &nuzky_engine::Project, renderer: &mut Renderer, t: i64) -> Vec<u8> {
    renderer.render(project, t, 270, 480, Wait::Exact, false).unwrap()
}

/// Every frame of an export, as RGBA.
fn frames(path: &Path) -> Vec<u8> {
    raw(path, &[])
}

#[test]
fn proxy_keeps_the_frame_times_colours_and_orientation_and_export_never_reads_it() {
    if !available() {
        return;
    }
    let d = dir("proxy");
    let Some(clip) = phone_clip(&d) else { return };
    let project = project(&clip, 2_700_000);
    // The app knows files by the absolute path the import stored, and so are their proxies.
    let source = PathBuf::from(&project.assets[0].path);
    let cache = d.join("cache");
    std::fs::remove_dir_all(&cache).ok();
    assert!(proxy::wanted(&source).unwrap(), "HEVC from a phone previews from a proxy");
    let small = d.join("small.mp4");
    ff(&["-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=0.2", "-c:v", "libx264", "-threads", "2"], &small);
    assert!(!proxy::wanted(&small).unwrap(), "small H.264 decodes fast enough");

    // A stopped job leaves nothing behind.
    assert!(proxy::ensure_proxy(&cache, &source, |_| anyhow::bail!("CANCELLED: stop")).is_err());
    let left: Vec<_> = std::fs::read_dir(cache.join("proxy")).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(left.iter().all(|name| name.to_string_lossy().ends_with(".lock")), "{left:?}");

    let made = proxy::ensure_proxy(&cache, &source, |_| Ok(())).unwrap();
    assert_eq!(proxy::ready(&cache, &source), Some(made.clone()));
    let stream = &info(&made)["streams"][0];
    assert_eq!(
        (stream["codec_name"].as_str(), stream["width"].as_u64(), stream["height"].as_u64()),
        (Some("h264"), Some(1920), Some(1080))
    );
    assert_eq!(stream["has_b_frames"].as_u64(), Some(0));

    // The proxy has the original's frames at the original's times, as FFmpeg reads them.
    let reference = pts(&source);
    assert_eq!(reference.len(), 48);
    assert_eq!(frame_times(VideoDecoder::open(&source).unwrap()), reference);
    assert_eq!(frame_times(VideoDecoder::open_proxy(&made).unwrap()), reference, "proxy frame times");

    // Its pictures match FFmpeg's of the original, HLG and its BT.2020 matrix included.
    let (w, h) = SMALL;
    let truth = run(Command::new("ffmpeg")
        .args(["-v", "error", "-noautorotate", "-i"])
        .arg(&source)
        .args(["-vf", &format!("scale={w}:{h}:flags=bilinear:in_color_matrix=bt2020:in_range=limited,format=rgba")])
        .args(["-f", "rawvideo", "-fps_mode", "passthrough", "-"]))
    .stdout;
    let size = (w * h * 4) as usize;
    assert_eq!(truth.len(), size * reference.len());
    let mut decoder = VideoDecoder::open_proxy(&made).unwrap();
    let mut worst = 0f64;
    for i in 0..reference.len() {
        let (t, f) = decoder.next_frame().unwrap().unwrap();
        let rgba = decoder.convert(&f, t, w, h).unwrap();
        assert_eq!(rgba.transfer, Transfer::Hlg, "frame {i} keeps its HDR transfer");
        worst = worst.max(mae(&rgba.data, &truth[i * size..(i + 1) * size]));
    }
    assert!(worst < 2.5, "proxy pixels differ from FFmpeg's original by MAE {worst:.2}");

    // The preview from the proxy shows what it shows from the original: turned upright and tone mapped.
    assert_eq!((project.assets[0].width, project.assets[0].height, project.assets[0].rotation), (1440, 2560, 90));
    let (mut original, mut preview) = (Renderer::new().unwrap(), Renderer::new().unwrap());
    preview.use_proxies(cache.clone());
    for t in [0, 100_000, 433_333, 1_000_000, 2_650_000] {
        let error = mae(&render(&project, &mut preview, t), &render(&project, &mut original, t));
        assert!(error < 2.0, "preview at {t} us differs from the original's by MAE {error:.2}");
    }

    // Swap in a proxy that is plainly another picture: the preview shows it, export still shows the file.
    let magenta = d.join("magenta.mp4");
    ff(&["-f", "lavfi", "-i", "color=magenta:s=192x108:r=30:d=3", "-c:v", "libx264", "-threads", "2"], &magenta);
    std::fs::rename(&magenta, &made).unwrap();
    let mut fresh = Renderer::new().unwrap();
    fresh.use_proxies(cache.clone());
    let centre = (240 * 270 + 135) * 4;
    let shown = render(&project, &mut fresh, 1_000_000)[centre..centre + 3].to_vec();
    assert!(shown[0] > 200 && shown[1] < 60 && shown[2] > 200, "the preview reads the proxy: {shown:?}");
    let options = ExportOptions {
        resolution: Some(270),
        preset: "ultrafast".into(),
        replace_existing: true,
        ..ExportOptions::default()
    };
    let (with, without) = (d.join("with-proxy.mp4"), d.join("without-proxy.mp4"));
    export(&project, &cache, &with, &options, &AtomicBool::new(false), |_| {}).unwrap();
    export(&project, &d.join("cache-without"), &without, &options, &AtomicBool::new(false), |_| {}).unwrap();
    assert!(frames(&with) == frames(&without), "export reads the original whether a proxy exists or not");
}
