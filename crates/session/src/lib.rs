//! One headless editing authority. The mutex serializes edits and disk writes together.
mod storage;
mod validate;

pub use storage::lock_project;
pub use validate::validate;

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use capopen_engine::{
    Project,
    edit::{EditCmd, EditOutcome, Editor, new_id},
    model::Clip,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const HISTORY_LIMIT: usize = 200;

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

#[derive(Deserialize)]
struct RecoveryCheckpoint {
    project: Project,
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

#[derive(Serialize)]
struct Checkpoint<'a> {
    run_id: &'a str,
    label: &'a str,
    session_epoch: &'a str,
    revision: u64,
    project: &'a Project,
}

struct Run {
    info: RunInfo,
    before: Project,
    selection: Vec<String>,
    touched: Instant,
}

struct History {
    run_id: String,
    before: Project,
    selection: Vec<String>,
}

struct Request {
    content: Value,
    expected_revision: Option<u64>,
    result: EditResult,
}

struct Inner {
    _lock: File,
    path: PathBuf,
    mode: Mode,
    project: Project,
    stamp: Stamp,
    run: Option<Run>,
    history: Vec<History>,
    requests: HashMap<(String, String), Request>,
    recovery: Option<PathBuf>,
    selection: Vec<String>,
    timeout: Duration,
}

pub struct ProjectSession {
    inner: Arc<Mutex<Inner>>,
    stop: Arc<(Mutex<bool>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl ProjectSession {
    pub fn open(path: impl AsRef<Path>, mode: Mode) -> Result<Self> {
        Self::open_with_idle_timeout(path, mode, IDLE_TIMEOUT)
    }

    pub fn open_with_idle_timeout(
        path: impl AsRef<Path>,
        mode: Mode,
        timeout: Duration,
    ) -> Result<Self> {
        ensure!(!timeout.is_zero(), "Idle timeout must be positive");
        let path = fs::canonicalize(path.as_ref()).context("Resolving project path")?;
        let lock = lock_project(&path, mode == Mode::Write)?;
        let project = serde_json::from_slice(&fs::read(&path).context("Reading project")?)
            .context("Parsing project")?;
        validate(&project)?;
        let checkpoint = storage::sidecar(&path, ".checkpoint.json");
        let inner = Arc::new(Mutex::new(Inner {
            _lock: lock,
            path,
            mode,
            project,
            stamp: Stamp {
                revision: 0,
                session_epoch: format!("{}{}", new_id(), new_id()),
            },
            run: None,
            history: Vec::new(),
            requests: HashMap::new(),
            recovery: checkpoint.exists().then_some(checkpoint),
            selection: Vec::new(),
            timeout,
        }));
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = spawn_idle_worker(&inner, &stop, timeout)?;
        Ok(Self {
            inner,
            stop,
            worker: Some(worker),
        })
    }

    pub fn state(&self) -> Result<SessionState> {
        let mut inner = self.inner.lock().unwrap();
        inner.touch()?;
        Ok(SessionState {
            project: inner.project.clone(),
            stamp: inner.stamp.clone(),
            open_run: inner.run.as_ref().map(|r| r.info.clone()),
            recovery_checkpoint: inner.recovery.clone(),
            selection: inner.selection.clone(),
            playhead_us: 0,
            undo_run_id: inner.history.last().map(|h| h.run_id.clone()),
            read_only: inner.mode == Mode::ReadOnly,
        })
    }

    pub fn resolve_recovery(&self, action: RecoveryAction) -> Result<Stamp> {
        let mut inner = self.inner.lock().unwrap();
        inner.writable()?;
        ensure!(inner.run.is_none(), "RUN_BUSY: end the open run before recovery");
        let path = inner.recovery.as_ref().context("NO_RECOVERY: no leftover checkpoint")?.clone();
        if matches!(action, RecoveryAction::Restore) {
            let checkpoint: RecoveryCheckpoint = serde_json::from_slice(
                &fs::read(&path).context("Reading recovery checkpoint")?,
            ).context("Parsing recovery checkpoint")?;
            validate(&checkpoint.project)?;
            storage::save(&inner.path, &checkpoint.project)?;
            if inner.project != checkpoint.project {
                inner.stamp.revision += 1;
            }
            inner.project = checkpoint.project;
        }
        // If cleanup fails, disk and memory agree and recovery remains retryable.
        fs::remove_file(&path).context("Removing recovery checkpoint")?;
        inner.recovery = None;
        Ok(inner.stamp.clone())
    }

    pub fn begin_run(&self, label: String) -> Result<RunResult> {
        let mut inner = self.inner.lock().unwrap();
        inner.touch()?;
        inner.writable()?;
        ensure!(inner.run.is_none(), "RUN_BUSY: a run is already open");
        ensure!(
            inner.recovery.is_none(),
            "RECOVERY_PENDING: inspect and resolve the leftover checkpoint before editing"
        );
        let run_id = new_id();
        let checkpoint = Checkpoint {
            run_id: &run_id,
            label: &label,
            session_epoch: &inner.stamp.session_epoch,
            revision: inner.stamp.revision,
            project: &inner.project,
        };
        storage::save(
            &storage::sidecar(&inner.path, ".checkpoint.json"),
            &checkpoint,
        )?;
        inner.run = Some(Run {
            info: RunInfo {
                run_id: run_id.clone(),
                label,
            },
            before: inner.project.clone(),
            selection: inner.selection.clone(),
            touched: Instant::now(),
        });
        Ok(RunResult {
            run_id,
            stamp: inner.stamp.clone(),
        })
    }

    pub fn apply_edits(
        &self,
        run_id: &str,
        request_id: &str,
        edits: Vec<EditCmd>,
        expected_revision: Option<u64>,
    ) -> Result<EditResult> {
        let mut inner = self.inner.lock().unwrap();
        inner.touch()?;
        inner.writable()?;
        ensure!(
            !request_id.is_empty(),
            "INVALID_REQUEST: request_id cannot be empty"
        );
        inner.owns_run(run_id)?;
        let key = (run_id.to_owned(), request_id.to_owned());
        let content = serde_json::to_value(&edits).context("Serializing edit request")?;
        if let Some(previous) = inner.requests.get(&key) {
            ensure!(
                previous.content == content
                    && previous.expected_revision == expected_revision,
                "REQUEST_CONFLICT: request_id was used with different content"
            );
            return Ok(previous.result.clone());
        }
        if let Some(expected) = expected_revision {
            ensure!(
                expected == inner.stamp.revision,
                "STALE_REVISION: expected {expected}, current {}",
                inner.stamp.revision
            );
        }
        let mut editor = Editor::new(inner.project.clone());
        // Untrusted numeric inputs can overflow inside an engine command before validation.
        // Keep that failure inside the disposable editor, without poisoning the session mutex.
        let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            editor.apply_batch(edits, None)
        }))
        .map_err(|_| {
            anyhow::anyhow!(
                "EDIT_REJECTED: engine failed while applying the batch; project unchanged"
            )
        })?;
        let mut outcome = applied.context("EDIT_REJECTED")?;
        validate(&editor.project)?;
        outcome.select.retain(|id| {
            editor
                .project
                .tracks
                .iter()
                .any(|t| t.clips.iter().any(|c| &c.id == id))
        });
        storage::save(&inner.path, &editor.project)?;
        let (changed, clips) = changes(&inner.project, &editor.project, &outcome);
        if inner.project != editor.project {
            inner.stamp.revision += 1;
        }
        inner.project = editor.project;
        inner.selection = outcome.select.clone();
        let result = EditResult {
            stamp: inner.stamp.clone(),
            outcome,
            changed,
            clips,
        };
        inner.requests.insert(
            key,
            Request {
                content,
                expected_revision,
                result: result.clone(),
            },
        );
        Ok(result)
    }

    pub fn end_run(&self, run_id: &str, action: EndAction) -> Result<RunResult> {
        let mut inner = self.inner.lock().unwrap();
        inner.touch()?;
        inner.writable()?;
        inner.owns_run(run_id)?;
        inner.finish(action)?;
        Ok(RunResult {
            run_id: run_id.into(),
            stamp: inner.stamp.clone(),
        })
    }

    pub fn undo_run(&self, run_id: &str) -> Result<RunResult> {
        let mut inner = self.inner.lock().unwrap();
        inner.touch()?;
        inner.writable()?;
        ensure!(
            inner.run.is_none(),
            "RUN_BUSY: end the open run before undo"
        );
        let last = inner
            .history
            .last()
            .context("UNDO_UNAVAILABLE: no history")?;
        ensure!(
            last.run_id == run_id,
            "UNDO_UNAVAILABLE: run is not the last history entry"
        );
        storage::save(&inner.path, &last.before)?;
        let changed = inner.project != last.before;
        inner.project = last.before.clone();
        inner.selection = inner
            .history
            .last()
            .map(|h| h.selection.clone())
            .unwrap_or_default();
        inner.history.pop();
        inner.requests.clear();
        if changed {
            inner.stamp.revision += 1;
        }
        Ok(RunResult {
            run_id: run_id.into(),
            stamp: inner.stamp.clone(),
        })
    }

    /// A clean transport disconnect keeps edits as one undoable run.
    pub fn disconnect(&self) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.run.is_some() {
            inner.finish(EndAction::Keep)?;
        }
        Ok(())
    }
}

