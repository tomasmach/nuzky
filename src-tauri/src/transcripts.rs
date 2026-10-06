//! The Transcript panel. Words belong to media files and are placed through the clips that play
//! them, so every edit keeps them right; cuts are planned exactly as for agents.

use anyhow::{Context, Result, ensure};
use capopen_engine::edit::TimeRange;
use capopen_mcp::transcript::{self, Pause};
use capopen_session::{Expect, host::Host};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::{AppState, CmdResult, Snapshot};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    i: usize,
    start_us: i64,
    end_us: i64,
    text: String,
    p: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptView {
    /// Sent back with a cut, which is refused once the speech moved or was recognised again.
    key: String,
    words: Vec<Word>,
    pauses: Vec<Pause>,
    /// Heard media without a transcript yet.
    untranscribed: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptCut {
    snapshot: Snapshot,
    /// Where the first cut starts, which is now where what followed it plays.
    start_us: i64,
    removed_us: i64,
}

enum Target {
    /// Inclusive word index ranges.
    Words(Vec<[usize; 2]>),
    /// The pauses of the view with this pause length, by index, or all of them.
    Pauses { pause_us: i64, only: Option<Vec<usize>> },
}

fn view(host: &Host, pause_us: i64) -> Result<TranscriptView> {
    let project = host.session.state()?.project;
    let derived = transcript::derive(&project, &host.transcripts)?;
    // Without words, or with voices overlapping so that no pause can be cut, there are none to show.
    let pauses = transcript::pauses(&project, &derived, pause_us).unwrap_or_default();
    Ok(TranscriptView {
        key: transcript::word_key(&project, &derived.words),
        words: derived
            .words
            .iter()
            .enumerate()
            .map(|(i, w)| Word { i, start_us: w.start_us, end_us: w.end_us, text: w.text.clone(), p: w.probability })
            .collect(),
        pauses,
        untranscribed: derived.untranscribed,
    })
}

/// Applies the cut as one user edit. Returns the clips to select, where the cut starts and how much it removed.
fn cut(host: &Host, key: &str, target: Target) -> Result<(Vec<String>, i64, i64)> {
    let state = host.session.state()?;
    let derived = transcript::derive(&state.project, &host.transcripts)?;
    transcript::check_key(&state.project, &derived, key)?;
    let ranges = match target {
        Target::Words(delete) => {
            transcript::edit_ranges(&state.project, &derived, Some(delete.as_slice()), None, None)?
        }
        Target::Pauses { pause_us, only } => {
            let pauses = transcript::pauses(&state.project, &derived, pause_us)?;
            let picked = match only {
                Some(only) => only
                    .iter()
                    .map(|&i| pauses.get(i).context("INVALID_PAUSE: no such pause"))
                    .collect::<Result<Vec<_>>>()?,
                None => pauses.iter().collect(),
            };
            picked.into_iter().map(|p| TimeRange { start_us: p.start_us, end_us: p.end_us }).collect()
        }
    };
    ensure!(!ranges.is_empty(), "Nothing to cut");
    let plan = transcript::plan_cut(&state.project, &derived, ranges)?;
    let edited = host.session.edit(
        vec![plan.edit],
        None,
        Expect { revision: None, speech_layout_key: Some(state.speech_layout_key) },
    )?;
    Ok((edited.outcome.select, plan.ranges[0].start_us, state.project.duration_us() - plan.preview.duration_us()))
}

/// Agents read error codes; the panel says what happened.
fn explain(error: anyhow::Error) -> String {
    let text = format!("{error:#}");
    match text.split_once(':').map(|(code, _)| code) {
        Some("SPEECH_CHANGED") => "The transcript changed in the meantime. Check the words and try again.".into(),
        Some("TRANSCRIPT_MISSING") => "Transcribe the remaining clips first.".into(),
        Some("OVERLAPPING_SPEECH" | "UNSAFE_CUT") => {
            "These words overlap other speech, so they cannot be cut on their own.".into()
        }
        _ => text,
    }
}

#[tauri::command]
pub async fn transcript_view(app: AppHandle, pause_us: i64) -> CmdResult<TranscriptView> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<AppState>().session.lock().unwrap().host.clone();
        view(&host, pause_us).map_err(explain)
    })
    .await
    .map_err(crate::err)?
}

#[tauri::command]
pub async fn cut_words(
    app: AppHandle,
    key: String,
    delete: Vec<[usize; 2]>,
    expected_epoch: Option<String>,
) -> CmdResult<TranscriptCut> {
    apply_cut(app, key, Target::Words(delete), expected_epoch).await
}

#[tauri::command]
pub async fn remove_pauses(
    app: AppHandle,
    key: String,
    pause_us: i64,
    only: Option<Vec<usize>>,
    expected_epoch: Option<String>,
) -> CmdResult<TranscriptCut> {
    apply_cut(app, key, Target::Pauses { pause_us, only }, expected_epoch).await
}

