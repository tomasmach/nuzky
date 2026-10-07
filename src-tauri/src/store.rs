//! Project files on disk. Every edit is saved automatically shortly after it happens.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::SystemTime;

use anyhow::{Context, Result};
use capopen_engine::Project;
use capopen_session::{Mode, ProjectSession, SessionEvent};
use serde::Serialize;

pub const EXTENSION: &str = "capopen";

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

pub fn open(path: &Path, events: Sender<SessionEvent>) -> Result<ProjectSession> {
    ProjectSession::open(path, Mode::Write, Some(events)).map_err(|error| {
        if error.to_string().starts_with("PROJECT_BUSY:") {
            anyhow::anyhow!(
                "This project is open in another CapOpen window or an AI agent is editing it. Close it there first."
            )
        } else {
            error
        }
    })
}

pub fn create(path: &Path, project: &Project) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _lock = capopen_session::lock_project(path, true)?;
    save(path, project)
}

/// Writes through a temporary file so a crash never leaves a half-written project.
pub fn save(path: &Path, project: &Project) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    capopen_session::write_json_atomic(path, project)
}

pub fn new_project_path() -> PathBuf {
    projects_dir().join(format!("{}.{EXTENSION}", capopen_engine::edit::new_id()))
}

pub fn list() -> Vec<ProjectSummary> {
    list_in(&projects_dir())
}

pub(crate) fn list_in(dir: &Path) -> Vec<ProjectSummary> {
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
    out.sort_by_key(|entry| std::cmp::Reverse(entry.modified_ms));
    out
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SavedEvent {
    pub revision: u64,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_session_owns_lock_and_reports_busy_projects() {
        let _children = crate::CHILD_SPAWN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!("capopen-store-{}", capopen_engine::edit::new_id()));
        let path = dir.join("project.capopen");
        let project = Project::new("Lock test");
        create(&path, &project).unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let desktop = open(&path, tx.clone()).unwrap();
        assert!(ProjectSession::open(&path, Mode::Write, None).is_err());
        assert!(ProjectSession::open(&path, Mode::ReadOnly, None).is_ok());
        assert_eq!(load(&path).unwrap(), project);
        assert_eq!(list_in(&dir)[0].name, project.name);
        drop(desktop);
        let agent = ProjectSession::open(&path, Mode::Write, None).unwrap();
        assert_eq!(
            open(&path, tx).err().unwrap().to_string(),
            "This project is open in another CapOpen window or an AI agent is editing it. Close it there first."
        );
        drop(agent);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
