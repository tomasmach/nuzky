mod audio_out;
mod engine;
mod jobs;
mod model_download;
mod preview_server;
mod store;
mod thumbs;
mod transcripts;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock};

use capopen_engine::Project;
use capopen_engine::edit::{EditCmd, new_id};
use capopen_engine::media::probe;
use capopen_session::{Expect, Origin, RecoveryAction, SessionEvent, host::Host};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use engine::{Engine, Msg, Transport};
use preview_server::PreviewServer;
use store::ProjectSummary;

pub struct AppState {
    app: AppHandle,
    session: Mutex<OpenSession>,
    engine: Engine,
    preview_url: String,
    cache_dir: PathBuf,
    jobs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Saving failed when the window was closed; the next close quits without retrying the warning.
    close_failed: AtomicBool,
    /// The user agreed to cancel running work when closing the window.
    quit_confirmed: AtomicBool,
    thumbs: Mutex<HashMap<String, String>>,
    filmstrips: Mutex<HashMap<String, Filmstrip>>,
    preview_locks: Mutex<HashMap<String, PreviewLock>>,
    bounds_text: Mutex<Option<capopen_engine::text::TextRenderer>>,
    fonts: OnceLock<FontFamilies>,
    /// Why the most recent project was not opened at startup.
    startup_notice: Option<String>,
}

/// Per asset id: the source file its cached previews show, and the lock of their decoding.
struct PreviewLock {
    source: String,
    lock: Arc<Mutex<()>>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(serialize_with = "snapshot_project")]
    project: Project,
    revision: u64,
    session_epoch: String,
    open_run_label: Option<String>,
    recovery: bool,
    can_undo: bool,
    can_redo: bool,
    path: String,
    select: Vec<String>,
    /// Set when agents cannot attach to this project live; editing works without them.
    agent_bridge_error: Option<String>,
}

fn snapshot_project<S: serde::Serializer>(project: &Project, serializer: S) -> Result<S::Ok, S::Error> {
    let mut value = serde_json::to_value(project).map_err(serde::ser::Error::custom)?;
    value["canvas"]["safeArea"] =
        serde_json::to_value(project.canvas.safe_area()).map_err(serde::ser::Error::custom)?;
    value.serialize(serializer)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Boot {
    limits: capopen_engine::edit::Limits,
    snapshot: Snapshot,
    preview_url: String,
    transport: Transport,
    engine_error: Option<String>,
    startup_notice: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    snapshot: Snapshot,
    added: Vec<String>,
    failed: Vec<ImportFailure>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFailure {
    path: String,
    error: String,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

struct OpenSession {
    host: Arc<Host>,
    #[cfg(unix)]
    listener: Option<capopen_mcp::ipc::Listener>,
    bridge_error: Option<String>,
    path: PathBuf,
    stopped: Arc<AtomicBool>,
}

fn lock_session<'a>(
    session: &'a Mutex<OpenSession>,
    expected_epoch: Option<&str>,
) -> CmdResult<std::sync::MutexGuard<'a, OpenSession>> {
    let current = session.lock().unwrap();
    if let Some(epoch) = expected_epoch
        && current.host.session.stamp().session_epoch != epoch
    {
        return Err("EPOCH_CHANGED: another project is open".into());
    }
    Ok(current)
}

/// Projects this app opened. A closed project keeps its lock until its jobs end.
static HOSTS: Mutex<Vec<(PathBuf, std::sync::Weak<Host>)>> = Mutex::new(Vec::new());

impl OpenSession {
    fn open(path: PathBuf) -> anyhow::Result<(Self, Receiver<SessionEvent>)> {
        {
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            let mut hosts = HOSTS.lock().unwrap();
            hosts.retain(|(_, host)| host.strong_count() > 0);
            anyhow::ensure!(
                !hosts.iter().any(|(open, _)| open == &canonical),
                "CapOpen is still finishing background work on this project. Try again in a few seconds."
            );
        }
        let (tx, rx) = mpsc::channel();
        let host = Arc::new(Host::new(store::open(&path, tx)?, store::cache_dir())?);
        HOSTS.lock().unwrap().push((host.session.locked_path()?, Arc::downgrade(&host)));
        let mut session = Self {
            host,
            #[cfg(unix)]
            listener: None,
            bridge_error: None,
            path,
            stopped: Arc::new(AtomicBool::new(false)),
        };
        session.start_bridge();
        Ok((session, rx))
    }

    /// The project stays open for editing when the live agent bridge cannot start.
    fn start_bridge(&mut self) {
        #[cfg(unix)]
        match capopen_mcp::ipc::Listener::start(self.host.clone()) {
            Ok(listener) => {
                self.listener = Some(listener);
                self.bridge_error = None;
            }
            Err(error) => {
                log::error!("Agent bridge unavailable: {error:#}");
                self.bridge_error = Some(format!("{error:#}"));
            }
        }
    }

    fn close_ipc(&mut self) {
        #[cfg(unix)]
        self.listener.take();
    }

    fn prepare_switch(&mut self) -> anyhow::Result<()> {
        self.close_ipc();
        if let Err(error) = self.host.session.disconnect() {
            self.start_bridge();
            return Err(error);
        }
        Ok(())
    }

    fn snapshot(&self, select: Vec<String>) -> CmdResult<Snapshot> {
        let (state, can_undo, can_redo) = self.host.session.view().map_err(err)?;
        Ok(Snapshot {
            project: state.project,
            revision: state.stamp.revision,
            session_epoch: state.stamp.session_epoch,
            open_run_label: state.open_run.map(|run| run.label),
            recovery: state.recovery_checkpoint.is_some(),
            can_undo,
            can_redo,
            path: self.path.to_string_lossy().into_owned(),
            select,
            agent_bridge_error: self.bridge_error.clone(),
        })
    }

    fn start_pump(&self, app: AppHandle, rx: Receiver<SessionEvent>) {
        let stopped = self.stopped.clone();
        let session = Arc::downgrade(&self.host);
        std::thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let event = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(event) => event,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let state = app.state::<AppState>();
                let current = state.session.lock().unwrap();
                if stopped.load(Ordering::Acquire) || !session.ptr_eq(&Arc::downgrade(&current.host)) {
                    break;
                }
                match event {
                    SessionEvent::TranscriptsChanged => {
                        app.emit("transcripts-changed", ()).ok();
                    }
                    SessionEvent::Changed { origin, .. } => {
                        if let Ok(snap) = current.snapshot(Vec::new()) {
                            state.publish_project(&snap.project);
                            // Media an agent added, or rebound to another file under the same id,
                            // needs its sound prepared too; prepared sources are skipped.
                            jobs::ensure_audio(&state, &snap.project);
                            // User edits already return this snapshot; agent undo also reaches the UI.
                            if !matches!(origin, Origin::User) {
                                app.emit("project-changed", snap).ok();
                            }
                        }
                    }
                    SessionEvent::Saved { revision, error } => {
                        app.emit("saved", store::SavedEvent { revision, error }).ok();
                    }
                    SessionEvent::Run(run) => {
                        app.emit("run-changed", run.map(|run| run.label)).ok();
                        if let Ok(snap) = current.snapshot(Vec::new()) {
                            app.emit("project-changed", snap).ok();
                        }
                    }
                }
            }
        });
    }
}

