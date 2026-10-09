//! Versions of the project, kept in `<project>.history.jsonl` beside it so they outlive the session.
//! A version is the project after one step: an agent's run, one of the user's edits (a whole drag or
//! typing burst), an undo, a redo or a restore. Its hash is the SHA-256 of the project's compact JSON.
//! The file holds one version per line and grows by appending; a line leaves out a project an earlier
//! line holds. Once it holds twice what is kept, it is written again with only the kept versions.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use nuzky_engine::{Project, edit::EditCmd};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

use super::*;

pub const HISTORY_SUFFIX: &str = ".history.jsonl";
/// The newest versions kept.
pub const MAX_VERSIONS: usize = 200;
/// Project JSON kept over all versions, each distinct project counted once.
pub const MAX_BYTES: usize = 32 << 20;
/// A longer file is not ours to read; it is started anew.
const READ_LIMIT: u64 = 4 * MAX_BYTES as u64;
/// Hex digits of a hash shown; `undo_to` takes any prefix of four or more.
const SHORT: usize = 12;

#[derive(Clone, Debug, Serialize)]
pub struct VersionInfo {
    /// Stays with the version across restarts and as older versions go.
    pub index: u64,
    pub hash: String,
    pub label: String,
    /// Unix time in milliseconds.
    pub at_ms: u64,
    /// The agent's run that made it; none for the user's own steps.
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HistoryList {
    /// The hash of the project as it is now; a version with it is the current one.
    pub current_hash: String,
    /// Newest first.
    pub versions: Vec<VersionInfo>,
    /// Older versions kept but not listed.
    pub older: usize,
}

#[derive(Clone, Debug)]
pub enum Target {
    Index(u64),
    /// A prefix of the hash.
    Hash(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct Restored {
    #[serde(flatten)]
    pub stamp: Stamp,
    pub restored: VersionInfo,
    /// False when the project already was that version.
    pub changed: bool,
}

#[derive(Clone)]
struct Version {
    index: u64,
    hash: String,
    label: String,
    at_ms: u64,
    run_id: Option<String>,
    json: Arc<str>,
}

impl Version {
    fn info(&self) -> VersionInfo {
        VersionInfo {
            index: self.index,
            hash: self.hash[..SHORT].to_owned(),
            label: self.label.clone(),
            at_ms: self.at_ms,
            run_id: self.run_id.clone(),
        }
    }
}

/// How the project as it is now came about, until it is kept as a version.
pub(crate) struct Tip {
    pub label: String,
    pub run_id: Option<String>,
}

impl Tip {
    pub(crate) fn user(label: impl Into<String>) -> Self {
        Self { label: label.into(), run_id: None }
    }
}

pub(crate) enum Write {
    Append(String),
    Replace(String),
}

#[derive(Default)]
pub(crate) struct History {
    versions: Vec<Version>,
    pub(crate) tip: Option<Tip>,
    next: u64,
    /// Hashes whose project the file holds, so a later line may leave it out.
    stored: HashSet<String>,
    lines: usize,
    bytes: usize,
    /// Set when the file is torn or a write failed: the next write replaces it whole.
    pub(crate) broken: Arc<AtomicBool>,
}

#[derive(Serialize)]
struct LineOut<'a> {
    index: u64,
    hash: &'a str,
    label: &'a str,
    at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<&'a str>,
}

#[derive(Deserialize)]
struct Line<'a> {
    index: u64,
    hash: String,
    label: String,
    at_ms: u64,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(borrow, default)]
    project: Option<&'a RawValue>,
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn line(version: &Version, with_project: bool) -> String {
    let meta = LineOut {
        index: version.index,
        hash: &version.hash,
        label: &version.label,
        at_ms: version.at_ms,
        run_id: version.run_id.as_deref(),
    };
    let mut line = serde_json::to_string(&meta).expect("plain fields serialize");
    if with_project {
        // Verbatim, so the bytes read back are the bytes hashed.
        line.pop();
        line.push_str(",\"project\":");
        line.push_str(&version.json);
        line.push('}');
    }
    line.push('\n');
    line
}

/// What a batch of the user's edits did, named by its first command.
pub(crate) fn label(cmds: &[EditCmd]) -> &'static str {
    let Some(cmd) = cmds.first() else { return "Edit" };
    match cmd {
        EditCmd::AddAssets { .. } => "Import media",
        EditCmd::RemoveAsset { .. } => "Remove media",
        EditCmd::AddClip { .. } => "Add clip",
        EditCmd::AddText { .. } => "Add text",
        EditCmd::MoveClip { .. } => "Move clip",
        EditCmd::TrimClip { .. } => "Trim clip",
        EditCmd::SplitClip { .. } => "Split",
        EditCmd::DeleteClips { .. } => "Delete",
        EditCmd::UpdateClip { .. } => "Change clip",
        EditCmd::SetAnimation { .. } => "Animation",
        EditCmd::SetTransition { .. } => "Transition",
        EditCmd::SetKeyframes { .. } => "Keyframes",
        EditCmd::DuplicateClip { .. } => "Duplicate",
        EditCmd::DetachAudio { .. } => "Detach audio",
        EditCmd::UpdateTrack { .. } => "Change track",
        EditCmd::SetCanvas { .. } => "Change format",
        EditCmd::AddCaptions { .. } => "Add captions",
        EditCmd::ReplaceCaptions { .. } => "Replace captions",
        EditCmd::RippleDeleteRanges { .. } => "Cut",
        EditCmd::RenameProject { .. } => "Rename project",
        EditCmd::CorrectWords { .. } => "Correct words",
        EditCmd::ZoomRanges { .. } => "Zoom",
    }
}

