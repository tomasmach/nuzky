//! Clean voice end to end in the engine: a clip with the setting plays and exports the cleaned
//! cache of its file, sample for sample where the raw cache would be, and plays the raw cache while
//! the cleaned one is not ready or the setting is off.
mod qa_support;
use nuzky_engine::{
    audio::{Mixer, ensure_pcm, pcm_path},
    edit::{EditCmd, TimeRange},
    export::{ExportOptions, export},
    media::probe,
    model::*,
    voice::{ensure_voice_pcm, voice_pcm_path},
};
use qa_support::*;
use std::{path::Path, process::Command, sync::atomic::AtomicBool};

const SECOND: usize = SAMPLE_RATE as usize;

/// A tone over loud white noise and 50 Hz hum, so the cleaned sound clearly differs from the raw.
fn noisy_source(path: &Path) {
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=gray:s=64x64:r=25:d=3",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:r=48000:d=3",
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=color=white:amplitude=0.05:seed=7:r=48000:d=3",
            "-f",
            "lavfi",
            "-i",
            "sine=f=50:r=48000:d=3",
            "-filter_complex",
            "[1:a]volume=0.4[t];[3:a]volume=0.2[h];[t][2:a][h]amix=inputs=3:normalize=0[a]",
            "-map",
            "0:v",
            "-map",
            "[a]",
            "-c:v",
            "libx264",
            "-c:a",
            "pcm_s16le",
        ],
        path,
    );
}

/// The decoded sound of an exported file, 48 kHz stereo.
fn decode(path: &Path) -> Vec<f32> {
    let out = run(Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-f", "f32le", "-ac", "2", "-ar", "48000", "-"]));
    out.stdout.as_chunks::<4>().0.iter().map(|&b| f32::from_le_bytes(b)).collect()
}

/// Level in dB of the left channel, and of 50 Hz hum and the 440 Hz tone in it: what AAC keeps,
/// unlike the exact waveform of noise.
fn levels(sound: &[f32]) -> [f64; 3] {
    let left: Vec<f64> = sound.iter().step_by(CHANNELS).map(|&s| s as f64).collect();
    let tone = |hz: f64| {
        let (re, im) = left.iter().enumerate().fold((0.0, 0.0), |(re, im), (n, &s)| {
            let a = std::f64::consts::TAU * hz * n as f64 / SAMPLE_RATE as f64;
            (re + s * a.cos(), im + s * a.sin())
        });
        20.0 * ((re * re + im * im).sqrt() * 2.0 / left.len() as f64).log10()
    };
    let rms = 10.0 * (left.iter().map(|s| s * s).sum::<f64>() / left.len() as f64).log10();
    [rms, tone(50.0), tone(440.0)]
}

#[test]
fn clean_voice_plays_and_exports_the_cleaned_cache_and_raw_without_it() {
    if !available() {
        return;
    }
    let d = dir(&format!("voice-{}", nuzky_engine::edit::new_id()));
    let source = d.join("take.mkv");
    noisy_source(&source);
    let cache = d.join("cache");
    let asset = probe(&source, "take".into()).unwrap();
    // A project from before Clean voice opens with it off.
    let old: ClipContent =
        serde_json::from_str(r#"{"type":"media","assetId":"take","sourceInUs":0,"volume":1.0}"#).unwrap();
    assert!(matches!(old, ClipContent::Media { clean_voice: false, .. }));

    let mut p = Project::new("voice");
    p.canvas.width = 64;
    p.canvas.height = 64;
    p.canvas.fps = 25;
    p.apply(EditCmd::AddAssets { assets: vec![asset.clone()] }).unwrap();
    p.apply(EditCmd::AddClip { asset_id: "take".into(), start_us: None, track_id: None }).unwrap();
    // A cut: 1.0 to 1.5 s of the take goes, so the second clip plays source 1.5 s from 1.0 s on.
    let cut = TimeRange { start_us: 1_000_000, end_us: 1_500_000 };
    p.apply(EditCmd::RippleDeleteRanges { ranges: vec![cut], keep_track_ids: None }).unwrap();
    let mut clean = p.clone();
    for id in clean.tracks[0].clips.iter().map(|c| c.id.clone()).collect::<Vec<_>>() {
        let json = serde_json::json!({"type": "updateClip", "clipId": id, "cleanVoice": true});
        clean.apply(serde_json::from_value(json).unwrap()).unwrap();
    }
    assert!(clean.tracks[0].clips.iter().all(|c| matches!(c.content, ClipContent::Media { clean_voice: true, .. })));

    let mix = |project: &Project, from: usize, frames: usize| {
        let mut out = vec![0.0; frames * CHANNELS];
        Mixer::new(cache.clone()).mix(project, from as i64, &mut out);
        out
    };
    let raw_path = ensure_pcm(&cache, &asset, |_| Ok(())).unwrap();
    let raw = pcm(&raw_path);
    let at = |cache: &[f32], from: usize, frames: usize| cache[from * CHANNELS..(from + frames) * CHANNELS].to_vec();
    // Before the cleaned cache exists, a clip with Clean voice plays the original.
    assert!(!voice_pcm_path(&cache, &asset).exists());
    assert!(mix(&clean, SECOND / 2, 960) == at(&raw, SECOND / 2, 960), "silence while the cleaned sound is prepared");

    let voice_path = ensure_voice_pcm(&cache, &asset, |_| Ok(())).unwrap();
    assert_eq!(voice_path, voice_pcm_path(&cache, &asset));
    assert_eq!(voice_path.parent(), Some(cache.join("voice").as_path()));
    let voice = pcm(&voice_path);
    assert_eq!(voice.len(), raw.len(), "the cleaned sound is as long as the raw");
    assert_eq!(pcm(&pcm_path(&cache, &asset)), raw, "the raw cache is left as it was");
    let (cleaned, original) = (levels(&at(&voice, SECOND / 2, SECOND / 2)), levels(&at(&raw, SECOND / 2, SECOND / 2)));
    eprintln!("QA voice: level, 50 Hz, 440 Hz in dB: raw {original:.1?}, cleaned {cleaned:.1?}");
    assert!(cleaned[1] < original[1] - 20.0, "the hum did not drop");

    // Playback: each side of the cut reads its own source position of the cache the setting picks.
    for (name, project, expected) in [("on", &clean, &voice), ("off", &p, &raw)] {
        assert!(mix(project, SECOND / 2, 960) == at(expected, SECOND / 2, 960), "playback {name} before the cut");
        assert!(mix(project, SECOND * 3 / 2, 960) == at(expected, SECOND * 2, 960), "playback {name} after the cut");
    }

    // Export: the decoded file has the levels of the cache the setting picks on both sides of the cut,
    // past its 20 ms crossfade, and not those of the other cache.
    let options = ExportOptions { preset: "ultrafast".into(), replace_existing: true, ..ExportOptions::default() };
    let side = SECOND * 2 / 5;
    let around =
        |sound: &[f32]| [levels(&at(sound, SECOND - 500 - side, side)), levels(&at(sound, SECOND + 500, side))];
    let expected =
        |cache: &[f32]| [levels(&at(cache, SECOND - 500 - side, side)), levels(&at(cache, SECOND * 3 / 2 + 500, side))];
    // The export cleans the voice itself when the app has not yet.
    std::fs::remove_dir_all(cache.join("voice")).unwrap();
    for (name, project, own, other) in [("on", &clean, &voice, &raw), ("off", &p, &raw, &voice)] {
        let out = d.join(format!("{name}.mp4"));
        export(project, &cache, &out, &options, &AtomicBool::new(false), |_| {}).unwrap();
        assert!(voice_path.exists(), "export {name} left the voice uncleaned");
        assert!(pcm(&voice_path) == voice, "cleaning again gave another sound");
        let decoded = around(&decode(&out));
        for (i, place) in ["before", "after"].into_iter().enumerate() {
            let (got, own, other) = (decoded[i], expected(own)[i], expected(other)[i]);
            eprintln!("QA voice: export {name} {place} the cut {got:.1?}, its cache {own:.1?}, the other {other:.1?}");
            for k in 0..3 {
                assert!((got[k] - own[k]).abs() < 1.0, "export {name} {place} the cut: {got:.1?} against {own:.1?}");
            }
            assert!((got[1] - other[1]).abs() > 10.0, "export {name} {place} the cut has the other cache's hum");
        }
    }
    std::fs::remove_dir_all(d).unwrap();
}

/// Resident memory in kB: anonymous (heap) and file-backed (the mapped caches).
fn resident() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status.lines().find_map(|l| l.strip_prefix(name)).and_then(|v| v.split_whitespace().next()?.parse().ok())
    };
    (field("RssAnon:").unwrap_or(0), field("RssFile:").unwrap_or(0))
}

