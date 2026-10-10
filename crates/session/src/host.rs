//! Transport-independent resources for one open project.
use anyhow::Context;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

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
        self.end_open_run(crate::EndAction::Keep)
    }

    /// Stops the agent's open run and its jobs; `Discard` also takes back what it changed.
    pub fn end_open_run(&self, action: crate::EndAction) -> Result<crate::RunResult> {
        self.session.stop_run_with(|run_id| self.jobs.cancel_run(run_id), action)
    }

    pub fn start_job(
        &self,
        client: &str,
        run: Option<&str>,
        kind: &'static str,
        stamp: crate::Stamp,
        work: impl FnOnce(Arc<AtomicBool>, crate::jobs::Progress) -> Result<Value> + Send + 'static,
    ) -> Result<Value> {
        self.session.register_job(run, || self.jobs.start(client, run, kind, stamp, work))
    }

    /// Retain the project lock while jobs drain, without blocking the app's command thread.
    pub fn retire(self: &Arc<Self>) -> Result<()> {
        self.jobs.begin_shutdown();
        let host = self.clone();
        std::thread::Builder::new()
            .name("nuzky-retire-host".into())
            .spawn(move || {
                host.jobs.shutdown();
                if let Err(error) = host.session.disconnect() {
                    eprintln!("SAVE_FAILED: closing host: {error:#}");
                }
            })
            .context("HOST_CLOSE_FAILED: starting cleanup worker")?;
        Ok(())
    }

    pub fn new(session: ProjectSession, cache_dir: PathBuf) -> Result<Self> {
        let transcripts = TranscriptStore::open()?.with_events(session.inner.lock().unwrap().events.clone());
        Ok(Self { session, jobs: Jobs::default(), transcripts, cache_dir })
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
    use crate::{EndAction, Mode};
    use nuzky_engine::{Project, edit::new_id};
    use serde_json::json;
    use std::sync::mpsc;

    #[test]
    fn transcript_publication_notifies_host_from_cloned_store() {
        use crate::transcripts::{Record, VERSION};
        use nuzky_engine::model::{Asset, AssetKind};
        let dir = std::env::temp_dir().join(format!("host-transcripts-{}", new_id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("p.nuzky");
        std::fs::write(&path, serde_json::to_vec(&Project::new("events")).unwrap()).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut host =
            Host::new(ProjectSession::open(&path, Mode::Write, Some(tx.clone())).unwrap(), dir.join("cache")).unwrap();
        // Keep test records out of the user's transcript store, retaining its host event channel.
        host.transcripts = TranscriptStore::at(dir.join("transcripts"))
            .unwrap()
            .with_events(host.session.inner.lock().unwrap().events.clone());
        let source = dir.join("source");
        std::fs::write(&source, b"source").unwrap();
        let asset = Asset {
            id: "source".into(),
            name: "source".into(),
            path: source.to_string_lossy().into(),
            kind: AssetKind::Audio,
            duration_us: 1_000_000,
            width: 0,
            height: 0,
            fps: 0.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
            credit: None,
        };
        let store = host.transcripts.clone();
        let mut record = Record {
            version: VERSION,
            fingerprint: store.fingerprint(&asset).unwrap(),
            duration_us: asset.duration_us,
            model: "test".into(),
            language: "en".into(),
            words: vec![],
            segments: vec![],
            alignment: None,
        };
        for _ in 0..2 {
            store.put(&asset, &record).unwrap();
            assert!(matches!(
                rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
                crate::SessionEvent::TranscriptsChanged
            ));
            assert_eq!(host.transcripts.get(&asset).unwrap(), Some(record.clone()));
        }
        record.version = 0;
        assert!(store.put(&asset, &record).is_err());
        assert!(rx.try_recv().is_err());
        drop(host);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stop_cancels_jobs_even_when_saving_fails() {
        let dir = std::env::temp_dir().join(format!("host-stop-{}", new_id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("p.nuzky");
        std::fs::write(&path, serde_json::to_vec(&Project::new("stop")).unwrap()).unwrap();
        let host = Host::new(ProjectSession::open(&path, Mode::Write, None).unwrap(), dir.join("cache")).unwrap();
        let run = host.session.begin_run("export".into()).unwrap();
        let (release, wait) = mpsc::channel();
        let job = host
            .start_job("client", Some(&run.run_id), "export", run.stamp.clone(), move |_, _| {
                wait.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                Ok(json!({}))
            })
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let error = host.stop_run().unwrap_err();
        let cancelled = host.jobs.get(job["job_id"].as_str().unwrap(), false).unwrap()["cancel_requested"].clone();
        release.send(()).unwrap();
        assert!(error.to_string().contains("SAVE_FAILED"));
        assert_eq!(cancelled, true);
        assert!(
            host.start_job("client", Some(&run.run_id), "export", run.stamp, |_, _| panic!())
                .unwrap_err()
                .to_string()
                .starts_with("RUN_STOPPED")
        );
        drop(host);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn client_disconnect_and_run_stop_cancel_only_owned_jobs() {
        let dir = std::env::temp_dir().join(format!("host-jobs-{}", new_id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("p.nuzky");
        std::fs::write(&path, serde_json::to_vec(&Project::new("jobs")).unwrap()).unwrap();
        let host = Host::new(ProjectSession::open(&path, Mode::Write, None).unwrap(), dir.join("cache")).unwrap();
        let run_a = host.session.begin_run("A".into()).unwrap();
        let (release_a, wait_a) = mpsc::channel();
        let a = host
            .start_job("client-a", Some(&run_a.run_id), "test", run_a.stamp.clone(), move |_, _| {
                wait_a.recv().unwrap();
                Ok(json!({}))
            })
            .unwrap();
        host.session.end_run(&run_a.run_id, EndAction::Keep).unwrap();
        let run_b = host.session.begin_run("B".into()).unwrap();
        let (release_b, wait_b) = mpsc::channel();
        let b = host
            .start_job("client-b", Some(&run_b.run_id), "test", run_b.stamp.clone(), move |_, _| {
                wait_b.recv().unwrap();
                Ok(json!({}))
            })
            .unwrap();
        let (release_idle, wait_idle) = mpsc::channel();
        let idle = host
            .start_job("client-b", None, "test", run_b.stamp.clone(), move |_, _| {
                wait_idle.recv().unwrap();
                Ok(json!({}))
            })
            .unwrap();
        let id = |v: &Value| v["job_id"].as_str().unwrap().to_owned();
        assert_eq!(a["owner"], "client-a");
        assert_eq!(a["run_id"], run_a.run_id);
        assert!(host.jobs.get_for(Some("client-b"), &id(&a), true).is_err());
        host.stop_run().unwrap();
        assert_eq!(host.jobs.get(&id(&a), false).unwrap()["cancel_requested"], false);
        assert_eq!(host.jobs.get(&id(&b), false).unwrap()["cancel_requested"], true);
        assert_eq!(host.jobs.get(&id(&idle), false).unwrap()["cancel_requested"], false);
        let rejected = host.start_job("client-b", Some(&run_b.run_id), "test", run_b.stamp.clone(), |_, _| {
            panic!("stopped run must not start")
        });
        assert!(rejected.unwrap_err().to_string().starts_with("RUN_STOPPED"));
        host.jobs.cancel_owner("client-a");
        assert_eq!(host.jobs.get(&id(&a), false).unwrap()["cancel_requested"], true);
        host.jobs.cancel_owner("client-b");
        assert_eq!(host.jobs.get(&id(&idle), false).unwrap()["cancel_requested"], true);
        assert!(
            host.start_job("client-b", None, "test", run_b.stamp, |_, _| panic!())
                .unwrap_err()
                .to_string()
                .starts_with("CLIENT_CLOSED")
        );
        release_a.send(()).unwrap();
        release_b.send(()).unwrap();
        release_idle.send(()).unwrap();
        drop(host);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
