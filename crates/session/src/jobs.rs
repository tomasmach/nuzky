use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread::JoinHandle;

use crate::Stamp;
use anyhow::{Context, Result, ensure};
use nuzky_engine::edit::new_id;
use serde_json::{Value, json};

const MAX_ACTIVE: usize = 4;
/// Results of the most recent finished jobs stay readable; older ones are forgotten.
const MAX_FINISHED: usize = 32;

pub struct JobState {
    pub id: String,
    pub owner: String,
    pub run_id: Option<String>,
    pub kind: &'static str,
    pub stamp: Stamp,
    pub status: &'static str,
    pub progress: Option<f32>,
    pub phase: &'static str,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub cancel: Arc<AtomicBool>,
    sequence: u64,
}

impl JobState {
    fn json(&self) -> Value {
        json!({"job_id": self.id, "kind": self.kind, "revision": self.stamp.revision,
            "session_epoch": self.stamp.session_epoch, "status": self.status,
            "progress": self.progress, "phase": self.phase, "cancel_requested": self.cancel.load(Ordering::Relaxed),
            "owner": self.owner, "run_id": self.run_id, "result": self.result, "error": self.error})
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
struct Lifecycle {
    stopped_runs: HashSet<String>,
    disconnected_clients: HashSet<String>,
    closing: bool,
}

#[derive(Default)]
pub struct Jobs {
    lifecycle: Mutex<Lifecycle>,
    entries: Mutex<HashMap<String, Arc<Mutex<JobState>>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    started: AtomicU64,
}

impl Jobs {
    pub fn start(
        &self,
        owner: &str,
        run_id: Option<&str>,
        kind: &'static str,
        stamp: Stamp,
        work: impl FnOnce(Arc<AtomicBool>, Progress) -> Result<Value> + Send + 'static,
    ) -> Result<Value> {
        let lifecycle = self.lifecycle.lock().unwrap();
        ensure!(!lifecycle.closing, "APP_CLOSED: host is closing");
        ensure!(!lifecycle.disconnected_clients.contains(owner), "CLIENT_CLOSED: client disconnected");
        ensure!(
            run_id.is_none_or(|run| !lifecycle.stopped_runs.contains(run)),
            "RUN_STOPPED: run was stopped by the user"
        );
        let mut entries = self.entries.lock().unwrap();
        ensure!(
            entries.values().filter(|j| j.lock().unwrap().status == "running").count() < MAX_ACTIVE,
            "JOB_LIMIT: wait for or cancel an active job"
        );
        let mut finished: Vec<_> = entries
            .iter()
            .filter_map(|(id, state)| {
                let state = state.lock().unwrap();
                (state.status != "running").then(|| (state.sequence, id.clone()))
            })
            .collect();
        finished.sort_unstable();
        for (_, id) in finished.iter().take(finished.len().saturating_sub(MAX_FINISHED)) {
            entries.remove(id);
        }
        let id = new_id();
        let state = Arc::new(Mutex::new(JobState {
            id: id.clone(),
            owner: owner.into(),
            run_id: run_id.map(str::to_owned),
            kind,
            stamp,
            status: "running",
            progress: None,
            phase: "starting",
            result: None,
            error: None,
            cancel: Arc::new(AtomicBool::new(false)),
            sequence: self.started.fetch_add(1, Ordering::Relaxed),
        }));
        let response = state.lock().unwrap().json();
        let owned = state.clone();
        let worker = std::thread::Builder::new()
            .name(format!("nuzky-{kind}"))
            .spawn(move || {
                let cancel = owned.lock().unwrap().cancel.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    work(cancel.clone(), Progress(owned.clone()))
                }));
                let result = result.unwrap_or_else(|_| Err(anyhow::anyhow!("JOB_FAILED: worker panicked")));
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
            .context("JOB_FAILED: starting thread")?;
        entries.insert(id, state);
        let mut workers = self.workers.lock().unwrap();
        let (done, running): (Vec<_>, Vec<_>) = workers.drain(..).partition(|worker| worker.is_finished());
        *workers = running;
        workers.push(worker);
        for worker in done {
            let _ = worker.join();
        }
        Ok(response)
    }

    pub fn get(&self, id: &str, cancel: bool) -> Result<Value> {
        self.get_for(None, id, cancel)
    }

    pub fn get_for(&self, client: Option<&str>, id: &str, cancel: bool) -> Result<Value> {
        let entries = self.entries.lock().unwrap();
        let state = entries.get(id).context("UNKNOWN_JOB: no such job in this session")?.lock().unwrap();
        ensure!(
            !cancel || client.is_none_or(|client| state.owner == client),
            "UNAUTHORIZED: job belongs to another client"
        );
        if cancel && state.status == "running" {
            state.cancel.store(true, Ordering::Relaxed);
        }
        Ok(state.json())
    }

    /// A running job of `kind`, to name again instead of starting the same work twice.
    pub fn running_id(&self, kind: &str) -> Option<String> {
        let entries = self.entries.lock().unwrap();
        entries
            .values()
            .map(|state| state.lock().unwrap())
            .find(|s| s.status == "running" && s.kind == kind)
            .map(|s| s.id.clone())
    }

    /// Kinds of the jobs that are still running.
    pub fn running(&self) -> Vec<&'static str> {
        let entries = self.entries.lock().unwrap();
        entries.values().map(|state| state.lock().unwrap()).filter(|s| s.status == "running").map(|s| s.kind).collect()
    }

