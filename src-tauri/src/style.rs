//! The creator's style, as the app shows and changes it.

use std::time::{Duration, SystemTime};

use nuzky_mcp::style::{Store, StyleAction, StyleView};
use tauri::{AppHandle, Emitter};

use crate::{CmdResult, err, jobs};

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
/// agents (their own processes), `nuzky style learn`, another window or a text editor. It looks
/// at the style's files twice a second, a few `stat`s each time.
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
