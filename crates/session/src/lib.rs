//! One editing authority for user edits and agent runs, with ordered background saves.
pub mod host;
pub mod jobs;
pub mod transcripts;

mod changes;
mod recovery;
mod run;
mod storage;
mod types;
mod validate;
mod writer;

pub use storage::lock_project;
pub use types::*;
pub use validate::validate;

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, mpsc::Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use capopen_engine::{
    Project,
    edit::{EditCmd, Editor, new_id},
    speech::speech_key,
};
use serde_json::Value;

use changes::changes;
use run::Run;
use writer::Writer;

pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Matches the desktop autosave quiet period in src-tauri/src/store.rs.
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(800);

struct Request {
    content: Value,
    expect: Expect,
    result: EditResult,
}

#[derive(Default)]
struct UiContext {
    selection: Vec<String>,
    playhead_us: i64,
}

struct Inner {
    // Writer joins before the exclusive lock is released.
    writer: Option<Writer>,
    _lock: Option<File>,
    path: PathBuf,
    mode: Mode,
    editor: Editor,
    epoch: String,
    run: Option<Run>,
    stopped_runs: HashSet<String>,
    requests: HashMap<(String, String), Request>,
    recovery: Option<PathBuf>,
    ui: UiContext,
    timeout: Duration,
    events: Option<Sender<SessionEvent>>,
}

pub struct ProjectSession {
    inner: Arc<Mutex<Inner>>,
    stop: Arc<(Mutex<bool>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl ProjectSession {
    pub fn open(
        path: impl AsRef<Path>,
        mode: Mode,
        events: Option<Sender<SessionEvent>>,
    ) -> Result<Self> {
        Self::open_with_idle_timeout(path, mode, IDLE_TIMEOUT, events)
    }

    pub fn open_with_idle_timeout(
        path: impl AsRef<Path>,
        mode: Mode,
        timeout: Duration,
        events: Option<Sender<SessionEvent>>,
    ) -> Result<Self> {
        ensure!(!timeout.is_zero(), "Idle timeout must be positive");
        let path = fs::canonicalize(path.as_ref()).context("Resolving project path")?;
        let lock = if mode == Mode::Write { Some(lock_project(&path, true)?) } else { None };
        let project = load(&path)?;
        let checkpoint = storage::sidecar(&path, ".checkpoint.json");
        let writer = if mode == Mode::Write { Some(Writer::start(path.clone(), events.clone())?) } else { None };
        let inner = Arc::new(Mutex::new(Inner {
            writer,
            _lock: lock,
            path,
            mode,
            editor: Editor::new(project),
            epoch: format!("{}{}", new_id(), new_id()),
            run: None,
            stopped_runs: HashSet::new(),
            requests: HashMap::new(),
            recovery: checkpoint.exists().then_some(checkpoint),
            ui: UiContext::default(),
            timeout,
            events,
        }));
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = if mode == Mode::Write { Some(spawn_idle_worker(&inner, &stop, timeout)?) } else { None };
        Ok(Self { inner, stop, worker })
    }

    pub fn state(&self) -> Result<SessionState> {
        let mut inner = self.inner.lock().unwrap();
        inner.touch()?;
        if inner.mode == Mode::ReadOnly {
            let project = load(&inner.path)?;
            if inner.editor.project != project {
                inner.editor.project = project;
                inner.editor.revision += 1;
            }
            let checkpoint = storage::sidecar(&inner.path, ".checkpoint.json");
            inner.recovery = checkpoint.exists().then_some(checkpoint);
        }
        Ok(SessionState {
            project: inner.editor.project.clone(),
            speech_key: speech_key(&inner.editor.project),
            stamp: inner.stamp(),
            open_run: inner.run.as_ref().map(|r| r.info.clone()),
            recovery_checkpoint: inner.recovery.clone(),
            selection: inner.ui.selection.clone(),
            playhead_us: inner.ui.playhead_us,
            undo_run_id: inner.editor.last_key().and_then(|key| key.strip_prefix("run:")).map(str::to_owned),
            read_only: inner.mode == Mode::ReadOnly,
        })
    }

    /// A consistent project view with the availability of undo and redo.
    pub fn view(&self) -> Result<(SessionState, bool, bool)> {
        loop {
            let state = self.state()?;
            let inner = self.inner.lock().unwrap();
            if state.stamp.revision == inner.editor.revision {
                return Ok((state, inner.editor.can_undo(), inner.editor.can_redo()));
            }
        }
    }

    pub fn set_ui_context(&self, selection: Vec<String>, playhead_us: i64) {
        self.inner.lock().unwrap().ui = UiContext { selection, playhead_us };
    }

    pub fn edit(
        &self,
        cmds: Vec<EditCmd>,
        coalesce: Option<String>,
        expect: Expect,
    ) -> Result<EditResult> {
        let mut inner = self.inner.lock().unwrap();
        inner.user_editable()?;
        ensure!(!coalesce.as_deref().is_some_and(|key| key.starts_with("run:")),
            "INVALID_REQUEST: run: coalesce keys are reserved");
        inner.check_expect(&expect)?;
        let result = inner.apply(cmds, coalesce, Origin::User)?;
        inner.schedule()?;
        Ok(result)
    }

    pub fn undo(&self) -> Result<Stamp> {
        self.history(false)
    }

    pub fn redo(&self) -> Result<Stamp> {
        self.history(true)
    }

    fn history(&self, redo: bool) -> Result<Stamp> {
        let mut inner = self.inner.lock().unwrap();
        inner.user_editable()?;
        let changed = if redo { inner.editor.redo() } else { inner.editor.undo() };
        if changed {
            inner.changed(if redo { Origin::Redo } else { Origin::Undo });
            inner.schedule()?;
        }
        Ok(inner.stamp())
    }

    /// IPC listeners may remove stale endpoints only while this session owns the project lock.
    pub fn locked_path(&self) -> Result<PathBuf> {
        let inner = self.inner.lock().unwrap();
        ensure!(inner._lock.is_some(), "READ_ONLY: IPC listener requires project ownership");
        Ok(inner.path.clone())
    }

    /// A clean transport disconnect keeps edits as one undoable run and drains autosave.
    pub fn disconnect(&self) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.run.is_some() {
            inner.finish(EndAction::Keep)?;
        } else if inner.mode == Mode::Write {
            inner.flush()?;
        }
        Ok(())
    }
}

impl Inner {
    fn stamp(&self) -> Stamp {
        Stamp { revision: self.editor.revision, session_epoch: self.epoch.clone() }
    }

