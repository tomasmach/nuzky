//! The home screen's library: every project file with what stops it from opening, collections,
//! posters, the Trash with Undo, and search through what was said in the projects.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use nuzky_engine::Project;
use nuzky_engine::edit::{EditCmd, new_id};
use nuzky_engine::render::{Renderer, Wait};
use nuzky_session::Expect;
use nuzky_session::hash::{FNV_OFFSET, hash_bytes};
use nuzky_session::transcripts::TranscriptStore;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use unicode_normalization::UnicodeNormalization;

use crate::store::sidecar;
use crate::{AppState, CmdResult, err, store};

/// Name of the projects that New project and the first start create.
pub const UNTITLED: &str = "Untitled project";
/// Undo in the toast brings a project back until it really goes to the Trash.
const TRASH_DELAY: Duration = Duration::from_secs(30);
/// Longest side of a poster in pixels: sharp on a 2× screen at the size of a card.
const POSTER_SIDE: u32 = 360;
/// A new value makes every cached poster render again.
const POSTER_VERSION: u8 = 1;
const MAX_HITS_PER_PROJECT: usize = 3;
const MAX_HITS: usize = 30;
const CONTEXT_BEFORE: usize = 5;
const CONTEXT_AFTER: usize = 6;
const MAX_COLLECTION_NAME: usize = 60;

#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Collection {
    pub id: String,
    pub name: String,
}

/// `library.json`: what the project files themselves do not hold.
#[derive(Serialize, Deserialize, Default)]
struct Index {
    #[serde(default)]
    collections: Vec<Collection>,
    /// The collection of each project, by project path.
    #[serde(default)]
    members: BTreeMap<String, String>,
    /// Projects opened from outside the projects folder.
    #[serde(default)]
    external: Vec<String>,
}

/// A project on the home screen.
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "LibraryProject"))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    path: String,
    name: String,
    modified_ms: u64,
    duration_us: i64,
    /// Canvas size; 0 for a project that cannot be read.
    width: u32,
    height: u32,
    collection: Option<String>,
    /// `broken`: the file cannot be read. `busy`: another Nuzky window or an agent has it open.
    /// `missing`: some of its media files are gone. `empty`: nothing on the timeline.
    #[cfg_attr(test, ts(type = r#""broken" | "busy" | "missing" | "empty" | null"#))]
    state: Option<&'static str>,
    missing: usize,
    /// Why a broken project cannot be read.
    error: Option<String>,
}

#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "Library"))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    projects: Vec<Entry>,
    collections: Vec<Collection>,
    /// Set when the collections could not be read.
    notice: Option<String>,
}

/// What Undo needs to bring a deleted collection back.
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeletedCollection {
    collection: Collection,
    /// Where it was in the list.
    position: usize,
    paths: Vec<String>,
}

/// Where words of a search were said: the project and its timeline time.
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaidHit {
    path: String,
    start_us: i64,
    before: String,
    text: String,
    after: String,
}

#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Said {
    hits: Vec<SaidHit>,
    /// Projects with speech that is not transcribed, so only their names were searched.
    untranscribed: Vec<String>,
    /// How many projects have a transcript to search.
    transcribed: usize,
}

struct SpokenWord {
    folded: String,
    text: String,
    start_us: i64,
}

/// The words on a project's timeline, as of its file and the transcripts at `stamp`.
struct Spoken {
    stamp: (u128, u64, u128),
    words: Vec<SpokenWord>,
    transcribed: bool,
    untranscribed: bool,
}

struct PosterRequest {
    project: Project,
    reply: Sender<Option<Vec<u8>>>,
}

#[derive(Default)]
pub struct Library {
    /// Serialises reading and writing `library.json`.
    index: Mutex<()>,
    /// Projects moved to the Trash that Undo can still bring back: path, and which move it was, so the
    /// timer of a move that was undone never takes a later one.
    pending_trash: Mutex<HashMap<String, u64>>,
    trash_moves: AtomicU64,
    spoken: Mutex<HashMap<String, Arc<Spoken>>>,
    transcripts: OnceLock<Option<TranscriptStore>>,
    posters: Mutex<Option<Sender<PosterRequest>>>,
    /// Poster cache keys whose rendering failed this session; they are not tried again.
    failed_posters: Mutex<HashSet<String>>,
}

