mod qa_support;
use capopen_engine::{
    audio::{Pcm, ensure_pcm, peaks},
    media::{extract_pcm, probe},
    model::*,
};
use qa_support::*;
use std::process::Command;

#[test]
fn pcm_rates_layouts_length_level_and_reference_samples() {
    if !available() {
        return;
    }
    let d = dir("pcm-matrix");
    let mut failures = Vec::new();
    for rate in [8000, 11025, 22050, 44100, 48000, 96000] {
        for channels in [1, 2, 6] {
            let source = d.join(format!("tone-{rate}-{channels}.wav"));
            let expr = std::iter::repeat_n("0.25*sin(2*PI*440*t)", channels).collect::<Vec<_>>().join("|");
            ff(&["-f", "lavfi", "-i", &format!("aevalsrc={expr}:s={rate}:d=1"), "-c:a", "pcm_f32le"], &source);
            let out = d.join(format!("tone-{rate}-{channels}.f32"));
            let frames = extract_pcm(&source, &out, |_| {}).unwrap();
            let samples = pcm(&out);
            let mut cmd = Command::new("ffmpeg");
            cmd.args(["-v", "error", "-i"]).arg(&source);
            if channels == 1 {
                cmd.args(["-af", "pan=stereo|c0=c0|c1=c0"]);
            }
            let reference = run(cmd.args(["-ar", "48000", "-ac", "2", "-f", "f32le", "-"])).stdout;
            let reference: Vec<f32> =
                reference.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
            let max_error = samples.iter().zip(&reference).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            let peak = samples.iter().copied().map(f32::abs).fold(0.0, f32::max);
            eprintln!("QA PCM {rate}/{channels}: {frames} frames, peak {peak:.6}, reference max error {max_error}");
            if (frames as i64 - 48000).abs() > 1024
                || (samples.len() as i64 - reference.len() as i64).abs() > 2048
                || max_error > 0.001
                || channels == 1 && (peak - 0.25).abs() > 0.005
            {
                failures.push(format!(
                    "{rate}/{channels}: frames={frames}, reference={}, peak={peak}, error={max_error}",
                    reference.len() / 2
                ));
            }
            assert!(!out.with_extension("part").exists());
            let p = Pcm::open(&out).unwrap();
            assert_eq!(p.frames(), frames as usize);
            assert!(!peaks(&p, 20).is_empty());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn aac_priming_and_nonzero_container_origin_preserve_alignment() {
    if !available() {
        return;
    }
    let d = dir("aac-origin");
    let mut failures = Vec::new();
    for offset in [0, 5] {
        let source = d.join(format!("offset-{offset}.m4a"));
        ff(
            &[
                "-f",
                "lavfi",
                "-i",
                "aevalsrc=if(lt(t\\,0.2)\\,0\\,0.25*sin(2*PI*440*t)):s=44100:d=1",
                "-c:a",
                "aac",
                "-output_ts_offset",
                &offset.to_string(),
            ],
            &source,
        );
        let out = d.join(format!("offset-{offset}.f32"));
        let n = extract_pcm(&source, &out, |_| {}).unwrap();
        let samples = pcm(&out);
        let onset = samples.chunks_exact(2).position(|s| s[0].abs() > 0.02).unwrap();
        // The container's origin can include the encoder priming packet at nonzero offset.
        let j = info(&source);
        let origin = j["format"]["start_time"].as_str().unwrap().parse::<f64>().unwrap();
        let expected = (0.2 + offset as f64 - origin) * 48000.0;
        let expected_end = (1.0 + offset as f64 - origin) * 48000.0;
        eprintln!(
            "QA AAC offset={offset} origin={origin} onset={onset}, expected={expected}, frames={n}, expected_end={expected_end}"
        );
        // One AAC frame at the SOURCE rate is 1115 output frames after 44.1 -> 48 kHz resampling.
        let aac_frame = (1024.0_f64 * 48000.0 / 44100.0).ceil();
        if (onset as f64 - expected).abs() > aac_frame || (n as f64 - expected_end).abs() > aac_frame {
            failures.push(format!("offset={offset}: onset={onset}/{expected}, frames={n}/{expected_end}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn late_audio_is_padded_to_video_container_origin() {
    if !available() {
        return;
    }
    let d = dir("late-audio");
    let source = d.join("late.mp4");
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:s=64x64:r=25:d=1.5",
            "-itsoffset",
            "0.5",
            "-f",
            "lavfi",
            "-i",
            "sine=f=880:r=48000:d=1",
            "-c:v",
            "libx264",
            "-threads",
            "2",
            "-c:a",
            "aac",
        ],
        &source,
    );
    let asset = probe(&source, "late-audio-qa".into()).unwrap();
    let cache = d.join("cache");
    let cached = capopen_engine::audio::pcm_path(&cache, &asset);
    if cached.exists() {
        std::fs::remove_file(&cached).unwrap();
    }
    let out = ensure_pcm(&cache, &asset, |_| {}).unwrap();
    let samples = pcm(&out);
    let onset = samples.chunks_exact(2).position(|s| s[0].abs() > 0.02).unwrap();
    eprintln!("QA late audio onset={onset}, frames={}", samples.len() / 2);
    assert!((onset as i64 - 24000).abs() <= 1024, "onset={onset}");
    assert!((samples.len() as i64 / 2 - 72000).abs() <= 1024, "length={}", samples.len() / 2);
    assert!(samples[..22000 * 2].iter().all(|v| v.abs() < 0.001));
}
#[test]
fn broken_audio_offset_is_rejected_without_writing_silence() {
    if !available() {
        return;
    }
    let d = dir("broken-lead");
    let source = d.join("broken.mkv");
    // The container reports 100 001 s, so its duration cannot bound the lead-in silence.
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:s=64x64:r=25:d=1.5",
            "-itsoffset",
            "100000",
            "-f",
            "lavfi",
            "-i",
            "sine=f=880:r=48000:d=1",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
        ],
        &source,
    );
    let out = d.join("broken.pcm");
    let _ = std::fs::remove_file(&out);
    let error = extract_pcm(&source, &out, |_| {}).unwrap_err();
    assert!(error.to_string().contains("timestamps look broken"), "{error}");
    assert!(!out.exists());
    let leftovers =
        std::fs::read_dir(&d).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".part"));
    assert_eq!(leftovers.count(), 0);
}
#[test]
fn microphone_switched_on_late_in_a_long_recording_is_kept() {
    if !available() {
        return;
    }
    let d = dir("late-microphone");
    let source = d.join("long.mkv");
    // 11 minutes of video with sound from 10:50, past the fallback limit but inside the video.
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:s=64x64:r=1:d=660",
            "-itsoffset",
            "650",
            "-f",
            "lavfi",
            "-i",
            "sine=f=880:r=48000:d=1",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
        ],
        &source,
    );
    let out = d.join("long.pcm");
    let frames = extract_pcm(&source, &out, |_| {}).unwrap();
    std::fs::remove_file(&out).unwrap();
    assert!((frames as i64 - 651 * 48_000).abs() <= 2048, "frames={frames}");
}
#[test]
fn mp3_attached_picture_remains_audio_asset() {
    if !available() {
        return;
    }
    let d = dir("mp3-cover");
    let cover = d.join("cover.jpg");
    let source = d.join("covered.mp3");
    ff(&["-f", "lavfi", "-i", "color=green:s=64x64", "-frames:v", "1"], &cover);
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:r=44100:d=1",
            "-i",
            cover.to_str().unwrap(),
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-c:a",
            "libmp3lame",
            "-c:v",
            "copy",
            "-id3v2_version",
            "3",
            "-disposition:v",
            "attached_pic",
        ],
        &source,
    );
    let a = probe(&source, "cover".into()).unwrap();
    assert_eq!(a.kind, AssetKind::Audio);
    assert_eq!((a.width, a.height), (0, 0));
    assert!(a.has_audio);
    assert!(capopen_engine::media::VideoDecoder::open(&source).is_err());
    let out = d.join("cover.f32");
    let n = extract_pcm(&source, &out, |_| {}).unwrap();
    assert!((n as i64 - 48000).abs() <= 1024, "frames={n}");
}
