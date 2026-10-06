//! Long-running work off the UI path: audio preparation, export, transcripts and auto captions.
//! Every job reports through `job` events and can be cancelled.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use capopen_analysis::CaptionGrouping;
use capopen_engine::audio::{ensure_pcm, has_audio};
use capopen_engine::edit::new_id;
use capopen_engine::export::{ExportOptions, Quality, export};
use capopen_analysis::models_dir;
use capopen_engine::model::{Asset, Project, TextStyle};
use capopen_mcp::transcript;
use capopen_session::{host::Host, transcripts::TranscriptStore};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobEvent {
    pub id: String,
    pub kind: &'static str,
    pub label: String,
    /// running | done | failed | cancelled
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

/// Decodes the audio of every asset once into the PCM cache used for playback,
/// waveforms, export and captions.
pub fn ensure_audio(state: &AppState, project: &Project) {
    let app = state.app.clone();
    for asset in project.assets.iter().filter(|a| has_audio(a)) {
        let path = capopen_engine::audio::pcm_path(&state.cache_dir, asset);
        if path.exists() {
            continue;
        }
        let id = format!("audio:{}", asset.id);
        let Some(_flag) = register(&app, &id) else { continue };
        let (app, asset, cache) = (app.clone(), asset.clone(), state.cache_dir.clone());
        std::thread::spawn(move || {
            let mut rep = Reporter::new(&app, &id, "audio", format!("Preparing audio for {}", asset.name));
            let result = ensure_pcm(&cache, &asset, |p| rep.progress(p, None)).map(|_| None);
            let ok = result.is_ok();
            rep.finish(result, false);
            unregister(&app, &id);
            if ok {
                app.emit("audio-ready", asset.id.clone()).ok();
            }
        });
    }
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    /// Short side in pixels: 720, 1080, 1440 or 2160.
    pub resolution: u32,
    pub fps: u32,
    /// "high" | "recommended" | "small"
    pub quality: Quality,
}

impl ExportRequest {
    fn options(&self) -> ExportOptions {
        ExportOptions { crf: self.quality.crf(), replace_existing: true, resolution: Some(self.resolution), fps: Some(self.fps), ..ExportOptions::default() }
    }
}

pub fn start_export(app: &AppHandle, out: PathBuf, request: ExportRequest, expected_epoch: Option<&str>) -> Result<String, String> {
    let state = app.state::<AppState>();
    let project = crate::lock_session(&state.session, expected_epoch)?.host.session.state().map_err(crate::err)?.project;
    if project.duration_us() <= 0 {
        return Err("Add something to the timeline before exporting.".into());
    }
    let id = format!("export:{}", new_id());
    let cancel = register(app, &id).ok_or("An export is already running")?;
    let (app, cache, job_id) = (app.clone(), state.cache_dir.clone(), id.clone());
    std::thread::Builder::new()
        .name("export".into())
        .spawn(move || {
            let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut rep = Reporter::new(&app, &job_id, "export", format!("Exporting {name}"));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                export(&project, &cache, &out, &request.options(), &cancel, |p| {
                    rep.progress(p.frame as f32 / p.total_frames.max(1) as f32, Some("Rendering"));
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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SpeechModel {
    pub id: &'static str,
    pub label: &'static str,
    pub size_mb: u32,
    pub downloaded: bool,
}

const MODELS: &[(&str, &str, u32)] = &[
    ("base", "Fast", 142),
    ("small", "Balanced", 466),
    ("large-v3-turbo-q5_0", "Most accurate", 547),
];

fn model_path(id: &str) -> PathBuf {
    models_dir().join(format!("ggml-{id}.bin"))
}

#[tauri::command]
pub fn speech_models() -> Vec<SpeechModel> {
    MODELS
        .iter()
        .map(|(id, label, size)| SpeechModel { id, label, size_mb: *size, downloaded: model_path(id).exists() })
        .collect()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptionRequest {
    pub model: String,
    /// ISO code such as "cs", or "auto".
    pub language: String,
    pub style: TextStyle,
    /// Most words on screen at once (reels use 1–3); `None` keeps whole phrases.
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
pub fn start_captions(app: AppHandle, request: CaptionRequest, expected_epoch: Option<String>) -> Result<String, String> {
    let (model, language) = (request.model.clone(), request.language.clone());
    start_speech(app, SpeechRequest { model, language, refresh: false, captions: Some(request) }, expected_epoch.as_deref())
}

#[tauri::command]
pub fn start_transcript(app: AppHandle, model: String, language: String, refresh: bool, expected_epoch: Option<String>) -> Result<String, String> {
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
        })).unwrap_or_else(|p| Err(anyhow::anyhow!("Speech recognition crashed: {}", panic_text(&p))));
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
    recognise(&store, &cache, missing, request, cancel, rep)?;
    check_cancelled(cancel)?;
    let app = rep.app.clone();
    let state = app.state::<AppState>();
    let current = state.session.lock().unwrap();
    anyhow::ensure!(host.ptr_eq(&Arc::downgrade(&current.host)), SWITCHED);
    let view = current.host.session.state()?;
    let derived = transcript::derive(&view.project, &current.host.transcripts)?;
    anyhow::ensure!(!derived.words.is_empty(), "No speech was recognised.");
    let Some(captions) = &request.captions else {
        return Ok(Some(count_label(derived.words.len(), "word", "words")));
    };
    anyhow::ensure!(derived.untranscribed.is_empty(), "A clip was added during recognition. Generate the captions again.");
    rep.progress(1.0, Some("Grouping captions"));
    let (cmd, count) = transcript::caption_edit(&derived.words, &view.project, captions.style.clone(), captions.grouping())?;
    current.host.session.edit(vec![cmd], None, capopen_session::Expect { revision: None, speech_layout_key: Some(view.speech_layout_key) })
        .context("Applying captions")?;
    // Not the frontend's own edit: send it the new timeline, as for an agent's edits.
    if let Ok(snap) = current.snapshot(Vec::new()) {
        app.emit("project-changed", snap).ok();
    }
    Ok(Some(count_label(count, "caption", "captions")))
}

/// Recognises each file once, in its own time, storing every transcript as soon as it is ready.
fn recognise(
    store: &TranscriptStore,
    cache: &Path,
    assets: Vec<Asset>,
    request: &SpeechRequest,
    cancel: &AtomicBool,
    rep: &mut Reporter,
) -> anyhow::Result<()> {
    if assets.is_empty() {
        rep.progress(1.0, Some("Using stored transcripts"));
        return Ok(());
    }
    let models = (download_model(&request.model, cancel, rep)?, download_vad(cancel, rep)?);
    let mut recognised = HashSet::new();
    for (i, asset) in assets.iter().enumerate() {
        check_cancelled(cancel)?;
        // The same file imported twice is recognised once.
        if !recognised.insert(store.fingerprint(asset)?) {
            continue;
        }
        rep.progress(i as f32 / assets.len() as f32, Some("Recognising speech"));
        transcript::recognise(store, asset, cache, &request.model, &models, &request.language, cancel)
            .with_context(|| format!("Transcribing {}", asset.name))?;
    }
    Ok(())
}

const VAD_MODEL: &str = "ggml-silero-v5.1.2.bin";

fn download_model(id: &str, cancel: &AtomicBool, rep: &mut Reporter) -> anyhow::Result<PathBuf> {
    let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{id}.bin");
    download(&url, &model_path(id), cancel, rep, "Downloading speech model")
}

/// Voice activity detection model; skipping non-speech stops Whisper from inventing
/// captions over music and silence.
fn download_vad(cancel: &AtomicBool, rep: &mut Reporter) -> anyhow::Result<PathBuf> {
    let url = format!("https://huggingface.co/ggml-org/whisper-vad/resolve/main/{VAD_MODEL}");
    download(&url, &models_dir().join(VAD_MODEL), cancel, rep, "Downloading voice detector")
}

fn download(url: &str, path: &Path, cancel: &AtomicBool, rep: &mut Reporter, phase: &str) -> anyhow::Result<PathBuf> {
    check_cancelled(cancel)?;
    let path = path.to_path_buf();
    if path.exists() {
        return Ok(path);
    }
    rep.progress(0.0, Some(phase));
    std::fs::create_dir_all(models_dir()).context("Creating speech model directory")?;
    let response = ureq::get(url).call().with_context(|| format!("{phase}: requesting model"))?;
    let total: u64 = response.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse().ok()).unwrap_or(0);
    let mut reader = response.into_body().into_reader();
    let tmp = path.with_extension("part");
    let mut file = std::io::BufWriter::new(std::fs::File::create(&tmp).context("Creating model download")?);
    let mut buf = vec![0u8; 1 << 16];
    let mut done: u64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            std::fs::remove_file(&tmp).ok();
            anyhow::bail!("CANCELLED: job cancelled");
        }
        let n = reader.read(&mut buf).context("Reading model download")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("Writing model download")?;
        done += n as u64;
        if total > 0 {
            rep.progress(done as f32 / total as f32, Some(phase));
        }
    }
    file.flush().context("Flushing model download")?;
    drop(file);
    check_cancelled(cancel)?;
    std::fs::rename(&tmp, &path).context("Installing downloaded model")?;
    Ok(path)
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn check_cancelled(cancel: &AtomicBool) -> anyhow::Result<()> {
    anyhow::ensure!(!cancel.load(Ordering::Relaxed), "CANCELLED: job cancelled");
    Ok(())
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown error".into())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_uses_shared_quality_and_confirmed_replacement() {
        for (quality, crf) in [("high", 17), ("recommended", 21), ("small", 26)] {
            let request: ExportRequest = serde_json::from_value(serde_json::json!({
                "resolution": 1080, "fps": 30, "quality": quality
            })).unwrap();
            assert_eq!(request.options().crf, crf);
            assert!(request.options().replace_existing);
        }
    }

    fn request(max_words: Option<u8>, max_chars: Option<u8>) -> CaptionRequest {
        CaptionRequest { model: "small".into(), language: "cs".into(), max_words, max_chars,
            style: TextStyle { font_family: Some("Inter".into()), font_size: 95.0, color: "#ffffff".into(),
                bold: false, stroke_width: 7.5, stroke_color: "#000000".into(), background: None, max_width: None } }
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