async fn apply_cut(
    app: AppHandle,
    key: String,
    target: Target,
    expected_epoch: Option<String>,
) -> CmdResult<TranscriptCut> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let current = crate::lock_session(&state.session, expected_epoch.as_deref())?;
        let (select, start_us, removed_us) = cut(&current.host, &key, target).map_err(explain)?;
        Ok(TranscriptCut { snapshot: current.snapshot(select)?, start_us, removed_us })
    })
    .await
    .map_err(crate::err)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use capopen_engine::{
        Project,
        edit::{EditCmd, new_id},
        model::{Asset, AssetKind},
        speech,
    };
    use capopen_session::{
        Mode, ProjectSession,
        transcripts::{Record, TranscriptStore, VERSION},
    };

    /// A 10 s talk with a word every second and its stored transcript, plus music on its own track.
    fn fixture() -> (std::path::PathBuf, Host) {
        let dir = std::env::temp_dir().join(format!("app-transcripts-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let asset = |id: &str, kind| {
            let path = dir.join(id);
            std::fs::write(&path, id).unwrap();
            Asset {
                id: id.into(),
                name: id.into(),
                path: path.to_string_lossy().into(),
                kind,
                duration_us: 10_000_000,
                width: 1080,
                height: 1920,
                fps: 30.0,
                has_audio: true,
                rotation: 0,
            }
        };
        let mut project = Project::new("transcript");
        project
            .apply(EditCmd::AddAssets {
                assets: vec![asset("talk", AssetKind::Video), asset("music", AssetKind::Audio)],
            })
            .unwrap();
        project.apply(EditCmd::AddClip { asset_id: "talk".into(), start_us: None, track_id: None }).unwrap();
        project.apply(EditCmd::AddClip { asset_id: "music".into(), start_us: Some(0), track_id: None }).unwrap();
        let path = dir.join("project.capopen");
        std::fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
        let store = TranscriptStore::at(dir.join("transcripts")).unwrap();
        let talk = &project.assets[0];
        let words = (0..8)
            .map(|i| speech::Word {
                start_us: 500_000 + i * 1_000_000,
                end_us: 900_000 + i * 1_000_000,
                text: format!("w{i}"),
                probability: 0.9,
            })
            .collect();
        store
            .put(
                talk,
                &Record {
                    version: VERSION,
                    fingerprint: store.fingerprint(talk).unwrap(),
                    duration_us: talk.duration_us,
                    model: "fixture".into(),
                    language: "cs".into(),
                    words,
                    segments: vec![],
                },
            )
            .unwrap();
        let session = ProjectSession::open(&path, Mode::Write, None).unwrap();
        (dir.clone(), Host { session, jobs: Default::default(), transcripts: store, cache_dir: dir.join("cache") })
    }

    #[test]
    fn edits_and_undo_keep_the_transcript_and_word_cuts_leave_music_in_place() {
        let (dir, host) = fixture();
        let placed = |host: &Host| view(host, 300_000).unwrap();
        let texts = |v: &TranscriptView| v.words.iter().map(|w| (w.text.clone(), w.start_us)).collect::<Vec<_>>();
        let first = placed(&host);
        assert_eq!(first.words.len(), 8);
        assert!(first.untranscribed.is_empty());

        let (_, start_us, removed_us) = cut(&host, &first.key, Target::Words(vec![[2, 3]])).unwrap();
        let cut_view = placed(&host);
        assert_eq!(cut_view.words.len(), 6);
        // The cut keeps 0.12 s after w1; w4 now follows it.
        assert_eq!(start_us, 1_900_000 + 120_000);
        assert_eq!((cut_view.words[2].text.as_str(), cut_view.words[2].start_us), ("w4", 4_500_000 - removed_us));
        let project = host.session.state().unwrap().project;
        let music = &project.tracks.iter().find(|t| t.keep_in_place).unwrap().clips[0];
        assert_eq!((music.start_us, music.duration_us), (0, 10_000_000));
        assert!(
            explain(cut(&host, &first.key, Target::Words(vec![[0, 0]])).unwrap_err())
                .starts_with("The transcript changed")
        );

        // Q over the first second and double speed: the words follow without recognising again.
        host.session
            .edit(
                vec![EditCmd::RippleDeleteRanges {
                    ranges: vec![TimeRange { start_us: 0, end_us: 1_000_000 }],
                    keep_track_ids: None,
                }],
                None,
                Expect::default(),
            )
            .unwrap();
        let after_q = placed(&host);
        assert_eq!(after_q.words[0].text, "w1");
        let clip = host.session.state().unwrap().project.tracks[0].clips[0].id.clone();
        host.session
            .edit(
                vec![
                    serde_json::from_value(serde_json::json!({"type": "updateClip", "clipId": clip, "speed": 2}))
                        .unwrap(),
                ],
                None,
                Expect::default(),
            )
            .unwrap();
        assert_eq!(placed(&host).words[0].start_us, after_q.words[0].start_us / 2);

        host.session.undo().unwrap();
        assert_eq!(texts(&placed(&host)), texts(&after_q));
        host.session.undo().unwrap();
        host.session.undo().unwrap();
        assert_eq!(texts(&placed(&host)), texts(&first));
        host.session.redo().unwrap();
        assert_eq!(texts(&placed(&host)), texts(&cut_view));

        let current = placed(&host);
        assert!(!current.pauses.is_empty());
        cut(&host, &current.key, Target::Pauses { pause_us: 300_000, only: None }).unwrap();
        let tight = placed(&host);
        assert_eq!(texts(&tight).len(), current.words.len());
        assert!(tight.pauses.is_empty());
        drop(host);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
