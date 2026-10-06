mod audio_out;
mod engine;
mod jobs;
mod preview_server;
mod store;
mod thumbs;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::mpsc::{self, Receiver};

use capopen_engine::edit::{EditCmd, new_id};
use capopen_engine::media::probe;
use capopen_engine::Project;
use capopen_session::{Expect, Origin, ProjectSession, RecoveryAction, SessionEvent};
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
    thumbs: Mutex<HashMap<String, String>>,
    filmstrips: Mutex<HashMap<String, Filmstrip>>,
    preview_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    bounds_text: Mutex<Option<capopen_engine::text::TextRenderer>>,
    fonts: OnceLock<FontFamilies>,
    transcript: Mutex<Option<jobs::CachedTranscript>>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    project: Project,
    revision: u64,
    session_epoch: String,
    open_run: Option<String>,
    recovery: bool,
    can_undo: bool,
    can_redo: bool,
    path: String,
    select: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Boot {
    snapshot: Snapshot,
    preview_url: String,
    transport: Transport,
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
    e.to_string()
}

struct OpenSession {
    session: Arc<ProjectSession>,
    path: PathBuf,
    stopped: Arc<AtomicBool>,
}

impl OpenSession {
    fn open(path: PathBuf) -> anyhow::Result<(Self, Receiver<SessionEvent>)> {
        let (tx, rx) = mpsc::channel();
        let session = Arc::new(store::open(&path, tx)?);
        Ok((Self { session, path, stopped: Arc::new(AtomicBool::new(false)) }, rx))
    }

    fn snapshot(&self, select: Vec<String>) -> CmdResult<Snapshot> {
        let (state, can_undo, can_redo) = self.session.view().map_err(err)?;
        Ok(Snapshot {
            project: state.project,
            revision: state.stamp.revision,
            session_epoch: state.stamp.session_epoch,
            open_run: state.open_run.map(|run| run.label),
            recovery: state.recovery_checkpoint.is_some(),
            can_undo,
            can_redo,
            path: self.path.to_string_lossy().into_owned(),
            select,
        })
    }

    fn start_pump(&self, app: AppHandle, rx: Receiver<SessionEvent>) {
        let stopped = self.stopped.clone();
        let session = Arc::downgrade(&self.session);
        std::thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let event = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(event) => event,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let state = app.state::<AppState>();
                let current = state.session.lock().unwrap();
                if stopped.load(Ordering::Acquire) || !session.ptr_eq(&Arc::downgrade(&current.session)) { break; }
                match event {
                    SessionEvent::Changed { origin, .. } => {
                        if let Ok(snap) = current.snapshot(Vec::new()) {
                            state.publish_project(&snap.project);
                            // The frontend's own edits, undo and redo already return this snapshot.
                            if !matches!(origin, Origin::User | Origin::Undo | Origin::Redo) {
                                app.emit("project-changed", snap).ok();
                            }
                        }
                    }
                    SessionEvent::Saved { revision, error } => { app.emit("saved", store::SavedEvent { revision, error }).ok(); }
                    SessionEvent::Run(run) => { app.emit("run-changed", run.map(|run| run.label)).ok(); }
                }
            }
        });
    }
}

impl Drop for OpenSession {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

impl AppState {
    fn project(&self) -> CmdResult<Project> {
        self.session.lock().unwrap().session.state().map(|state| state.project).map_err(err)
    }

    fn publish_project(&self, project: &Project) {
        self.thumbs.lock().unwrap().retain(|id, _| project.asset(id).is_some());
        self.filmstrips.lock().unwrap().retain(|id, _| project.asset(id).is_some());
        self.preview_locks.lock().unwrap().retain(|id, _| project.asset(id).is_some());
        self.engine.send(Msg::Project(Arc::new(project.clone())));
    }

    fn apply_batch(&self, cmds: Vec<EditCmd>, coalesce: Option<String>, expect: Expect) -> CmdResult<Snapshot> {
        let current = self.session.lock().unwrap();
        let result = current.session.edit(cmds, coalesce, expect).map_err(err)?;
        current.snapshot(result.outcome.select)
    }

    pub fn apply(&self, cmd: EditCmd, coalesce: Option<String>) -> CmdResult<Snapshot> {
        self.apply_batch(vec![cmd], coalesce, Expect::default())
    }

