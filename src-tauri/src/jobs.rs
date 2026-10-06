//! Long-running work off the UI path: audio preparation, export and auto captions.
//! Every job reports through `job` events and can be cancelled.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use capopen_engine::audio::{Mixer, ensure_pcm, has_audio, us_to_samples};
use capopen_engine::edit::{CaptionSegment, EditCmd, new_id};
use capopen_engine::export::{ExportOptions, export};
use capopen_engine::model::{CHANNELS, Project, TextStyle, TrackKind};
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

/// Registers a cancellable job. Export and captions run one at a time, so for them any
/// running job of the same kind (the prefix before ':') blocks a new one.
fn register(app: &AppHandle, id: &str) -> Option<Arc<AtomicBool>> {
    let state = app.state::<AppState>();
    let mut jobs = state.jobs.lock().unwrap();
    let kind = id.split(':').next().unwrap_or(id);
    let exclusive = matches!(kind, "export" | "captions");
    if jobs.contains_key(id) || (exclusive && jobs.keys().any(|k| k.split(':').next() == Some(kind))) {
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
        let path = capopen_engine::media::pcm_path(&state.cache_dir, asset);
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
    pub quality: String,
}

impl ExportRequest {
    fn options(&self) -> ExportOptions {
        let crf = match self.quality.as_str() {
            "high" => 17,
            "small" => 26,
            _ => 21,
        };
        ExportOptions { crf, resolution: Some(self.resolution), fps: Some(self.fps), ..ExportOptions::default() }
    }
}

pub fn start_export(app: &AppHandle, out: PathBuf, request: ExportRequest) -> Result<String, String> {
    let state = app.state::<AppState>();
    let project = state.editor.lock().unwrap().project.clone();
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
pub struct CaptionModel {
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

fn models_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("capopen").join("models")
}

fn model_path(id: &str) -> PathBuf {
    models_dir().join(format!("ggml-{id}.bin"))
}

#[tauri::command]
pub fn caption_models() -> Vec<CaptionModel> {
    MODELS
        .iter()
        .map(|(id, label, size)| CaptionModel { id, label, size_mb: *size, downloaded: model_path(id).exists() })
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
    /// Most characters per caption; `None` means no limit.
    #[serde(default)]
    pub max_chars: Option<u8>,
}

#[tauri::command]
pub fn start_captions(app: AppHandle, request: CaptionRequest) -> Result<String, String> {
    if !MODELS.iter().any(|(id, ..)| *id == request.model) {
        return Err("Unknown caption model".into());
    }
    let state = app.state::<AppState>();
    let project = state.editor.lock().unwrap().project.clone();
    let project_path = state.project_path.lock().unwrap().clone();
    let has_speech = project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video && !t.muted)
        .flat_map(|t| &t.clips)
        .any(|c| matches!(&c.content, capopen_engine::model::ClipContent::Media { asset_id, .. } if project.asset(asset_id).is_some_and(has_audio)));
    if !has_speech {
        return Err("No video clip with sound on the timeline to caption.".into());
    }
    let id = format!("captions:{}", new_id());
    let cancel = register(&app, &id).ok_or("Captions are already being generated")?;
    let (app2, cache, job_id) = (app.clone(), state.cache_dir.clone(), id.clone());
    std::thread::Builder::new()
        .name("captions".into())
        .spawn(move || {
            let mut rep = Reporter::new(&app2, &job_id, "captions", "Auto captions".into());
            // A panic inside whisper.cpp bindings must still end the job, not leave it running forever.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_captions(&project, &cache, &request, &cancel, &mut rep)))
                .unwrap_or_else(|p| Err(anyhow::anyhow!("Speech recognition crashed: {}", panic_text(&p))));
            let cancelled = cancel.load(Ordering::Relaxed);
            let result = result.and_then(|segments| {
                let count = segments.len();
                if count == 0 {
                    anyhow::bail!("No speech was recognised.");
                }
                let state = app2.state::<AppState>();
                // The user may have opened another project while recognition ran.
                if *state.project_path.lock().unwrap() != project_path {
                    anyhow::bail!("Another project was opened, so the captions were not added.");
                }
                // Regenerating replaces the previous captions instead of stacking another track.
                let existing = state.editor.lock().unwrap().project.tracks.iter().find(|t| t.name == "Captions").map(|t| t.id.clone());
                let style = request.style.clone();
                let cmd = match existing {
                    Some(track_id) => EditCmd::ReplaceCaptions { track_id, segments, style },
                    None => EditCmd::AddCaptions { segments, style },
                };
                let snap = state.apply(cmd, None).map_err(anyhow::Error::msg)?;
                app2.emit("project-changed", &snap).ok();
                Ok(Some(format!("{count} captions")))
            });
            rep.finish(result, cancelled);
            unregister(&app2, &job_id);
        })
        .map_err(|e| e.to_string())?;
    Ok(id)
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
    let path = path.to_path_buf();
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(models_dir())?;
    let response = ureq::get(url).call()?;
    let total: u64 = response.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse().ok()).unwrap_or(0);
    let mut reader = response.into_body().into_reader();
    let tmp = path.with_extension("part");
    let mut file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    let mut buf = vec![0u8; 1 << 16];
    let mut done: u64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            std::fs::remove_file(&tmp).ok();
            anyhow::bail!("cancelled");
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if total > 0 {
            rep.progress(done as f32 / total as f32, Some(phase));
        }
    }
    file.flush()?;
    drop(file);
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Mixes the sound of video tracks (music tracks would confuse recognition) and
/// downsamples it to 16 kHz mono for Whisper.
fn speech_audio(project: &Project, cache: &Path) -> anyhow::Result<Vec<f32>> {
    for asset in project.assets.iter().filter(|a| has_audio(a)) {
        ensure_pcm(cache, asset, |_| {})?;
    }
    let mut speech = project.clone();
    for t in &mut speech.tracks {
        if t.kind == TrackKind::Audio {
            t.muted = true;
        }
    }
    let total = us_to_samples(project.duration_us());
    let mut mixer = Mixer::new(cache.to_path_buf());
    let mut out = Vec::with_capacity(total as usize / 3 + 1);
    let chunk = 48_000usize;
    let mut buf = vec![0f32; chunk * CHANNELS];
    let mut pos = 0i64;
    while pos < total {
        mixer.mix(&speech, pos, &mut buf);
        let frames = (chunk as i64).min(total - pos) as usize;
        for tri in buf[..frames * CHANNELS].chunks(3 * CHANNELS) {
            let sum: f32 = tri.iter().sum();
            out.push(sum / tri.len() as f32);
        }
        pos += chunk as i64;
    }
    Ok(out)
}