impl Drop for OpenSession {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.close_ipc();
        if let Err(error) = self.host.retire() {
            log::error!("{error:#}");
        }
    }
}

impl AppState {
    fn project(&self) -> CmdResult<Project> {
        self.session.lock().unwrap().host.session.state().map(|state| state.project).map_err(err)
    }

    fn publish_project(&self, project: &Project) {
        // Previews stay while their asset id still names the same file; an id rebound to other
        // media, by removal and re-adding in one batch, is decoded again.
        let mut locks = self.preview_locks.lock().unwrap();
        locks.retain(|id, entry| project.asset(id).is_some_and(|asset| asset.path == entry.source));
        self.thumbs.lock().unwrap().retain(|id, _| locks.contains_key(id));
        self.filmstrips.lock().unwrap().retain(|id, _| locks.contains_key(id));
        drop(locks);
        self.engine.send(Msg::Project(Arc::new(project.clone())));
    }

    fn apply_batch(
        &self,
        cmds: Vec<EditCmd>,
        coalesce: Option<String>,
        expect: Expect,
        expected_epoch: Option<&str>,
    ) -> CmdResult<Snapshot> {
        let current = lock_session(&self.session, expected_epoch)?;
        let result = current.host.session.edit(cmds, coalesce, expect).map_err(err)?;
        current.snapshot(result.outcome.select)
    }

    fn asset_preview<T: Clone>(
        &self,
        asset_id: &str,
        cache: &Mutex<HashMap<String, T>>,
        decode: impl FnOnce(&capopen_engine::model::Asset) -> anyhow::Result<Option<T>>,
    ) -> CmdResult<Option<T>> {
        let (asset, lock) = {
            let current = self.session.lock().unwrap();
            let Some(asset) = current.host.session.state().map_err(err)?.project.asset(asset_id).cloned() else {
                return Ok(None);
            };
            let mut locks = self.preview_locks.lock().unwrap();
            let entry = locks
                .entry(asset_id.into())
                .or_insert_with(|| PreviewLock { source: asset.path.clone(), lock: Arc::default() });
            let lock = entry.lock.clone();
            (asset, lock)
        };
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        {
            let _current = self.session.lock().unwrap();
            if !self
                .preview_locks
                .lock()
                .unwrap()
                .get(asset_id)
                .is_some_and(|current| Arc::ptr_eq(&current.lock, &lock))
            {
                return Ok(None);
            }
            if let Some(value) = cache.lock().unwrap().get(asset_id) {
                return Ok(Some(value.clone()));
            }
        }
        let value = decode(&asset).map_err(err)?;
        let _current = self.session.lock().unwrap();
        // Removal or replacement invalidates in-flight decodes, even if the ID is reused.
        if !self.preview_locks.lock().unwrap().get(asset_id).is_some_and(|current| Arc::ptr_eq(&current.lock, &lock)) {
            return Ok(None);
        }
        if let Some(value) = &value {
            cache.lock().unwrap().insert(asset_id.into(), value.clone());
        }
        Ok(value)
    }