    fn asset_preview<T: Clone>(&self, asset_id: &str, cache: &Mutex<HashMap<String, T>>, decode: impl FnOnce(&capopen_engine::model::Asset) -> anyhow::Result<Option<T>>) -> CmdResult<Option<T>> {
        let (asset, lock) = {
            let current = self.session.lock().unwrap();
            let Some(asset) = current.session.state().map_err(err)?.project.asset(asset_id).cloned() else { return Ok(None) };
            let lock = self.preview_locks.lock().unwrap().entry(asset_id.into()).or_default().clone();
            (asset, lock)
        };
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        {
            let _current = self.session.lock().unwrap();
            if !self.preview_locks.lock().unwrap().get(asset_id).is_some_and(|current| Arc::ptr_eq(current, &lock)) { return Ok(None); }
            if let Some(value) = cache.lock().unwrap().get(asset_id) { return Ok(Some(value.clone())); }
        }
        let value = decode(&asset).map_err(err)?;
        let _current = self.session.lock().unwrap();
        // Removal or replacement invalidates in-flight decodes, even if the ID is reused.
        if !self.preview_locks.lock().unwrap().get(asset_id).is_some_and(|current| Arc::ptr_eq(current, &lock)) { return Ok(None); }
        if let Some(value) = &value { cache.lock().unwrap().insert(asset_id.into(), value.clone()); }
        Ok(value)
    }

    fn replace_project(&self, current: &mut OpenSession, path: PathBuf) -> CmdResult<Snapshot> {
        let (next, rx) = OpenSession::open(path).map_err(err)?;
        let snap = next.snapshot(Vec::new())?;
        current.session.disconnect().map_err(err)?;
        *current = next;
        // The startup picker uses modification time, including projects opened without edits.
        if let Err(error) = current.session.disconnect() {
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
        snapshot: state.session.lock().unwrap().snapshot(Vec::new())?,
        preview_url: state.preview_url.clone(),
        transport: *state.engine.transport.lock().unwrap(),
    })
}

#[tauri::command]
fn apply_edit(state: State<'_, AppState>, cmd: EditCmd, coalesce: Option<String>, expect_revision: Option<u64>, expect_speech_key: Option<String>) -> CmdResult<Snapshot> {
    state.apply_batch(vec![cmd], coalesce, Expect { revision: expect_revision, speech_key: expect_speech_key })
}

#[tauri::command]
fn apply_edits(state: State<'_, AppState>, cmds: Vec<EditCmd>, coalesce: Option<String>, expect_revision: Option<u64>, expect_speech_key: Option<String>) -> CmdResult<Snapshot> {
    state.apply_batch(cmds, coalesce, Expect { revision: expect_revision, speech_key: expect_speech_key })
}

#[tauri::command]
fn undo(state: State<'_, AppState>) -> CmdResult<Snapshot> {
    let current = state.session.lock().unwrap();
    current.session.undo().map_err(err)?;
    current.snapshot(Vec::new())
}

#[tauri::command]
fn redo(state: State<'_, AppState>) -> CmdResult<Snapshot> {
    let current = state.session.lock().unwrap();
    current.session.redo().map_err(err)?;
    current.snapshot(Vec::new())
}

#[tauri::command]
fn set_ui_context(state: State<'_, AppState>, selection: Vec<String>, playhead_us: i64) {
    state.session.lock().unwrap().session.set_ui_context(selection, playhead_us);
}

/// Stop in the "AI is editing" bar: the run ends with its changes kept, as one undo step.
#[tauri::command]
fn stop_run(state: State<'_, AppState>) -> CmdResult<Snapshot> {
    let current = state.session.lock().unwrap();
    current.session.stop_run().map_err(err)?;
    current.snapshot(Vec::new())
}

#[tauri::command]
fn resolve_recovery(state: State<'_, AppState>, action: String) -> CmdResult<Snapshot> {
    let action = match action.as_str() {
        "keep" => RecoveryAction::Keep,
        "restore" => RecoveryAction::Restore,
        _ => return Err("Unknown recovery action".into()),
    };
    let current = state.session.lock().unwrap();
    current.session.resolve_recovery(action).map_err(err)?;
    current.snapshot(Vec::new())
}

#[tauri::command]
async fn import_media(app: AppHandle, paths: Vec<String>) -> CmdResult<ImportResult> {
    let probed = tauri::async_runtime::spawn_blocking(move || {
        paths.into_iter().map(|p| (p.clone(), probe(Path::new(&p), new_id()))).collect::<Vec<_>>()
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
        state.session.lock().unwrap().snapshot(Vec::new())?
    } else {
        let snap = state.apply(EditCmd::AddAssets { assets }, None)?;
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
    }).await.map_err(err)?
}

#[tauri::command]
async fn waveform(app: AppHandle, asset_id: String) -> CmdResult<Option<Vec<u8>>> {
    let state = app.state::<AppState>();
    let asset = state.project()?.asset(&asset_id).cloned();
    let Some(asset) = asset else { return Ok(None) };
    let path = capopen_engine::media::pcm_path(&state.cache_dir, &asset);
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
    state.fonts.get_or_init(|| {
        use capopen_engine::text::{BUNDLED_FONT_FAMILIES, TextRenderer};
        let mut renderer = state.bounds_text.lock().unwrap();
        let families = renderer.get_or_insert_with(TextRenderer::new).font_families();
        let (bundled, system) = families.into_iter().partition(|name| BUNDLED_FONT_FAMILIES.contains(&name.as_str()));
        FontFamilies { bundled, system }
    }).clone()
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
    if current.path == path { return current.snapshot(Vec::new()); }
    state.replace_project(&mut current, path)
}

#[tauri::command]
fn start_export(app: AppHandle, path: String, options: jobs::ExportRequest) -> CmdResult<String> {
    jobs::start_export(&app, PathBuf::from(path), options)
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
        capopen_engine::render::layer_bounds(&project, t_us, text).into_iter().map(|(clip_id, corners)| LayerBounds { clip_id, corners }).collect()
    }).await.map_err(err)
}