/// The path a project is known by: canonical where it exists, so every list and the editor agree.
pub fn key(path: &Path) -> String {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned()
}

fn index_path() -> PathBuf {
    store::data_dir().join("library.json")
}

fn modified(meta: &std::fs::Metadata) -> u128 {
    meta.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos())
}

/// Lowercase without accents and punctuation, so "Západ," finds "zapad".
pub fn fold(text: &str) -> String {
    text.nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Nothing to lose: the name it was created with, no media, no clips and no corrections.
fn pristine(project: &Project) -> bool {
    project.name == UNTITLED
        && project.assets.is_empty()
        && project.word_corrections.is_empty()
        && project.tracks.iter().all(|t| t.clips.is_empty())
}

fn busy(path: &Path) -> bool {
    nuzky_session::lock_project(path, false).is_err_and(|e| e.to_string().starts_with("PROJECT_BUSY"))
}

impl Library {
    /// The index, or an empty one with a notice when it is not valid; that file is kept aside. A file
    /// that cannot be read is an error, so that nothing writes over the collections in it.
    fn read_index(&self) -> Result<(Index, Option<String>)> {
        let path = index_path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((Index::default(), None)),
            Err(error) => {
                log::error!("Cannot read {}: {error}", path.display());
                bail!("Your collections could not be read ({error}).");
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(index) => Ok((index, None)),
            Err(error) => {
                let aside = sidecar(&path, &format!(".unreadable-{}", new_id()));
                log::error!("{} is not valid ({error}); keeping it as {}", path.display(), aside.display());
                std::fs::rename(&path, &aside).context("Your collections could not be read")?;
                Ok((
                    Index::default(),
                    Some(format!(
                        "Your collections could not be read, so projects show without them. The old file was kept as {}.",
                        aside.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                    )),
                ))
            }
        }
    }

    /// The index for showing: without collections, and saying why, when it cannot be read.
    fn shown_index(&self) -> (Index, Option<String>) {
        let _guard = self.index.lock().unwrap();
        self.read_index().unwrap_or_else(|error| (Index::default(), Some(format!("{error:#}"))))
    }

    fn update<T>(&self, change: impl FnOnce(&mut Index) -> Result<T>) -> Result<T> {
        let _guard = self.index.lock().unwrap();
        let (mut index, _) = self.read_index()?;
        let value = change(&mut index)?;
        std::fs::create_dir_all(store::data_dir()).context("Creating the Nuzky data folder")?;
        nuzky_session::write_json_atomic(&index_path(), &index).context("Saving your collections")?;
        Ok(value)
    }

    /// Every project file in the projects folder and every project opened from elsewhere, newest first.
    fn paths(&self, index: &Index) -> Vec<PathBuf> {
        let dir = store::projects_dir();
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        let mut seen = HashSet::new();
        let pending: HashSet<String> = self.pending_trash.lock().unwrap().keys().cloned().collect();
        std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == store::EXTENSION))
            .chain(index.external.iter().map(PathBuf::from).filter(|p| p.is_file()))
            .filter(|p| seen.insert(key(p)) && !pending.contains(&key(p)))
            .collect()
    }

    pub fn list(&self, current: &str) -> Listing {
        let (index, notice) = self.shown_index();
        let projects_dir = key(&store::projects_dir());
        let mut projects = Vec::new();
        for path in self.paths(&index) {
            let path_key = key(&path);
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let modified_ms = (modified(&meta) / 1_000_000) as u64;
            let collection =
                index.members.get(&path_key).filter(|c| index.collections.iter().any(|x| &x.id == *c)).cloned();
            let project = store::load(&path).and_then(|p| nuzky_session::validate(&p).map(|()| p));
            let project = match project {
                Ok(project) => project,
                Err(error) => {
                    projects.push(Entry {
                        path: path_key,
                        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                        modified_ms,
                        duration_us: 0,
                        width: 0,
                        height: 0,
                        collection,
                        state: Some("broken"),
                        missing: 0,
                        error: Some(format!("{error:#}")),
                    });
                    continue;
                }
            };
            // Projects left empty by New project or a first start go away once something else is open.
            if path_key != current
                && pristine(&project)
                && path_key.starts_with(&projects_dir)
                && self.remove_pristine(&path)
            {
                continue;
            }
            let missing = project.assets.iter().filter(|a| !Path::new(&a.path).exists()).count();
            let duration_us = project.duration_us();
            let state = if path_key != current && busy(&path) {
                Some("busy")
            } else if missing > 0 {
                Some("missing")
            } else if duration_us == 0 {
                Some("empty")
            } else {
                None
            };
            projects.push(Entry {
                path: path_key,
                name: project.name,
                modified_ms,
                duration_us,
                width: project.canvas.width,
                height: project.canvas.height,
                collection,
                state,
                missing,
                error: None,
            });
        }
        projects.sort_by_key(|p| std::cmp::Reverse(p.modified_ms));
        Listing { projects, collections: index.collections, notice }
    }

    /// Deletes an empty untitled project nobody has open; there is nothing in it to lose.
    fn remove_pristine(&self, path: &Path) -> bool {
        let Ok(lock) = nuzky_session::lock_project(path, true) else { return false };
        // Read again under the lock: it may have changed since it was listed.
        if !store::load(path).is_ok_and(|p| pristine(&p)) || sidecar(path, ".checkpoint.json").exists() {
            return false;
        }
        if let Err(error) = std::fs::remove_file(path) {
            log::warn!("Cannot remove empty project {}: {error}", path.display());
            return false;
        }
        drop(lock);
        let _ = std::fs::remove_file(sidecar(path, ".lock"));
        true
    }

    /// Projects opened from elsewhere stay in the list.
    pub fn remember(&self, path: &Path) {
        let path_key = key(path);
        if path_key.starts_with(&key(&store::projects_dir())) {
            return;
        }
        let added = self.update(|index| {
            if !index.external.contains(&path_key) {
                index.external.push(path_key.clone());
            }
            Ok(())
        });
        if let Err(error) = added {
            log::error!("Cannot remember {path_key}: {error:#}");
        }
    }

    pub fn rename(&self, path: &Path, name: &str) -> Result<()> {
        ensure!(!name.trim().is_empty(), "A project needs a name.");
        let session = store::open(path, None)?;
        session.edit(vec![EditCmd::RenameProject { name: name.into() }], None, Expect::default())?;
        session.disconnect()
    }

    /// A copy next to the other projects, in the same collection. Returns its path.
    pub fn duplicate(&self, path: &Path, open: Option<Project>) -> Result<String> {
        let mut project = match open {
            Some(project) => project,
            None => {
                let project = store::load(path)?;
                nuzky_session::validate(&project)?;
                project
            }
        };
        project.name = format!("{} copy", project.name);
        let target = store::new_project_path();
        store::create(&target, &project)?;
        let (from, to) = (key(path), key(&target));
        self.update(|index| {
            if let Some(collection) = index.members.get(&from).cloned() {
                index.members.insert(to.clone(), collection);
            }
            Ok(())
        })?;
        Ok(to)
    }

    /// Hides the projects at once and moves them to the Trash after `TRASH_DELAY`, unless restored.
    /// A move that fails then, for example because an agent opened the project meanwhile, is told
    /// to the window with `trash-failed`, and the project shows again.
    pub fn trash(self: &Arc<Self>, app: AppHandle, paths: Vec<String>, current: &str) -> Result<()> {
        for path in &paths {
            ensure!(path != current, "This project is open. Open another project first, then move it to the Trash.");
            if busy(Path::new(path)) {
                bail!(store::BUSY);
            }
        }
        let move_id = self.trash_moves.fetch_add(1, Ordering::Relaxed);
        self.pending_trash.lock().unwrap().extend(paths.iter().map(|p| (p.clone(), move_id)));
        let library = self.clone();
        std::thread::Builder::new()
            .name("nuzky-trash".into())
            .spawn(move || {
                std::thread::sleep(TRASH_DELAY);
                for message in library.commit_trash(&paths, Some(move_id)) {
                    app.emit("trash-failed", message).ok();
                }
            })
            .context("Starting the Trash timer")?;
        Ok(())
    }

    pub fn restore(&self, paths: &[String]) -> Result<()> {
        let mut pending = self.pending_trash.lock().unwrap();
        ensure!(
            paths.iter().all(|p| pending.contains_key(p)),
            "It is already in the Trash. Put it back from there, then open it with Open file."
        );
        for path in paths {
            pending.remove(path);
        }
        Ok(())
    }

    /// When the app quits, nothing waits for Undo any more; returns what failed, in words.
    pub fn flush_trash(&self) -> Vec<String> {
        let pending: Vec<String> = self.pending_trash.lock().unwrap().keys().cloned().collect();
        self.commit_trash(&pending, None)
    }

    /// Moves the projects still waiting from move `move_id` (any, with `None`); returns what failed, in words.
    fn commit_trash(&self, paths: &[String], move_id: Option<u64>) -> Vec<String> {
        let mut failed = Vec::new();
        for path in paths {
            {
                let mut pending = self.pending_trash.lock().unwrap();
                if pending.get(path).is_none_or(|id| move_id.is_some_and(|m| m != *id)) {
                    continue;
                }
                pending.remove(path);
            }
            if let Err(error) = self.move_to_trash(Path::new(path)) {
                log::error!("Cannot move {path} to the Trash: {error:#}");
                let name = store::load(Path::new(path)).map(|p| p.name).unwrap_or_else(|_| "A project".into());
                failed.push(if error.to_string().starts_with("PROJECT_BUSY") {
                    format!("“{name}” was opened in another Nuzky window or by an AI agent, so it was not moved to the Trash.")
                } else {
                    format!("“{name}” could not be moved to the Trash ({error:#}). It is back in your projects.")
                });
            }
        }
        failed
    }

    fn move_to_trash(&self, path: &Path) -> Result<()> {
        // Nobody may be editing it: another window or an agent could have opened it meanwhile.
        let lock = nuzky_session::lock_project(path, true)?;
        trash::delete(path).context("Moving the project to the Trash")?;
        let checkpoint = sidecar(path, ".checkpoint.json");
        if checkpoint.exists() {
            trash::delete(&checkpoint).context("Moving the unfinished AI edit to the Trash")?;
        }
        drop(lock);
        let _ = std::fs::remove_file(sidecar(path, ".lock"));
        let path_key = key(path);
        self.update(|index| {
            index.members.remove(&path_key);
            index.external.retain(|p| p != &path_key);
            Ok(())
        })
    }

    pub fn create_collection(&self, name: &str) -> Result<Collection> {
        self.update(|index| {
            let name = collection_name(index, name, None)?;
            let collection = Collection { id: new_id(), name };
            index.collections.push(collection.clone());
            Ok(collection)
        })
    }

    pub fn rename_collection(&self, id: &str, name: &str) -> Result<()> {
        self.update(|index| {
            let name = collection_name(index, name, Some(id))?;
            index.collections.iter_mut().find(|c| c.id == id).context("That collection no longer exists.")?.name = name;
            Ok(())
        })
    }

    /// The projects stay; they are no longer in a collection. Returns what `restore_collection` needs.
    pub fn delete_collection(&self, id: &str) -> Result<DeletedCollection> {
        self.update(|index| {
            let position =
                index.collections.iter().position(|c| c.id == id).context("That collection no longer exists.")?;
            let collection = index.collections.remove(position);
            let paths: Vec<String> = index.members.iter().filter(|(_, c)| *c == id).map(|(p, _)| p.clone()).collect();
            index.members.retain(|_, c| c != id);
            Ok(DeletedCollection { collection, position, paths })
        })
    }

    pub fn restore_collection(&self, deleted: DeletedCollection) -> Result<()> {
        self.update(|index| {
            if !index.collections.iter().any(|c| c.id == deleted.collection.id) {
                let position = deleted.position.min(index.collections.len());
                index.collections.insert(position, deleted.collection.clone());
            }
            for path in &deleted.paths {
                index.members.entry(path.clone()).or_insert_with(|| deleted.collection.id.clone());
            }
            Ok(())
        })
    }

    /// `None` takes the projects out of their collection.
    pub fn set_collection(&self, paths: &[String], collection: Option<&str>) -> Result<()> {
        self.update(|index| {
            if let Some(id) = collection {
                ensure!(index.collections.iter().any(|c| c.id == id), "That collection no longer exists.");
            }
            for path in paths {
                match collection {
                    Some(id) => index.members.insert(path.clone(), id.to_owned()),
                    None => index.members.remove(path),
                };
            }
            Ok(())
        })
    }

    fn transcripts(&self) -> Option<&TranscriptStore> {
        self.transcripts
            .get_or_init(|| {
                TranscriptStore::open()
                    .inspect_err(|error| log::error!("Transcripts unavailable for search: {error:#}"))
                    .ok()
            })
            .as_ref()
    }

    /// The words on the timeline of the project at `path`, cached until the file or a transcript changes.
    fn spoken(&self, path: &Path, transcripts_changed: u128) -> Option<Arc<Spoken>> {
        let meta = std::fs::metadata(path).ok()?;
        let stamp = (modified(&meta), meta.len(), transcripts_changed);
        let path_key = key(path);
        if let Some(hit) = self.spoken.lock().unwrap().get(&path_key).filter(|s| s.stamp == stamp) {
            return Some(hit.clone());
        }
        let project = store::load(path).ok()?;
        nuzky_session::validate(&project).ok()?;
        let heard = nuzky_mcp::transcript::heard_assets(&project);
        let mut sources = HashMap::new();
        let mut untranscribed = Vec::new();
        if let Some(store) = self.transcripts() {
            for asset in project.assets.iter().filter(|a| heard.contains(&a.id)) {
                match store.get(asset) {
                    Ok(Some(record)) => {
                        sources.insert(asset.id.clone(), record.words);
                    }
                    Ok(None) => untranscribed.push(asset.id.clone()),
                    // A missing or unreadable file has nothing to search.
                    Err(_) => {}
                }
            }
        }
        let derived = nuzky_mcp::transcript::Derived::new(&project, sources, untranscribed);
        let words = derived
            .words
            .iter()
            .map(|w| SpokenWord { folded: fold(&w.text), text: w.text.trim().to_owned(), start_us: w.start_us })
            .filter(|w| !w.folded.is_empty())
            .collect();
        let spoken = Arc::new(Spoken {
            stamp,
            words,
            transcribed: !derived.sources.is_empty(),
            untranscribed: !derived.untranscribed.is_empty(),
        });
        self.spoken.lock().unwrap().insert(path_key, spoken.clone());
        Some(spoken)
    }

    /// Where the words of `query` were said, in the projects of `collection` (all with `None`), newest first.
    pub fn search(&self, query: &str, collection: Option<&str>) -> Said {
        let tokens: Vec<String> = query.split_whitespace().map(fold).filter(|t| !t.is_empty()).collect();
        let mut said = Said { hits: Vec::new(), untranscribed: Vec::new(), transcribed: 0 };
        if tokens.iter().map(|t| t.chars().count()).sum::<usize>() < 2 {
            return said;
        }
        let (index, _) = self.shown_index();
        let transcripts_changed = std::fs::metadata(store::data_dir().join("transcripts")).map_or(0, |m| modified(&m));
        let mut paths = self.paths(&index);
        if let Some(collection) = collection {
            paths.retain(|p| index.members.get(&key(p)).is_some_and(|c| c == collection));
        }
        paths.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).map_or(0, |m| modified(&m))));
        for path in paths {
            let Some(spoken) = self.spoken(&path, transcripts_changed) else { continue };
            if spoken.untranscribed {
                said.untranscribed.push(key(&path));
            }
            if spoken.transcribed {
                said.transcribed += 1;
            }
            let mut found = 0;
            for start in 0..spoken.words.len() {
                if found == MAX_HITS_PER_PROJECT || said.hits.len() == MAX_HITS {
                    break;
                }
                let Some(matched) = spoken.words.get(start..start + tokens.len()) else { break };
                let last = tokens.len() - 1;
                let hit = matched.iter().zip(&tokens).enumerate().all(|(i, (word, token))| {
                    if i == last { word.folded.starts_with(token.as_str()) } else { &word.folded == token }
                });
                if !hit {
                    continue;
                }
                let text = |words: &[SpokenWord]| words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
                let end = start + tokens.len();
                said.hits.push(SaidHit {
                    path: key(&path),
                    start_us: matched[0].start_us,
                    before: text(&spoken.words[start.saturating_sub(CONTEXT_BEFORE)..start]),
                    text: text(matched),
                    after: text(&spoken.words[end..(end + CONTEXT_AFTER).min(spoken.words.len())]),
                });
                found += 1;
            }
        }
        said
    }

    /// A PNG of the project's picture, rendered like the preview and cached until the file changes.
    pub fn poster(&self, path: &Path) -> Result<Option<String>> {
        let meta = std::fs::metadata(path).context("The project file is gone")?;
        let mut path_hash = FNV_OFFSET;
        hash_bytes(&mut path_hash, key(path).as_bytes());
        let mut stamp = FNV_OFFSET;
        hash_bytes(&mut stamp, &modified(&meta).to_le_bytes());
        hash_bytes(&mut stamp, &meta.len().to_le_bytes());
        hash_bytes(&mut stamp, &[POSTER_VERSION]);
        let dir = store::cache_dir().join("posters");
        let file = dir.join(format!("{path_hash:016x}-{stamp:016x}.png"));
        let url =
            |png: &[u8]| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png));
        if let Ok(png) = std::fs::read(&file) {
            return Ok(Some(url(&png)));
        }
        let cache_key = file.to_string_lossy().into_owned();
        if self.failed_posters.lock().unwrap().contains(&cache_key) {
            return Ok(None);
        }
        let project = store::load(path)?;
        nuzky_session::validate(&project)?;
        if project.duration_us() == 0 {
            return Ok(None);
        }
        let Some(png) = self.render_poster(project) else {
            self.failed_posters.lock().unwrap().insert(cache_key);
            return Ok(None);
        };
        std::fs::create_dir_all(&dir).context("Creating the poster cache")?;
        let part = sidecar(&file, &format!(".{}.part", new_id()));
        std::fs::write(&part, &png).and_then(|()| std::fs::rename(&part, &file)).context("Caching the poster")?;
        // Older posters of the same project are not needed any more.
        let prefix = format!("{path_hash:016x}-");
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && entry.path() != file {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(Some(url(&png)))
    }

    /// One renderer draws posters one after another; it is freed after a while without requests.
    fn render_poster(&self, project: Project) -> Option<Vec<u8>> {
        let (reply, answer) = mpsc::channel();
        let mut request = Some(PosterRequest { project, reply });
        let mut posters = self.posters.lock().unwrap();
        for _ in 0..2 {
            let sender = posters.get_or_insert_with(spawn_poster_worker);
            match sender.send(request.take()?) {
                Ok(()) => break,
                // The worker is gone; start another one with the same request.
                Err(mpsc::SendError(back)) => {
                    request = Some(back);
                    *posters = None;
                }
            }
        }
        drop(posters);
        answer.recv_timeout(Duration::from_secs(60)).ok().flatten()
    }
}

