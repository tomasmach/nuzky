//! Suggested zooms in the Transcript panel: punch-ins on the sentences said with emphasis, planned
//! and applied exactly as for agents (`analyze(kind: "emphasis")`, `apply_zooms`).

use anyhow::Result;
use nuzky_engine::edit::EditCmd;
use nuzky_mcp::{
    transcript,
    zooms::{self, WordZoom},
};
use nuzky_session::{Expect, host::Host};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::{AppState, CmdResult, Snapshot};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedZoom {
    /// Inclusive word numbers of the transcript view.
    from: usize,
    to: usize,
    start_us: i64,
    end_us: i64,
    text: String,
    score: f64,
    scale: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoomSuggestions {
    /// The transcript key the word numbers belong to; applying is refused once it changed.
    key: String,
    zooms: Vec<SuggestedZoom>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoomsApplied {
    snapshot: Snapshot,
    /// False when every clip in reach has keyframes, so nothing changed and there is nothing to undo.
    changed: bool,
    /// Clips with keyframes the zooms left alone.
    skipped: usize,
}

fn suggest(host: &Host) -> Result<ZoomSuggestions> {
    let project = host.session.state()?.project;
    let derived = transcript::derive(&project, &host.transcripts)?;
    let zooms = zooms::suggest(&project, &derived, &host.cache_dir)?
        .into_iter()
        .map(|z| SuggestedZoom {
            from: z.from,
            to: z.to,
            start_us: z.start_us,
            end_us: z.end_us,
            text: z.text,
            score: z.score,
            scale: z.scale,
        })
        .collect();
    Ok(ZoomSuggestions { key: transcript::word_key(&project, &derived.words), zooms })
}

/// Applies the zooms as one user edit. Returns whether the project changed and how many clips with
/// keyframes were left alone.
fn apply(host: &Host, key: &str, zooms: &[WordZoom]) -> Result<(bool, usize)> {
    let state = host.session.state()?;
    let derived = transcript::derive(&state.project, &host.transcripts)?;
    transcript::check_key(&state.project, &derived, key)?;
    let ranges = zooms::ranges(&state.project, &derived.words, zooms)?;
    let edited = host.session.edit(
        vec![EditCmd::ZoomRanges { ranges }],
        None,
        Expect { revision: None, speech_layout_key: Some(state.speech_layout_key) },
    )?;
    Ok((edited.stamp.revision != state.stamp.revision, edited.outcome.skipped.len()))
}

/// Agents read error codes; the panel says what happened.
fn explain(error: anyhow::Error) -> String {
    let text = format!("{error:#}");
    match text.split_once(':').map(|(code, _)| code) {
        Some("SPEECH_CHANGED") => "The transcript changed in the meantime. Suggest zooms again.".into(),
        Some("TRANSCRIPT_MISSING") => "Transcribe the remaining clips first.".into(),
        _ => text,
    }
}

#[tauri::command]
pub async fn suggest_zooms(app: AppHandle) -> CmdResult<ZoomSuggestions> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<AppState>().session.lock().unwrap().host.clone();
        suggest(&host).map_err(explain)
    })
    .await
    .map_err(crate::err)?
}

