//! Transport-independent resources for one open project.
use std::path::PathBuf;

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
        self.jobs.cancel_owner(&run.run_id);
        Ok(run)
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