fn collection_name(index: &Index, name: &str, renaming: Option<&str>) -> Result<String> {
    let name = name.trim();
    ensure!(!name.is_empty(), "A collection needs a name.");
    ensure!(name.chars().count() <= MAX_COLLECTION_NAME, "Use a name of at most {MAX_COLLECTION_NAME} characters.");
    ensure!(
        !index
            .collections
            .iter()
            .any(|c| Some(c.id.as_str()) != renaming && c.name.to_lowercase() == name.to_lowercase()),
        "There is already a collection called “{name}”."
    );
    Ok(name.to_owned())
}

fn spawn_poster_worker() -> Sender<PosterRequest> {
    let (tx, rx) = mpsc::channel::<PosterRequest>();
    let spawned = std::thread::Builder::new().name("nuzky-posters".into()).spawn(move || {
        let mut renderer: Option<Renderer> = None;
        loop {
            let request = match rx.recv_timeout(Duration::from_secs(20)) {
                Ok(request) => request,
                Err(RecvTimeoutError::Timeout) => {
                    renderer = None;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            // A project that trips the renderer gets no poster; the others still do.
            let png = std::panic::catch_unwind(AssertUnwindSafe(|| -> Result<Vec<u8>> {
                if renderer.is_none() {
                    renderer = Some(Renderer::new()?);
                }
                draw_poster(renderer.as_mut().expect("renderer was just created"), &request.project)
            }));
            let png = match png {
                Ok(Ok(png)) => Some(png),
                Ok(Err(error)) => {
                    log::warn!("No poster for “{}”: {error:#}", request.project.name);
                    None
                }
                Err(_) => {
                    log::error!("The renderer failed on the poster of “{}”", request.project.name);
                    renderer = None;
                    None
                }
            };
            let _ = request.reply.send(png);
        }
    });
    if let Err(error) = spawned {
        log::error!("Cannot start the poster renderer: {error}");
    }
    tx
}

/// Width and height with the canvas's shape, the longer side `POSTER_SIDE`, both even.
fn poster_size(width: u32, height: u32) -> (u32, u32) {
    let scale = f64::from(POSTER_SIDE) / f64::from(width.max(height).max(1));
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).clamp(16, POSTER_SIDE) & !1;
    (side(width), side(height))
}

fn draw_poster(renderer: &mut Renderer, project: &Project) -> Result<Vec<u8>> {
    let (width, height) = poster_size(project.canvas.width, project.canvas.height);
    // Just after the start: past a fade in, still the opening shot.
    let t = (project.duration_us() / 10).min(1_500_000);
    let rgba = renderer.render(project, t, width, height, Wait::Exact, false)?;
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.write_header()?.write_image_data(&rgba)?;
    }
    Ok(png)
}

