use serde::Deserialize;

use super::*;

#[derive(Deserialize)]
struct RecoveryCheckpoint {
    project: Project,
}

impl ProjectSession {
    pub fn resolve_recovery(&self, action: RecoveryAction) -> Result<Stamp> {
        let mut inner = self.inner.lock().unwrap();
        inner.writable()?;
        ensure!(inner.run.is_none(), "RUN_BUSY: end the open run before recovery");
        let path = inner.recovery.as_ref().context("NO_RECOVERY: no leftover checkpoint")?.clone();
        if matches!(action, RecoveryAction::Restore) {
            let checkpoint: RecoveryCheckpoint =
                serde_json::from_slice(&fs::read(&path).context("Reading recovery checkpoint")?)
                    .context("Parsing recovery checkpoint")?;
            inner.record();
            let revision = inner.editor.revision;
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                inner.editor.replace_project_checked(checkpoint.project, None, validate)
            }))
            .map_err(|_| {
                anyhow::anyhow!("EDIT_REJECTED: engine failed while restoring the checkpoint; project unchanged")
            })??;
            if inner.editor.revision != revision {
                inner.changed(Origin::Recovery);
                inner.history.tip = Some(Tip::user("Restore previous version"));
                inner.record();
            }
        }
        inner.flush()?;
        fs::remove_file(&path).context("Removing recovery checkpoint")?;
        inner.recovery = None;
        Ok(inner.stamp())
    }
}
