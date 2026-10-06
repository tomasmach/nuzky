//! Transport-independent resources for one open project.
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};
use anyhow::Context;
use serde_json::Value;

use anyhow::Result;

use crate::{ProjectSession, jobs::Jobs, transcripts::TranscriptStore};

pub struct Host {
    pub session: ProjectSession,
    pub jobs: Jobs,
    pub transcripts: TranscriptStore,
    pub cache_dir: PathBuf,
}

impl Host {
    pub fn stop_run(&self) -> Result<crate::RunResult> {
        let run = self.session.stop_run()?;
        self.jobs.cancel_run(&run.run_id);
        Ok(run)
    }

    pub fn start_job(
        &self, client: &str, run: Option<&str>, kind: &'static str, stamp: crate::Stamp,
        work: impl FnOnce(Arc<AtomicBool>, crate::jobs::Progress) -> Result<Value> + Send + 'static,
    ) -> Result<Value> {
        self.session.register_job(run, || self.jobs.start(client, run, kind, stamp, work))
    }

    /// Retain the project lock while jobs drain, without blocking the app's command thread.
    pub fn retire(self: &Arc<Self>) -> Result<()> {
        self.jobs.begin_shutdown();
        let host = self.clone();
        std::thread::Builder::new().name("capopen-retire-host".into()).spawn(move || {
            host.jobs.shutdown();
            if let Err(error) = host.session.disconnect() { eprintln!("SAVE_FAILED: closing host: {error:#}"); }
        }).context("HOST_CLOSE_FAILED: starting cleanup worker")?;
        Ok(())
    }

    pub fn new(session: ProjectSession, cache_dir: PathBuf) -> Result<Self> {
        Ok(Self {
            session,
            jobs: Jobs::default(),
            transcripts: TranscriptStore::open()?,
            cache_dir,
        })
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.jobs.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mode, EndAction};
    use capopen_engine::{Project, edit::new_id};
    use serde_json::json;
    use std::sync::mpsc;

    #[test]
    fn client_disconnect_and_run_stop_cancel_only_owned_jobs() {
        let dir = std::env::temp_dir().join(format!("host-jobs-{}", new_id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("p.capopen");
        std::fs::write(&path, serde_json::to_vec(&Project::new("jobs")).unwrap()).unwrap();
        let host = Host::new(ProjectSession::open(&path, Mode::Write, None).unwrap(), dir.join("cache")).unwrap();
        let run_a = host.session.begin_run("A".into()).unwrap();
        let (release_a, wait_a) = mpsc::channel();
        let a = host.start_job("client-a", Some(&run_a.run_id), "test", run_a.stamp.clone(), move |_, _| { wait_a.recv().unwrap(); Ok(json!({})) }).unwrap();
        host.session.end_run(&run_a.run_id, EndAction::Keep).unwrap();
        let run_b = host.session.begin_run("B".into()).unwrap();
        let (release_b, wait_b) = mpsc::channel();
        let b = host.start_job("client-b", Some(&run_b.run_id), "test", run_b.stamp.clone(), move |_, _| { wait_b.recv().unwrap(); Ok(json!({})) }).unwrap();
        let (release_idle, wait_idle) = mpsc::channel();
        let idle = host.start_job("client-b", None, "test", run_b.stamp.clone(), move |_, _| { wait_idle.recv().unwrap(); Ok(json!({})) }).unwrap();
        let id = |v: &Value| v["job_id"].as_str().unwrap().to_owned();
        assert_eq!(a["owner"], "client-a"); assert_eq!(a["run_id"], run_a.run_id);
        assert!(host.jobs.get_for(Some("client-b"), &id(&a), true).is_err());
        host.stop_run().unwrap();
        assert_eq!(host.jobs.get(&id(&a), false).unwrap()["cancel_requested"], false);
        assert_eq!(host.jobs.get(&id(&b), false).unwrap()["cancel_requested"], true);
        assert_eq!(host.jobs.get(&id(&idle), false).unwrap()["cancel_requested"], false);
        let rejected = host.start_job("client-b", Some(&run_b.run_id), "test", run_b.stamp.clone(), |_, _| panic!("stopped run must not start"));
        assert!(rejected.unwrap_err().to_string().starts_with("RUN_STOPPED"));
        host.jobs.cancel_owner("client-a");
        assert_eq!(host.jobs.get(&id(&a), false).unwrap()["cancel_requested"], true);
        host.jobs.cancel_owner("client-b");
        assert_eq!(host.jobs.get(&id(&idle), false).unwrap()["cancel_requested"], true);
        assert!(host.start_job("client-b", None, "test", run_b.stamp, |_, _| panic!()).unwrap_err().to_string().starts_with("CLIENT_CLOSED"));
        release_a.send(()).unwrap(); release_b.send(()).unwrap(); release_idle.send(()).unwrap();
        drop(host); std::fs::remove_dir_all(dir).unwrap();
    }
}