fn run_captions(
    project: &Project,
    cache: &Path,
    request: &CaptionRequest,
    cancel: &Arc<AtomicBool>,
    rep: &mut Reporter,
) -> anyhow::Result<Vec<CaptionSegment>> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    let model = download_model(&request.model, cancel, rep)?;
    let vad = download_vad(cancel, rep)?.to_string_lossy().into_owned();
    rep.progress(0.0, Some("Preparing audio"));
    let audio = speech_audio(project, cache)?;
    rep.progress(0.0, Some("Loading speech model"));
    let ctx = WhisperContext::new_with_params(&model, WhisperContextParameters::default())
        .map_err(|e| anyhow::anyhow!("Cannot load the speech model: {e}"))?;
    let mut state = ctx.create_state().map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16) as i32;
    params.set_n_threads(threads);
    let language = if request.language == "auto" { None } else { Some(request.language.as_str()) };
    params.set_language(language);
    params.set_token_timestamps(true);
    params.set_max_len(36);
    params.set_split_on_word(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_suppress_nst(true);
    let (app, mut event) = (rep.app.clone(), rep.event.clone());
    params.set_progress_callback_safe(move |p: i32| {
        event.progress = p as f32 / 100.0;
        event.phase = Some("Recognising speech".into());
        app.emit("job", &event).ok();
    });
    let abort = cancel.clone();
    params.set_abort_callback_safe(move || abort.load(Ordering::Relaxed));
    rep.progress(0.0, Some("Recognising speech"));

    // whisper.cpp's built-in VAD loses the original timestamps, so detect speech here,
    // transcribe only the speech and map times back ourselves.
    let regions = speech_regions(&vad, &audio)?;
    log::debug!("speech regions (s): {:?}", regions.iter().map(|(a, b)| (*a as f32 / 16000.0, *b as f32 / 16000.0)).collect::<Vec<_>>());
    if regions.is_empty() {
        return Ok(Vec::new());
    }
    let (speech, table) = compact(&audio, &regions);
    state.full(params, &speech).map_err(|e| anyhow::anyhow!("Speech recognition failed: {e}"))?;
    if cancel.load(Ordering::Relaxed) {
        anyhow::bail!("cancelled");
    }
    let mut segments = Vec::new();
    for seg in state.as_iter() {
        let text = seg.to_str_lossy().map(|s| s.trim().to_string()).unwrap_or_default();
        log::debug!("segment {}..{} cs, no_speech {:.2}: {text}", seg.start_timestamp(), seg.end_timestamp(), seg.no_speech_probability());
        if text.is_empty() || is_annotation(&text) || seg.no_speech_probability() > 0.6 {
            continue;
        }
        let at = |cs: i64| to_original(&table, (cs.max(0) as usize) * CS) as i64 * 1_000_000 / SPEECH_RATE as i64;
        segments.push(CaptionSegment { start_us: at(seg.start_timestamp()), end_us: at(seg.end_timestamp()), text });
    }
    Ok(segments)
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown error".into())
}