impl Inner {
    fn writable(&self) -> Result<()> {
        ensure!(
            self.mode == Mode::Write,
            "READ_ONLY: restart with --allow-write to edit"
        );
        Ok(())
    }

    fn owns_run(&self, id: &str) -> Result<()> {
        ensure!(
            self.run.as_ref().is_some_and(|r| r.info.run_id == id),
            "INVALID_RUN: run is not open or has ended"
        );
        Ok(())
    }

    fn expire(&mut self) -> Result<()> {
        if self
            .run
            .as_ref()
            .is_some_and(|r| r.touched.elapsed() >= self.timeout)
        {
            self.finish(EndAction::Keep)?;
        }
        Ok(())
    }

    fn touch(&mut self) -> Result<()> {
        self.expire()?;
        if let Some(run) = &mut self.run {
            run.touched = Instant::now();
        }
        Ok(())
    }

    fn finish(&mut self, action: EndAction) -> Result<()> {
        let run = self.run.as_ref().context("INVALID_RUN: no open run")?;
        let project = match action {
            EndAction::Keep => &self.project,
            EndAction::Discard => &run.before,
        };
        storage::save(&self.path, project)?;
        // Do not acknowledge completion unless the recovery marker can be removed.
        if let Err(error) = fs::remove_file(storage::sidecar(&self.path, ".checkpoint.json")) {
            // A failed discard must leave disk and memory on the same live revision.
            if matches!(action, EndAction::Discard) {
                storage::save(&self.path, &self.project)
                    .context("Restoring live project after checkpoint cleanup failed")?;
            }
            return Err(error).context("Removing run checkpoint");
        }
        let run = self.run.take().context("INVALID_RUN: no open run")?;
        match action {
            EndAction::Keep => {
                self.history.push(History {
                    run_id: run.info.run_id,
                    before: run.before,
                    selection: run.selection,
                });
                if self.history.len() > HISTORY_LIMIT {
                    self.history.remove(0);
                }
            }
            EndAction::Discard => {
                if self.project != run.before {
                    self.stamp.revision += 1;
                }
                self.project = run.before;
                self.selection = run.selection;
            }
        }
        self.requests.clear();
        Ok(())
    }
}