    fn replace_project(&self, current: &mut OpenSession, path: PathBuf) -> CmdResult<Snapshot> {
        let (next, rx) = OpenSession::open(path).map_err(err)?;
        let snap = next.snapshot(Vec::new())?;
        current.prepare_switch().map_err(err)?;
        *current = next;
        // The startup picker uses modification time, including projects opened without edits.
        if let Err(error) = current.host.session.disconnect() {
            log::error!("Cannot save opened project: {error:#}");
        }
        self.thumbs.lock().unwrap().clear();
        self.filmstrips.lock().unwrap().clear();
        self.preview_locks.lock().unwrap().clear();
        self.engine.send(Msg::Seek(0));
        self.publish_project(&snap.project);
        current.start_pump(self.app.clone(), rx);
        jobs::ensure_audio(self, &snap.project);
        Ok(snap)
    }
}

#[tauri::command]
fn boot(state: State<'_, AppState>) -> CmdResult<Boot> {
    Ok(Boot {
        limits: capopen_engine::edit::LIMITS,
        snapshot: state.session.lock().unwrap().snapshot(Vec::new())?,
        preview_url: state.preview_url.clone(),
        transport: *state.engine.transport.lock().unwrap(),
        engine_error: state.engine.error.lock().unwrap().clone(),
        startup_notice: state.startup_notice.clone(),
    })
}

#[tauri::command]
fn apply_edit(
    state: State<'_, AppState>,
    cmd: EditCmd,
    coalesce: Option<String>,
    expected_revision: Option<u64>,
    expected_speech_layout_key: Option<String>,
    expected_epoch: Option<String>,
) -> CmdResult<Snapshot> {
    state.apply_batch(
        vec![cmd],
        coalesce,
        Expect { revision: expected_revision, speech_layout_key: expected_speech_layout_key },
        expected_epoch.as_deref(),
    )
}

#[tauri::command]
fn apply_edits(
    state: State<'_, AppState>,
    cmds: Vec<EditCmd>,
    coalesce: Option<String>,
    expected_revision: Option<u64>,
    expected_speech_layout_key: Option<String>,
    expected_epoch: Option<String>,
) -> CmdResult<Snapshot> {
    state.apply_batch(
        cmds,
        coalesce,
        Expect { revision: expected_revision, speech_layout_key: expected_speech_layout_key },
        expected_epoch.as_deref(),
    )
}

#[tauri::command]
fn undo(state: State<'_, AppState>, expected_epoch: Option<String>) -> CmdResult<Snapshot> {
    let current = lock_session(&state.session, expected_epoch.as_deref())?;
    current.host.session.undo().map_err(err)?;
    current.snapshot(Vec::new())
}

#[tauri::command]
fn redo(state: State<'_, AppState>, expected_epoch: Option<String>) -> CmdResult<Snapshot> {
    let current = lock_session(&state.session, expected_epoch.as_deref())?;
    current.host.session.redo().map_err(err)?;
    current.snapshot(Vec::new())
}

#[tauri::command]
fn set_ui_context(state: State<'_, AppState>, selection: Vec<String>, playhead_us: i64) {
    state.session.lock().unwrap().host.session.set_ui_context(selection, playhead_us);
}

/// Stop in the "AI is editing" bar: the run ends with its changes kept, as one undo step.
#[tauri::command]
fn stop_run(state: State<'_, AppState>, expected_epoch: Option<String>) -> CmdResult<Snapshot> {
    let current = lock_session(&state.session, expected_epoch.as_deref())?;
    current.host.stop_run().map_err(err)?;
    current.snapshot(Vec::new())
}

#[tauri::command]
fn resolve_recovery(state: State<'_, AppState>, action: String, expected_epoch: Option<String>) -> CmdResult<Snapshot> {
    let action = match action.as_str() {
        "keep" => RecoveryAction::Keep,
        "restore" => RecoveryAction::Restore,
        _ => return Err("Unknown recovery action".into()),
    };
    let current = lock_session(&state.session, expected_epoch.as_deref())?;
    current.host.session.resolve_recovery(action).map_err(err)?;
    current.snapshot(Vec::new())
}

