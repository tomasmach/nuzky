use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

use anyhow::{Context, Result, ensure};
use capopen_engine::edit::new_id;
use capopen_session::Stamp;
use serde_json::{Value, json};

use crate::transcript::TranscriptRecord;

const MAX_ACTIVE: usize = 4;

pub enum Output {
    Json(Value),
    Transcript(TranscriptRecord),
}

pub struct JobState {
    pub id: String,
    pub kind: &'static str,
    pub stamp: Stamp,
    pub status: &'static str,
    pub progress: Option<f32>,
    pub phase: &'static str,
    pub result: Option<Output>,
    pub error: Option<String>,
    pub cancel: Arc<AtomicBool>,
}

impl JobState {
    fn json(&self) -> Value {
        let result = match &self.result {
            Some(Output::Json(v)) => v.clone(),
            Some(Output::Transcript(record)) => {
                json!({"transcript_id": self.id, "target": record.target, "transcript": record.transcript})
            }
            None => Value::Null,
        };
        json!({"job_id": self.id, "kind": self.kind, "revision": self.stamp.revision,
            "session_epoch": self.stamp.session_epoch, "status": self.status,
            "progress": self.progress, "phase": self.phase, "cancel_requested": self.cancel.load(Ordering::Relaxed),
            "result": result, "error": self.error})
    }
}

#[derive(Clone)]
pub struct Progress(Arc<Mutex<JobState>>);
impl Progress {
    pub fn set(&self, phase: &'static str, fraction: Option<f32>) {
        let mut state = self.0.lock().unwrap();
        state.phase = phase;
        state.progress = fraction;
    }
}

#[derive(Default)]
pub struct Jobs {
    entries: Mutex<HashMap<String, Arc<Mutex<JobState>>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl Jobs {
    pub fn start(
        &self,
        kind: &'static str,
        stamp: Stamp,
        work: impl FnOnce(Arc<AtomicBool>, Progress) -> Result<Output> + Send + 'static,
    ) -> Result<Value> {
        let mut entries = self.entries.lock().unwrap();
        ensure!(
            entries
                .values()
                .filter(|j| j.lock().unwrap().status == "running")
                .count()
                < MAX_ACTIVE,
            "JOB_LIMIT: wait for or cancel an active job"
        );
        let id = new_id();
        let state = Arc::new(Mutex::new(JobState {
            id: id.clone(),
            kind,
            stamp,
            status: "running",
            progress: None,
            phase: "starting",
            result: None,
            error: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }));
        let response = state.lock().unwrap().json();
        let owned = state.clone();
        let worker = std::thread::Builder::new()
            .name(format!("capopen-{kind}"))
            .spawn(move || {
                let cancel = owned.lock().unwrap().cancel.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    work(cancel.clone(), Progress(owned.clone()))
                }));
                let result = result.unwrap_or_else(|_| Err(anyhow::anyhow!("Worker panicked")));
                let mut state = owned.lock().unwrap();
                // An export that has already atomically published its file is complete.
                if cancel.load(Ordering::Relaxed) && !(kind == "export" && result.is_ok()) {
                    state.status = "cancelled";
                    state.phase = "cancelled";
                } else {
                    match result {
                        Ok(output) => {
                            state.status = "done";
                            state.phase = "done";
                            state.progress = Some(1.0);
                            state.result = Some(output);
                        }
                        Err(error) => {
                            state.status = "failed";
                            state.phase = "failed";
                            state.error = Some(format!("{error:#}"));
                        }
                    }
                }
            })
            .context("Starting job thread")?;
        entries.insert(id, state);
        self.workers.lock().unwrap().push(worker);
        Ok(response)
    }

    pub fn get(&self, id: &str, cancel: bool) -> Result<Value> {
        let entries = self.entries.lock().unwrap();
        let state = entries
            .get(id)
            .context("UNKNOWN_JOB: no such job in this session")?
            .lock()
            .unwrap();
        if cancel && state.status == "running" {
            state.cancel.store(true, Ordering::Relaxed);
        }
        Ok(state.json())
    }

    pub fn transcript(&self, id: &str) -> Result<TranscriptRecord> {
        let entries = self.entries.lock().unwrap();
        let state = entries
            .get(id)
            .context("UNKNOWN_TRANSCRIPT: use a completed transcription job id")?
            .lock()
            .unwrap();
        match &state.result {
            Some(Output::Transcript(record)) => Ok(record.clone()),
            _ => anyhow::bail!("TRANSCRIPT_NOT_READY: job is {}", state.status),
        }
    }

    pub fn shutdown(&self) {
        for state in self.entries.lock().unwrap().values() {
            state.lock().unwrap().cancel.store(true, Ordering::Relaxed);
        }
        for worker in self.workers.lock().unwrap().drain(..) {
            let _ = worker.join();
        }
    }
}

pub fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "Job cancelled");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancel_does_not_publish_late_results() {
        let jobs = Jobs::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let started = jobs
            .start(
                "test",
                Stamp {
                    revision: 7,
                    session_epoch: "epoch".into(),
                },
                move |_, _| {
                    rx.recv().unwrap();
                    Ok(Output::Json(json!({"late":true})))
                },
            )
            .unwrap();
        let id = started["job_id"].as_str().unwrap();
        let cancel = jobs.get(id, true).unwrap();
        assert_eq!(cancel["cancel_requested"], true);
        assert_eq!(cancel["status"], "running");
        tx.send(()).unwrap();
        jobs.shutdown();
        let result = jobs.get(id, false).unwrap();
        assert_eq!(result["status"], "cancelled");
        assert_eq!(result["result"], Value::Null);
        assert_eq!(result["revision"], 7);
    }
}
