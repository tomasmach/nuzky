use serde::Serialize;

use super::*;

#[derive(Serialize)]
struct Checkpoint<'a> {
    run_id: &'a str,
    label: &'a str,
    session_epoch: &'a str,
    revision: u64,
    project: &'a Project,
}

pub(crate) struct Run {
    pub(crate) info: RunInfo,
    touched: Instant,
    ending: Option<EndAction>,
}

fn run_key(id: &str) -> String {
    format!("run:{id}")
}

impl ProjectSession {
    pub(crate) fn register_job<T>(&self, run: Option<&str>, register: impl FnOnce() -> Result<T>) -> Result<T> {
        let inner = self.inner.lock().unwrap();
        if let Some(run) = run { inner.owns_run(run)?; }
        register()
    }

    pub fn check_run(&self, run_id: &str) -> Result<()> {
        self.inner.lock().unwrap().owns_run(run_id)
    }

    pub fn begin_run(&self, label: String) -> Result<RunResult> {
        let mut inner = self.inner.lock().unwrap();
        inner.touch()?;
        inner.writable()?;
        ensure!(inner.run.is_none(), "RUN_BUSY: a run is already open");
        inner.recovered()?;
        let run_id = new_id();
        inner.editor.seal();
        let checkpoint = Checkpoint {
            run_id: &run_id,
            label: &label,
            session_epoch: &inner.epoch,
            revision: inner.editor.revision,
            project: &inner.editor.project,
        };
        inner
            .writer
            .as_ref()
            .context("READ_ONLY: no writer")?
            .checkpoint(serde_json::to_value(checkpoint).context("Serializing checkpoint")?)?;
        let info = RunInfo {
            run_id: run_id.clone(),
            label,
        };
        inner.run = Some(Run {
            info: info.clone(),
            touched: Instant::now(),
            ending: None,
        });
        inner.emit(SessionEvent::Run(Some(info)));
        Ok(RunResult {
            run_id,
            stamp: inner.stamp(),
        })
    }

    pub fn apply_edits(
        &self,
        run_id: &str,
        request_id: &str,
        cmds: Vec<EditCmd>,
        expect: Expect,
    ) -> Result<EditResult> {
        let mut inner = self.inner.lock().unwrap();
        inner.writable()?;
        inner.not_stopped(run_id)?;
        inner.touch()?;
        ensure!(
            !request_id.is_empty(),
            "INVALID_REQUEST: request_id cannot be empty"
        );
        inner.owns_run(run_id)?;
        ensure!(
            inner.run.as_ref().is_some_and(|run| run.ending.is_none()),
            "RUN_ENDING: retry ending the run before editing"
        );
        let key = (run_id.to_owned(), request_id.to_owned());
        let content = serde_json::to_value(&cmds).context("Serializing edit request")?;
        if let Some(previous) = inner.requests.get(&key) {
            ensure!(
                previous.content == content && previous.expect == expect,
                "REQUEST_CONFLICT: request_id was used with different content"
            );
            // A failed save must never cause the batch to execute twice on retry.
            inner.flush()?;
            return Ok(previous.result.clone());
        }
        inner.check_expect(&expect)?;
        let origin = inner.run_origin()?;
        let result = inner.apply(cmds, Some(run_key(run_id)), origin)?;
        inner.requests.insert(
            key,
            Request {
                content,
                expect,
                result: result.clone(),
            },
        );
        inner.flush()?;
        Ok(result)
    }

    pub fn end_run(&self, run_id: &str, action: EndAction) -> Result<RunResult> {
        let mut inner = self.inner.lock().unwrap();
        inner.writable()?;
        inner.not_stopped(run_id)?;
        inner.touch()?;
        inner.owns_run(run_id)?;
        inner.finish(action)?;
        Ok(RunResult {
            run_id: run_id.into(),
            stamp: inner.stamp(),
        })
    }

    pub fn stop_run(&self) -> Result<RunResult> {
        let mut inner = self.inner.lock().unwrap();
        inner.writable()?;
        let run_id = inner
            .run
            .as_ref()
            .context("INVALID_RUN: no open run")?
            .info
            .run_id
            .clone();
        inner.stopped_runs.insert(run_id.clone());
        inner.finish(EndAction::Keep)?;
        Ok(RunResult {
            run_id,
            stamp: inner.stamp(),
        })
    }

    pub fn undo_run(&self, run_id: &str) -> Result<RunResult> {
        let mut inner = self.inner.lock().unwrap();
        inner.user_editable()?;
        ensure!(
            inner.editor.last_key() == Some(run_key(run_id).as_str()),
            "UNDO_UNAVAILABLE: run is not the last history entry"
        );
        inner.editor.undo();
        inner.changed(Origin::Undo);
        inner.flush()?;
        Ok(RunResult {
            run_id: run_id.into(),
            stamp: inner.stamp(),
        })
    }
}

impl Inner {
    fn not_stopped(&self, id: &str) -> Result<()> {
        ensure!(
            !self.stopped_runs.contains(id),
            "RUN_STOPPED: run was stopped by the user"
        );
        Ok(())
    }

    fn owns_run(&self, id: &str) -> Result<()> {
        self.not_stopped(id)?;
        ensure!(
            self.run.as_ref().is_some_and(|r| r.info.run_id == id),
            "INVALID_RUN: run is not open or has ended"
        );
        Ok(())
    }

    fn run_origin(&self) -> Result<Origin> {
        let info = &self.run.as_ref().context("INVALID_RUN: no open run")?.info;
        Ok(Origin::Run {
            run_id: info.run_id.clone(),
            label: info.label.clone(),
        })
    }

    pub(super) fn expire(&mut self) -> Result<()> {
        if self
            .run
            .as_ref()
            .is_some_and(|r| r.touched.elapsed() >= self.timeout)
        {
            self.finish(EndAction::Keep)?;
        }
        Ok(())
    }

    pub(super) fn touch(&mut self) -> Result<()> {
        self.expire()?;
        if let Some(run) = &mut self.run {
            run.touched = Instant::now();
        }
        Ok(())
    }

    pub(super) fn finish(&mut self, action: EndAction) -> Result<()> {
        let run = self.run.as_mut().context("INVALID_RUN: no open run")?;
        let action = *run.ending.get_or_insert(action);
        if matches!(action, EndAction::Discard) && self.editor.drop_last(&run_key(&run.info.run_id))
        {
            self.changed(self.run_origin()?);
        }
        self.editor.seal();
        self.flush()?;
        // On failure leave the marker and the run available for another finish attempt.
        fs::remove_file(storage::sidecar(&self.path, ".checkpoint.json"))
            .context("Removing run checkpoint")?;
        self.run = None;
        self.requests.clear();
        self.emit(SessionEvent::Run(None));
        Ok(())
    }
}
