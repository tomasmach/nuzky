mod qa_support;
use nuzky_engine::{
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
            let frames = extract_pcm(&source, &out, |_| Ok(())).unwrap();
            let samples = pcm(&out);
            let mut cmd = Command::new("ffmpeg");
            cmd.args(["-v", "error", "-i"]).arg(&source);
            if channels == 1 {
                cmd.args(["-af", "pan=stereo|c0=c0|c1=c0"]);
            }
            let reference = run(cmd.args(["-ar", "48000", "-ac", "2", "-f", "f32le", "-"])).stdout;
            let reference: Vec<f32> = reference.as_chunks::<4>().0.iter().map(|&b| f32::from_le_bytes(b)).collect();
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
        let n = extract_pcm(&source, &out, |_| Ok(())).unwrap();
        let samples = pcm(&out);
        let onset = samples.as_chunks::<2>().0.iter().position(|s| s[0].abs() > 0.02).unwrap();
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
    let cached = nuzky_engine::audio::pcm_path(&cache, &asset);
    if cached.exists() {
        std::fs::remove_file(&cached).unwrap();
    }
    let out = ensure_pcm(&cache, &asset, |_| Ok(())).unwrap();
    let samples = pcm(&out);
    let onset = samples.as_chunks::<2>().0.iter().position(|s| s[0].abs() > 0.02).unwrap();
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
    let error = extract_pcm(&source, &out, |_| Ok(())).unwrap_err();
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
    // 11 minutes of video with sound from 10:50, past the fallback limit but inside the video,
    // stored a second ahead of the video like many muxers do.
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:s=64x64:r=25:d=660",
            "-itsoffset",
            "650.005",
            "-f",
            "lavfi",
            "-i",
            "sine=f=880:r=48000:d=1",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-audio_preload",
            "1000000",
        ],
        &source,
    );
    let out = d.join("long.pcm");
    let frames = extract_pcm(&source, &out, |_| Ok(())).unwrap();
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
    assert!(nuzky_engine::media::VideoDecoder::open(&source).is_err());
    let out = d.join("cover.f32");
    let n = extract_pcm(&source, &out, |_| Ok(())).unwrap();
    assert!((n as i64 - 48000).abs() <= 1024, "frames={n}");
}

