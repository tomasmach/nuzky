mod qa_support;
use capopen_engine::{
    export::{ExportOptions, export},
    media::probe,
    model::*,
};
use qa_support::*;
use std::{
    path::Path,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
fn source(path: &Path) {
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:s=64x64:r=25:d=0.4",
            "-c:v",
            "libx264",
            "-threads",
            "2",
        ],
        path,
    );
}
fn options() -> ExportOptions {
    ExportOptions {
        preset: "ultrafast".into(),
        replace_existing: true,
        ..ExportOptions::default()
    }
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
    for (id, color, tone, start, duration) in [
        ("a", "red", 440, 0, 600_000),
        ("b", "blue", 880, 600_000, 400_000),
    ] {
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
    export(
        &p,
        &cache,
        &out,
        &options(),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let j = info(&out);
    let streams = j["streams"].as_array().unwrap();
    let video = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
    let audio = streams.iter().find(|s| s["codec_type"] == "audio").unwrap();
    assert_eq!(video["width"].as_u64().unwrap() % 2, 0);
    assert_eq!(video["height"].as_u64().unwrap() % 2, 0);
    assert_eq!(video["nb_frames"].as_str().unwrap(), "25");
    let mut failures = Vec::new();
    for (name, s, tolerance) in [
        ("video", video, 0.001),
        ("audio", audio, 1024.0 / 48000.0 + 0.001),
    ] {
        let duration = s["duration"].as_str().unwrap().parse::<f64>().unwrap();
        let start = s["start_time"].as_str().unwrap().parse::<f64>().unwrap();
        eprintln!("QA export {name} start={start} duration={duration}");
        if start.abs() > 0.001 || (duration - 1.0).abs() > tolerance {
            failures.push(format!("{name}: start={start}, duration={duration}"));
        }
    }
    let pixels = raw(&out, &[]);
    let frame_len =
        (video["width"].as_u64().unwrap() * video["height"].as_u64().unwrap() * 4) as usize;
    let center = (video["height"].as_u64().unwrap() / 2 * video["width"].as_u64().unwrap()
        + video["width"].as_u64().unwrap() / 2) as usize
        * 4;
    if pixels.len() / frame_len != 25 {
        failures.push(format!(
            "decoded {} frames instead of 25",
            pixels.len() / frame_len
        ));
    }
    for (frame, red) in [(14, true), (15, false), (24, false)] {
        if (frame + 1) * frame_len > pixels.len() {
            continue;
        }
        let px = &pixels[frame * frame_len + center..frame * frame_len + center + 3];
        if if red {
            px[0] < 200 || px[2] > 30
        } else {
            px[2] < 200 || px[0] > 30
        } {
            failures.push(format!("frame {frame}: {px:?}"));
        }
    }
    let audio = run(Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-f", "f32le", "-ac", "1", "-ar", "48000", "-"]))
    .stdout;
    let samples: Vec<f32> = audio
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
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
    let switch = (26000..32000)
        .step_by(48)
        .find(|&n| power(n, 880.0) > power(n, 440.0))
        .unwrap();
    eprintln!(
        "QA tone switch {switch} samples = {:.3}s",
        switch as f64 / 48000.0
    );
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
    let result = export(
        &p,
        &d.join("cache"),
        &out,
        &options(),
        &cancel,
        |progress| {
            last = progress.frame;
            if progress.frame == 2 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    );
    assert!(result.is_err());
    assert!(format!("{:#}", result.unwrap_err()).contains("cancel"));
    assert!(
        now.elapsed().as_secs_f64() < 5.0,
        "cancellation took {:?}",
        now.elapsed()
    );
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
    let result = export(
        &p,
        &d.join("cache"),
        &input,
        &options(),
        &AtomicBool::new(false),
        |_| {},
    );
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
    let result = export(
        &p,
        &d.join("cache"),
        &out,
        &options(),
        &AtomicBool::new(false),
        |_| {},
    );
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
    let result = export(
        &p,
        &d.join("cache"),
        &out,
        &options(),
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(result.is_err());
    assert!(
        !out.with_extension("capopen-part.mp4").exists(),
        "complete temporary export remains after failed rename"
    );
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
        p.assets.push(Asset { id: id.into(), name: id.into(), path: d.join(format!("{id}.wav")).to_string_lossy().into(),
            kind: AssetKind::Audio, duration_us: 100_000, width: 0, height: 0, fps: 0.0, has_audio: true, rotation: 0 });
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
    if let ClipContent::Media { volume, .. } = &mut zero.clips[0].content { *volume = 0.0; }
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
