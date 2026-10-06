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
    thumbs: Mutex<HashMap<String, String>>,
    filmstrips: Mutex<HashMap<String, Filmstrip>>,
    preview_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    bounds_text: Mutex<Option<capopen_engine::text::TextRenderer>>,
    fonts: OnceLock<FontFamilies>,
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
    path: PathBuf,
    stopped: Arc<AtomicBool>,
}

fn lock_session<'a>(
    session: &'a Mutex<OpenSession>,
    expected_epoch: Option<&str>,
) -> CmdResult<std::sync::MutexGuard<'a, OpenSession>> {
    let current = session.lock().unwrap();
    if let Some(epoch) = expected_epoch
        && current.host.session.state().map_err(err)?.stamp.session_epoch != epoch
    {
        return Err("EPOCH_CHANGED: another project is open".into());
    }
    Ok(current)
}

impl OpenSession {
    fn open(path: PathBuf) -> anyhow::Result<(Self, Receiver<SessionEvent>)> {
        let (tx, rx) = mpsc::channel();
        let host = Arc::new(Host::new(store::open(&path, tx)?, store::cache_dir())?);
        #[cfg(unix)]
        let listener = Some(capopen_mcp::ipc::Listener::start(host.clone())?);
        Ok((
            Self {
                host,
                #[cfg(unix)]
                listener,
                path,
                stopped: Arc::new(AtomicBool::new(false)),
            },
            rx,
        ))
    }

    fn close_ipc(&mut self) {
        #[cfg(unix)]
        self.listener.take();
    }

    fn prepare_switch(&mut self) -> anyhow::Result<()> {
        self.close_ipc();
        if let Err(error) = self.host.session.disconnect() {
            #[cfg(unix)]
            {
                self.listener = Some(
                    capopen_mcp::ipc::Listener::start(self.host.clone())
                        .map_err(|restore| anyhow::anyhow!("{error:#}; restoring IPC failed: {restore:#}"))?,
                );
            }
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
        self.thumbs.lock().unwrap().retain(|id, _| project.asset(id).is_some());
        self.filmstrips.lock().unwrap().retain(|id, _| project.asset(id).is_some());
        self.preview_locks.lock().unwrap().retain(|id, _| project.asset(id).is_some());
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
            let lock = self.preview_locks.lock().unwrap().entry(asset_id.into()).or_default().clone();
            (asset, lock)
        };
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        {
            let _current = self.session.lock().unwrap();
            if !self.preview_locks.lock().unwrap().get(asset_id).is_some_and(|current| Arc::ptr_eq(current, &lock)) {
                return Ok(None);
            }
            if let Some(value) = cache.lock().unwrap().get(asset_id) {
                return Ok(Some(value.clone()));
            }
        }
        let value = decode(&asset).map_err(err)?;
        let _current = self.session.lock().unwrap();
        // Removal or replacement invalidates in-flight decodes, even if the ID is reused.
        if !self.preview_locks.lock().unwrap().get(asset_id).is_some_and(|current| Arc::ptr_eq(current, &lock)) {
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

#[tauri::command]
fn start_export(
    app: AppHandle,
    path: String,
    options: jobs::ExportRequest,
    expected_epoch: Option<String>,
) -> CmdResult<String> {
    jobs::start_export(&app, PathBuf::from(path), options, expected_epoch.as_deref())
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

/// The most recent project, or a new one when it cannot be opened, e.g. while an agent edits it.
fn initial_project() -> anyhow::Result<(OpenSession, Receiver<SessionEvent>)> {
    if let Some(recent) = store::list().first() {
        let opened = std::fs::canonicalize(&recent.path).map_err(anyhow::Error::from).and_then(OpenSession::open);
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
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let server = Arc::new(PreviewServer::start()?);
            let (session, events) = initial_project()?;
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
                thumbs: Mutex::new(HashMap::new()),
                filmstrips: Mutex::new(HashMap::new()),
                preview_locks: Mutex::new(HashMap::new()),
                bounds_text: Mutex::new(None),
                fonts: OnceLock::new(),
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
            tauri::RunEvent::WindowEvent { event: tauri::WindowEvent::CloseRequested { api, .. }, .. } => {
                let state = app.state::<AppState>();
                let saved = state.session.lock().unwrap().host.session.disconnect();
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
                let mut current = state.session.lock().unwrap();
                current.close_ipc();
                if let Err(error) = current.host.retire() {
                    log::error!("{error:#}");
                }
                current.stopped.store(true, Ordering::Release);
                if let Err(error) = current.host.session.disconnect() {
                    log::error!("Cannot save project on exit: {error:#}");
                }
            }
            _ => {}
        });
}

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
        release.send(()).unwrap();
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
