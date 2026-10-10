//! Making the reels the user picked from a long video, the same way agents do with make_reels.

use anyhow::Result;
use nuzky_mcp::reels::{self, Framing, MadeReel};
use nuzky_mcp::transcript;
use nuzky_session::{Expect, host::Host};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::{AppState, CmdResult, Snapshot};

#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReelsMade {
    snapshot: Snapshot,
    reels: Vec<MadeReel>,
}

/// Writes the reels' projects beside this one and marks them made in it as one user edit.
fn make(host: &Host, ids: &[String], framing: Framing) -> Result<Vec<MadeReel>> {
    let state = host.session.state()?;
    let derived = transcript::derive(&state.project, &host.transcripts)?;
    let (made, edits) = reels::make(&host.session.path(), &state.project, &derived, ids, framing)?;
    let expect = Expect { revision: Some(state.stamp.revision), speech_layout_key: None };
    if let Err(error) = host.session.edit(edits, None, expect) {
        reels::forget(&made, &host.session.state()?.project);
        return Err(error);
    }
    Ok(made)
}

#[tauri::command]
pub async fn make_reels(
    app: AppHandle,
    ids: Vec<String>,
    canvas: Option<Framing>,
    expected_epoch: Option<String>,
) -> CmdResult<ReelsMade> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let current = crate::lock_session(&state.session, expected_epoch.as_deref())?;
        let reels = make(&current.host, &ids, canvas.unwrap_or_default()).map_err(crate::err)?;
        Ok(ReelsMade { snapshot: current.snapshot(Vec::new())?, reels })
    })
    .await
    .map_err(crate::err)?
}