// --- Commands ---------------------------------------------------------------------------------------

fn current_path(state: &AppState) -> String {
    key(&state.session.lock().unwrap().path)
}

#[tauri::command]
pub async fn library(app: AppHandle) -> CmdResult<Listing> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let current = current_path(&state);
        state.library.list(&current)
    })
    .await
    .map_err(err)
}

#[tauri::command]
pub async fn project_poster(app: AppHandle, path: String) -> CmdResult<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || app.state::<AppState>().library.poster(Path::new(&path)))
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
pub async fn search_said(app: AppHandle, query: String, collection: Option<String>) -> CmdResult<Said> {
    tauri::async_runtime::spawn_blocking(move || app.state::<AppState>().library.search(&query, collection.as_deref()))
        .await
        .map_err(err)
}

/// For a project that is not open here; the open one is renamed through the editor.
#[tauri::command]
pub async fn rename_project(app: AppHandle, path: String, name: String) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        if key(Path::new(&path)) == current_path(&state) {
            return Err("Rename the open project in the editor.".to_string());
        }
        state.library.rename(Path::new(&path), &name).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn duplicate_project(app: AppHandle, path: String) -> CmdResult<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let open = {
            let current = state.session.lock().unwrap();
            (key(&current.path) == key(Path::new(&path))).then(|| state_project(&current)).transpose()?
        };
        state.library.duplicate(Path::new(&path), open).map_err(err)
    })
    .await
    .map_err(err)?
}