#[tauri::command]
async fn import_media(app: AppHandle, paths: Vec<String>, expected_epoch: Option<String>) -> CmdResult<ImportResult> {
    let probed = tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|p| {
                // FFmpeg would open URLs and protocol prefixes; only local files are media.
                let local = capopen_session::local_media_path(&p).and_then(|()| {
                    anyhow::ensure!(Path::new(&p).is_file(), "{p} is not a file");
                    Ok(())
                });
                let result = local.and_then(|()| probe(Path::new(&p), new_id()));
                (p, result)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(err)?;
    let state = app.state::<AppState>();
    let mut assets = Vec::new();
    let mut failed = Vec::new();
    for (path, result) in probed {
        match result {
            Ok(a) => assets.push(a),
            Err(e) => failed.push(ImportFailure { path, error: format!("{e:#}") }),
        }
    }
    let added: Vec<String> = assets.iter().map(|a| a.id.clone()).collect();
    let snapshot = if assets.is_empty() {
        lock_session(&state.session, expected_epoch.as_deref())?.snapshot(Vec::new())?
    } else {
        let snap = state.apply_batch(
            vec![EditCmd::AddAssets { assets }],
            None,
            Expect::default(),
            expected_epoch.as_deref(),
        )?;
        jobs::ensure_audio(&state, &snap.project);
        snap
    };
    Ok(ImportResult { snapshot, added, failed })
}

#[tauri::command]
async fn thumbnail(app: AppHandle, asset_id: String) -> CmdResult<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.asset_preview(&asset_id, &state.thumbs, thumbs::thumbnail)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
async fn waveform(app: AppHandle, asset_id: String) -> CmdResult<Option<Vec<u8>>> {
    let state = app.state::<AppState>();
    let asset = state.project()?.asset(&asset_id).cloned();
    let Some(asset) = asset else { return Ok(None) };
    let path = capopen_engine::audio::pcm_path(&state.cache_dir, &asset);
    tauri::async_runtime::spawn_blocking(move || {
        let pcm = capopen_engine::audio::Pcm::open(&path).ok()?;
        Some(capopen_engine::audio::peaks(&pcm, 50))
    })
    .await
    .map_err(err)
}

#[tauri::command]
fn play(state: State<'_, AppState>) {
    state.engine.send(Msg::Play);
}

#[tauri::command]
fn pause(state: State<'_, AppState>) {
    state.engine.send(Msg::Pause);
}

#[tauri::command]
fn seek(state: State<'_, AppState>, t_us: i64) {
    state.engine.send(Msg::Seek(t_us));
}

#[tauri::command]
fn set_preview_box(state: State<'_, AppState>, width: u32, height: u32) {
    state.engine.send(Msg::Resize(width, height));
}

#[tauri::command]
fn list_projects() -> Vec<ProjectSummary> {
    store::list()
}

#[derive(Serialize, Clone)]
pub struct FontFamilies {
    bundled: Vec<String>,
    system: Vec<String>,
}

#[tauri::command]
fn list_fonts(state: State<'_, AppState>) -> FontFamilies {
    state
        .fonts
        .get_or_init(|| {
            use capopen_engine::text::{BUNDLED_FONT_FAMILIES, TextRenderer};
            let mut renderer = state.bounds_text.lock().unwrap();
            let families = renderer.get_or_insert_with(TextRenderer::new).font_families();
            let (bundled, system) =
                families.into_iter().partition(|name| BUNDLED_FONT_FAMILIES.contains(&name.as_str()));
            FontFamilies { bundled, system }
        })
        .clone()
}

#[tauri::command]
fn new_project(state: State<'_, AppState>, width: u32, height: u32) -> CmdResult<Snapshot> {
    let mut project = Project::new("Untitled project");
    project.canvas.width = width & !1;
    project.canvas.height = height & !1;
    let path = store::new_project_path();
    store::create(&path, &project).map_err(err)?;
    state.replace_project(&mut state.session.lock().unwrap(), path)
}

#[tauri::command]
fn open_project(state: State<'_, AppState>, path: String) -> CmdResult<Snapshot> {
    let path = std::fs::canonicalize(path).map_err(err)?;
    let mut current = state.session.lock().unwrap();
    if current.path == path {
        return current.snapshot(Vec::new());
    }
    state.replace_project(&mut current, path)
}

/// `replace_existing` is true once the user confirmed this exact path; false fails with
/// DESTINATION_EXISTS when the file is already there. Absent means true.
#[tauri::command]
fn start_export(
    app: AppHandle,
    path: String,
    options: jobs::ExportRequest,
    replace_existing: Option<bool>,
    expected_epoch: Option<String>,
) -> CmdResult<String> {
    jobs::start_export(&app, PathBuf::from(path), options, replace_existing.unwrap_or(true), expected_epoch.as_deref())
}

/// Filmstrip for timeline clips: one horizontal sprite of evenly spaced frames.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Filmstrip {
    /// PNG or JPEG data URL of `count` frames side by side.
    url: String,
    frame_width: u32,
    frame_height: u32,
    /// Source time between consecutive frames.
    interval_us: i64,
    count: u32,
}

