mod qa_support;
use capopen_engine::{
    export::{Delivery, ExportOptions, ExportPhase, export},
    loudness::Meter,
    media::probe,
    model::*,
};
use qa_support::*;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
fn source(path: &Path) {
    ff(&["-f", "lavfi", "-i", "color=red:s=64x64:r=25:d=0.4", "-c:v", "libx264", "-threads", "2"], path);
}
fn options() -> ExportOptions {
    ExportOptions { preset: "ultrafast".into(), replace_existing: true, ..ExportOptions::default() }
}
#[test]
fn export_odd_canvas_duration_and_av_cut_match_timeline() {
    if !available() {
        return;
    }
    let d = dir("export-cut");
    let mut p = Project::new("Cut QA");
    p.canvas.width = 65;
    p.canvas.height = 67;
    p.canvas.fps = 25;
    for (id, color, tone, start, duration) in [("a", "red", 440, 0, 600_000), ("b", "blue", 880, 600_000, 400_000)] {
        let path = d.join(format!("{id}.mp4"));
        ff(
            &[
                "-f",
                "lavfi",
                "-i",
                &format!("color={color}:s=64x64:r=25:d=1"),
                "-f",
                "lavfi",
                "-i",
                &format!("sine=f={tone}:r=48000:d=1"),
                "-c:v",
                "libx264",
                "-threads",
                "2",
                "-c:a",
                "aac",
            ],
            &path,
        );
        p.assets.push(probe(&path, id.into()).unwrap());
        let mut c = clip(id, id, start, duration);
        if let ClipContent::Media { source_in_us, .. } = &mut c.content {
            *source_in_us = 200_000;
        }
        p.tracks[0].clips.push(c);
    }
    // Recreate only this test's caches: input IDs are intentionally stable for reproduction.
    let cache = d.join("cache");
    for a in &p.assets {
        let f = capopen_engine::audio::pcm_path(&cache, a);
        if f.exists() {
            std::fs::remove_file(f).unwrap();
        }
    }
    let out = d.join("cut.mp4");
    export(&p, &cache, &out, &options(), &AtomicBool::new(false), |_| {}).unwrap();
    let j = info(&out);
    let streams = j["streams"].as_array().unwrap();
    let video = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
    let audio = streams.iter().find(|s| s["codec_type"] == "audio").unwrap();
    assert_eq!(video["width"].as_u64().unwrap() % 2, 0);
    assert_eq!(video["height"].as_u64().unwrap() % 2, 0);
    assert_eq!(video["nb_frames"].as_str().unwrap(), "25");
    let mut failures = Vec::new();
    for (name, s, tolerance) in [("video", video, 0.001), ("audio", audio, 1024.0 / 48000.0 + 0.001)] {
        let duration = s["duration"].as_str().unwrap().parse::<f64>().unwrap();
        let start = s["start_time"].as_str().unwrap().parse::<f64>().unwrap();
        eprintln!("QA export {name} start={start} duration={duration}");
        if start.abs() > 0.001 || (duration - 1.0).abs() > tolerance {
            failures.push(format!("{name}: start={start}, duration={duration}"));
        }
    }
    let pixels = raw(&out, &[]);
    let frame_len = (video["width"].as_u64().unwrap() * video["height"].as_u64().unwrap() * 4) as usize;
    let center = (video["height"].as_u64().unwrap() / 2 * video["width"].as_u64().unwrap()
        + video["width"].as_u64().unwrap() / 2) as usize
        * 4;
    if pixels.len() / frame_len != 25 {
        failures.push(format!("decoded {} frames instead of 25", pixels.len() / frame_len));
    }
    for (frame, red) in [(14, true), (15, false), (24, false)] {
        if (frame + 1) * frame_len > pixels.len() {
            continue;
        }
        let px = &pixels[frame * frame_len + center..frame * frame_len + center + 3];
        if if red { px[0] < 200 || px[2] > 30 } else { px[2] < 200 || px[0] > 30 } {
            failures.push(format!("frame {frame}: {px:?}"));
        }
    }
    let audio = run(Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-f", "f32le", "-ac", "1", "-ar", "48000", "-"]))
    .stdout;
    let samples: Vec<f32> = audio.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    let power = |center: usize, f: f64| {
        let mut re = 0.0;
        let mut im = 0.0;
        for (n, &x) in samples[center - 240..center + 240].iter().enumerate() {
            let a = std::f64::consts::TAU * f * n as f64 / 48000.0;
            re += x as f64 * a.cos();
            im += x as f64 * a.sin();
        }
        re * re + im * im
    };
    let switch = (26000..32000).step_by(48).find(|&n| power(n, 880.0) > power(n, 440.0)).unwrap();
    eprintln!("QA tone switch {switch} samples = {:.3}s", switch as f64 / 48000.0);
    if (switch as i64 - 28800).abs() > 1024 {
        failures.push(format!("tone switch sample={switch}, expected=28800"));
    }
    assert!(!out.with_extension("capopen-part.mp4").exists());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn export_cancellation_is_prompt_and_preserves_destination() {
    if !available() {
        return;
    }
    let d = dir("export-cancel");
    let input = d.join("source.mp4");
    source(&input);
    let p = project(&input, 2_000_000);
    let out = d.join("destination.mp4");
    std::fs::write(&out, b"existing destination").unwrap();
    let cancel = AtomicBool::new(false);
    let now = Instant::now();
    let mut last = 0;
    let result = export(&p, &d.join("cache"), &out, &options(), &cancel, |progress| {
        last = progress.frame;
        if progress.frame == 2 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert!(result.is_err());
    assert!(format!("{:#}", result.unwrap_err()).contains("cancel"));
    assert!(now.elapsed().as_secs_f64() < 5.0, "cancellation took {:?}", now.elapsed());
    assert_eq!(last, 2);
    assert_eq!(std::fs::read(&out).unwrap(), b"existing destination");
    assert!(!out.with_extension("capopen-part.mp4").exists());
}
#[test]
fn export_refuses_to_overwrite_source() {
    if !available() {
        return;
    }
    let d = dir("export-source");
    let input = d.join("source.mp4");
    source(&input);
    let before = std::fs::read(&input).unwrap();
    let p = project(&input, 200_000);
    let result = export(&p, &d.join("cache"), &input, &options(), &AtomicBool::new(false), |_| {});
    assert!(result.is_err());
    assert_eq!(std::fs::read(&input).unwrap(), before);
}
#[test]
fn export_temp_path_must_not_destroy_a_source() {
    if !available() {
        return;
    }
    let d = dir("export-temp-source");
    let input = d.join("result.capopen-part.mp4");
    source(&input);
    let before = std::fs::read(&input).unwrap();
    let p = project(&input, 200_000);
    let out = d.join("result.mp4");
    let result = export(&p, &d.join("cache"), &out, &options(), &AtomicBool::new(false), |_| {});
    assert!(
        std::fs::read(&input).is_ok_and(|bytes| bytes == before),
        "source destroyed by temporary export path, export result={result:?}"
    );
}
#[test]
fn failed_final_rename_leaves_no_temporary_export() {
    if !available() {
        return;
    }
    let d = dir("export-rename-failure");
    let input = d.join("source.mp4");
    source(&input);
    let p = project(&input, 200_000);
    let out = d.join("existing-directory.mp4");
    std::fs::create_dir_all(&out).unwrap();
    let result = export(&p, &d.join("cache"), &out, &options(), &AtomicBool::new(false), |_| {});
    assert!(result.is_err());
    assert!(!out.with_extension("capopen-part.mp4").exists(), "complete temporary export remains after failed rename");
}

#[test]
fn missing_image_fails_exact_render_and_export_but_preview_survives() {
    let d = dir(&format!("missing-image-{}", capopen_engine::edit::new_id()));
    let path = d.join("deleted-image.ppm");
    std::fs::write(&path, b"P6\n2 2\n255\nabcdefghijkl").unwrap();
    let mut project = Project::new("missing image");
    project.canvas.width = 64;
    project.canvas.height = 64;
    project.assets.push(probe(&path, "image".into()).unwrap());
    project.tracks[0].clips.push(clip("clip", "image", 0, 100_000));
    std::fs::remove_file(&path).unwrap();
    let mut renderer = capopen_engine::Renderer::new().unwrap();
    let error = renderer.render(&project, 0, 64, 64, capopen_engine::Wait::Exact, false).unwrap_err();
    assert!(error.to_string().contains("deleted-image.ppm"), "{error}");
    renderer.render(&project, 0, 64, 64, capopen_engine::Wait::Ready, false).unwrap();
    let out = d.join("export.mp4");
    let error = export(&project, &d.join("cache"), &out, &options(), &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(error.to_string().contains("deleted-image.ppm"), "{error}");
    assert!(!out.exists());
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn export_ignores_missing_unused_muted_and_zero_volume_audio() {
    let d = dir(&format!("unused-audio-{}", capopen_engine::edit::new_id()));
    let image = d.join("image.ppm");
    std::fs::write(&image, b"P6\n2 2\n255\nabcdefghijkl").unwrap();
    let mut p = Project::new("used audio only");
    p.canvas.width = 64;
    p.canvas.height = 64;
    p.assets.push(probe(&image, "image".into()).unwrap());
    p.tracks[0].clips.push(clip("image", "image", 0, 100_000));
    for id in ["unused", "muted", "zero"] {
        p.assets.push(Asset {
            id: id.into(),
            name: id.into(),
            path: d.join(format!("{id}.wav")).to_string_lossy().into(),
            kind: AssetKind::Audio,
            duration_us: 100_000,
            width: 0,
            height: 0,
            fps: 0.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
        });
    }
    let mut muted = p.tracks[0].clone();
    muted.id = "muted".into();
    muted.kind = TrackKind::Audio;
    muted.muted = true;
    muted.clips = vec![clip("muted", "muted", 0, 100_000)];
    let mut zero = muted.clone();
    zero.id = "zero".into();
    zero.muted = false;
    zero.clips = vec![clip("zero", "zero", 0, 100_000)];
    if let ClipContent::Media { volume, .. } = &mut zero.clips[0].content {
        *volume = 0.0;
    }
    p.tracks.extend([muted, zero]);
    let out = d.join("out.mp4");
    export(&p, &d.join("cache"), &out, &options(), &AtomicBool::new(false), |_| {}).unwrap();
    assert!(std::fs::metadata(&out).unwrap().len() > 0);
    assert!(!d.join("cache/pcm").exists());
    p.tracks[1].muted = false;
    p.tracks[1].hidden = true;
    let error = export(&p, &d.join("cache"), &out, &options(), &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(format!("{error:#}").contains("muted.wav"));
    std::fs::remove_dir_all(d).unwrap();
}

// The Reels & TikTok preset: what Instagram and TikTok take as it is, with the sound levelled.

fn reels() -> ExportOptions {
    ExportOptions { delivery: Some(Delivery::Reels), replace_existing: true, ..ExportOptions::default() }
}

/// Voiced syllables at 3.3 per second in phrases of 2.2 s, each phrase opening with a plosive
/// burst: speech's 19 dB between true peak and loudness, without needing a voice synthesiser.
const SPEECH: &str = "(sin(2*PI*130*t)+0.7*sin(2*PI*260*t)+0.5*sin(2*PI*390*t)+0.35*sin(2*PI*700*t)\
    +0.25*sin(2*PI*1100*t)+0.15*sin(2*PI*2500*t))*pow(max(0\\,sin(2*PI*3.3*t))\\,4)*between(mod(t\\,3)\\,0\\,2.2)\
    +4.5*exp(-150*mod(t\\,3))*sin(2*PI*2200*t)";

/// A 9:16 talking head of `seconds`: a still picture, the speech-like sound through `level`
/// (an FFmpeg audio filter) and quiet pink room tone. Sound stays float PCM, so only the export
/// changes it.
fn talking_head(path: &Path, seconds: u32, level: &str) {
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("color=c=0x2b3a4a:s=108x192:r=30:d={seconds}"),
            "-f",
            "lavfi",
            "-i",
            &format!("aevalsrc={SPEECH}|{SPEECH}:s=48000:d={seconds}"),
            "-f",
            "lavfi",
            "-i",
            &format!("anoisesrc=color=pink:amplitude=0.004:seed=1:sample_rate=48000:duration={seconds}"),
            "-filter_complex",
            &format!("[1:a]{level}[s];[s][2:a]amix=inputs=2:normalize=0[a]"),
            "-map",
            "0:v",
            "-map",
            "[a]",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "pcm_f32le",
        ],
        path,
    );
}

/// Integrated loudness (LUFS) and true peak (dBTP) as FFmpeg's EBU R128 meter reads the file.
fn ebur128(path: &Path) -> (f64, f64) {
    let out = run(Command::new("ffmpeg").args(["-nostdin", "-hide_banner", "-i"]).arg(path).args([
        "-vn",
        "-af",
        "ebur128=peak=true",
        "-f",
        "null",
        "-",
    ]));
    let log = String::from_utf8_lossy(&out.stderr);
    let summary = &log[log.rfind("Summary:").expect("ebur128 summary")..];
    let value = |key: &str, unit: &str| -> f64 {
        let rest = &summary[summary.find(key).unwrap() + key.len()..];
        rest[..rest.find(unit).unwrap()].trim().parse().unwrap()
    };
    (value("I:", "LUFS"), value("Peak:", "dBFS"))
}

/// The sound of a file as FFmpeg decodes it: 48 kHz stereo float.
fn decode(path: &Path) -> Vec<f32> {
    let out = run(Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-f", "f32le", "-ac", "2", "-ar", "48000", "-"]));
    out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect()
}

/// Top-level MP4 boxes in file order.
fn atoms(path: &Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap();
    let mut found = Vec::new();
    let mut at = 0usize;
    while at + 8 <= bytes.len() {
        let mut size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        found.push(String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned());
        if size == 1 {
            size = u64::from_be_bytes(bytes[at + 8..at + 16].try_into().unwrap()) as usize;
        } else if size == 0 {
            break;
        }
        at += size.max(8);
    }
    found
}

fn reels_project(path: &Path, seconds: u32) -> Project {
    project(path, seconds as i64 * 1_000_000)
}

/// Checks the format Instagram and TikTok expect; returns the integrated loudness and true peak.
fn check_reels_file(out: &Path, seconds: f64) -> (f64, f64) {
    let j = info(out);
    let streams = j["streams"].as_array().unwrap();
    let video = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
    let audio = streams.iter().find(|s| s["codec_type"] == "audio").unwrap();
    let fields = |stream: &serde_json::Value, keys: &[&str]| -> Vec<String> {
        keys.iter().map(|k| stream[*k].to_string().trim_matches('"').to_owned()).collect()
    };
    assert_eq!(
        fields(video, &["codec_name", "profile", "width", "height", "r_frame_rate", "avg_frame_rate", "pix_fmt"]),
        ["h264", "High", "1080", "1920", "30/1", "30/1", "yuv420p"]
    );
    assert_eq!(
        fields(video, &["color_space", "color_transfer", "color_primaries", "color_range"]),
        ["bt709", "bt709", "bt709", "tv"]
    );
    assert_eq!(fields(audio, &["codec_name", "sample_rate", "channels"]), ["aac", "48000", "2"]);
    let boxes = atoms(out);
    let position = |name: &str| boxes.iter().position(|b| b == name).unwrap();
    assert!(position("moov") < position("mdat"), "moov must come first so playback starts at once: {boxes:?}");
    for stream in [video, audio] {
        let duration: f64 = stream["duration"].as_str().unwrap().parse().unwrap();
        let kind = &stream["codec_type"];
        assert!((duration - seconds).abs() <= 1024.0 / 48000.0 + 0.001, "{kind} lasts {duration} s");
    }
    ebur128(out)
}

#[test]
fn reels_preset_writes_the_platform_format_at_minus_14_lufs() {
    if !available() {
        return;
    }
    let d = dir("reels-loudness");
    // (a) quiet speech near -26 LUFS whose plosives peak 19 dB above it, (b) a loud, clipped mix
    // near -6 LUFS with peaks over 0 dBTP that must come down.
    for (name, level, source_lufs) in [("quiet", "volume=0.1", -26.0), ("loud", "volume=3,asoftclip=type=tanh", -6.0)] {
        let input = d.join(format!("{name}.mkv"));
        talking_head(&input, 6, level);
        let (lufs, peak) = ebur128(&input);
        eprintln!("QA reels {name} source: {lufs} LUFS, {peak} dBTP");
        assert!((lufs - source_lufs).abs() < 1.0, "{name} source is {lufs} LUFS");
        let out = d.join(format!("{name}-reel.mp4"));
        let started = Instant::now();
        let mut phases = Vec::new();
        export(&reels_project(&input, 6), &d.join("cache"), &out, &reels(), &AtomicBool::new(false), |p| {
            if phases.last() != Some(&p.phase) {
                phases.push(p.phase);
            }
        })
        .unwrap();
        let (lufs, peak) = check_reels_file(&out, 6.0);
        eprintln!("QA reels {name} export: {lufs} LUFS, {peak} dBTP in {:?}", started.elapsed());
        assert!((lufs + 14.0).abs() <= 1.0, "{name}: {lufs} LUFS");
        assert!(peak <= -1.0, "{name}: true peak {peak} dBTP");
        assert_eq!(phases, [ExportPhase::Loudness, ExportPhase::Rendering]);
    }
}

#[test]
fn reels_preset_keeps_silence_silent() {
    if !available() {
        return;
    }
    let d = dir("reels-silence");
    let input = d.join("silent.mkv");
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=gray:s=108x192:r=30:d=2",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=stereo",
            "-t",
            "2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "pcm_f32le",
        ],
        &input,
    );
    let out = d.join("silent-reel.mp4");
    export(&reels_project(&input, 2), &d.join("cache"), &out, &reels(), &AtomicBool::new(false), |_| {}).unwrap();
    check_reels_file(&out, 2.0);
    let sound = decode(&out);
    assert!(sound.len() >= 2 * 48_000 * 2 - 2048, "{} samples", sound.len());
    assert!(sound.iter().all(|&x| x == 0.0), "silence came out as sound");
}

/// Where a click starts: the first sample above a third of the loudest one.
fn onset(sound: &[f32]) -> usize {
    let loudest = sound.iter().fold(0f32, |m, x| m.max(x.abs()));
    sound.chunks_exact(2).position(|f| f[0].abs() > loudest / 3.0).unwrap()
}

#[test]
fn reels_levelling_keeps_a_click_on_its_flash() {
    if !available() {
        return;
    }
    let d = dir("reels-sync");
    let input = d.join("clap.mkv");
    // Frame 45 (1.5 s) is white and a click starts at 1.5 s over a quiet hum, so the levelling
    // raises the hum by the full +24 dB and the limiter holds the click.
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=108x192:r=30:d=3",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=0.0005*sin(2*PI*300*t)+0.5*between(t\\,1.5\\,1.504)*sin(2*PI*3000*(t-1.5)):s=48000:d=3",
            "-vf",
            "drawbox=c=white:t=fill:enable='eq(n\\,45)'",
            "-ac",
            "2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "pcm_f32le",
        ],
        &input,
    );
    let project = reels_project(&input, 3);
    let cache = d.join("cache");
    let plain = d.join("plain.mp4");
    let plain_options = ExportOptions { resolution: Some(108), ..options() };
    export(&project, &cache, &plain, &plain_options, &AtomicBool::new(false), |_| {}).unwrap();
    let leveled = d.join("reel.mp4");
    export(&project, &cache, &leveled, &reels(), &AtomicBool::new(false), |_| {}).unwrap();

    let (before, after) = (decode(&plain), decode(&leveled));
    assert_eq!(before.len(), after.len(), "the levelled file has another number of samples");
    let (click, moved) = (onset(&before), onset(&after) as i64 - onset(&before) as i64);
    let hum = |s: &[f32]| (s[20_000..40_000].iter().map(|x| x * x).sum::<f32>() / 20_000.0).sqrt();
    eprintln!("QA reels sync: click at sample {click}, moved {moved}; hum {} -> {}", hum(&before), hum(&after));
    assert!(hum(&after) > hum(&before) * 10.0, "the quiet hum was not raised");
    // The look-ahead is 248 samples; levelling may move nothing by more than a millisecond.
    assert!(moved.abs() <= 48, "levelling moved the click by {moved} samples");

    let pixels = raw(&leveled, &["-vf", "scale=8:8"]);
    let flash = pixels.chunks_exact(8 * 8 * 4).position(|f| f[4 * 27] > 200).unwrap();
    let flash_sample = flash as i64 * 48_000 / 30;
    let click = onset(&after) as i64;
    eprintln!("QA reels sync: flash on frame {flash} ({flash_sample}), click at {click}");
    assert_eq!(flash, 45);
    assert!((click - flash_sample).abs() <= 1024, "click {click} is not on the flash at {flash_sample}");
}

#[test]
fn reels_preset_refuses_what_it_cannot_deliver() {
    if !available() {
        return;
    }
    let d = dir(&format!("reels-refuse-{}", capopen_engine::edit::new_id()));
    let input = d.join("wide.mkv");
    ff(&["-f", "lavfi", "-i", "color=c=gray:s=192x108:r=30:d=1", "-c:v", "libx264", "-pix_fmt", "yuv420p"], &input);
    let out = d.join("reel.mp4");
    let wide = project(&input, 1_000_000);
    let error = export(&wide, &d.join("cache"), &out, &reels(), &AtomicBool::new(false), |_| {}).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Reels & TikTok needs a 9:16 video and this one is 16:9. Switch the format to 9:16, then export again."
    );
    let mut tall = wide.clone();
    (tall.canvas.width, tall.canvas.height) = (1080, 1920);
    let small = ExportOptions { resolution: Some(720), ..reels() };
    let error = export(&tall, &d.join("cache"), &out, &small, &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(error.to_string().contains("resolution 1080"), "{error}");
    let fast = ExportOptions { fps: Some(60), ..reels() };
    assert!(export(&tall, &d.join("cache"), &out, &fast, &AtomicBool::new(false), |_| {}).is_err());
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "only the input is left");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn reels_loudness_passes_stop_within_a_second_of_cancel() {
    if !available() {
        return;
    }
    let d = dir("reels-cancel");
    let input = d.join("long.mkv");
    // Two minutes of speech, which needs the limiter and so several levelling passes.
    talking_head(&input, 120, "volume=0.1");
    let out = d.join("destination.mp4");
    std::fs::write(&out, b"existing destination").unwrap();
    let cancel = AtomicBool::new(false);
    let mut asked: Option<Instant> = None;
    let mut seen = Vec::new();
    let result = export(&reels_project(&input, 120), &d.join("cache"), &out, &reels(), &cancel, |p| {
        seen.push((p.phase, p.fraction));
        // A pass reports every second of sound: stop in the middle of the second pass, the
        // first one through the limiter.
        if asked.is_none() && seen.len() == 180 {
            cancel.store(true, Ordering::Relaxed);
            asked = Some(Instant::now());
        }
    });
    let stopped = asked.expect("the loudness passes reported progress").elapsed();
    let error = format!("{:#}", result.unwrap_err());
    eprintln!("QA reels cancel: stopped {stopped:?} after the request, {} reports", seen.len());
    assert!(error.starts_with("CANCELLED"), "{error}");
    assert!(stopped < Duration::from_secs(1), "stopping took {stopped:?}");
    assert_eq!(seen.len(), 180, "the pass went on after the request");
    assert!(seen.iter().all(|(phase, _)| *phase == ExportPhase::Loudness), "rendering started");
    assert!(seen.windows(2).all(|w| w[0].1 <= w[1].1), "progress went back");
    assert_eq!(std::fs::read(&out).unwrap(), b"existing destination");
    let left: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(left.iter().all(|n| !n.to_string_lossy().starts_with(".capopen-part")), "{left:?}");
}

/// The loudness meter reads what FFmpeg's EBU R128 meter reads: K-weighting, gating and the
/// oversampled true peak on noise, speech-like sound, clipped sound and low and high tones.
#[test]
fn meter_agrees_with_ffmpeg_ebur128() {
    if !available() {
        return;
    }
    let d = dir("reels-meter");
    let sources: [(&str, String); 4] = [
        ("pink", "anoisesrc=color=pink:amplitude=0.3:seed=7:sample_rate=48000:duration=10".into()),
        ("speech", format!("aevalsrc=0.1*({SPEECH})|0.1*({SPEECH}):s=48000:d=10")),
        ("clipped", format!("aevalsrc=3*({SPEECH}):s=48000:d=10,asoftclip=type=tanh")),
        (
            "tones",
            "aevalsrc=0.3*sin(2*PI*40*t)+0.1*sin(2*PI*4000*t)*lt(t\\,5)|0.2*sin(2*PI*15000*t+1):s=48000:d=10".into(),
        ),
    ];
    for (name, graph) in sources {
        let path = d.join(format!("{name}.wav"));
        ff(&["-f", "lavfi", "-i", &graph, "-ac", "2", "-c:a", "pcm_f32le"], &path);
        let (lufs, peak) = ebur128(&path);
        let mut meter = Meter::new();
        meter.push(&decode(&path));
        let ours = meter.finish();
        let (our_lufs, our_peak) = (ours.integrated.unwrap(), ours.true_peak_db());
        eprintln!("QA meter {name}: FFmpeg {lufs} LUFS {peak} dBTP, ours {our_lufs:.2} LUFS {our_peak:.2} dBTP");
        // FFmpeg prints one decimal.
        assert!((our_lufs - lufs).abs() <= 0.1, "{name}: {our_lufs} LUFS, FFmpeg {lufs}");
        assert!((our_peak - peak).abs() <= 0.3, "{name}: {our_peak} dBTP, FFmpeg {peak}");
    }
}

/// How far AAC lifts the true peak over the limiter's ceiling on sound harder than speech: claps
/// over speech, sharp high clicks, and noise, a square wave and music under bursts of noise that
/// carry most of their loudness, so the limiter takes 10 dB and more off. Every decoded file must
/// stay at or under -1 dBTP and within 1 LU of -14; prints the margin left.
#[test]
#[ignore = "a minute of rendering; run when the limiter or the AAC settings change"]
fn reels_true_peak_survives_aac_on_hard_material() {
    let d = dir("reels-margin");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp-test");
    let bursts = "aevalsrc=0.9*lt(mod(t\\,0.5)\\,0.01)*(2*random(0)-1):s=48000:d=8[b]";
    let with_bursts = |base: &str| format!("{base}[a];{bursts};[a][b]amix=inputs=2:normalize=0");
    let claps = format!("aevalsrc=0.1*({SPEECH})+0.9*exp(-60*mod(t+0.7\\,1.5))*(2*random(0)-1):s=48000:d=8");
    let sources: [(&str, String); 5] = [
        ("pink", with_bursts("anoisesrc=color=pink:amplitude=0.12:seed=3:sample_rate=48000:duration=8")),
        ("claps", claps),
        ("clicks", "aevalsrc=0.06*sin(2*PI*220*t)+0.95*lt(mod(t\\,0.25)\\,0.0004)*sin(2*PI*9000*t):s=48000:d=8".into()),
        ("square", with_bursts("aevalsrc=0.04*(2*lt(mod(t*997\\,1)\\,0.5)-1):s=48000:d=8")),
        ("music", with_bursts(&format!("amovie={},volume=0.25", root.join("music.mp3").display()))),
    ];
    let mut failures = Vec::new();
    for (name, graph) in sources {
        let input = d.join(format!("{name}.mkv"));
        ff(
            &[
                "-f",
                "lavfi",
                "-i",
                "color=c=gray:s=108x192:r=30:d=8",
                "-f",
                "lavfi",
                "-i",
                &graph,
                "-t",
                "8",
                "-ac",
                "2",
                "-ar",
                "48000",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "pcm_f32le",
            ],
            &input,
        );
        let out = d.join(format!("{name}-reel.mp4"));
        export(&reels_project(&input, 8), &d.join("cache"), &out, &reels(), &AtomicBool::new(false), |_| {}).unwrap();
        let (source, (lufs, peak)) = (ebur128(&input), ebur128(&out));
        eprintln!("QA margin {name}: source {source:?}, reel {lufs} LUFS {peak} dBTP");
        if peak > -1.0 || (lufs + 14.0).abs() > 1.0 {
            failures.push(format!("{name}: {lufs} LUFS, {peak} dBTP"));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

/// The three Czech takes of `scripts/fixtures.sh` one after the other (about 46 s at 1080x1920),
/// exported as they are and with the Reels preset: the time each takes and the loudness.
#[test]
#[ignore = "requires tmp-test/reel-{1,2,3}.mp4 from scripts/fixtures.sh"]
fn reels_preset_on_the_three_czech_takes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp-test");
    let d = dir("reels-takes");
    let mut p = Project::new("Takes");
    let mut start = 0;
    for n in 1..=3 {
        let asset = probe(&root.join(format!("reel-{n}.mp4")), format!("take-{n}")).unwrap();
        p.tracks[0].clips.push(clip(&asset.id, &asset.id, start, asset.duration_us));
        start += asset.duration_us;
        p.assets.push(asset);
    }
    let cache = d.join("cache");
    let seconds = start as f64 / 1e6;
    let plain =
        ExportOptions { resolution: Some(1080), fps: Some(30), replace_existing: true, ..ExportOptions::default() };
    for (name, options) in [("plain", plain), ("reels", reels())] {
        let out = d.join(format!("{name}.mp4"));
        let started = Instant::now();
        export(&p, &cache, &out, &options, &AtomicBool::new(false), |_| {}).unwrap();
        let took = started.elapsed();
        let (lufs, peak) = ebur128(&out);
        eprintln!("QA takes {name}: {seconds:.1} s exported in {took:.1?}: {lufs} LUFS, {peak} dBTP");
        if name == "reels" {
            check_reels_file(&out, seconds);
            assert!((lufs + 14.0).abs() <= 1.0 && peak <= -1.0, "{lufs} LUFS, {peak} dBTP");
        }
    }
}
