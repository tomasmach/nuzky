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

#[cfg(test)]
mod tests {
    use super::*;
    use nuzky_engine::edit::EditCmd;
    use nuzky_engine::model::ReelStatus;
    use nuzky_mcp::reels::ReelProposal;
    use nuzky_session::EndAction;

    /// The app makes a reel as agents do, as one undo step, and never while an agent's run holds the project.
    #[test]
    fn the_app_makes_a_reel_as_one_undo_and_never_during_a_run() {
        let (dir, host) = crate::transcripts::tests::fixture();
        let state = host.session.state().unwrap();
        let derived = transcript::derive(&state.project, &host.transcripts).unwrap();
        let proposal = ReelProposal { from: 2, to: 4, title: "Middle".into(), why: String::new(), score: 0.5 };
        let candidates =
            reels::plan(&state.project, &derived, &[proposal], &[], reels::DEFAULT_MAX_DURATION_US).unwrap();
        let proposed = EditCmd::SetReelCandidates { candidates: candidates.clone() };
        host.session.edit(vec![proposed], None, Expect::default()).unwrap();
        let ids = vec![candidates[0].id.clone()];
        let reel = std::fs::canonicalize(&dir).unwrap().join("project-reel-1.nuzky");

        let run = host.session.begin_run("Agent".into()).unwrap();
        let refused = make(&host, &ids, Framing::Crop).unwrap_err().to_string();
        assert!(refused.starts_with("RUN_ACTIVE"), "{refused}");
        assert!(!reel.exists(), "a reel the project does not name was left behind");
        host.session.end_run(&run.run_id, EndAction::Keep).unwrap();

        let made = make(&host, &ids, Framing::Crop).unwrap();
        assert_eq!(made[0].path, reel);
        let status = |host: &Host| host.session.state().unwrap().project.reel_candidates[0].status;
        assert_eq!(status(&host), ReelStatus::Made);
        host.session.undo().unwrap();
        assert_eq!(status(&host), ReelStatus::Proposed);
        assert!(reel.is_file());
        drop(host);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