    pub fn cancel_owner(&self, owner: &str) {
        let mut lifecycle = self.lifecycle.lock().unwrap();
        lifecycle.disconnected_clients.insert(owner.into());
        for state in self.entries.lock().unwrap().values() {
            let state = state.lock().unwrap();
            if state.owner == owner && state.status == "running" {
                state.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn cancel_run(&self, run_id: &str) {
        let mut lifecycle = self.lifecycle.lock().unwrap();
        lifecycle.stopped_runs.insert(run_id.into());
        for state in self.entries.lock().unwrap().values() {
            let state = state.lock().unwrap();
            if state.run_id.as_deref() == Some(run_id) {
                state.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn begin_shutdown(&self) {
        let mut lifecycle = self.lifecycle.lock().unwrap();
        lifecycle.closing = true;
        for state in self.entries.lock().unwrap().values() {
            state.lock().unwrap().cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn shutdown(&self) {
        self.begin_shutdown();
        for worker in self.workers.lock().unwrap().drain(..) {
            let _ = worker.join();
        }
    }
}

pub fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "CANCELLED: job cancelled");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_jobs_and_their_threads_are_pruned() {
        let jobs = Jobs::default();
        let mut ids = Vec::new();
        for _ in 0..MAX_FINISHED + 8 {
            let stamp = Stamp { revision: 1, session_epoch: "epoch".into() };
            let started = jobs.start("client", None, "test", stamp, |_, _| Ok(json!({}))).unwrap();
            let id = started["job_id"].as_str().unwrap().to_owned();
            while jobs.get(&id, false).unwrap()["status"] == "running" {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            ids.push(id);
        }
        assert!(jobs.entries.lock().unwrap().len() <= MAX_FINISHED + 1);
        assert!(jobs.workers.lock().unwrap().len() < 8);
        assert!(jobs.get(&ids[0], false).unwrap_err().to_string().starts_with("UNKNOWN_JOB"));
        assert_eq!(jobs.get(ids.last().unwrap(), false).unwrap()["status"], "done");
        assert_eq!(jobs.get(&ids[ids.len() - MAX_FINISHED], false).unwrap()["status"], "done");
    }
    #[test]
    fn cancel_does_not_publish_late_results() {
        let jobs = Jobs::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let started = jobs
            .start("client", None, "test", Stamp { revision: 7, session_epoch: "epoch".into() }, move |_, _| {
                rx.recv().unwrap();
                Ok(json!({"late":true}))
            })
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