/// How long cleaning a four-minute phone recording takes and how much memory it holds, in stereo and in
/// mono stored as two equal channels.
#[test]
#[ignore = "four minutes of sound; run when the cleaning chain changes"]
fn four_minutes_clean_in_seconds_with_little_memory() {
    if !available() {
        return;
    }
    let d = dir("voice-four-minutes");
    for layout in ["stereo", "mono"] {
        let source = d.join(format!("{layout}.m4a"));
        if !source.exists() {
            let mix = format!("[0:a][1:a]amix=inputs=2,aformat=channel_layouts={layout}");
            let noise = "anoisesrc=color=pink:amplitude=0.1:seed=3:r=48000:d=240";
            ff(
                &["-f", "lavfi", "-i", noise, "-f", "lavfi", "-i", "sine=f=180:r=48000:d=240"]
                    .into_iter()
                    .chain(["-filter_complex", &mix, "-c:a", "aac"])
                    .collect::<Vec<_>>(),
                &source,
            );
        }
        let cache = d.join(format!("cache-{layout}"));
        let asset = probe(&source, layout.into()).unwrap();
        ensure_pcm(&cache, &asset, |_| Ok(())).unwrap();
        std::fs::remove_dir_all(cache.join("voice")).ok();
        let before = resident();
        let mut peak = before;
        let started = std::time::Instant::now();
        ensure_voice_pcm(&cache, &asset, |_| {
            let now = resident();
            peak = (peak.0.max(now.0), peak.1.max(now.1));
            Ok(())
        })
        .unwrap();
        let seconds = started.elapsed().as_secs_f64();
        eprintln!(
            "QA voice: 240 s {layout} cleaned in {seconds:.1} s ({:.0}x real time); heap +{} kB at most, mapped files {} MB at most",
            240.0 / seconds,
            peak.0.saturating_sub(before.0),
            peak.1 / 1024
        );
        assert!(seconds < 60.0, "cleaning took {seconds:.1} s");
    }
}