impl History {
    /// What the file holds. Lines that cannot be read are left out, and the next write replaces them.
    pub(crate) fn load(path: &Path) -> Self {
        let mut history = Self::default();
        let text = match fs::metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return history,
            Ok(meta) if meta.len() > READ_LIMIT => Err(anyhow::anyhow!("{} bytes", meta.len())),
            _ => fs::read_to_string(path).context("Reading"),
        };
        let text = match text {
            Ok(text) => text,
            Err(error) => {
                eprintln!("Cannot read the versions in {}, keeping new ones only: {error:#}", path.display());
                history.broken.store(true, Ordering::Relaxed);
                return history;
            }
        };
        let mut clean = text.is_empty() || text.ends_with('\n');
        let mut projects: HashMap<String, Arc<str>> = HashMap::new();
        for text in text.lines() {
            history.lines += 1;
            let line = serde_json::from_str::<Line>(text).ok().filter(|line| {
                line.hash.len() == 64 && line.hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            });
            let json = line.as_ref().and_then(|line| match line.project {
                Some(raw) => {
                    let json: Arc<str> = Arc::from(raw.get());
                    projects.insert(line.hash.clone(), json.clone());
                    history.stored.insert(line.hash.clone());
                    Some(json)
                }
                None => projects.get(&line.hash).cloned(),
            });
            let (Some(line), Some(json)) = (line, json) else {
                clean = false;
                continue;
            };
            history.next = history.next.max(line.index.saturating_add(1));
            history.versions.push(Version {
                index: line.index,
                hash: line.hash,
                label: line.label,
                at_ms: line.at_ms,
                run_id: line.run_id,
                json,
            });
        }
        history.bytes = text.len();
        history.evict();
        if !clean {
            history.broken.store(true, Ordering::Relaxed);
        }
        history
    }

    /// Keeps `project` as the newest version unless it already is; returns what the file needs.
    pub(crate) fn push(&mut self, project: &Project, tip: Tip) -> Option<Write> {
        let json = serde_json::to_string(project).ok()?;
        let hash = sha256(json.as_bytes());
        if self.versions.last().is_some_and(|v| v.hash == hash) {
            return None;
        }
        let json = self.versions.iter().find(|v| v.hash == hash).map_or_else(|| Arc::from(json), |v| v.json.clone());
        let at_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let label = tip.label.chars().take(200).collect();
        self.versions.push(Version { index: self.next, hash, label, at_ms, run_id: tip.run_id, json });
        self.next += 1;
        self.evict();
        let version = self.versions.last().expect("just pushed");
        if self.broken.swap(false, Ordering::Relaxed)
            || self.lines >= 2 * MAX_VERSIONS
            || self.bytes + version.json.len() > 2 * MAX_BYTES
        {
            self.stored.clear();
            let mut text = String::new();
            for version in &self.versions {
                text.push_str(&line(version, self.stored.insert(version.hash.clone())));
            }
            (self.lines, self.bytes) = (self.versions.len(), text.len());
            return Some(Write::Replace(text));
        }
        let line = line(version, self.stored.insert(version.hash.clone()));
        self.lines += 1;
        self.bytes += line.len();
        Some(Write::Append(line))
    }

    fn evict(&mut self) {
        let kept = |versions: &[Version]| {
            let mut seen = HashSet::new();
            versions.iter().filter(|v| seen.insert(v.hash.as_str())).map(|v| v.json.len()).sum::<usize>()
        };
        while self.versions.len() > 1 && (self.versions.len() > MAX_VERSIONS || kept(&self.versions) > MAX_BYTES) {
            self.versions.remove(0);
        }
    }

    pub(crate) fn list(&self, project: &Project, limit: usize) -> HistoryList {
        let current = serde_json::to_string(project).map(|json| sha256(json.as_bytes())).unwrap_or_default();
        HistoryList {
            current_hash: current.chars().take(SHORT).collect(),
            versions: self.versions.iter().rev().take(limit).map(Version::info).collect(),
            older: self.versions.len().saturating_sub(limit),
        }
    }

    fn find(&self, target: &Target) -> Result<&Version> {
        match target {
            Target::Index(index) => self.versions.iter().rev().find(|v| v.index == *index).with_context(|| {
                format!("UNKNOWN_VERSION: version {index} is not kept; list_history shows those that are")
            }),
            Target::Hash(prefix) => {
                let prefix = prefix.trim().to_ascii_lowercase();
                ensure!(
                    prefix.len() >= 4 && prefix.bytes().all(|b| b.is_ascii_hexdigit()),
                    "INVALID_ARGUMENTS: give 4 or more hex digits of the hash"
                );
                let mut found = self.versions.iter().rev().filter(|v| v.hash.starts_with(&prefix));
                let first =
                    found.next().with_context(|| format!("UNKNOWN_VERSION: no kept version has hash {prefix}"))?;
                ensure!(
                    found.all(|v| v.hash == first.hash),
                    "UNKNOWN_VERSION: {prefix} starts several versions' hashes; give more of it"
                );
                Ok(first)
            }
        }
    }

    /// The label of the newest version a run made.
    fn run_label(&self, run_id: &str) -> Option<&str> {
        self.versions.iter().rev().find(|v| v.run_id.as_deref() == Some(run_id)).map(|v| v.label.as_str())
    }
}

