use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

pub(crate) fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Temporary JSON writes live beside their destination, inside its sidecar namespace.
pub fn json_temp_path(path: &Path) -> PathBuf {
    sidecar(path, &format!(".{}.tmp", nuzky_engine::edit::new_id()))
}

pub fn save(path: &Path, value: &impl Serialize) -> Result<()> {
    save_bytes(path, &serde_json::to_vec_pretty(value).context("Serializing project state")?)
}

pub(crate) fn save_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = json_temp_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("Creating {}", tmp.display()))?;
        file.write_all(bytes).context("Writing project state")?;
        file.sync_all().context("Syncing project state")?;
        fs::rename(&tmp, path).with_context(|| format!("Saving {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Writes a new project file without ever replacing one, also one created since the caller looked.
pub fn publish_new(path: &Path, value: &impl Serialize) -> Result<()> {
    let tmp = json_temp_path(path);
    let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let result = (|| {
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        // Unlike rename, hard_link refuses a destination created since the existence check.
        match fs::hard_link(&tmp, path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(anyhow::Error::new(error).context("Publishing new project"));
            }
            // FAT32 and exFAT have no hard links: claim the name first, so nothing is replaced.
            Err(_) => {
                OpenOptions::new().write(true).create_new(true).open(path).context("Publishing new project")?;
                if let Err(error) = fs::rename(&tmp, path) {
                    let _ = fs::remove_file(path);
                    return Err(anyhow::Error::new(error).context("Publishing new project"));
                }
            }
        }
        Ok(())
    })();
    drop(file);
    let _ = fs::remove_file(tmp);
    result
}

/// No project at `path`, and no history or checkpoint left from one, which a new project there would inherit.
pub fn vacant(path: &Path) -> bool {
    [PathBuf::from(path), sidecar(path, crate::HISTORY_SUFFIX), sidecar(path, ".checkpoint.json")]
        .iter()
        .all(|p| p.symlink_metadata().is_err())
}

/// The returned file owns the OS lock; keep it alive through any pending writes.
pub fn lock_project(path: &Path, exclusive: bool) -> Result<File> {
    let path = if path.exists() {
        fs::canonicalize(path).context("Resolving project path")?
    } else {
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        fs::canonicalize(parent)
            .context("Resolving project directory")?
            .join(path.file_name().context("Project needs a filename")?)
    };
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(sidecar(&path, ".lock"))
        .context("Opening project lock")?;
    let result = if exclusive { lock.try_lock() } else { lock.try_lock_shared() };
    match result {
        Ok(()) => Ok(lock),
        Err(TryLockError::WouldBlock) => bail!(
            "PROJECT_BUSY: This project is open in another Nuzky window or an AI agent is editing it. Close it there first."
        ),
        Err(TryLockError::Error(error)) => Err(error).context("Acquiring project lock"),
    }
}