fn state_project(current: &crate::OpenSession) -> CmdResult<Project> {
    current.host.session.state().map(|s| s.project).map_err(err)
}

#[tauri::command]
pub fn trash_projects(app: AppHandle, state: State<'_, AppState>, paths: Vec<String>) -> CmdResult<()> {
    let current = current_path(&state);
    state.library.trash(app.clone(), paths, &current).map_err(err)
}

#[tauri::command]
pub fn restore_projects(state: State<'_, AppState>, paths: Vec<String>) -> CmdResult<()> {
    state.library.restore(&paths).map_err(err)
}

#[tauri::command]
pub fn create_collection(state: State<'_, AppState>, name: String) -> CmdResult<Collection> {
    state.library.create_collection(&name).map_err(err)
}

#[tauri::command]
pub fn rename_collection(state: State<'_, AppState>, id: String, name: String) -> CmdResult<()> {
    state.library.rename_collection(&id, &name).map_err(err)
}

#[tauri::command]
pub fn delete_collection(state: State<'_, AppState>, id: String) -> CmdResult<DeletedCollection> {
    state.library.delete_collection(&id).map_err(err)
}

#[tauri::command]
pub fn restore_collection(state: State<'_, AppState>, deleted: DeletedCollection) -> CmdResult<()> {
    state.library.restore_collection(deleted).map_err(err)
}

#[tauri::command]
pub fn set_collection(state: State<'_, AppState>, paths: Vec<String>, collection: Option<String>) -> CmdResult<()> {
    state.library.set_collection(&paths, collection.as_deref()).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_ignores_case_accents_and_punctuation() {
        assert_eq!(fold("Západ,"), "zapad");
        assert_eq!(fold("SUNSET!"), "sunset");
        assert_eq!(fold("—"), "");
        assert_eq!(fold("Ščřž"), "scrz");
    }

    #[test]
    fn poster_keeps_the_canvas_shape_with_even_sides() {
        assert_eq!(poster_size(1080, 1920), (202, 360));
        assert_eq!(poster_size(1920, 1080), (360, 202));
        assert_eq!(poster_size(1080, 1350), (288, 360));
        assert_eq!(poster_size(7680, 16), (360, 16));
    }
}
