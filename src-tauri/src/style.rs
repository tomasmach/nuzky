//! The creator's style, as the app shows and changes it, and learning from what the user does.

use std::path::Path;
use std::time::{Duration, SystemTime};

use nuzky_analysis::style as learning;
use nuzky_mcp::style::{Evidence, Moment, Store, StyleAction, StyleView, TimelinePlan, moment, plan};
use nuzky_session::host::Host;
use nuzky_session::transcripts::TranscriptStore;
use tauri::{AppHandle, Emitter, Manager};

use crate::{AppState, CmdResult, err, jobs};

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

/// What each timeline file offers to learn from, read at once.
#[tauri::command]
pub async fn style_read_timelines(paths: Vec<String>) -> CmdResult<Vec<TimelinePlan>> {
    tauri::async_runtime::spawn_blocking(move || paths.iter().map(|p| plan(Path::new(p))).collect()).await.map_err(err)
}

#[tauri::command]
pub fn start_timeline_learning(app: AppHandle, paths: Vec<String>) -> CmdResult<String> {
    jobs::start_timeline_learning(&app, paths)
}

/// Makes what the timeline learning job `job_id` learned the style, over the version `seen`: the
/// one the creator saw when they chose to use it, and agreed to replace. What it learned is kept
/// from then on, as learning from videos keeps it.
#[tauri::command]
pub async fn style_use_learned(app: AppHandle, job_id: String, seen: u64) -> CmdResult<StyleView> {
    let kept = app.state::<AppState>().timeline_lessons.lock().unwrap().clone();
    let evidence = kept.filter(|(id, _)| *id == job_id).map(|(_, e)| e).ok_or("NOT_LEARNED: learn again first")?;
    tauri::async_runtime::spawn_blocking(move || {
        let sources: Vec<learning::Source> = evidence.iter().map(Evidence::source).collect();
        // One key per recording of a timeline file: "timeline:<file>:<recording>".
        let projects: std::collections::BTreeSet<&str> =
            evidence.iter().map(|e| e.key.rsplit_once(':').map_or(e.key.as_str(), |(file, _)| file)).collect();
        let label = match projects.len() {
            1 => "Learned from 1 project".to_owned(),
            n => format!("Learned from {n} projects"),
        };
        let store = Store::default();
        store.replace(&learning::learned(&sources), &label, Some(seen))?;
        for evidence in &evidence {
            store.keep_evidence(evidence.clone())?;
        }
        store.view()
    })
    .await
    .map_err(err)?
    .map_err(err)
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
