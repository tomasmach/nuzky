use std::path::PathBuf;

use capopen_engine::{Project, edit::EditOutcome, model::Clip};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    ReadOnly,
    Write,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndAction {
    Keep,
    Discard,
}

#[derive(Clone, Copy, Debug)]
pub enum RecoveryAction {
    Keep,
    Restore,
}

#[derive(Clone, Debug, Serialize)]
pub struct Stamp {
    pub revision: u64,
    pub session_epoch: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunInfo {
    pub run_id: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionState {
    pub project: Project,
    pub speech_layout_key: String,
    #[serde(flatten)]
    pub stamp: Stamp,
    pub open_run: Option<RunInfo>,
    pub recovery_checkpoint: Option<PathBuf>,
    pub selection: Vec<String>,
    pub playhead_us: i64,
    pub undo_run_id: Option<String>,
    pub read_only: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ClipChange {
    pub track_id: String,
    #[serde(flatten)]
    pub clip: Clip,
}

#[derive(Clone, Debug, Serialize)]
pub struct EditResult {
    #[serde(flatten)]
    pub stamp: Stamp,
    #[serde(flatten)]
    pub outcome: EditOutcome,
    pub changed: Vec<String>,
    /// Actual positions after magnetic packing, for created and changed clips.
    pub clips: Vec<ClipChange>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunResult {
    #[serde(flatten)]
    pub stamp: Stamp,
    pub run_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expect {
    pub revision: Option<u64>,
    pub speech_layout_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    User,
    Run { run_id: String, label: String },
    Undo,
    Redo,
    Recovery,
}

#[derive(Clone, Debug)]
pub enum SessionEvent {
    TranscriptsChanged,
    Changed {
        revision: u64,
        origin: Origin,
    },
    Run(Option<RunInfo>),
    Saved {
        revision: u64,
        error: Option<String>,
    },
}
