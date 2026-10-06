use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use capopen_engine::Project;

use crate::{SAVE_DEBOUNCE, SessionEvent, storage};

struct Snapshot {
    project: Project,
    revision: u64,
}

enum Message {
    Schedule(Snapshot),
    Flush(Snapshot, Sender<Result<()>>),
    Checkpoint(serde_json::Value, Sender<Result<()>>),
    Stop,
}

pub(crate) struct Writer {
    tx: Sender<Message>,
    worker: Option<JoinHandle<()>>,
}

impl Writer {
    pub(crate) fn start(path: PathBuf, events: Option<Sender<SessionEvent>>) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("capopen-session-writer".into())
            .spawn(move || write_loop(path, events, rx))
            .context("Starting project writer")?;
        Ok(Self {
            tx,
            worker: Some(worker),
        })
    }

    pub(crate) fn checkpoint(&self, value: serde_json::Value) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Message::Checkpoint(value, tx))
            .context("SAVE_FAILED: project writer stopped")?;
        rx.recv().context("SAVE_FAILED: project writer stopped")?
    }

    pub(crate) fn schedule(&self, project: &Project, revision: u64) -> Result<()> {
        self.tx
            .send(Message::Schedule(Snapshot {
                project: project.clone(),
                revision,
            }))
            .context("SAVE_FAILED: project writer stopped")
    }

    pub(crate) fn flush(&self, project: &Project, revision: u64) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Message::Flush(
                Snapshot {
                    project: project.clone(),
                    revision,
                },
                tx,
            ))
            .context("SAVE_FAILED: project writer stopped")?;
        rx.recv().context("SAVE_FAILED: project writer stopped")?
    }
}

fn write_loop(path: PathBuf, events: Option<Sender<SessionEvent>>, rx: Receiver<Message>) {
    let mut pending = None;
    loop {
        let message = if pending.is_some() {
            match rx.recv_timeout(SAVE_DEBOUNCE) {
                Ok(message) => Some(message),
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(snapshot) = pending.take() {
                        let _ = save(&path, &events, snapshot);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => None,
            }
        } else {
            rx.recv().ok()
        };
        match message {
            Some(Message::Schedule(snapshot)) => pending = Some(snapshot),
            Some(Message::Flush(snapshot, reply)) => {
                pending = None;
                let _ = reply.send(save(&path, &events, snapshot));
            }
            Some(Message::Checkpoint(value, reply)) => {
                let result = storage::save(&storage::sidecar(&path, ".checkpoint.json"), &value)
                    .context("SAVE_FAILED: saving checkpoint");
                let _ = reply.send(result);
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

fn save(
    path: &std::path::Path,
    events: &Option<Sender<SessionEvent>>,
    snapshot: Snapshot,
) -> Result<()> {
    let result = storage::save(path, &snapshot.project).context("SAVE_FAILED: saving project");
    if let Some(events) = events {
        let _ = events.send(SessionEvent::Saved {
            revision: snapshot.revision,
            error: result.as_ref().err().map(|error| format!("{error:#}")),
        });
    }
    result
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.tx.send(Message::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
