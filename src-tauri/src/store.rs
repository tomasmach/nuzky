//! Project files on disk. Every edit is saved automatically shortly after it happens.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use capopen_engine::Project;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const EXTENSION: &str = "capopen";
const DEBOUNCE: Duration = Duration::from_millis(800);

pub fn projects_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("capopen").join("projects")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("capopen")
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub path: String,
    pub name: String,
    pub modified_ms: u64,
    pub duration_us: i64,
}

pub fn load(path: &Path) -> Result<Project> {
    let json = std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    serde_json::from_str(&json).with_context(|| format!("{} is not a CapOpen project", path.display()))
}

fn project_lock(path: &Path) -> Result<Arc<File>> {
    capopen_session::lock_project(path, true).map(Arc::new).map_err(|error| {
        if error.to_string().starts_with("PROJECT_BUSY:") {
            anyhow::anyhow!("An AI agent is editing this project outside CapOpen. Close it there first.")
        } else {
            error
        }
    })
}

pub fn open(path: &Path) -> Result<(Project, Arc<File>)> {
    let lock = project_lock(path)?;
    Ok((load(path)?, lock))
}

pub fn create(path: &Path, project: &Project) -> Result<Arc<File>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let lock = project_lock(path)?;
    save(path, project)?;
    Ok(lock)
}

/// Writes through a temporary file so a crash never leaves a half-written project.
pub fn save(path: &Path, project: &Project) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(project)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn new_project_path() -> PathBuf {
    projects_dir().join(format!("{}.{EXTENSION}", capopen_engine::edit::new_id()))
}

pub fn list() -> Vec<ProjectSummary> {
    list_in(&projects_dir())
}

fn list_in(dir: &Path) -> Vec<ProjectSummary> {
    let mut out: Vec<ProjectSummary> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == EXTENSION))
        .filter_map(|e| {
            let project = load(&e.path()).ok()?;
            let modified = e.metadata().ok()?.modified().ok()?.duration_since(SystemTime::UNIX_EPOCH).ok()?;
            Some(ProjectSummary {
                path: e.path().to_string_lossy().into_owned(),
                name: project.name.clone(),
                modified_ms: modified.as_millis() as u64,
                duration_us: project.duration_us(),
            })
        })
        .collect();
    out.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    out
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SavedEvent {
    pub revision: u64,
    pub error: Option<String>,
}

/// Debounced background saver.
pub struct Saver {
    tx: Sender<(PathBuf, Project, u64, Arc<File>)>,
}

impl Saver {
    pub fn start(app: AppHandle) -> Self {
        let (tx, rx) = channel::<(PathBuf, Project, u64, Arc<File>)>();
        std::thread::Builder::new()
            .name("autosave".into())
            .spawn(move || {
                let write = |(path, project, revision, _lock): (PathBuf, Project, u64, Arc<File>)| {
                    let error = save(&path, &project).err().map(|e| format!("{e:#}"));
                    if let Some(e) = &error {
                        log::error!("Autosave failed: {e}");
                    }
                    app.emit("saved", SavedEvent { revision, error }).ok();
                };
                while let Ok(mut job) = rx.recv() {
                    // Keep only the newest version once edits stop for a moment, but write a
                    // pending project right away when another one takes its place.
                    while let Ok(newer) = rx.recv_timeout(DEBOUNCE) {
                        if newer.0 != job.0 {
                            write(std::mem::replace(&mut job, newer));
                        } else {
                            job = newer;
                        }
                    }
                    write(job);
                }
            })
            .expect("spawn autosave");
        Self { tx }
    }

    pub fn schedule(&self, path: PathBuf, project: Project, revision: u64, lock: Arc<File>) {
        self.tx.send((path, project, revision, lock)).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capopen_session::{Mode, ProjectSession};

    #[test]
    fn desktop_and_session_locks_exclude_each_other_and_pending_saves_keep_lock() {
        let dir = std::env::temp_dir().join(format!("capopen-store-{}", capopen_engine::edit::new_id()));
        let path = dir.join("project.capopen");
        let project = Project::new("Lock test");
        let created_lock = create(&path, &project).unwrap();
        assert!(ProjectSession::open(&path, Mode::Write).err().unwrap().to_string().contains("PROJECT_BUSY"));
        drop(created_lock);
        let agent = ProjectSession::open(&path, Mode::Write).unwrap();
        assert_eq!(open(&path).unwrap_err().to_string(), "An AI agent is editing this project outside CapOpen. Close it there first.");
        assert_eq!(load(&path).unwrap(), project);
        drop(agent);
        let (loaded, app_lock) = open(&path).unwrap();
        assert_eq!(loaded, project);
        let listed = list_in(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, project.name);
        for mode in [Mode::Write, Mode::ReadOnly] {
            assert!(ProjectSession::open(&path, mode).err().unwrap().to_string().contains("PROJECT_BUSY"));
        }
        let pending_save_lock = app_lock.clone();
        drop(app_lock);
        assert!(ProjectSession::open(&path, Mode::Write).is_err());
        save(&path, &loaded).unwrap();
        drop(pending_save_lock);
        drop(ProjectSession::open(&path, Mode::Write).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