impl ProjectSession {
    /// The newest `limit` versions. The user's last step becomes a version first; an agent's open
    /// run becomes one only when it ends.
    pub fn list_history(&self, limit: usize) -> Result<HistoryList> {
        let mut inner = self.inner.lock().unwrap();
        if inner.mode == Mode::ReadOnly {
            inner.refresh()?;
            return Ok(History::load(&storage::sidecar(&inner.path, HISTORY_SUFFIX)).list(&inner.editor.project, limit));
        }
        if inner.run.is_none() {
            inner.record();
        }
        Ok(inner.history.list(&inner.editor.project, limit))
    }

    /// Restores a kept version as a new undo step. The project as it was stays a version too.
    pub fn undo_to(&self, target: Target) -> Result<Restored> {
        let mut inner = self.inner.lock().unwrap();
        inner.user_editable()?;
        inner.record();
        let version = inner.history.find(&target)?.clone();
        ensure!(
            sha256(version.json.as_bytes()) == version.hash,
            "HISTORY_CORRUPT: version {} does not match its hash, so it was not restored",
            version.index
        );
        let project: Project = serde_json::from_str(&version.json)
            .with_context(|| format!("HISTORY_CORRUPT: version {} cannot be read", version.index))?;
        let revision = inner.editor.revision;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            inner.editor.replace_project_checked(project, None, validate)
        }))
        .map_err(|_| {
            anyhow::anyhow!("EDIT_REJECTED: engine failed while restoring the version; project unchanged")
        })??;
        let changed = inner.editor.revision != revision;
        if changed {
            inner.changed(Origin::Restore);
            let label = version.label.strip_prefix("Restore: ").unwrap_or(&version.label);
            inner.history.tip = Some(Tip::user(format!("Restore: {label}")));
            inner.record();
        }
        inner.flush()?;
        Ok(Restored { stamp: inner.stamp(), restored: version.info(), changed })
    }
}

impl Inner {
    /// Keeps the step that made the project as it is now as a version.
    pub(crate) fn record(&mut self) {
        let Some(tip) = self.history.tip.take() else { return };
        if let (Some(writer), Some(write)) = (&self.writer, self.history.push(&self.editor.project, tip)) {
            writer.history(write);
        }
    }

    pub(crate) fn undo_label(&self, run_id: &str) -> String {
        format!("Undo: {}", self.history.run_label(run_id).unwrap_or("AI edit"))
    }
}