const SPEECH_RATE: usize = 16_000;
/// Samples per Whisper centisecond.
const CS: usize = SPEECH_RATE / 100;
const GAP: usize = SPEECH_RATE * 2 / 5;

/// Speech regions as sample ranges, padded and merged when they nearly touch.
fn speech_regions(vad_model: &str, audio: &[f32]) -> anyhow::Result<Vec<(usize, usize)>> {
    use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};
    let mut ctx = WhisperVadContext::new(vad_model, WhisperVadContextParams::new())
        .map_err(|e| anyhow::anyhow!("Cannot load the voice detector: {e}"))?;
    let mut params = WhisperVadParams::new();
    params.set_speech_pad(200);
    params.set_min_silence_duration(300);
    let segments = ctx.segments_from_samples(params, audio).map_err(|e| anyhow::anyhow!("Voice detection failed: {e}"))?;
    let mut out: Vec<(usize, usize)> = Vec::new();
    for seg in segments {
        let start = ((seg.start.max(0.0) as usize) * CS).min(audio.len());
        let end = ((seg.end.max(0.0) as usize) * CS).min(audio.len());
        if end <= start {
            continue;
        }
        match out.last_mut() {
            Some(last) if start <= last.1 + GAP => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    Ok(out)
}

/// Joins the speech regions with short silences. The table maps each region's offset in
/// the joined audio (`compact_start`) to its timeline offset (`original_start`, `len`).
fn compact(audio: &[f32], regions: &[(usize, usize)]) -> (Vec<f32>, Vec<(usize, usize, usize)>) {
    let mut joined = Vec::new();
    let mut table = Vec::new();
    for &(start, end) in regions {
        if !joined.is_empty() {
            joined.extend(std::iter::repeat_n(0.0, GAP));
        }
        table.push((joined.len(), start, end - start));
        joined.extend_from_slice(&audio[start..end]);
    }
    (joined, table)
}

fn to_original(table: &[(usize, usize, usize)], t: usize) -> usize {
    let Some(&(cs, os, len)) = table.iter().rev().find(|(cs, ..)| *cs <= t).or(table.first()) else { return t };
    os + t.saturating_sub(cs).min(len)
}

/// Whisper marks non-speech as "[Music]", "(laughs)", "*applause*" or "♪".
fn is_annotation(text: &str) -> bool {
    let t = text.trim_matches(|c: char| c.is_whitespace() || c == '.');
    let wrapped = |a: char, b: char| t.starts_with(a) && t.ends_with(b);
    wrapped('[', ']') || wrapped('(', ')') || wrapped('*', '*') || t.chars().all(|c| c == '♪' || c.is_whitespace())
}

#[cfg(test)]
mod tests {
    #[test]
    fn filters_non_speech_annotations() {
        for t in ["[Music]", "(Zvukáží)", "*potlesk*", "♪ ♪", " (smích). "] {
            assert!(super::is_annotation(t), "{t}");
        }
        assert!(!super::is_annotation("Ahoj (jak se máš)"));
        assert!(!super::is_annotation("Dnes si ukážeme střih."));
    }

    #[test]
    fn maps_compacted_speech_back_to_the_timeline() {
        let audio = vec![0.5f32; 16_000 * 10];
        // Speech at 2–3 s and 7–8 s.
        let (joined, table) = super::compact(&audio, &[(32_000, 48_000), (112_000, 128_000)]);
        assert_eq!(joined.len(), 16_000 + super::GAP + 16_000);
        assert_eq!(super::to_original(&table, 0), 32_000);
        assert_eq!(super::to_original(&table, 8_000), 40_000);
        // Inside the inserted gap: clamps to the end of the first region.
        assert_eq!(super::to_original(&table, 16_000 + 100), 48_000);
        assert_eq!(super::to_original(&table, 16_000 + super::GAP + 4_000), 116_000);
    }
}
