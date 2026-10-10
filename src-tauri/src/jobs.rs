//! Long-running work off the UI path: audio preparation, export, transcripts and auto captions.
//! Every job reports through `job` events.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use anyhow::Context;
use nuzky_analysis::CaptionGrouping;
use nuzky_analysis::models_dir;
use nuzky_engine::audio::{ensure_pcm, has_audio};
use nuzky_engine::edit::new_id;
use nuzky_engine::export::{Delivery, ExportOptions, Quality, check_options, export};
use nuzky_engine::model::{Asset, AssetKind, ClipContent, Project, TextStyle};
use nuzky_engine::proxy;
use nuzky_engine::voice::{ensure_voice_pcm, voice_pcm_path};
use nuzky_mcp::model_download::{self, Integrity};
use nuzky_mcp::transcript;
use nuzky_session::{host::Host, transcripts::TranscriptStore};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobEvent {
    pub id: String,
    #[cfg_attr(test, ts(type = r#""audio" | "proxy" | "export" | "captions" | "transcript""#))]
    pub kind: &'static str,
    pub label: String,
    #[cfg_attr(test, ts(type = r#""running" | "done" | "failed" | "cancelled""#))]
    pub status: &'static str,
    pub progress: f32,
    pub phase: Option<String>,
    pub message: Option<String>,
    pub output: Option<String>,
}

struct Reporter {
    app: AppHandle,
    event: JobEvent,
    last: Instant,
}

impl Reporter {
    fn new(app: &AppHandle, id: &str, kind: &'static str, label: String) -> Self {
        let event = JobEvent {
            id: id.into(),
            kind,
            label,
            status: "running",
            progress: 0.0,
            phase: None,
            message: None,
            output: None,
        };
        app.emit("job", &event).ok();
        Self { app: app.clone(), event, last: Instant::now() }
    }

    fn progress(&mut self, p: f32, phase: Option<&str>) {
        let phase_changed = phase.map(String::from) != self.event.phase;
        self.event.progress = p.clamp(0.0, 1.0);
        self.event.phase = phase.map(String::from);
        if phase_changed || self.last.elapsed() > Duration::from_millis(100) {
            self.last = Instant::now();
            self.app.emit("job", &self.event).ok();
        }
    }

    fn finish(mut self, result: anyhow::Result<Option<String>>, cancelled: bool) {
        match result {
            Ok(output) => {
                self.event.status = "done";
                self.event.progress = 1.0;
                self.event.output = output;
            }
            Err(_) if cancelled => self.event.status = "cancelled",
            Err(e) => {
                self.event.status = "failed";
                self.event.message = Some(format!("{e:#}"));
            }
        }
        self.app.emit("job", &self.event).ok();
    }
}

/// Exports run one at a time; captions and transcripts share one recognition slot.
fn register(app: &AppHandle, id: &str) -> Option<Arc<AtomicBool>> {
    let state = app.state::<AppState>();
    let mut jobs = state.jobs.lock().unwrap();
    let kind = id.split(':').next().unwrap_or(id);
    let conflicts = |running: &str| {
        let running = running.split(':').next().unwrap_or(running);
        match kind {
            "export" => running == "export",
            "captions" | "transcript" => matches!(running, "captions" | "transcript"),
            "vision-models" => running == "vision-models",
            // Each one decodes a whole video; the next file waits, so the preview keeps some of the machine.
            "proxy" => running == "proxy",
            _ => false,
        }
    };
    if jobs.contains_key(id) || jobs.keys().any(|id| conflicts(id)) {
        return None;
    }
    let flag = Arc::new(AtomicBool::new(false));
    jobs.insert(id.to_string(), flag.clone());
    Some(flag)
}

fn unregister(app: &AppHandle, id: &str) {
    app.state::<AppState>().jobs.lock().unwrap().remove(id);
}

