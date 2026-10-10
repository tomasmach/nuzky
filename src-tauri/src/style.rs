//! The creator's style, as the app shows and changes it, and learning from what the user does.

use std::time::{Duration, SystemTime};

use nuzky_mcp::style::{Moment, Store, StyleAction, StyleView, moment};
use nuzky_session::host::Host;
use nuzky_session::transcripts::TranscriptStore;
use tauri::{AppHandle, Emitter};

use crate::{CmdResult, err, jobs};

/// After the user's last edit of an AI's cut, learning waits this long for the next one.
pub const LEARN_AFTER: Duration = Duration::from_secs(10);

#[tauri::command]
pub async fn style_view() -> CmdResult<StyleView> {
    tauri::async_runtime::spawn_blocking(|| Store::default().view()).await.map_err(err)?.map_err(err)
}

#[tauri::command]
pub async fn style_act(action: StyleAction) -> CmdResult<StyleView> {
    tauri::async_runtime::spawn_blocking(move || Store::default().act(action)).await.map_err(err)?.map_err(err)
}

#[tauri::command]
pub fn start_style_learning(app: AppHandle, pairs: Vec<jobs::StylePair>) -> CmdResult<String> {
    jobs::start_style_learning(&app, pairs)
}

/// Tells the UI whenever the style changes, whoever changes it: the AI panel's agent and other
/// agents (their own processes), learning, `nuzky style learn`, another window or a text editor.
/// It looks at the style's files twice a second, a few `stat`s each time.
pub fn watch(app: AppHandle) {
    let stamp = || -> Vec<Option<(SystemTime, u64)>> {
        Store::default()
            .files()
            .iter()
            .map(|p| std::fs::metadata(p).ok().and_then(|m| Some((m.modified().ok()?, m.len()))))
            .collect()
    };
    let spawned = std::thread::Builder::new().name("style-watch".into()).spawn(move || {
        let mut last = stamp();
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let now = stamp();
            if now != last {
                last = now;
                app.emit("style-changed", ()).ok();
            }
        }
    });
    if let Err(error) = spawned {
        log::error!("Changes of the style made elsewhere will show only when the page opens again: {error}");
    }
}

/// Learns from the project as it is now, after the user edited an AI's cut: what it needs is taken
/// at once, and the rest runs on its own thread, so closing or switching the project never waits.
pub fn learn_edits(host: &Host) {
    let taken = host.session.state().and_then(|s| moment(&host.session, s.project, s.open_run.is_some(), true));
    learn(taken, host.transcripts.clone());
}

/// Works out and keeps what a moment taught, on its own thread. Learning never changes the style
/// itself, so a failure only loses a lesson; the page shows new suggestions through `watch`.
pub fn learn(moment: anyhow::Result<Option<Moment>>, transcripts: TranscriptStore) {
    let learned = move || -> anyhow::Result<()> {
        let Some(moment) = moment? else { return Ok(()) };
        if let Some(evidence) = moment.lesson(&transcripts)? {
            Store::default().keep_evidence(evidence)?;
        }
        Ok(())
    };
    let spawned = std::thread::Builder::new().name("style-learn".into()).spawn(move || {
        if let Err(error) = learned() {
            log::error!("Learning the style failed: {error:#}");
        }
    });
    if let Err(error) = spawned {
        log::error!("Learning the style could not start: {error}");
    }
}
