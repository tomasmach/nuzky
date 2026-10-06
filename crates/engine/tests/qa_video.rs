mod qa_support;
use capopen_engine::{
    Renderer, Wait,
    media::{VideoDecoder, decode_size, probe},
    model::*,
    worker::VideoWorker,
};
use qa_support::*;
use std::{path::Path, process::Command, time::Instant};

fn check_seek(path: &Path) {
    let timestamps = pts(path);
    let a = probe(path, "seek".into()).unwrap();
    let truth = raw(path, &[]);
    let frame_len = (a.width * a.height * 4) as usize;
    assert_eq!(truth.len() / frame_len, timestamps.len());
    let mut decoder = VideoDecoder::open(path).unwrap();
    let mut worker = VideoWorker::spawn(path.into());
    let mut rng = Rng(0xCAFE_1234);
    let mut times = vec![0, 1, *timestamps.last().unwrap()];
    for _ in 0..12 {
        let index = (rng.next() as usize) % (timestamps.len() - 1);
        times.extend([timestamps[index], timestamps[index + 1] - 1]);
    }
    let mut failures = Vec::new();
    for t in times {
        let expected = timestamps.iter().rposition(|&p| p <= t).unwrap();
        decoder.seek(t).unwrap();
        let mut candidate = None;
        while let Some((ts, frame)) = decoder.next_frame().unwrap() {
            if ts > t {
                break;
            }
            candidate = Some((ts, frame));
        }
        if let Some((ts, frame)) = candidate {
            let rgba = decoder.convert(&frame, ts, a.width, a.height).unwrap();
            let error = mae(&rgba.data, &truth[expected * frame_len..(expected + 1) * frame_len]);
            if ts != timestamps[expected] || error > 1.0 {
                failures.push(format!("decoder t={t}: pts={ts} expected={} MAE={error}", timestamps[expected]));
            }
        } else {
            failures.push(format!("decoder t={t}: no candidate"));
        }
        let rgba = worker.get(t, (a.width, a.height), false, true).expect("worker frame");
        let error = mae(&rgba.data, &truth[expected * frame_len..(expected + 1) * frame_len]);
        if !worker.exact || rgba.t_us != timestamps[expected] || error > 1.0 {
            failures.push(format!(
                "worker t={t}: pts={} expected={} exact={} MAE={error}",
                rgba.t_us, timestamps[expected], worker.exact
            ));
        }
    }
    assert!(failures.is_empty(), "{}\n{}", path.display(), failures.join("\n"));
}