#[tauri::command]
async fn filmstrip(app: AppHandle, asset_id: String) -> CmdResult<Option<Filmstrip>> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.asset_preview(&asset_id, &state.filmstrips, thumbs::filmstrip)
    }).await.map_err(err)?
}

#[tauri::command]
fn cancel_job(state: State<'_, AppState>, id: String) {
    if let Some(flag) = state.jobs.lock().unwrap().get(&id) {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The most recent project, or a new one when it cannot be opened, e.g. while an agent edits it.
fn initial_project() -> anyhow::Result<(OpenSession, Receiver<SessionEvent>)> {
    if let Some(recent) = store::list().first() {
        let opened = std::fs::canonicalize(&recent.path)
            .map_err(anyhow::Error::from)
            .and_then(OpenSession::open);
        match opened {
            Ok(found) => return Ok(found),
            Err(error) => log::warn!("Starting with a new project: {error:#}"),
        }
    }
    let project = Project::new("Untitled project");
    let path = store::new_project_path();
    store::create(&path, &project)?;
    OpenSession::open(path)
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn")).init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let server = Arc::new(PreviewServer::start()?);
            let (session, events) = initial_project()?;
            let project = session.session.state()?.project;
            let cache_dir = store::cache_dir();
            let engine = Engine::start(app.handle().clone(), server.clone(), Arc::new(project.clone()), cache_dir.clone());
            let state = AppState {
                app: app.handle().clone(),
                session: Mutex::new(session),
                engine,
                preview_url: server.url.clone(),
                cache_dir,
                jobs: Mutex::new(HashMap::new()),
                close_failed: AtomicBool::new(false),
                thumbs: Mutex::new(HashMap::new()),
                filmstrips: Mutex::new(HashMap::new()),
                preview_locks: Mutex::new(HashMap::new()),
                bounds_text: Mutex::new(None),
                fonts: OnceLock::new(),
                transcript: Mutex::new(None),
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
            jobs::caption_models,
            jobs::start_transcript,
            jobs::get_transcript,
        ])
        .build(tauri::generate_context!())
        .expect("error while building CapOpen")
        .run(|app, event| match event {
            tauri::RunEvent::WindowEvent { event: tauri::WindowEvent::CloseRequested { api, .. }, .. } => {
                let state = app.state::<AppState>();
                let saved = state.session.lock().unwrap().session.disconnect();
                // A second close after a failed save quits anyway; the user has been told what is lost.
                if let Err(error) = saved
                    && !state.close_failed.swap(true, Ordering::AcqRel)
                {
                    api.prevent_close();
                    app.emit("close-save-failed", format!("{error:#}")).ok();
                }
            }
            tauri::RunEvent::Exit => {
                let state = app.state::<AppState>();
                let current = state.session.lock().unwrap();
                current.stopped.store(true, Ordering::Release);
                if let Err(error) = current.session.disconnect() {
                    log::error!("Cannot save project on exit: {error:#}");
                }
            }
            _ => {}
        });
}