/// Decodes the audio of every asset once into the PCM cache used for playback, waveforms, export and
/// captions, cleans the voice of files whose clips ask for it and makes the preview proxies of heavy video.
pub fn prepare_media(state: &AppState, project: &Project) {
    let app = state.app.clone();
    // Missing media waits for relinking, and a source that failed is not retried on every edit.
    for asset in project.assets.iter().filter(|a| has_audio(a) && Path::new(&a.path).is_file()) {
        let path = nuzky_engine::audio::pcm_path(&state.cache_dir, asset);
        if path.exists() || state.audio_failed.lock().unwrap().contains(&path) {
            continue;
        }
        let id = format!("audio:{}", asset.id);
        let Some(flag) = register(&app, &id) else { continue };
        let (app, asset, cache) = (app.clone(), asset.clone(), state.cache_dir.clone());
        std::thread::spawn(move || {
            let mut rep = Reporter::new(&app, &id, "audio", format!("Preparing audio for {}", asset.name));
            let result = ensure_pcm(&cache, &asset, |p| {
                check_cancelled(&flag)?;
                rep.progress(p, None);
                Ok(())
            })
            .map(|_| None);
            let ok = result.is_ok();
            let cancelled = flag.load(Ordering::Relaxed);
            if !ok && !cancelled {
                app.state::<AppState>().audio_failed.lock().unwrap().insert(path.clone());
            }
            rep.finish(result, cancelled);
            unregister(&app, &id);
            if ok {
                app.emit("audio-ready", asset.id.clone()).ok();
            }
            // A project opened meanwhile can use this asset id for another file (a copy keeps the
            // ids); its request found this job running and was skipped, so it is made now.
            let state = app.state::<AppState>();
            if let Ok(open) = state.project()
                && open.assets.iter().any(|a| a.id == asset.id && nuzky_engine::audio::pcm_path(&cache, a) != path)
            {
                prepare_media(&state, &open);
            }
        });
    }
    ensure_voice(state, project);
    ensure_proxies(state, project);
}

/// Files a clip with Clean voice plays.
fn voice_assets(project: &Project) -> HashSet<&str> {
    project
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter_map(|c| match &c.content {
            ClipContent::Media { asset_id, clean_voice: true, .. } => Some(asset_id.as_str()),
            _ => None,
        })
        .collect()
}

/// Cleans the voice of every file a clip with Clean voice plays, once, as a job per file; playback
/// switches to the cleaned sound when it is ready. Preparation that no clip needs any more (Clean
/// voice turned off, the clip deleted, another project opened) stops.
fn ensure_voice(state: &AppState, project: &Project) {
    let app = state.app.clone();
    let wanted = voice_assets(project);
    for (id, cancel) in state.jobs.lock().unwrap().iter() {
        if id.strip_prefix("voice:").is_some_and(|asset| !wanted.contains(asset)) {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    for asset in project.assets.iter().filter(|a| wanted.contains(a.id.as_str()) && has_audio(a)) {
        if !Path::new(&asset.path).is_file() {
            continue;
        }
        let path = voice_pcm_path(&state.cache_dir, asset);
        if path.exists() || state.audio_failed.lock().unwrap().contains(&path) {
            continue;
        }
        let id = format!("voice:{}", asset.id);
        let Some(flag) = register(&app, &id) else { continue };
        let (worker, job, asset, cache) = (app.clone(), id.clone(), asset.clone(), state.cache_dir.clone());
        let spawned = std::thread::Builder::new().name("clean-voice".into()).spawn(move || {
            let (app, id) = (worker, job);
            let mut rep = Reporter::new(&app, &id, "audio", format!("Cleaning voice in {}", asset.name));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ensure_voice_pcm(&cache, &asset, |p| {
                    check_cancelled(&flag)?;
                    rep.progress(p, None);
                    Ok(())
                })
            }))
            .unwrap_or_else(|p| Err(anyhow::anyhow!("Cleaning the voice crashed: {}", panic_text(&p))));
            let cancelled = flag.load(Ordering::Relaxed);
            if result.is_err() && !cancelled {
                app.state::<AppState>().audio_failed.lock().unwrap().insert(path);
            }
            rep.finish(result.map(|_| None), cancelled);
            unregister(&app, &id);
            // Turned off and on again while this ran, or another file under the same id: prepare
            // what the open project needs now.
            let state = app.state::<AppState>();
            if let Ok(open) = state.project() {
                ensure_voice(&state, &open);
            }
        });
        if spawned.is_err() {
            unregister(&app, &id);
        }
    }
}