#[tauri::command]
pub async fn apply_zooms(
    app: AppHandle,
    key: String,
    zooms: Vec<WordZoom>,
    expected_epoch: Option<String>,
) -> CmdResult<ZoomsApplied> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let current = crate::lock_session(&state.session, expected_epoch.as_deref())?;
        let (changed, skipped) = apply(&current.host, &key, &zooms).map_err(explain)?;
        Ok(ZoomsApplied { snapshot: current.snapshot(Vec::new())?, changed, skipped })
    })
    .await
    .map_err(crate::err)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuzky_engine::{
        Project,
        audio::pcm_path,
        edit::new_id,
        model::{Asset, AssetKind, CHANNELS, ClipContent},
        speech,
    };
    use nuzky_session::{
        Mode, ProjectSession,
        transcripts::{Record, TranscriptStore, VERSION},
    };

    /// A 30 s talk of six sentences, the fourth said 6 dB louder, with its sound prepared.
    fn fixture() -> (std::path::PathBuf, Host) {
        let dir = std::env::temp_dir().join(format!("app-zooms-{}", new_id()));
        std::fs::create_dir_all(dir.join("cache/pcm")).unwrap();
        let path = dir.join("talk.mp4");
        std::fs::write(&path, "talk").unwrap();
        let asset = Asset {
            id: "talk".into(),
            name: "talk".into(),
            path: path.to_string_lossy().into(),
            kind: AssetKind::Video,
            duration_us: 30_000_000,
            width: 1080,
            height: 1920,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
        };
        let mut words = Vec::new();
        let mut samples = vec![0f32; 30 * 48_000 * CHANNELS];
        for sentence in 0..6 {
            let start = 1_000_000 + sentence * 5_000_000;
            for word in 0..4 {
                let at = start + word * 500_000;
                let last = word == 3;
                words.push(speech::Word {
                    start_us: at,
                    end_us: at + 400_000,
                    text: format!("w{sentence}{word}{}", if last { "." } else { "" }),
                    probability: 0.9,
                });
                let level = if sentence == 3 { 0.2 } else { 0.1 };
                let from = (at * 48 / 1000) as usize * CHANNELS;
                samples[from..from + 19_200 * CHANNELS].fill(level);
            }
        }
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_ne_bytes()).collect();
        std::fs::write(pcm_path(&dir.join("cache"), &asset), bytes).unwrap();
        let mut project = Project::new("zooms");
        project.apply(EditCmd::AddAssets { assets: vec![asset.clone()] }).unwrap();
        project.apply(EditCmd::AddClip { asset_id: "talk".into(), start_us: None, track_id: None }).unwrap();
        let file = dir.join("project.nuzky");
        std::fs::write(&file, serde_json::to_vec(&project).unwrap()).unwrap();
        let store = TranscriptStore::at(dir.join("transcripts")).unwrap();
        let record = Record {
            version: VERSION,
            fingerprint: store.fingerprint(&asset).unwrap(),
            duration_us: asset.duration_us,
            model: "fixture".into(),
            language: "cs".into(),
            words,
            segments: vec![],
        };
        store.put(&asset, &record).unwrap();
        let session = ProjectSession::open(&file, Mode::Write, None).unwrap();
        (dir.clone(), Host { session, jobs: Default::default(), transcripts: store, cache_dir: dir.join("cache") })
    }

    #[test]
    fn suggested_zooms_apply_as_one_undo_step_and_refuse_stale_words() {
        let (dir, host) = fixture();
        let before = host.session.state().unwrap().project;
        let found = suggest(&host).unwrap();
        // The hook and the loud fourth sentence; 30 s allows three, but the rest says nothing louder.
        let picked: Vec<_> = found.zooms.iter().map(|z| (z.from, z.to, z.scale)).collect();
        assert_eq!(picked, [(0, 3, 1.17), (12, 15, 1.24)]);
        let zooms: Vec<WordZoom> =
            found.zooms.iter().map(|z| WordZoom { from: z.from, to: z.to, scale: z.scale }).collect();
        assert_eq!(apply(&host, &found.key, &zooms).unwrap(), (true, 0));
        let scales: Vec<_> = host.session.state().unwrap().project.tracks[0]
            .clips
            .iter()
            .map(|c| {
                let ClipContent::Media { transform, .. } = &c.content else { panic!() };
                (c.start_us, transform.scale)
            })
            .collect();
        // Each zoom starts 150 ms before its first word and ends 150 ms after its last; the hook
        // takes the silence before it too, so no piece of the take is left without words.
        assert_eq!(scales, [(0, 1.17), (3_050_000, 1.0), (15_850_000, 1.24), (18_050_000, 1.0)]);
        assert!(explain(apply(&host, &found.key, &zooms).unwrap_err()).starts_with("The transcript changed"));
        host.session.undo().unwrap();
        assert_eq!(host.session.state().unwrap().project, before);
        drop(host);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
