//! Project files on disk. Every edit is saved automatically shortly after it happens.

use std::path::{Path, PathBuf};
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
    let mut out: Vec<ProjectSummary> = std::fs::read_dir(projects_dir())
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
    tx: Sender<(PathBuf, Project, u64)>,
}

impl Saver {
    pub fn start(app: AppHandle) -> Self {
        let (tx, rx) = channel::<(PathBuf, Project, u64)>();
        std::thread::Builder::new()
            .name("autosave".into())
            .spawn(move || {
                let write = |(path, project, revision): (PathBuf, Project, u64)| {
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

    pub fn schedule(&self, path: PathBuf, project: Project, revision: u64) {
        self.tx.send((path, project, revision)).ok();
    }
}