/// Makes the preview proxy of each video that decodes slowly (`nuzky_engine::proxy`), one file at a time; the
/// preview switches to it once it is there. A file that leaves the project stops its proxy, and starts it
/// again when it comes back. One the user stopped, or that failed, is not tried again until the app restarts.
fn ensure_proxies(state: &AppState, project: &Project) {
    let app = state.app.clone();
    let wanted: HashSet<&str> = project.assets.iter().map(|a| a.id.as_str()).collect();
    for (id, cancel) in state.jobs.lock().unwrap().iter() {
        if id.strip_prefix("proxy:").is_some_and(|asset| !wanted.contains(asset)) {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    for asset in project.assets.iter().filter(|a| a.kind == AssetKind::Video && Path::new(&a.path).is_file()) {
        let path = proxy::proxy_path(&state.cache_dir, Path::new(&asset.path));
        if path.exists() || state.proxy_skipped.lock().unwrap().contains(&path) {
            continue;
        }
        let id = format!("proxy:{}", asset.id);
        let Some(flag) = register(&app, &id) else { continue };
        let (worker, job, asset, cache) = (app.clone(), id.clone(), asset.clone(), state.cache_dir.clone());
        let spawned = std::thread::Builder::new().name("preview-proxy".into()).spawn(move || {
            let (app, id) = (worker, job);
            let source = PathBuf::from(&asset.path);
            let skip = |path| app.state::<AppState>().proxy_skipped.lock().unwrap().insert(path);
            // Video that decodes fast enough needs none; a file that cannot be read shows its error in the preview.
            if !proxy::wanted(&source).unwrap_or(false) {
                skip(path);
            } else {
                let mut rep = Reporter::new(&app, &id, "proxy", format!("Preparing preview of {}", asset.name));
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    proxy::ensure_proxy(&cache, &source, |p| {
                        check_cancelled(&flag)?;
                        rep.progress(p, None);
                        Ok(())
                    })
                }))
                .unwrap_or_else(|p| Err(anyhow::anyhow!("Preparing the preview crashed: {}", panic_text(&p))));
                let cancelled = flag.load(Ordering::Relaxed);
                let in_project =
                    app.state::<AppState>().project().is_ok_and(|p| p.assets.iter().any(|a| a.path == asset.path));
                if result.is_err() && (!cancelled || in_project) {
                    skip(path);
                }
                rep.finish(result.map(|_| None), cancelled);
            }
            unregister(&app, &id);
            // The next file was waiting for this one.
            let state = app.state::<AppState>();
            if let Ok(open) = state.project() {
                ensure_proxies(&state, &open);
            }
        });
        if spawned.is_err() {
            unregister(&app, &id);
        }
    }
}

#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    /// Short side in pixels: 720, 1080, 1440 or 2160.
    pub resolution: u32,
    pub fps: u32,
    /// "high" | "recommended" | "small"
    pub quality: Quality,
    /// "reels" fixes the format and levels the sound; resolution and fps are then its own.
    #[serde(default)]
    #[cfg_attr(test, ts(optional = nullable))]
    pub preset: Option<Delivery>,
}

impl ExportRequest {
    fn options(&self, replace_existing: bool) -> ExportOptions {
        ExportOptions {
            crf: self.quality.crf(),
            replace_existing,
            resolution: Some(self.resolution),
            fps: Some(self.fps),
            delivery: self.preset,
            ..ExportOptions::default()
        }
    }
}

/// Without confirmed replacement, an existing destination fails before any work starts.
fn check_destination(out: &Path, replace_existing: bool) -> Result<(), String> {
    if !replace_existing && out.symlink_metadata().is_ok() {
        let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Err(format!("DESTINATION_EXISTS: {name} already exists"));
    }
    Ok(())
}