/// Where a visual layer sits on the canvas at a given time, for selecting and dragging
/// layers directly in the preview.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerBounds {
    clip_id: String,
    /// Top-left, top-right, bottom-right, bottom-left in canvas pixels.
    corners: [[f32; 2]; 4],
}

/// Layers visible at `t_us`, bottom to top, with animations and keyframes applied.
#[tauri::command]
async fn layer_bounds(app: AppHandle, t_us: i64) -> CmdResult<Vec<LayerBounds>> {
    let project = app.state::<AppState>().project()?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut text = state.bounds_text.lock().unwrap();
        let text = text.get_or_insert_with(capopen_engine::text::TextRenderer::new);
        capopen_engine::render::layer_bounds(&project, t_us, text)
            .into_iter()
            .map(|(clip_id, corners)| LayerBounds { clip_id, corners })
            .collect()
    })
    .await
    .map_err(err)
}

#[tauri::command]
async fn filmstrip(app: AppHandle, asset_id: String) -> CmdResult<Option<Filmstrip>> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.asset_preview(&asset_id, &state.filmstrips, thumbs::filmstrip)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn cancel_job(state: State<'_, AppState>, id: String) {
    if let Some(flag) = state.jobs.lock().unwrap().get(&id) {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The most recent project that opens, with a notice when a newer one could not, e.g. while an
/// agent edits it. A new project is created only when none opens, so a busy project does not
/// leave a new empty file behind on every start.
fn initial_project() -> anyhow::Result<(OpenSession, Receiver<SessionEvent>, Option<String>)> {
    let (opened, notice) = open_recent(store::list());
    let (session, events) = match opened {
        Some(opened) => opened,
        None => {
            let path = store::new_project_path();
            store::create(&path, &Project::new("Untitled project"))?;
            OpenSession::open(path)?
        }
    };
    Ok((session, events, notice))
}

fn open_recent(recent: Vec<ProjectSummary>) -> (Option<(OpenSession, Receiver<SessionEvent>)>, Option<String>) {
    let mut skipped: Option<(String, anyhow::Error)> = None;
    for project in recent {
        match std::fs::canonicalize(&project.path).map_err(anyhow::Error::from).and_then(OpenSession::open) {
            Ok(opened) => {
                return (Some(opened), skipped.map(|(name, error)| skipped_notice(&name, Some(&project.name), &error)));
            }
            Err(error) => {
                log::warn!("Cannot open {}: {error:#}", project.path);
                skipped.get_or_insert((project.name, error));
            }
        }
    }
    (None, skipped.map(|(name, error)| skipped_notice(&name, None, &error)))
}

/// Why the most recent project did not open, which one did instead and what to do.
fn skipped_notice(name: &str, instead: Option<&str>, error: &anyhow::Error) -> String {
    let instead = instead
        .map_or_else(|| "A new project opened instead.".to_string(), |other| format!("“{other}” opened instead."));
    if error.to_string() == store::BUSY {
        format!(
            "“{name}” is open in another CapOpen window or an AI agent is editing it, so {}. Close it there, then open it from Projects.",
            lowercase_first(&instead).trim_end_matches('.')
        )
    } else {
        format!("“{name}” could not be opened ({error:#}). {instead}")
    }
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_lowercase().chain(chars).collect())
}

pub fn run() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let server = Arc::new(PreviewServer::start()?);
            let (session, events, startup_notice) = initial_project()?;
            let project = session.host.session.state()?.project;
            let cache_dir = store::cache_dir();
            let engine =
                Engine::start(app.handle().clone(), server.clone(), Arc::new(project.clone()), cache_dir.clone());
            let state = AppState {
                app: app.handle().clone(),
                session: Mutex::new(session),
                engine,
                preview_url: server.url.clone(),
                cache_dir,
                jobs: Mutex::new(HashMap::new()),
                close_failed: AtomicBool::new(false),
                quit_confirmed: AtomicBool::new(false),
                thumbs: Mutex::new(HashMap::new()),
                filmstrips: Mutex::new(HashMap::new()),
                preview_locks: Mutex::new(HashMap::new()),
                bounds_text: Mutex::new(None),
                fonts: OnceLock::new(),
                startup_notice,
            };
            // Jobs look the state up from their threads, so it must be managed first.
            app.manage(state);
            app.state::<AppState>().session.lock().unwrap().start_pump(app.handle().clone(), events);
            jobs::ensure_audio(&app.state::<AppState>(), &project);
            app.emit("ready", ()).ok();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            boot,
            set_ui_context,
            resolve_recovery,
            stop_run,
            apply_edit,
            apply_edits,
            undo,
            redo,
            import_media,
            thumbnail,
            waveform,
            play,
            pause,
            seek,
            set_preview_box,
            list_projects,
            list_fonts,
            new_project,
            open_project,
            start_export,
            filmstrip,
            layer_bounds,
            cancel_job,
            jobs::start_captions,
            jobs::speech_models,
            jobs::start_transcript,
            transcripts::transcript_view,
            transcripts::cut_words,
            transcripts::remove_pauses,
        ])
        .build(tauri::generate_context!())
        .expect("error while building CapOpen")
        .run(|app, event| match event {
            tauri::RunEvent::WindowEvent { label, event: tauri::WindowEvent::CloseRequested { api, .. }, .. } => {
                let state = app.state::<AppState>();
                if !state.quit_confirmed.load(Ordering::Acquire)
                    && let Some(question) = state.running_work()
                {
                    api.prevent_close();
                    confirm_quit(app, &label, question);
                    return;
                }
                let saved = state.session.lock().unwrap().host.session.disconnect();
                // A second close after a failed save quits anyway; the user has been told what is lost.
                if let Err(error) = saved
                    && !state.close_failed.swap(true, Ordering::AcqRel)
                {
                    api.prevent_close();
                    app.emit("close-save-failed", format!("{error:#}")).ok();
                }
            }
            tauri::RunEvent::Exit => app.state::<AppState>().quit(),
            _ => {}
        });
}