/// A long phone recording (AAC, 44.1 kHz mono, so it is resampled and doubled to stereo): the waveform
/// of its start can be read while the rest still decodes, and it is the waveform of the finished cache.
/// A cache extracted before peaks were written gets the same peaks from its samples.
#[test]
fn waveform_of_the_start_of_a_long_recording_comes_while_it_decodes() {
    use nuzky_engine::audio::{pcm_path, waveform_peaks};
    use std::time::{Duration, Instant};
    if !available() {
        return;
    }
    let d = dir("waveform-peaks");
    let source = d.join("long.m4a");
    let speech = "volume='if(gt(sin(2*PI*0.11*t)+0.3,0),0.2+0.6*abs(sin(2*PI*0.37*t)),0)':eval=frame";
    ff(&["-f", "lavfi", "-i", "sine=frequency=180:sample_rate=44100:duration=900", "-af", speech, "-ac", "1"], &source);
    let asset = probe(&source, "long".into()).unwrap();
    let cache = d.join("cache");
    std::fs::remove_dir_all(&cache).ok();
    let worker = std::thread::spawn({
        let (cache, asset) = (cache.clone(), asset.clone());
        move || ensure_pcm(&cache, &asset, |_| Ok(())).unwrap()
    });
    // The first 30 s, as the timeline asks for them right after the import.
    let started = Instant::now();
    let (early, complete) = loop {
        let (peaks, complete) = waveform_peaks(&cache, &asset, 0, 1500).unwrap();
        if peaks.len() == 1500 || worker.is_finished() {
            break (peaks, complete);
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let shown = started.elapsed();
    let pcm = worker.join().unwrap();
    let decoded = started.elapsed();
    eprintln!("QA waveform: first 30 s after {shown:?}, whole 15 min decoded after {decoded:?}");
    assert!(!complete, "the start was read only once the whole file was decoded ({decoded:?})");
    let reference = peaks(&Pcm::open(&pcm).unwrap(), 50);
    assert_eq!(early, reference[..1500], "the start read during decoding differs from the finished cache");
    assert_eq!(waveform_peaks(&cache, &asset, 0, usize::MAX).unwrap(), (reference.clone(), true));
    assert_eq!(pcm, pcm_path(&cache, &asset));
    // Without peaks next to the cache, the asked part is measured from its samples.
    for entry in std::fs::read_dir(pcm.parent().unwrap()).unwrap().flatten() {
        if entry.path().extension().is_some_and(|e| e == "peaks") {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }
    let middle = waveform_peaks(&cache, &asset, 22_000, 23_600).unwrap();
    assert_eq!(middle, (reference[22_000..23_600].to_vec(), true));
    let end = waveform_peaks(&cache, &asset, reference.len() - 10, reference.len() + 500).unwrap();
    assert_eq!(end, (reference[reference.len() - 10..].to_vec(), true));
}

/// Level in dBFS of the `hz` tone in `samples` (one channel of 48 kHz stereo) from `at` for `len` seconds,
/// through a Hann window, so speech next to it in the spectrum does not count.
fn tone_db(samples: &[f32], hz: f64, at: f64, len: f64) -> f64 {
    let (from, n) = ((at * 48_000.0) as usize, (len * 48_000.0) as usize);
    let (mut re, mut im, mut weight) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
        let x = samples[(from + i) * 2] as f64 * w;
        let phase = 2.0 * std::f64::consts::PI * hz * (from + i) as f64 / 48_000.0;
        (re, im, weight) = (re + x * phase.cos(), im + x * phase.sin(), weight + w);
    }
    20.0 * (2.0 * (re * re + im * im).sqrt() / weight).log10()
}

/// Music on an audio track with ducking goes down by its `duck_db` while the talking head speaks and comes
/// back in the pause, smoothly, the same in playback-sized buffers as in one go, and in the Reels export
/// levelled to -14 LUFS. Speech is a voice-like sound in 0.5–2.5 s and 5–7 s; the music is a 5 kHz tone.
#[test]
fn music_ducks_under_speech_and_comes_back_in_the_pause() {
    use nuzky_engine::audio::Mixer;
    use nuzky_engine::export::{Delivery, ExportOptions, export};
    use std::sync::atomic::AtomicBool;
    if !available() {
        return;
    }
    let d = dir("ducking");
    let (talk, music) = (d.join("talk.mkv"), d.join("music.wav"));
    let voice = "(sin(2*PI*130*t)+0.7*sin(2*PI*260*t)+0.5*sin(2*PI*390*t)+0.35*sin(2*PI*700*t))\
        *pow(max(0\\,sin(2*PI*3.3*t))\\,4)*(between(t\\,0.5\\,2.5)+between(t\\,5\\,7))*0.15";
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0x2b3a4a:s=108x192:r=30:d=8",
            "-f",
            "lavfi",
            "-i",
            &format!("aevalsrc={voice}|{voice}:s=48000:d=8"),
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=color=pink:amplitude=0.004:seed=1:sample_rate=48000:duration=8",
            "-filter_complex",
            "[1:a][2:a]amix=inputs=2:normalize=0[a]",
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
        &talk,
    );
    ff(&["-f", "lavfi", "-i", "sine=f=5000:r=48000:d=8", "-c:a", "pcm_f32le"], &music);
    let mut p = project(&talk, 8_000_000);
    let song = probe(&music, "qa-music".into()).unwrap();
    p.tracks.push(Track {
        id: "music".into(),
        kind: TrackKind::Audio,
        name: "Music".into(),
        muted: false,
        hidden: false,
        keep_in_place: true,
        clips: vec![clip("qa-song", &song.id, 0, 8_000_000)],
    });
    p.assets.push(song.clone());
    let cache = d.join("cache");
    for asset in &p.assets {
        ensure_pcm(&cache, asset, |_| Ok(())).unwrap();
    }
    let mix = |p: &Project, chunk: usize| {
        let mut out = vec![0.0; 8 * 48_000 * CHANNELS];
        let mut mixer = Mixer::new(cache.clone());
        for (index, part) in out.chunks_mut(chunk * CHANNELS).enumerate() {
            mixer.mix(p, (index * chunk) as i64, part);
        }
        out
    };
    let plain = mix(&p, 8 * 48_000);
    let ClipContent::Media { duck_db, .. } = &mut p.tracks[1].clips[0].content else { unreachable!() };
    *duck_db = 12.0;
    let ducked = mix(&p, 8 * 48_000);
    assert!(mix(&p, 1024) == ducked, "playback-sized buffers mix differently from one buffer");

    // The music's own gain at every sample, where the tone is far enough from a zero crossing to read it.
    let tone = pcm(&ensure_pcm(&cache, &song, |_| Ok(())).unwrap());
    let gain: Vec<Option<f32>> = (0..8 * 48_000)
        .map(|i| (tone[i * 2].abs() > 0.05).then(|| 1.0 + (ducked[i * 2] - plain[i * 2]) / tone[i * 2]))
        .collect();
    let at = |t: f64| (t * 48_000.0) as usize;
    let near = |range: std::ops::Range<usize>, want: f32| gain[range].iter().flatten().all(|g| (g - want).abs() < 1e-4);
    // Read across zero crossings too: the tone crosses zero where the 10 ms speech blocks start.
    let read: Vec<f32> = gain[at(0.01)..at(7.99)].iter().flatten().copied().collect();
    let step = read.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0f32, f32::max);
    let down = 10f32.powf(-12.0 / 20.0);
    eprintln!("QA ducking: largest gain step between samples {step:.6}");
    assert!(near(at(0.8)..at(2.3), down) && near(at(5.3)..at(6.8), down), "the music is not 12 dB down under speech");
    assert!(near(at(0.01)..at(0.3), 1.0) && near(at(3.4)..at(4.8), 1.0), "the music is not back in the pause");
    // 12 dB over the 150 ms attack moves the gain by 0.0002 a sample, a few samples apart at most where
    // the tone is too near zero to read; a click would jump.
    assert!(step < 0.002, "the gain jumps by {step} between two samples");

    let out = d.join("ducked-reel.mp4");
    let options = ExportOptions { delivery: Some(Delivery::Reels), replace_existing: true, ..ExportOptions::default() };
    export(&p, &cache, &out, &options, &AtomicBool::new(false), |_| {}).unwrap();
    let decoded = run(Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-vn", "-f", "f32le", "-ac", "2", "-ar", "48000", "-"]))
    .stdout;
    let decoded: Vec<f32> = decoded.as_chunks::<4>().0.iter().map(|&b| f32::from_le_bytes(b)).collect();
    let speech = [1.4, 5.9].map(|t| tone_db(&decoded, 5000.0, t, 0.2));
    let pause = tone_db(&decoded, 5000.0, 3.7, 0.2);
    eprintln!("QA ducking export: music {speech:.2?} dBFS under speech, {pause:.2} dBFS in the pause");
    for level in speech {
        assert!((level - pause + 12.0).abs() < 1.0, "the exported music is {:.2} dB under speech", level - pause);
    }
}

/// How far one channel of 48 kHz stereo strays from the steadiest `hz` tone in each 20 ms from `at` to
/// `to` seconds: the largest error left after fitting a sine, against the level of the sound.
fn tone_error(samples: &[f32], hz: f64, at: f64, to: f64) -> f64 {
    let window = 960;
    let mut worst = 0.0f64;
    for from in ((at * 48_000.0) as usize..(to * 48_000.0) as usize - window).step_by(window) {
        let (mut ss, mut sc, mut cc, mut ys, mut yc, mut yy) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for i in from..from + window {
            let phase = std::f64::consts::TAU * hz * i as f64 / 48_000.0;
            let (s, c, y) = (phase.sin(), phase.cos(), samples[i * 2] as f64);
            (ss, sc, cc, ys, yc, yy) = (ss + s * s, sc + s * c, cc + c * c, ys + y * s, yc + y * c, yy + y * y);
        }
        let det = ss * cc - sc * sc;
        let (a, b) = ((ys * cc - yc * sc) / det, (yc * ss - ys * sc) / det);
        let fitted = a * ys + b * yc;
        worst = worst.max(((yy - fitted).max(0.0) / yy.max(1e-12)).sqrt());
    }
    worst
}

/// A 440 Hz tone at 1.25x with Keep pitch still sounds at 440 Hz, plays 4 s of source in 3.2 s, holds its level
/// across the pieces of the stretch and across a split, mixes the same in playback-sized buffers as in one go,
/// and exports so. Without Keep pitch it rises to 550 Hz as it always did.
#[test]
fn speed_keeps_the_pitch_and_the_length() {
    use nuzky_engine::audio::Mixer;
    use nuzky_engine::edit::EditCmd;
    use nuzky_engine::export::{ExportOptions, export};
    use std::sync::atomic::AtomicBool;
    if !available() {
        return;
    }
    let d = dir("keep-pitch");
    let source = d.join("tone.mkv");
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0x2b3a4a:s=108x192:r=30:d=4",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=0.25*sin(2*PI*440*t)|0.25*sin(2*PI*440*t):s=48000:d=4",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "pcm_f32le",
        ],
        &source,
    );
    let mut p = project(&source, 4_000_000);
    let cache = d.join("cache");
    ensure_pcm(&cache, &p.assets[0], |_| Ok(())).unwrap();
    let speed = serde_json::json!({"type": "updateClip", "clipId": "qa-clip", "speed": 1.25});
    p.apply(serde_json::from_value::<EditCmd>(speed).unwrap()).unwrap();
    let ClipContent::Media { keep_pitch, .. } = p.tracks[0].clips[0].content else { unreachable!() };
    assert!(keep_pitch, "moving the clip off 1x does not turn Keep pitch on");
    assert_eq!(p.tracks[0].clips[0].duration_us, 3_200_000);

    let mix = |p: &Project, chunk: usize| {
        let mut out = vec![0.0; 4 * 48_000 * CHANNELS];
        let mut mixer = Mixer::new(cache.clone());
        for (index, part) in out.chunks_mut(chunk * CHANNELS).enumerate() {
            mixer.mix(p, (index * chunk) as i64, part);
        }
        out
    };
    let rms = |s: &[f32], at: f64, to: f64| {
        let part = &s[(at * 48_000.0) as usize * 2..(to * 48_000.0) as usize * 2];
        (part.iter().map(|x| x * x).sum::<f32>() / part.len() as f32).sqrt()
    };
    let started = std::time::Instant::now();
    let kept = mix(&p, 4 * 48_000);
    eprintln!("QA keep pitch: 3.2 s stretched in {:?}", started.elapsed());
    assert!(mix(&p, 1024) == kept, "playback-sized buffers mix differently from one buffer");
    let (at_440, at_550) = (tone_db(&kept, 440.0, 1.0, 1.0), tone_db(&kept, 550.0, 1.0, 1.0));
    let error = tone_error(&kept, 440.0, 0.05, 3.15);
    let (level, after) = (rms(&kept, 0.05, 3.15), rms(&kept, 3.21, 4.0));
    eprintln!("QA keep pitch: 440 Hz {at_440:.2} dBFS, 550 Hz {at_550:.2} dBFS, error {error:.4}, level {level:.4}");
    assert!((at_440 + 12.04).abs() < 0.3 && at_550 < -50.0, "the tone moved from 440 Hz");
    assert!(error < 0.03, "the stretched tone strays {error:.4} from a steady sine");
    assert!((level - 0.25 / 2f32.sqrt()).abs() < 0.004 && after < 1e-4, "the sound is not 3.2 s long at its level");

    let split = serde_json::json!({"type": "splitClip", "clipId": "qa-clip", "atUs": 1_234_567});
    let mut halves = p.clone();
    halves.apply(serde_json::from_value::<EditCmd>(split).unwrap()).unwrap();
    let joined = mix(&halves, 1024);
    let step = joined.iter().zip(&kept).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(step < 1e-4, "a split changes the stretched sound by {step}");

    let out = d.join("kept.mp4");
    let options = ExportOptions { replace_existing: true, ..ExportOptions::default() };
    export(&p, &cache, &out, &options, &AtomicBool::new(false), |_| {}).unwrap();
    let decoded = run(Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-vn", "-f", "f32le", "-ac", "2", "-ar", "48000", "-"]))
    .stdout;
    let decoded: Vec<f32> = decoded.as_chunks::<4>().0.iter().map(|&b| f32::from_le_bytes(b)).collect();
    let length = decoded.len() as f64 / 2.0 / 48_000.0;
    let (at_440, at_550) = (tone_db(&decoded, 440.0, 1.0, 1.0), tone_db(&decoded, 550.0, 1.0, 1.0));
    eprintln!("QA keep pitch export: {length:.3} s, 440 Hz {at_440:.2} dBFS, 550 Hz {at_550:.2} dBFS");
    assert!((length - 3.2).abs() < 0.05, "the export is {length:.3} s long");
    assert!((at_440 + 12.04).abs() < 0.5 && at_550 < -40.0, "the exported tone moved from 440 Hz");

    let ClipContent::Media { keep_pitch, .. } = &mut p.tracks[0].clips[0].content else { unreachable!() };
    *keep_pitch = false;
    let raised = mix(&p, 1024);
    let (at_440, at_550) = (tone_db(&raised, 440.0, 1.0, 1.0), tone_db(&raised, 550.0, 1.0, 1.0));
    assert!(at_440 < -50.0 && (at_550 + 12.04).abs() < 0.3, "without Keep pitch the tone is not at 550 Hz");
}