pub fn start_export(
    app: &AppHandle,
    out: PathBuf,
    request: ExportRequest,
    replace_existing: bool,
    expected_epoch: Option<&str>,
) -> Result<String, String> {
    check_destination(&out, replace_existing)?;
    let state = app.state::<AppState>();
    let project =
        crate::lock_session(&state.session, expected_epoch)?.host.session.state().map_err(crate::err)?.project;
    if project.duration_us() <= 0 {
        return Err("Add something to the timeline before exporting.".into());
    }
    check_options(&project, &request.options(replace_existing)).map_err(|e| e.to_string())?;
    let id = format!("export:{}", new_id());
    let cancel = register(app, &id).ok_or("An export is already running")?;
    let (app, cache, job_id) = (app.clone(), state.cache_dir.clone(), id.clone());
    std::thread::Builder::new()
        .name("export".into())
        .spawn(move || {
            let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut rep = Reporter::new(&app, &job_id, "export", format!("Exporting {name}"));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                export(&project, &cache, &out, &request.options(replace_existing), &cancel, |p| {
                    rep.progress(p.fraction, Some(p.phase.label()));
                })
            }))
            .unwrap_or_else(|p| Err(anyhow::anyhow!("Export crashed: {}", panic_text(&p))));
            let cancelled = cancel.load(Ordering::Relaxed);
            rep.finish(result.map(|_| Some(out.to_string_lossy().into_owned())), cancelled);
            unregister(&app, &job_id);
        })
        .map_err(|e| e.to_string())?;
    Ok(id)
}

#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SpeechModel {
    pub id: &'static str,
    pub label: &'static str,
    pub size_mb: u32,
    pub downloaded: bool,
}

