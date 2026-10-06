mod audio_out;
mod engine;
mod jobs;
mod preview_server;
mod store;
mod thumbs;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use capopen_engine::edit::{EditCmd, EditOutcome, new_id};
use capopen_engine::media::probe;
use capopen_engine::{Editor, Project};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use engine::{Engine, Msg, Transport};
use preview_server::PreviewServer;
use store::{ProjectSummary, Saver};

pub struct AppState {
    app: AppHandle,
    editor: Mutex<Editor>,
    project_path: Mutex<PathBuf>,
    engine: Engine,
    saver: Saver,
    preview_url: String,
    cache_dir: PathBuf,
    jobs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    thumbs: Mutex<HashMap<String, String>>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    project: Project,
    revision: u64,
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

impl AppState {
    fn snapshot(&self, editor: &Editor, select: Vec<String>) -> Snapshot {
        Snapshot {
            project: editor.project.clone(),
            revision: editor.revision,
            can_undo: editor.can_undo(),
            can_redo: editor.can_redo(),
            path: self.project_path.lock().unwrap().to_string_lossy().into_owned(),
            select,
        }
    }

    /// Pushes the current project to the preview and schedules an autosave.
    fn commit(&self, editor: &Editor, select: Vec<String>) -> Snapshot {
        self.engine.send(Msg::Project(Arc::new(editor.project.clone())));
        let path = self.project_path.lock().unwrap().clone();
        self.saver.schedule(path, editor.project.clone(), editor.revision);
        self.snapshot(editor, select)
    }

    pub fn apply(&self, cmd: EditCmd, coalesce: Option<String>) -> Result<Snapshot, String> {
        let mut editor = self.editor.lock().unwrap();
        let EditOutcome { select } = editor.apply(cmd, coalesce).map_err(err)?;
        Ok(self.commit(&editor, select))
    }

    fn replace_project(&self, project: Project, path: PathBuf) -> Snapshot {
        let mut editor = self.editor.lock().unwrap();
        *editor = Editor::new(project);
        *self.project_path.lock().unwrap() = path;
        self.engine.send(Msg::Seek(0));
        let snap = self.commit(&editor, Vec::new());
        jobs::ensure_audio(self, &editor.project);
        snap
    }
}

#[tauri::command]
fn boot(state: State<'_, AppState>) -> Boot {
    let editor = state.editor.lock().unwrap();
    Boot {
        snapshot: state.snapshot(&editor, Vec::new()),
        preview_url: state.preview_url.clone(),
        transport: *state.engine.transport.lock().unwrap(),
    }
}

#[tauri::command]
fn apply_edit(state: State<'_, AppState>, cmd: EditCmd, coalesce: Option<String>) -> CmdResult<Snapshot> {
    state.apply(cmd, coalesce)
}

#[tauri::command]
fn undo(state: State<'_, AppState>) -> Snapshot {
    let mut editor = state.editor.lock().unwrap();
    editor.undo();
    state.commit(&editor, Vec::new())
}

#[tauri::command]
fn redo(state: State<'_, AppState>) -> Snapshot {
    let mut editor = state.editor.lock().unwrap();
    editor.redo();
    state.commit(&editor, Vec::new())
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
        let editor = state.editor.lock().unwrap();
        state.snapshot(&editor, Vec::new())
    } else {
        let snap = state.apply(EditCmd::AddAssets { assets }, None)?;
        jobs::ensure_audio(&state, &snap.project);
        snap
    };
    Ok(ImportResult { snapshot, added, failed })
}

#[tauri::command]
async fn thumbnail(app: AppHandle, asset_id: String) -> CmdResult<Option<String>> {
    let state = app.state::<AppState>();
    if let Some(t) = state.thumbs.lock().unwrap().get(&asset_id) {
        return Ok(Some(t.clone()));
    }
    let asset = state.editor.lock().unwrap().project.asset(&asset_id).cloned();
    let Some(asset) = asset else { return Ok(None) };
    let url = tauri::async_runtime::spawn_blocking(move || thumbs::thumbnail(&asset)).await.map_err(err)?.map_err(err)?;
    if let Some(u) = &url {
        state.thumbs.lock().unwrap().insert(asset_id, u.clone());
    }
    Ok(url)
}

#[tauri::command]
async fn waveform(app: AppHandle, asset_id: String) -> CmdResult<Option<Vec<u8>>> {
    let state = app.state::<AppState>();
    let asset = state.editor.lock().unwrap().project.asset(&asset_id).cloned();
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

#[tauri::command]
fn new_project(state: State<'_, AppState>, width: u32, height: u32) -> CmdResult<Snapshot> {
    let mut project = Project::new("Untitled project");
    project.canvas.width = width & !1;
    project.canvas.height = height & !1;
    let path = store::new_project_path();
    store::save(&path, &project).map_err(err)?;
    Ok(state.replace_project(project, path))
}

#[tauri::command]
fn open_project(state: State<'_, AppState>, path: String) -> CmdResult<Snapshot> {
    let path = PathBuf::from(path);
    let project = store::load(&path).map_err(err)?;
    Ok(state.replace_project(project, path))
}

#[tauri::command]
fn start_export(app: AppHandle, path: String) -> CmdResult<String> {
    jobs::start_export(&app, PathBuf::from(path))
}

#[tauri::command]
fn cancel_job(state: State<'_, AppState>, id: String) {
    if let Some(flag) = state.jobs.lock().unwrap().get(&id) {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn initial_project() -> (Project, PathBuf) {
    if let Some(recent) = store::list().first() {
        let path = PathBuf::from(&recent.path);
        if let Ok(p) = store::load(&path) {
            return (p, path);
        }
    }
    (Project::new("Untitled project"), store::new_project_path())
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn")).init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let server = Arc::new(PreviewServer::start()?);
            let (project, path) = initial_project();
            let cache_dir = store::cache_dir();
            let engine = Engine::start(app.handle().clone(), server.clone(), Arc::new(project.clone()), cache_dir.clone());
            let state = AppState {
                app: app.handle().clone(),
                editor: Mutex::new(Editor::new(project)),
                project_path: Mutex::new(path),
                engine,
                saver: Saver::start(app.handle().clone()),
                preview_url: server.url.clone(),
                cache_dir,
                jobs: Mutex::new(HashMap::new()),
                thumbs: Mutex::new(HashMap::new()),
            };
            let project = state.editor.lock().unwrap().project.clone();
            // Jobs look the state up from their threads, so it must be managed first.
            app.manage(state);
            jobs::ensure_audio(&app.state::<AppState>(), &project);
            app.emit("ready", ()).ok();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            boot,
            apply_edit,
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
            new_project,
            open_project,
            start_export,
            cancel_job,
            jobs::start_captions,
            jobs::caption_models,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CapOpen");
}