/// Keep pitch plays only the sound the clip keeps. A clip of the silent second between two tones, slowed to 0.1x
/// or sped up, stays silent: its pieces reach past where speed puts them but not into the cut-away tones, also
/// through a split right after its start.
#[test]
fn keep_pitch_does_not_bring_back_cut_sound() {
    use nuzky_engine::audio::Mixer;
    use nuzky_engine::edit::EditCmd;
    if !available() {
        return;
    }
    let d = dir("keep-pitch-cut");
    let source = d.join("tones.wav");
    let tone = "0.25*sin(2*PI*440*t)*(lt(t\\,1)+gte(t\\,2))";
    ff(&["-f", "lavfi", "-i", &format!("aevalsrc={tone}|{tone}:s=48000:d=3"), "-c:a", "pcm_f32le"], &source);
    let cache = d.join("cache");
    for speed in [0.1, 1.25, 2.0] {
        let asset = probe(&source, "qa-tones".into()).unwrap();
        ensure_pcm(&cache, &asset, |_| Ok(())).unwrap();
        let mut p = Project::new("QA");
        let mut gap = clip("qa-gap", &asset.id, 0, 1_000_000);
        let ClipContent::Media { source_in_us, .. } = &mut gap.content else { unreachable!() };
        *source_in_us = 1_000_000;
        p.tracks[0].clips.push(gap);
        p.assets.push(asset);
        let apply =
            |p: &mut Project, cmd: serde_json::Value| p.apply(serde_json::from_value::<EditCmd>(cmd).unwrap()).unwrap();
        apply(&mut p, serde_json::json!({"type": "updateClip", "clipId": "qa-gap", "speed": speed}));
        apply(&mut p, serde_json::json!({"type": "splitClip", "clipId": "qa-gap", "atUs": 50_000}));
        let length = p.tracks[0].clips.iter().map(|c| c.end_us()).max().unwrap() * 48 / 1000;
        let mut out = vec![0.0; length as usize * CHANNELS];
        let mut mixer = Mixer::new(cache.clone());
        for (index, part) in out.chunks_mut(1024 * CHANNELS).enumerate() {
            mixer.mix(&p, index as i64 * 1024, part);
        }
        let loudest = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        eprintln!("QA keep pitch at a cut, {speed}x: loudest sample {loudest:.6}");
        assert!(loudest < 0.001, "{speed}x brings back cut-away sound at {loudest}");
    }
}