/// How long quitting waits for cancelled jobs, so exports can remove their unfinished files.
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

impl AppState {
    /// Running work that closing the window would cancel. Audio preparation is not asked
    /// about: it starts again with the project.
    fn running_work(&self) -> Option<&'static str> {
        let mut kinds: Vec<String> =
            self.jobs.lock().unwrap().keys().map(|id| id.split(':').next().unwrap_or(id).to_owned()).collect();
        let host = self.session.lock().unwrap().host.clone();
        kinds.extend(host.jobs.running().into_iter().map(String::from));
        quit_question(&kinds)
    }

    /// Quitting stops agents, cancels every job, gives exports a moment to remove their
    /// unfinished files, then saves and releases the project.
    fn quit(&self) {
        let host = {
            let mut current = self.session.lock().unwrap();
            current.stopped.store(true, Ordering::Release);
            current.close_ipc();
            current.host.clone()
        };
        if !drain(&self.jobs, &host, QUIT_WAIT) {
            log::warn!("Quitting before every cancelled job stopped");
        }
        if let Err(error) = host.session.disconnect() {
            log::error!("Cannot save project on exit: {error:#}");
        }
    }
}

fn quit_question(kinds: &[String]) -> Option<&'static str> {
    if kinds.iter().any(|kind| kind == "export") {
        Some("An export is still running. Quitting cancels it and removes the unfinished file.")
    } else if kinds.iter().any(|kind| kind != "audio") {
        Some("Speech recognition or analysis is still running. Quitting cancels it.")
    } else {
        None
    }
}

fn confirm_quit(app: &AppHandle, label: &str, question: &'static str) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let window = app.get_webview_window(label);
    let mut dialog = app
        .dialog()
        .message(question)
        .title("Quit CapOpen?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Quit".into(), "Keep working".into()));
    if let Some(window) = &window {
        dialog = dialog.parent(window);
    }
    let app = app.clone();
    dialog.show(move |quit| {
        if quit {
            app.state::<AppState>().quit_confirmed.store(true, Ordering::Release);
            if let Some(window) = window {
                window.close().ok();
            }
        }
    });
}