fn changes(
    before: &Project,
    after: &Project,
    outcome: &EditOutcome,
) -> (Vec<String>, Vec<ClipChange>) {
    let old: HashMap<_, _> = before
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(move |c| (&c.id, (&t.id, c))))
        .collect();
    let mut changed = Vec::new();
    let mut clips = Vec::new();
    for track in &after.tracks {
        for clip in &track.clips {
            let differs = old
                .get(&clip.id)
                .is_some_and(|(track_id, c)| *track_id != &track.id || *c != clip);
            if differs {
                changed.push(clip.id.clone());
            }
            if differs || outcome.created.contains(&clip.id) {
                clips.push(ClipChange {
                    track_id: track.id.clone(),
                    clip: clip.clone(),
                });
            }
        }
    }
    (changed, clips)
}

fn spawn_idle_worker(
    inner: &Arc<Mutex<Inner>>,
    stop: &Arc<(Mutex<bool>, Condvar)>,
    timeout: Duration,
) -> Result<JoinHandle<()>> {
    let (inner, stop) = (Arc::downgrade(inner), stop.clone());
    std::thread::Builder::new()
        .name("capopen-session-idle".into())
        .spawn(move || {
            let (flag, wake) = &*stop;
            loop {
                let stopped = flag.lock().unwrap();
                let (stopped, _) = wake
                    .wait_timeout_while(stopped, timeout.min(Duration::from_secs(1)), |stop| !*stop)
                    .unwrap();
                if *stopped {
                    break;
                }
                drop(stopped);
                let Some(inner) = inner.upgrade() else { break };
                if let Err(error) = inner.lock().unwrap().expire() {
                    eprintln!("Cannot end idle run: {error:#}");
                }
            }
        })
        .context("Starting session idle timer")
}

impl Drop for ProjectSession {
    fn drop(&mut self) {
        *self.stop.0.lock().unwrap() = true;
        self.stop.1.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Err(error) = self.disconnect() {
            eprintln!("Cannot finish disconnected run: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests;