#[test]
fn cfr_h264_long_gop_b_frames_seek_matches_ffmpeg_pts_and_pixels() {
    if !available() {
        return;
    }
    let d = dir("seek-cfr");
    let mut failures = Vec::new();
    for rate in ["25", "30000/1001", "30", "60"] {
        let path = d.join(format!("counter-{}.mp4", rate.replace('/', "_")));
        ff(
            &[
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=size=96x64:rate={rate}:duration={}", if rate == "25" { 10.4 } else { 3.0 }),
                "-vf",
                "drawtext=text='%{n}':x=4:y=4:fontsize=22:fontcolor=white:box=1:boxcolor=black",
                "-c:v",
                "libx264",
                "-threads",
                "2",
                "-g",
                "250",
                "-bf",
                "3",
                "-crf",
                "18",
                "-pix_fmt",
                "yuv420p",
            ],
            &path,
        );
        if std::panic::catch_unwind(|| check_seek(&path)).is_err() {
            failures.push(rate);
        }
    }
    assert!(failures.is_empty(), "Failed rates: {failures:?}");
}
#[test]
fn vfr_and_hevc_10_bit_seek_matches_ffmpeg() {
    if !available() {
        return;
    }
    let d = dir("seek-vfr-hevc");
    let encoders = run(Command::new("ffmpeg").args(["-hide_banner", "-encoders"]));
    let has_x265 = String::from_utf8_lossy(&encoders.stdout).contains("libx265");
    let mut failures = Vec::new();
    for hevc in [false, true] {
        let path = d.join(if hevc { "hevc10.mp4" } else { "vfr.mp4" });
        let filter = "drawtext=text='%{n}':x=4:y=4:fontsize=22:fontcolor=white:box=1:boxcolor=black,setpts='if(lt(N,20),N/(30*TB),(20/30+(N-20)/12)/TB)'";
        let vaapi_filter = format!("{filter},format=p010le,hwupload");
        let mut args =
            vec!["-f", "lavfi", "-i", "testsrc2=size=96x64:rate=30:duration=2", "-vf", filter, "-fps_mode", "vfr"];
        if hevc && has_x265 {
            args.extend([
                "-c:v",
                "libx265",
                "-x265-params",
                "pools=1:frame-threads=1:keyint=250:bframes=3:log-level=error",
                "-pix_fmt",
                "yuv420p10le",
            ]);
        } else if hevc {
            args[5] = &vaapi_filter;
            args.splice(0..0, ["-vaapi_device", "/dev/dri/renderD128"]);
            args.extend(["-c:v", "hevc_vaapi", "-profile:v", "main10"]);
        } else {
            args.extend(["-c:v", "libx264", "-threads", "2", "-g", "250", "-bf", "3"]);
        }
        ff(&args, &path);
        if std::panic::catch_unwind(|| check_seek(&path)).is_err() {
            failures.push(hevc);
        }
    }
    assert!(failures.is_empty(), "Failed HEVC flags {failures:?}");
}
#[test]
fn rotation_display_matrices_match_ffmpeg_autorotate() {
    if !available() {
        return;
    }
    let d = dir("rotation");
    let source = d.join("base.mp4");
    ff(&["-f", "lavfi", "-i", "testsrc2=size=96x64:rate=25:duration=0.2", "-c:v", "libx264", "-threads", "2"], &source);
    let mut renderer = Renderer::new().unwrap();
    for angle in [90, 180, 270] {
        let path = d.join(format!("rotate-{angle}.mp4"));
        ff(&["-display_rotation", &angle.to_string(), "-i", source.to_str().unwrap(), "-c", "copy"], &path);
        assert!(
            info(&path)["streams"][0]["side_data_list"].as_array().is_some_and(|a| !a.is_empty()),
            "fixture has no display matrix"
        );
        let p = project(&path, 200_000);
        let (w, h) = if angle == 180 { (96, 64) } else { (64, 96) };
        assert_eq!((p.assets[0].width, p.assets[0].height), (w, h));
        assert_eq!(p.assets[0].rotation, (360 - angle) % 360);
        let got = renderer.render(&p, 0, w, h, Wait::Exact, false).unwrap();
        let expected = raw(&path, &["-frames:v", "1"]);
        let error = mae(&got, &expected);
        assert!(error < 3.0, "rotation {angle}: pixel MAE={error}");
    }
}
#[test]
fn odd_tiny_and_4k_frames_decode_without_corruption() {
    if !available() {
        return;
    }
    let d = dir("sizes");
    for (w, h) in [(1, 1), (3, 5), (1080, 1918), (3840, 2160)] {
        let p = d.join(format!("{w}x{h}.{}", if w == 3840 { "mp4" } else { "png" }));
        if w == 3840 {
            ff(
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "color=white:s=3840x2160:r=25",
                    "-frames:v",
                    "1",
                    "-c:v",
                    "libx264",
                    "-threads",
                    "2",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    "yuv420p",
                ],
                &p,
            );
        } else {
            ff(
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("color=white:s=2x2,scale={w}:{h}"),
                    "-frames:v",
                    "1",
                    "-pix_fmt",
                    "rgb24",
                ],
                &p,
            );
        }
        let start = Instant::now();
        let mut decoder = VideoDecoder::open(&p).unwrap();
        assert_eq!(decoder.source_size(), (w, h));
        let (t, f) = decoder.next_frame().unwrap().unwrap();
        let size = decode_size((w, h), 0, (w as f32, h as f32));
        let rgba = decoder.convert(&f, t, size.0, size.1).unwrap();
        assert_eq!(rgba.data.len(), (size.0 * size.1 * 4) as usize);
        assert!(rgba.data.iter().all(|&v| v >= 253));
        eprintln!("QA decode {w}x{h}: {:?}", start.elapsed());
    }
}
#[test]
fn full_range_444_rgb_alpha_and_gif() {
    if !available() {
        return;
    }
    let d = dir("pixels");
    let mut failures = Vec::new();
    for (name, codec, pix) in [
        ("full.mkv", "mjpeg", "yuvj420p"),
        ("444.mkv", "libx264", "yuv444p"),
        ("rgb.png", "png", "rgb24"),
        ("alpha.png", "png", "rgba"),
        ("anim.gif", "gif", "rgb8"),
    ] {
        let p = d.join(name);
        ff(
            &[
                "-f",
                "lavfi",
                "-i",
                "nullsrc=s=64x64:r=10:d=0.3,format=rgba,geq=r='16+X*3':g='16+X*3':b='16+X*3':a=128",
                "-frames:v",
                if name.ends_with("gif") { "3" } else { "1" },
                "-c:v",
                codec,
                "-threads",
                "2",
                "-pix_fmt",
                pix,
            ],
            &p,
        );
        let mut decoder = VideoDecoder::open(&p).unwrap();
        let (t, f) = decoder.next_frame().unwrap().unwrap();
        let rgba = decoder.convert(&f, t, 64, 64).unwrap();
        let expected = raw(&p, &["-frames:v", "1"]);
        let error = mae(&rgba.data, &expected);
        if error > 1.0 {
            failures.push(format!("{name} MAE={error}"));
        }
        if name == "full.mkv" && ((rgba.data[0] as i16 - 16).abs() > 4 || (rgba.data[63 * 4] as i16 - 205).abs() > 4) {
            failures.push(format!("range {}..{}", rgba.data[0], rgba.data[63 * 4]));
        }
        if name == "alpha.png" {
            assert_eq!(rgba.data[3], 128);
        }
        if name == "anim.gif" {
            assert_eq!(probe(&p, "gif".into()).unwrap().kind, AssetKind::Video);
            assert_eq!(pts(&p).len(), 3);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn png_alpha_composites_over_video() {
    if !available() {
        return;
    }
    let d = dir("alpha-composite");
    let video = d.join("blue.mp4");
    let png = d.join("red-half.png");
    ff(&["-f", "lavfi", "-i", "color=blue:s=64x64:r=25:d=0.2", "-c:v", "libx264", "-threads", "2"], &video);
    ff(&["-f", "lavfi", "-i", "color=red@0.5:s=64x64,format=rgba", "-frames:v", "1", "-pix_fmt", "rgba"], &png);
    let mut p = project(&video, 200_000);
    let asset = probe(&png, "overlay".into()).unwrap();
    p.tracks.push(Track {
        id: "overlay-track".into(),
        kind: TrackKind::Video,
        name: String::new(),
        muted: false,
        hidden: false,
        keep_in_place: false,
        clips: vec![clip("over", &asset.id, 0, 200_000)],
    });
    p.assets.push(asset);
    let frame = Renderer::new().unwrap().render(&p, 0, 64, 64, Wait::Exact, false).unwrap();
    let pixel = &frame[(32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4];
    assert!(
        (pixel[0] as i16 - 127).abs() < 5 && pixel[1] < 5 && (pixel[2] as i16 - 127).abs() < 5 && pixel[3] == 255,
        "half-red over blue: {pixel:?}"
    );
}

#[test]
fn preview_decodes_new_media_after_a_clip_is_relinked() {
    if !available() {
        return;
    }
    let d = dir("relink");
    let (blue, red) = (d.join("blue.mp4"), d.join("red.mp4"));
    ff(&["-f", "lavfi", "-i", "color=blue:s=64x64:r=25:d=0.2", "-c:v", "libx264", "-threads", "2"], &blue);
    ff(&["-f", "lavfi", "-i", "color=red:s=64x64:r=25:d=0.2", "-c:v", "libx264", "-threads", "2"], &red);
    // The preview keeps one renderer across projects; a copied project keeps its clip ids.
    let mut renderer = Renderer::new().unwrap();
    let mut p = project(&blue, 200_000);
    let centre = |frame: &[u8]| frame[(32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 3].to_vec();
    let before = centre(&renderer.render(&p, 0, 64, 64, Wait::Exact, false).unwrap());
    assert!(before[2] > 200 && before[0] < 50, "blue: {before:?}");
    p.assets[0].path = red.to_string_lossy().into_owned();
    let after = centre(&renderer.render(&p, 0, 64, 64, Wait::Exact, false).unwrap());
    assert!(after[0] > 200 && after[2] < 50, "red: {after:?}");
    // The decoder of the old file goes at once, not after idling, even when the new file fails.
    assert_eq!(renderer.decoders(), 1);
    let broken = d.join("broken.mp4");
    std::fs::write(&broken, b"not a video").unwrap();
    p.assets[0].path = broken.to_string_lossy().into_owned();
    assert!(renderer.render(&p, 0, 64, 64, Wait::Exact, false).is_err());
    assert_eq!(renderer.decoders(), 1);
}

#[test]
fn cold_seek_immediately_before_keyframe_uses_previous_frame() {
    if !available() {
        return;
    }
    let d = dir("seek-keyframe-boundary");
    let path = d.join("gop10.mp4");
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=96x64:r=25:d=0.8",
            "-c:v",
            "libx264",
            "-threads",
            "2",
            "-g",
            "10",
            "-keyint_min",
            "10",
            "-sc_threshold",
            "0",
            "-bf",
            "3",
        ],
        &path,
    );
    let truth = raw(&path, &[]);
    let timestamps = pts(&path);
    let mut failures = Vec::new();
    for t in [399_900, 399_990, 399_999, 400_000, 400_001] {
        let expected = timestamps.iter().rposition(|&p| p <= t).unwrap();
        let mut decoder = VideoDecoder::open(&path).unwrap();
        decoder.seek(t).unwrap();
        let first = decoder.next_frame().unwrap().unwrap().0;
        let mut worker = VideoWorker::spawn(path.clone());
        let frame = worker.get(t, (96, 64), false, true).unwrap();
        let error = mae(&frame.data, &truth[expected * 96 * 64 * 4..(expected + 1) * 96 * 64 * 4]);
        eprintln!(
            "QA keyframe boundary want={t} first_after_seek={first} worker={} expected={} exact={} MAE={error}",
            frame.t_us, timestamps[expected], worker.exact
        );
        if frame.t_us != timestamps[expected] || error > 1.0 {
            failures.push(format!(
                "want={t}: returned {}, expected {}, exact={}",
                frame.t_us, timestamps[expected], worker.exact
            ));
        }
    }
    // Diagnose the proposed repair without changing VideoDecoder: seek in stream ticks,
    // explicitly rounding the upper bound down. A 1/12800 tick must not round up to .4s.
    let mut input = ffmpeg_next::format::input(&path).unwrap();
    let stream = input.streams().best(ffmpeg_next::media::Type::Video).unwrap();
    let index = stream.index();
    let tb = stream.time_base();
    let tick = 399_999_i64 * tb.denominator() as i64 / (1_000_000 * tb.numerator() as i64);
    let result =
        unsafe { ffmpeg_next::ffi::avformat_seek_file(input.as_mut_ptr(), index as i32, i64::MIN, tick, tick, 0) };
    assert_eq!(result, 0);
    let first = input.packets().find(|(s, _)| s.index() == index).unwrap().1.pts().unwrap();
    eprintln!("QA stream-tick diagnostic: time_base={tb}, floor_tick={tick}, first_packet_pts={first}");
    assert!(first <= tick);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