    fn emit(&self, event: SessionEvent) {
        if let Some(events) = &self.events {
            let _ = events.send(event);
        }
    }

    fn changed(&self, origin: Origin) {
        self.emit(SessionEvent::Changed { revision: self.editor.revision, origin });
    }

    fn writable(&self) -> Result<()> {
        ensure!(self.mode == Mode::Write, "READ_ONLY: restart with --allow-write to edit");
        Ok(())
    }

    fn recovered(&self) -> Result<()> {
        ensure!(self.recovery.is_none(), "RECOVERY_PENDING: inspect and resolve the leftover checkpoint before editing");
        Ok(())
    }

    fn user_editable(&mut self) -> Result<()> {
        self.touch()?;
        self.writable()?;
        ensure!(self.run.is_none(), "RUN_ACTIVE: stop the open run before editing");
        self.recovered()
    }

    fn check_expect(&self, expect: &Expect) -> Result<()> {
        if let Some(expected) = expect.revision {
            ensure!(expected == self.editor.revision, "STALE_REVISION: expected {expected}, current {}", self.editor.revision);
        }
        if let Some(expected) = &expect.speech_key {
            ensure!(expected == &speech_key(&self.editor.project), "SPEECH_CHANGED: timeline speech changed");
        }
        Ok(())
    }

    fn apply(
        &mut self,
        cmds: Vec<EditCmd>,
        coalesce: Option<String>,
        origin: Origin,
    ) -> Result<EditResult> {
        let before = self.editor.project.clone();
        let mut outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.editor.apply_batch_checked(cmds, coalesce, validate)
        }))
        .map_err(|_| anyhow::anyhow!("EDIT_REJECTED: engine failed while applying the batch; project unchanged"))?
        .map_err(|error| anyhow::anyhow!("EDIT_REJECTED: {error:#}"))?;
        outcome.select.retain(|id| {
            self.editor.project.tracks.iter().any(|t| t.clips.iter().any(|c| &c.id == id))
        });
        let (changed, clips) = changes(&before, &self.editor.project, &outcome);
        if self.editor.project != before {
            self.changed(origin);
        }
        Ok(EditResult { stamp: self.stamp(), outcome, changed, clips })
    }

    fn schedule(&self) -> Result<()> {
        self.writer.as_ref().context("READ_ONLY: no writer")?
            .schedule(&self.editor.project, self.editor.revision)
    }

    fn flush(&self) -> Result<()> {
        self.writer.as_ref().context("READ_ONLY: no writer")?
            .flush(&self.editor.project, self.editor.revision)
    }
}

fn load(path: &Path) -> Result<Project> {
    let project = serde_json::from_slice(&fs::read(path).context("Reading project")?).context("Parsing project")?;
    validate(&project)?;
    Ok(project)
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
