use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Context, Result};
use nuzky_engine::Project;

use crate::history::{HISTORY_SUFFIX, Write};
use crate::{SAVE_DEBOUNCE, SAVE_MAX_WAIT, SessionEvent, storage};

struct Snapshot {
    project: Project,
    revision: u64,
}

enum Message {
    Schedule(Snapshot),
    Flush(Snapshot, Sender<Result<()>>),
    Checkpoint(serde_json::Value, Sender<Result<()>>),
    History(Write),
    Stop,
}

pub(crate) struct Writer {
    tx: Sender<Message>,
    worker: Option<JoinHandle<()>>,
}

impl Writer {
    /// A failed write of the versions sets `broken`, so the next one replaces the file whole.
    pub(crate) fn start(path: PathBuf, events: Option<Sender<SessionEvent>>, broken: Arc<AtomicBool>) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("nuzky-session-writer".into())
            .spawn(move || write_loop(path, events, broken, rx))
            .context("Starting project writer")?;
        Ok(Self { tx, worker: Some(worker) })
    }

    pub(crate) fn checkpoint(&self, value: serde_json::Value) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Message::Checkpoint(value, tx)).context("SAVE_FAILED: project writer stopped")?;
        rx.recv().context("SAVE_FAILED: project writer stopped")?
    }

    /// Versions are written in order with the saves, without waiting for them.
    pub(crate) fn history(&self, write: Write) {
        let _ = self.tx.send(Message::History(write));
    }

    pub(crate) fn schedule(&self, project: &Project, revision: u64) -> Result<()> {
        self.tx
            .send(Message::Schedule(Snapshot { project: project.clone(), revision }))
            .context("SAVE_FAILED: project writer stopped")
    }

    pub(crate) fn flush(&self, project: &Project, revision: u64) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Message::Flush(Snapshot { project: project.clone(), revision }, tx))
            .context("SAVE_FAILED: project writer stopped")?;
        rx.recv().context("SAVE_FAILED: project writer stopped")?
    }
}

fn write_loop(path: PathBuf, events: Option<Sender<SessionEvent>>, broken: Arc<AtomicBool>, rx: Receiver<Message>) {
    let mut pending: Option<Snapshot> = None;
    let mut oldest = Instant::now();
    // The revision this writer put on disk, with the file it left. Flushing it again while the file
    // is unchanged would only move its modification time, which the home screen reads as when the
    // project was last opened: a project closed after another one opened would look newer than it.
    let mut saved = None;
    loop {
        let message = if pending.is_some() {
            // A steady stream of edits must not postpone the save past the maximum wait.
            let wait = SAVE_DEBOUNCE.min(SAVE_MAX_WAIT.saturating_sub(oldest.elapsed()));
            let received = if wait.is_zero() { Err(RecvTimeoutError::Timeout) } else { rx.recv_timeout(wait) };
            match received {
                Ok(message) => Some(message),
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(snapshot) = pending.take() {
                        let revision = snapshot.revision;
                        if save(&path, &events, snapshot).is_ok() {
                            saved = on_disk(&path).map(|file| (revision, file));
                        }
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => None,
            }
        } else {
            rx.recv().ok()
        };
        match message {
            Some(Message::Schedule(snapshot)) => {
                if pending.is_none() {
                    oldest = Instant::now();
                }
                pending = Some(snapshot);
            }
            Some(Message::Flush(snapshot, reply)) => {
                pending = None;
                let revision = snapshot.revision;
                if saved.is_some_and(|(at, file)| at == revision && on_disk(&path) == Some(file)) {
                    notify(&events, revision, None);
                    let _ = reply.send(Ok(()));
                    continue;
                }
                let result = save(&path, &events, snapshot);
                if result.is_ok() {
                    saved = on_disk(&path).map(|file| (revision, file));
                }
                let _ = reply.send(result);
            }
            Some(Message::Checkpoint(value, reply)) => {
                let result = storage::save(&storage::sidecar(&path, ".checkpoint.json"), &value)
                    .context("SAVE_FAILED: saving checkpoint");
                let _ = reply.send(result);
            }
            Some(Message::History(write)) => {
                let file = storage::sidecar(&path, HISTORY_SUFFIX);
                let result = match write {
                    Write::Append(line) => append(&file, &line),
                    Write::Replace(text) => storage::save_bytes(&file, text.as_bytes()),
                };
                if let Err(error) = result {
                    eprintln!("Cannot save the project's versions: {error:#}");
                    broken.store(true, Ordering::Relaxed);
                }
            }
            Some(Message::Stop) | None => {
                if let Some(snapshot) = pending {
                    let _ = save(&path, &events, snapshot);
                }
                break;
            }
        }
    }
}

fn save(path: &std::path::Path, events: &Option<Sender<SessionEvent>>, snapshot: Snapshot) -> Result<()> {
    let result = storage::save(path, &snapshot.project).context("SAVE_FAILED: saving project");
    notify(events, snapshot.revision, result.as_ref().err().map(|error| format!("{error:#}")));
    result
}

fn notify(events: &Option<Sender<SessionEvent>>, revision: u64, error: Option<String>) {
    if let Some(events) = events {
        let _ = events.send(SessionEvent::Saved { revision, error });
    }
}

fn append(path: &Path, text: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new().append(true).create(true).open(path).context("Opening versions")?;
    file.write_all(text.as_bytes()).context("Writing versions")
}

/// The file's size and modification time, to notice it changed or went away since the last save.
fn on_disk(path: &std::path::Path) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.tx.send(Message::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