/// Cancels desktop and agent jobs and waits until they end, or until the timeout.
fn drain(jobs: &Mutex<HashMap<String, Arc<AtomicBool>>>, host: &Host, timeout: std::time::Duration) -> bool {
    host.jobs.begin_shutdown();
    for flag in jobs.lock().unwrap().values() {
        flag.store(true, Ordering::Relaxed);
    }
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if jobs.lock().unwrap().is_empty() && host.jobs.running().is_empty() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// A spawned child shares this process's open files, project locks included, until it execs.
/// Tests that spawn children and tests that expect a released lock to be free take turns.
#[cfg(test)]
static CHILD_SPAWN: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod ipc_lifecycle_tests {
    #[test]
    fn snapshot_canvas_exposes_safe_area_without_changing_saved_project() {
        #[derive(serde::Serialize)]
        struct View<'a> {
            #[serde(serialize_with = "super::snapshot_project")]
            project: &'a capopen_engine::Project,
        }
        let mut project = capopen_engine::Project::new("test");
        project.canvas.width = 1080;
        project.canvas.height = 1920;
        let snapshot = serde_json::to_value(View { project: &project }).unwrap();
        assert_eq!(
            snapshot["project"]["canvas"]["safeArea"],
            serde_json::json!({"left":60.0,"top":250.0,"right":900.0,"bottom":1420.0})
        );
        assert!(serde_json::to_value(&project).unwrap()["canvas"].get("safeArea").is_none());
        project.canvas.height = 1080;
        assert!(serde_json::to_value(View { project: &project }).unwrap()["project"]["canvas"]["safeArea"].is_null());
        let limits = serde_json::to_value(capopen_engine::edit::LIMITS).unwrap();
        assert_eq!(limits["maxTransitionUs"], 2_000_000);
        assert_eq!(limits["maxSpeed"], 10.0);
    }

    #[test]
    fn command_error_preserves_anyhow_causes() {
        let error = anyhow::anyhow!("root cause").context("operation failed");
        assert_eq!(super::err(error), "operation failed: root cause");
    }

    use super::*;
    use std::time::{Duration, Instant};

    #[cfg(unix)]
    #[test]
    fn refused_switch_restores_ipc_after_save_failure() {
        let dir = std::env::temp_dir().join(format!("capopen-save-failure-{}", new_id()));
        let path = dir.join("project.capopen");
        store::create(&path, &Project::new("still open")).unwrap();
        let (mut current, _) = OpenSession::open(path.clone()).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(current.prepare_switch().unwrap_err().to_string().contains("SAVE_FAILED"));
        assert!(current.listener.is_some());
        let remote = capopen_mcp::ipc::Remote::connect(&path, false).unwrap().unwrap();
        let result = remote.call("get_state".into(), serde_json::json!({})).unwrap();
        assert_ne!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap()["name"], "still open");
        drop(remote);
        std::fs::remove_dir(&path).unwrap();
        current.host.session.disconnect().unwrap();
        drop(current);
    }

    #[cfg(unix)]
    #[test]
    fn squatted_agent_endpoint_still_opens_the_project() {
        use std::os::unix::fs::DirBuilderExt;
        let dir = std::env::temp_dir().join(format!("capopen-squatted-{}", new_id()));
        let path = dir.join("project.capopen");
        store::create(&path, &Project::new("no bridge")).unwrap();
        let socket = capopen_mcp::ipc::socket_path(&path).unwrap();
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(socket.parent().unwrap()).unwrap();
        std::fs::create_dir(&socket).unwrap();
        let opened = OpenSession::open(path.clone());
        std::fs::remove_dir(&socket).unwrap();
        let (current, _) = opened.unwrap();
        assert!(current.listener.is_none());
        let snapshot = current.snapshot(Vec::new()).unwrap();
        assert_eq!(snapshot.project.name, "no bridge");
        assert!(snapshot.agent_bridge_error.unwrap().starts_with("IPC_UNAVAILABLE"));
        current
            .host
            .session
            .edit(vec![EditCmd::RenameProject { name: "edited".into() }], None, Expect::default())
            .unwrap();
        drop(current);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn busy_recent_project_opens_the_next_one_with_a_notice() {
        let dir = std::env::temp_dir().join(format!("capopen-busy-{}", new_id()));
        let (older, newer) = (dir.join("older.capopen"), dir.join("newer.capopen"));
        store::create(&older, &Project::new("Older")).unwrap();
        store::create(&newer, &Project::new("Newer")).unwrap();
        let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&older).unwrap().set_modified(an_hour_ago).unwrap();
        let agent = capopen_session::ProjectSession::open(&newer, capopen_session::Mode::Write, None).unwrap();
        let (opened, notice) = open_recent(store::list_in(&dir));
        let (current, _) = opened.unwrap();
        assert_eq!(current.snapshot(Vec::new()).unwrap().project.name, "Older");
        let notice = notice.unwrap();
        assert_eq!(
            notice,
            "“Newer” is open in another CapOpen window or an AI agent is editing it, so “Older” opened instead. Close it there, then open it from Projects."
        );
        let (opened, notice) = open_recent(store::list_in(&dir));
        assert!(opened.is_none() && notice.is_some());
        assert_eq!(store::list_in(&dir).len(), 2, "no new project is created while projects are busy");
        drop((current, agent));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn quitting_asks_about_real_work_and_waits_for_cancelled_jobs_to_clean_up() {
        let kinds = |list: &[&str]| list.iter().map(|kind| kind.to_string()).collect::<Vec<_>>();
        assert_eq!(quit_question(&kinds(&["audio"])), None);
        assert!(quit_question(&kinds(&["audio", "captions"])).unwrap().starts_with("Speech recognition"));
        assert!(quit_question(&kinds(&["transcription", "export"])).unwrap().starts_with("An export"));
        let dir = std::env::temp_dir().join(format!("capopen-quit-{}", new_id()));
        let path = dir.join("project.capopen");
        store::create(&path, &Project::new("quit")).unwrap();
        let (current, _) = OpenSession::open(path).unwrap();
        let unfinished = dir.join(".capopen-part-agent.mp4");
        std::fs::write(&unfinished, b"partial").unwrap();
        let stamp = current.host.session.stamp();
        let file = unfinished.clone();
        current
            .host
            .start_job("agent", None, "export", stamp, move |cancel, _| {
                while !cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                std::thread::sleep(Duration::from_millis(100));
                std::fs::remove_file(&file).unwrap();
                anyhow::bail!("CANCELLED: job cancelled")
            })
            .unwrap();
        let jobs = Arc::new(Mutex::new(HashMap::new()));
        let workers = ["export:desktop", "audio:clip"].map(|id| {
            let flag = Arc::new(AtomicBool::new(false));
            jobs.lock().unwrap().insert(id.to_string(), flag.clone());
            let desktop = jobs.clone();
            std::thread::spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                std::thread::sleep(Duration::from_millis(100));
                desktop.lock().unwrap().remove(id);
            })
        });
        assert!(drain(&jobs, &current.host, Duration::from_secs(5)));
        assert!(!unfinished.exists(), "the agent export removed its unfinished file before quitting");
        assert!(jobs.lock().unwrap().is_empty(), "audio preparation also stops and removes its partial cache");
        for worker in workers {
            worker.join().unwrap();
        }
        drop(current);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pending_mutation_rejects_replaced_session_epoch() {
        let dir = std::env::temp_dir().join(format!("capopen-epoch-{}", new_id()));
        let old_path = dir.join("old.capopen");
        let next_path = dir.join("next.capopen");
        store::create(&old_path, &Project::new("old")).unwrap();
        store::create(&next_path, &Project::new("next")).unwrap();
        let (old, _) = OpenSession::open(old_path).unwrap();
        let epoch = old.host.session.state().unwrap().stamp.session_epoch;
        let session = Mutex::new(old);
        assert!(lock_session(&session, Some(&epoch)).is_ok());
        let (next, _) = OpenSession::open(next_path).unwrap();
        *session.lock().unwrap() = next;
        let image = dir.join("import.ppm");
        std::fs::write(&image, b"P6\n2 2\n255\nabcdefghijkl").unwrap();
        let asset = probe(&image, new_id()).unwrap();
        let apply = || -> CmdResult<()> {
            let current = lock_session(&session, Some(&epoch))?;
            current
                .host
                .session
                .edit(vec![EditCmd::AddAssets { assets: vec![asset] }], None, Expect::default())
                .map_err(err)?;
            Ok(())
        };
        assert_eq!(apply().unwrap_err(), "EPOCH_CHANGED: another project is open");
        let current = lock_session(&session, None).unwrap();
        assert!(current.host.session.state().unwrap().project.assets.is_empty());
        let epoch = current.host.session.state().unwrap().stamp.session_epoch;
        drop(current);
        assert!(lock_session(&session, Some(&epoch)).is_ok());
        drop(session);
    }

    #[test]
    fn switching_with_slow_job_returns_promptly_and_retains_project_lock() {
        let dir = std::env::temp_dir().join(format!("capopen-switch-{}", new_id()));
        let old_path = dir.join("old.capopen");
        let next_path = dir.join("next.capopen");
        store::create(&old_path, &Project::new("old")).unwrap();
        store::create(&next_path, &Project::new("next")).unwrap();
        let (mut current, _) = OpenSession::open(old_path.clone()).unwrap();
        let (next, _) = OpenSession::open(next_path).unwrap();
        let (release, wait) = mpsc::channel();
        let stamp = current.host.session.state().unwrap().stamp;
        current
            .host
            .start_job("client", None, "test", stamp, move |_, _| {
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(serde_json::json!({}))
            })
            .unwrap();
        let began = Instant::now();
        current.prepare_switch().unwrap();
        current = next;
        let elapsed = began.elapsed();
        let locked = capopen_session::ProjectSession::open(&old_path, capopen_session::Mode::Write, None).is_err();
        let reopened = OpenSession::open(old_path.clone()).err().unwrap().to_string();
        release.send(()).unwrap();
        assert!(reopened.starts_with("CapOpen is still finishing background work"), "{reopened}");
        assert!(elapsed < Duration::from_millis(300), "switch took {elapsed:?}");
        assert!(locked, "old project lock must survive until work ends");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if capopen_session::ProjectSession::open(&old_path, capopen_session::Mode::Write, None).is_ok() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(current);
    }
}