// File sizes and SHA-256 LFS object IDs from the publishers' Hugging Face repositories, checked 2026-10-06:
// https://huggingface.co/api/models/ggerganov/whisper.cpp/tree/main
// https://huggingface.co/api/models/ggml-org/whisper-vad/tree/main
const MODELS: &[(&str, &str, u32, Integrity)] = &[
    (
        "base",
        "Fast",
        142,
        Integrity { size: 147_951_465, sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe" },
    ),
    (
        "small",
        "Balanced",
        466,
        Integrity { size: 487_601_967, sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b" },
    ),
    (
        "large-v3-turbo-q5_0",
        "Most accurate",
        547,
        Integrity { size: 574_041_195, sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2" },
    ),
];
const VAD_INTEGRITY: Integrity =
    Integrity { size: 885_098, sha256: "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf" };

fn model_path(id: &str) -> PathBuf {
    models_dir().join(format!("ggml-{id}.bin"))
}

#[tauri::command]
pub fn speech_models() -> Vec<SpeechModel> {
    MODELS
        .iter()
        .map(|(id, label, size, integrity)| SpeechModel {
            id,
            label,
            size_mb: *size,
            downloaded: std::fs::metadata(model_path(id))
                .is_ok_and(|file| file.is_file() && file.len() == integrity.size),
        })
        .collect()
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VisionModels {
    /// Download size of what is missing.
    pub size_mb: u64,
    pub downloaded: bool,
    /// Why covers cannot run on this computer at all, so nothing should be downloaded.
    pub unavailable: Option<String>,
}

/// The face and subject models covers need: whether they can run, are installed and what is left
/// to get. Async, so loading ONNX Runtime the first time never holds up the window.
#[tauri::command]
pub async fn vision_models() -> VisionModels {
    let missing = nuzky_vision::models::missing(nuzky_vision::models::ALL, &models_dir());
    VisionModels {
        size_mb: missing.iter().map(|m| m.size).sum::<u64>().div_ceil(1_000_000),
        downloaded: missing.is_empty(),
        unavailable: nuzky_vision::runtime::require().err().map(|e| format!("{e:#}")),
    }
}

/// Downloads the missing cover models as one job, each checked against its pinned SHA-256.
#[tauri::command]
pub fn start_vision_models(app: AppHandle) -> Result<String, String> {
    nuzky_vision::runtime::require().map_err(|e| format!("{e:#}"))?;
    let id = format!("vision-models:{}", new_id());
    let cancel = register(&app, &id).ok_or("The cover models are already downloading")?;
    let (worker_app, job_id) = (app.clone(), id.clone());
    let spawn = std::thread::Builder::new().name("vision-models".into()).spawn(move || {
        let mut rep = Reporter::new(&worker_app, &job_id, "vision-models", "Cover models".into());
        let dir = models_dir();
        let missing = nuzky_vision::models::missing(nuzky_vision::models::ALL, &dir);
        let total = missing.iter().map(|m| m.size).sum::<u64>().max(1) as f32;
        let mut before = 0;
        let result = missing.iter().try_for_each(|model| {
            let integrity = Integrity { size: model.size, sha256: model.sha256 };
            model_download::download(model.url, &model.path(&dir), integrity, &cancel, |part| {
                rep.progress((before as f32 + part * model.size as f32) / total, Some("Downloading cover models"))
            })?;
            before += model.size;
            anyhow::Ok(())
        });
        rep.finish(result.map(|()| None), cancel.load(Ordering::Relaxed));
        unregister(&worker_app, &job_id);
    });
    if let Err(error) = spawn {
        unregister(&app, &id);
        return Err(format!("Starting the cover model download: {error}"));
    }
    Ok(id)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptionRequest {
    pub model: String,
    /// ISO code such as "cs", or "auto".
    pub language: String,
    pub style: TextStyle,
    /// Most words on screen at once (reels use 1–3); `None` uses phrase grouping,
    /// capped by PHRASE_MAX_WORDS and PHRASE_MAX_CHARS unless a limit is supplied.
    #[serde(default)]
    pub max_words: Option<u8>,
    /// Most characters per caption; `None` uses the phrase limit of 42.
    #[serde(default)]
    pub max_chars: Option<u8>,
}

const PHRASE_MAX_WORDS: usize = 12;
const PHRASE_MAX_CHARS: usize = 42;

impl CaptionRequest {
    fn grouping(&self) -> CaptionGrouping {
        CaptionGrouping {
            max_words: self.max_words.map(usize::from).unwrap_or(PHRASE_MAX_WORDS),
            max_chars: self.max_chars.map(usize::from).unwrap_or(PHRASE_MAX_CHARS),
            ..CaptionGrouping::default()
        }
    }
}

/// What a recognition job does; captions are built from the words once they are all there.
struct SpeechRequest {
    model: String,
    language: String,
    /// Recognise every heard file again, not only those without a transcript.
    refresh: bool,
    captions: Option<CaptionRequest>,
}

#[tauri::command]
pub fn start_captions(
    app: AppHandle,
    request: CaptionRequest,
    expected_epoch: Option<String>,
) -> Result<String, String> {
    let (model, language) = (request.model.clone(), request.language.clone());
    start_speech(
        app,
        SpeechRequest { model, language, refresh: false, captions: Some(request) },
        expected_epoch.as_deref(),
    )
}

#[tauri::command]
pub fn start_transcript(
    app: AppHandle,
    model: String,
    language: String,
    refresh: bool,
    expected_epoch: Option<String>,
) -> Result<String, String> {
    start_speech(app, SpeechRequest { model, language, refresh, captions: None }, expected_epoch.as_deref())
}

fn start_speech(app: AppHandle, request: SpeechRequest, expected_epoch: Option<&str>) -> Result<String, String> {
    if !MODELS.iter().any(|(id, ..)| *id == request.model) {
        return Err("Unknown speech model".into());
    }
    let language = &request.language;
    if language != "auto" && (language.contains('\0') || whisper_rs::get_lang_id(language).is_none()) {
        return Err(format!("Unknown language: {language}"));
    }
    let state = app.state::<AppState>();
    let (host, project) = {
        let current = crate::lock_session(&state.session, expected_epoch)?;
        (Arc::downgrade(&current.host), current.host.session.state().map_err(crate::err)?.project)
    };
    let heard = transcript::heard_assets(&project);
    if heard.is_empty() {
        return Err("No video clip with sound on the timeline to transcribe.".into());
    }
    let assets: Vec<Asset> = project.assets.into_iter().filter(|a| heard.contains(&a.id)).collect();
    let kind = if request.captions.is_some() { "captions" } else { "transcript" };
    let id = format!("{kind}:{}", new_id());
    let cancel = register(&app, &id).ok_or("Speech recognition is already running")?;
    let (worker_app, job_id) = (app.clone(), id.clone());
    let spawn = std::thread::Builder::new().name(kind.into()).spawn(move || {
        let label = if request.captions.is_some() { "Auto captions" } else { "Transcript" };
        let mut rep = Reporter::new(&worker_app, &job_id, kind, label.into());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_speech_job(&host, assets, &request, &cancel, &mut rep)
        }))
        .unwrap_or_else(|p| Err(anyhow::anyhow!("Speech recognition crashed: {}", panic_text(&p))));
        rep.finish(result, cancel.load(Ordering::Relaxed));
        unregister(&worker_app, &job_id);
    });
    if let Err(error) = spawn {
        unregister(&app, &id);
        return Err(format!("Starting speech recognition: {error}"));
    }
    Ok(id)
}

fn run_speech_job(
    host: &Weak<Host>,
    assets: Vec<Asset>,
    request: &SpeechRequest,
    cancel: &AtomicBool,
    rep: &mut Reporter,
) -> anyhow::Result<Option<String>> {
    const SWITCHED: &str = "Another project was opened, so the speech recognition result was not applied.";
    let (store, cache) = {
        let host = host.upgrade().context(SWITCHED)?;
        (host.transcripts.clone(), host.cache_dir.clone())
    };
    let mut missing = Vec::new();
    for asset in assets {
        if request.refresh || store.get(&asset)?.is_none() {
            missing.push(asset);
        }
    }
    let estimated = recognise(&store, &cache, missing, request, cancel, rep)?;
    check_cancelled(cancel)?;
    // Recognition goes on without the word timing model, for example offline; say so.
    let note = if estimated { ". Word times are estimated: the word timing model could not be loaded" } else { "" };
    let app = rep.app.clone();
    let state = app.state::<AppState>();
    let current = state.session.lock().unwrap();
    anyhow::ensure!(host.ptr_eq(&Arc::downgrade(&current.host)), SWITCHED);
    let view = current.host.session.state()?;
    let derived = transcript::derive(&view.project, &current.host.transcripts)?;
    anyhow::ensure!(!derived.words.is_empty(), "No speech was recognised.");
    let Some(captions) = &request.captions else {
        return Ok(Some(count_label(derived.words.len(), "word", "words") + note));
    };
    anyhow::ensure!(
        derived.untranscribed.is_empty(),
        "A clip was added during recognition. Generate the captions again."
    );
    rep.progress(1.0, Some("Grouping captions"));
    let (cmd, count) =
        transcript::caption_edit(&derived.words, &view.project, captions.style.clone(), captions.grouping())?;
    current
        .host
        .session
        .edit(
            vec![cmd],
            None,
            nuzky_session::Expect { revision: None, speech_layout_key: Some(view.speech_layout_key) },
        )
        .context("Applying captions")?;
    // Not the frontend's own edit: send it the new timeline, as for an agent's edits.
    if let Ok(snap) = current.snapshot(Vec::new()) {
        app.emit("project-changed", snap).ok();
    }
    Ok(Some(count_label(count, "caption", "captions") + note))
}

/// Recognises each file once, in its own time, storing every transcript as soon as it is ready.
/// True when a file's words could have been measured but kept Whisper's estimates.
fn recognise(
    store: &TranscriptStore,
    cache: &Path,
    assets: Vec<Asset>,
    request: &SpeechRequest,
    cancel: &AtomicBool,
    rep: &mut Reporter,
) -> anyhow::Result<bool> {
    if assets.is_empty() {
        rep.progress(1.0, Some("Using stored transcripts"));
        return Ok(false);
    }
    let mut estimated = false;
    let models = (download_model(&request.model, cancel, rep)?, download_vad(cancel, rep)?);
    let mut recognised = HashSet::new();
    for (i, asset) in assets.iter().enumerate() {
        check_cancelled(cancel)?;
        // The same file imported twice is recognised once.
        if !recognised.insert(store.fingerprint(asset)?) {
            continue;
        }
        let done = i as f32 / assets.len() as f32;
        rep.progress(done, Some("Recognising speech"));
        let stage = |stage| match stage {
            transcript::Stage::Waiting => rep.progress(done, Some("Waiting for another transcription")),
            transcript::Stage::Recognising => rep.progress(done, Some("Recognising speech")),
            transcript::Stage::DownloadingAligner(part) => rep.progress(part, Some("Downloading word timing model")),
            transcript::Stage::Aligning => rep.progress(done, Some("Measuring word times")),
        };
        let record =
            transcript::recognise(store, asset, cache, &request.model, &models, &request.language, cancel, stage)
                .with_context(|| format!("Transcribing {}", asset.name))?;
        estimated |= record.alignment.is_none() && nuzky_analysis::align_model(&record.language).is_some();
    }
    Ok(estimated)
}

use nuzky_analysis::VAD_MODEL;

fn download_model(id: &str, cancel: &AtomicBool, rep: &mut Reporter) -> anyhow::Result<PathBuf> {
    let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{id}.bin");
    let integrity = MODELS.iter().find(|(model, ..)| *model == id).context("Unknown speech model")?.3;
    model_download::download(&url, &model_path(id), integrity, cancel, |value| {
        rep.progress(value, Some("Downloading speech model"))
    })
}

/// Voice activity detection model; skipping non-speech stops Whisper from inventing
/// captions over music and silence.
fn download_vad(cancel: &AtomicBool, rep: &mut Reporter) -> anyhow::Result<PathBuf> {
    let url = format!("https://huggingface.co/ggml-org/whisper-vad/resolve/main/{VAD_MODEL}");
    model_download::download(&url, &models_dir().join(VAD_MODEL), VAD_INTEGRITY, cancel, |value| {
        rep.progress(value, Some("Downloading voice detector"))
    })
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn check_cancelled(cancel: &AtomicBool) -> anyhow::Result<()> {
    anyhow::ensure!(!cancel.load(Ordering::Relaxed), "CANCELLED: job cancelled");
    Ok(())
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown error".into())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_uses_shared_quality_and_confirmed_replacement() {
        for (quality, crf) in [("high", 17), ("recommended", 21), ("small", 26)] {
            let request: ExportRequest = serde_json::from_value(serde_json::json!({
                "resolution": 1080, "fps": 30, "quality": quality
            }))
            .unwrap();
            assert_eq!(request.options(true).crf, crf);
            assert!(request.options(true).replace_existing);
            assert!(!request.options(false).replace_existing);
            assert_eq!(request.options(true).delivery, None, "a request from before presets is a plain export");
        }
        let reels: ExportRequest = serde_json::from_value(serde_json::json!({
            "resolution": 1080, "fps": 30, "quality": "recommended", "preset": "reels"
        }))
        .unwrap();
        assert_eq!(reels.options(false).delivery, Some(Delivery::Reels));
    }

    #[test]
    fn unconfirmed_existing_destination_fails_before_export() {
        let dir = std::env::temp_dir().join(format!("nuzky-destination-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("clip.mp4");
        check_destination(&out, false).unwrap();
        std::fs::write(&out, b"earlier export").unwrap();
        assert_eq!(check_destination(&out, false).unwrap_err(), "DESTINATION_EXISTS: clip.mp4 already exists");
        check_destination(&out, true).unwrap();
        #[cfg(unix)]
        {
            let link = dir.join("dangling.mp4");
            std::os::unix::fs::symlink(dir.join("missing.mp4"), &link).unwrap();
            assert!(check_destination(&link, false).unwrap_err().starts_with("DESTINATION_EXISTS"));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn request(max_words: Option<u8>, max_chars: Option<u8>) -> CaptionRequest {
        CaptionRequest {
            model: "small".into(),
            language: "cs".into(),
            max_words,
            max_chars,
            style: TextStyle {
                font_family: Some("Inter".into()),
                font_size: 95.0,
                color: "#ffffff".into(),
                bold: false,
                stroke_width: 7.5,
                stroke_color: "#000000".into(),
                background: None,
                max_width: None,
                highlight: None,
            },
        }
    }

    #[test]
    fn caption_request_selects_reels_or_phrases() {
        let reels = request(Some(3), Some(15)).grouping();
        assert_eq!((reels.max_words, reels.max_chars), (3, 15));
        let phrases = request(None, None).grouping();
        assert_eq!((phrases.max_words, phrases.max_chars), (12, 42));
        assert_eq!(phrases.break_gap_us, CaptionGrouping::default().break_gap_us);
        assert_eq!(request(Some(1), None).grouping().max_chars, 42);
        assert_eq!(request(None, Some(15)).grouping().max_words, 12);
    }
}
