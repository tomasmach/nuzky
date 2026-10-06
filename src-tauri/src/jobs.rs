//! Long-running work off the UI path: audio preparation, export and auto captions.
//! Every job reports through `job` events and can be cancelled.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use capopen_analysis::{AudioSource, CaptionGrouping, Transcript, group_words, transcribe_words};
use capopen_engine::audio::{Mixer, ensure_pcm, has_audio, us_to_samples};
use capopen_engine::edit::{CaptionSegment, EditCmd, new_id};
use capopen_engine::export::{ExportOptions, Quality, export};
use capopen_analysis::models_dir;
use capopen_engine::model::{AssetKind, CHANNELS, Clip, ClipContent, Project, TextStyle, Track};
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
    pub quality: Quality,
}

impl ExportRequest {
    fn options(&self) -> ExportOptions {
        ExportOptions { crf: self.quality.crf(), replace_existing: true, resolution: Some(self.resolution), fps: Some(self.fps), ..ExportOptions::default() }
    }
}

pub fn start_export(app: &AppHandle, out: PathBuf, request: ExportRequest) -> Result<String, String> {
    let state = app.state::<AppState>();
    let project = state.project()?;
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

#[derive(Clone)]
struct SpeechSnapshot {
    project: Project,
    path: PathBuf,
    revision: u64,
}

pub struct CachedTranscript {
    source: SpeechSnapshot,
    model: String,
    requested_language: String,
    transcript: Transcript,
}

impl CachedTranscript {
    /// Text, styling or zoom changes keep a transcript valid; only what can be heard matters.
    fn matches(&self, source: &SpeechSnapshot, model: &str, language: &str) -> bool {
        self.source.path == source.path
            && speech_signature(&self.source.project) == speech_signature(&source.project)
            && self.model == model && self.requested_language == language
    }

    fn view(&self) -> TimelineTranscript {
        TimelineTranscript {
            revision: self.source.revision,
            language: self.transcript.language.clone(),
            words: self.transcript.words.iter().map(|word| TimelineWord {
                start_us: word.start_us, end_us: word.end_us,
                text: word.text.clone(), probability: word.probability,
            }).collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineWord {
    start_us: i64,
    end_us: i64,
    text: String,
    probability: f32,
}

#[derive(Serialize)]
pub struct TimelineTranscript {
    revision: u64,
    language: String,
    words: Vec<TimelineWord>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptReady {
    job_id: String,
    transcript: TimelineTranscript,
}

#[tauri::command]
pub fn get_transcript(state: tauri::State<'_, AppState>) -> Option<TimelineTranscript> {
    let current = state.session.lock().unwrap();
    let view = current.session.state().ok()?;
    let stored = state.transcript.lock().unwrap();
    let cached = stored.as_ref()?;
    if cached.source.path != current.path || cached.source.revision != view.stamp.revision
        || cached.source.project != view.project {
        return None;
    }
    Some(cached.view())
}

#[tauri::command]
pub fn start_captions(app: AppHandle, request: CaptionRequest) -> Result<String, String> {
    start_speech(app, request.model.clone(), request.language.clone(), Some(request))
}

#[tauri::command]
pub fn start_transcript(app: AppHandle, model: String, language: String) -> Result<String, String> {
    start_speech(app, model, language, None)
}

fn start_speech(app: AppHandle, model: String, language: String, captions: Option<CaptionRequest>) -> Result<String, String> {
    if !MODELS.iter().any(|(id, ..)| *id == model) {
        return Err("Unknown speech model".into());
    }
    if language != "auto" && (language.contains('\0') || whisper_rs::get_lang_id(&language).is_none()) {
        return Err(format!("Unknown language: {language}"));
    }
    let state = app.state::<AppState>();
    let source = {
        let current = state.session.lock().unwrap();
        let view = current.session.state().map_err(crate::err)?;
        SpeechSnapshot { project: view.project, revision: view.stamp.revision, path: current.path.clone() }
    };
    if speech_clips(&source.project).next().is_none() {
        return Err("No video clip with sound on the timeline to transcribe.".into());
    }
    let kind = if captions.is_some() { "captions" } else { "transcript" };
    let id = format!("{kind}:{}", new_id());
    let cancel = register(&app, &id).ok_or("Speech recognition is already running")?;
    let (worker_app, job_id) = (app.clone(), id.clone());
    let spawn = std::thread::Builder::new().name(kind.into()).spawn(move || {
        let label = if captions.is_some() { "Auto captions" } else { "Timeline transcript" };
        let mut rep = Reporter::new(&worker_app, &job_id, kind, label.into());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_speech_job(&source, &model, &language, captions.as_ref(), &cancel, &mut rep)
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
    source: &SpeechSnapshot,
    model: &str,
    language: &str,
    captions: Option<&CaptionRequest>,
    cancel: &AtomicBool,
    rep: &mut Reporter,
) -> anyhow::Result<Option<String>> {
    let app = rep.app.clone();
    let state = app.state::<AppState>();
    let cached = state.transcript.lock().unwrap().as_ref()
        .filter(|cached| cached.matches(source, model, language))
        .map(|cached| cached.transcript.clone());
    let transcript = match cached {
        Some(transcript) => {
            rep.progress(1.0, Some("Using timeline transcript"));
            transcript
        }
        None => {
            let model_path = download_model(model, cancel, rep)?;
            let vad = download_vad(cancel, rep)?;
            transcribe_timeline(&source.project, &state.cache_dir, &model_path, &vad, language, cancel,
                |progress, phase| rep.progress(progress, Some(phase)))?
        }
    };
    check_cancelled(cancel)?;
    anyhow::ensure!(!transcript.words.is_empty(), "No speech was recognised.");
    let cached = CachedTranscript { source: source.clone(), model: model.into(), requested_language: language.into(), transcript };
    let current = state.session.lock().unwrap();
    let view = current.session.state()?;
    anyhow::ensure!(current.path == source.path,
        "Another project was opened, so the speech recognition result was not applied.");
    check_cancelled(cancel)?;
    let result = if let Some(request) = captions {
        rep.progress(1.0, Some("Grouping captions"));
        let segments = group_words(&cached.transcript.words, request.grouping());
        let count = segments.len();
        let cmd = caption_edit(&view.project, segments, request.style.clone());
        current.session.edit(vec![cmd], None, capopen_session::Expect {
            revision: Some(view.stamp.revision),
            speech_key: Some(capopen_engine::speech::speech_key(&source.project)),
        }).context("Applying captions")?;
        format!("{count} captions")
    } else {
        format!("{} words", cached.transcript.words.len())
    };
    let ready = TranscriptReady { job_id: rep.event.id.clone(), transcript: cached.view() };
    *state.transcript.lock().unwrap() = Some(cached);
    app.emit("transcript-ready", &ready).ok();
    Ok(Some(result))
}

fn caption_edit(project: &Project, segments: Vec<CaptionSegment>, style: TextStyle) -> EditCmd {
    match project.tracks.iter().find(|track| track.name == "Captions") {
        Some(track) => EditCmd::ReplaceCaptions { track_id: track.id.clone(), segments, style },
        None => EditCmd::AddCaptions { segments, style },
    }
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
            anyhow::bail!("cancelled");
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

/// Mixes the sound of video tracks (music tracks would confuse recognition) and
/// downsamples it to 16 kHz mono for Whisper.
/// Speech is the sound of video files (the camera), wherever the clip sits, including sound
/// detached onto an audio track. Separate audio files are treated as music and left out.
fn is_speech(project: &Project, track: &Track, clip: &Clip) -> bool {
    let ClipContent::Media { asset_id, volume, .. } = &clip.content else { return false };
    !track.muted && *volume > 0.0 && project.asset(asset_id).is_some_and(|a| a.kind == AssetKind::Video && has_audio(a))
}

fn speech_clips(project: &Project) -> impl Iterator<Item = &Clip> {
    project.tracks.iter().flat_map(move |t| t.clips.iter().filter(move |c| is_speech(project, t, c)))
}

/// The project reduced to its speech clips, for mixing and for comparing transcripts.
fn speech_project(project: &Project) -> Project {
    let mut speech = project.clone();
    for track in &mut speech.tracks {
        let original = project.tracks.iter().find(|t| t.id == track.id).cloned();
        track.clips.retain(|c| original.as_ref().is_some_and(|t| is_speech(project, t, c)));
    }
    speech
}

/// Everything about the speech clips that changes what is heard and when.
fn speech_signature(project: &Project) -> Vec<(String, i64, i64, i64, u32, u32, i64, i64, Option<i64>)> {
    speech_clips(project)
        .filter_map(|c| match &c.content {
            ClipContent::Media { asset_id, source_in_us, volume, speed, fade_in_us, fade_out_us, .. } => Some((
                asset_id.clone(),
                c.start_us,
                c.duration_us,
                *source_in_us,
                speed.to_bits(),
                volume.to_bits(),
                *fade_in_us,
                *fade_out_us,
                c.transition_in.map(|t| t.duration_us),
            )),
            ClipContent::Text { .. } => None,
        })
        .collect()
}

fn speech_audio(project: &Project, cache: &Path, cancel: &AtomicBool) -> anyhow::Result<Vec<f32>> {
    for asset in project.assets.iter().filter(|a| has_audio(a)) {
        check_cancelled(cancel)?;
        ensure_pcm(cache, asset, |_| {}).with_context(|| format!("Preparing audio for {}", asset.name))?;
    }
    let speech = speech_project(project);
    let total = us_to_samples(project.duration_us());
    let mut mixer = Mixer::new(cache.to_path_buf());
    let mut out = Vec::with_capacity(total as usize / 3 + 1);
    let chunk = 48_000usize;
    let mut buf = vec![0f32; chunk * CHANNELS];
    let mut pos = 0i64;
    while pos < total {
        check_cancelled(cancel)?;
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

fn transcribe_timeline(
    project: &Project,
    cache: &Path,
    model: &Path,
    vad: &Path,
    language: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32, &str),
) -> anyhow::Result<Transcript> {
    check_cancelled(cancel)?;
    progress(0.0, "Preparing audio");
    let audio = speech_audio(project, cache, cancel)?;
    check_cancelled(cancel)?;
    progress(0.0, "Recognising speech");
    // The analysis API has no abort callback; cancelled results must never be applied.
    let transcript = transcribe_words(AudioSource::TimelineAudio(&audio), model, vad, language)
        .context("Transcribing timeline audio")?;
    check_cancelled(cancel)?;
    progress(1.0, "Speech recognised");
    Ok(transcript)
}

fn check_cancelled(cancel: &AtomicBool) -> anyhow::Result<()> {
    anyhow::ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
    Ok(())
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown error".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    use capopen_engine::model::{Asset, TrackKind};
    use capopen_analysis::Word;
    use capopen_engine::Editor;

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

    fn transcript() -> Transcript {
        Transcript { language: "cs".into(), segments: Vec::new(), words: vec![
            Word { start_us: 2_000_000, end_us: 2_400_000, text: "Ahoj".into(), probability: 0.9 },
            Word { start_us: 2_400_000, end_us: 2_800_000, text: "světe.".into(), probability: 0.8 },
        ] }
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

    #[test]
    fn transcript_reuse_follows_what_is_heard_not_the_revision() {
        use capopen_engine::model::{Asset, Transform};
        let mut project = Project::new("Test");
        project.assets.push(Asset { id: "cam".into(), name: "cam".into(), path: String::new(), kind: AssetKind::Video,
            duration_us: 10_000_000, width: 1080, height: 1920, fps: 30.0, has_audio: true, rotation: 0 });
        project.apply(EditCmd::AddClip { asset_id: "cam".into(), start_us: None, track_id: None }).unwrap();
        let source = SpeechSnapshot { project, path: "test.json".into(), revision: 3 };
        let cached = CachedTranscript { source: source.clone(), model: "small".into(), requested_language: "cs".into(), transcript: transcript() };
        assert!(cached.matches(&source, "small", "cs"));
        assert!(!cached.matches(&source, "base", "cs"));
        assert!(!cached.matches(&source, "small", "auto"));

        // Captions, zoom and a new revision do not change what is heard.
        let mut changed = source.clone();
        changed.revision += 5;
        changed.project.apply(EditCmd::AddCaptions { segments: vec![CaptionSegment { start_us: 0, end_us: 1_000_000, text: "Ahoj".into() }],
            style: TextStyle { font_family: None, font_size: 90.0, color: "#fff".into(), bold: false, stroke_width: 7.0, stroke_color: "#000".into(), background: None, max_width: None } }).unwrap();
        let clip = changed.project.tracks[0].clips[0].id.clone();
        changed.project.apply(EditCmd::UpdateClip { clip_id: clip.clone(), transform: Some(Transform { scale: 1.1, ..Transform::default() }),
            volume: None, text: None, style: None, speed: None, adjust: None, fade_in_us: None, fade_out_us: None }).unwrap();
        assert!(cached.matches(&changed, "small", "cs"));

        // Cutting the speech or muting it does.
        let mut cut = changed.clone();
        cut.project.apply(EditCmd::RippleDeleteRanges { ranges: vec![capopen_engine::edit::TimeRange { start_us: 1_000_000, end_us: 2_000_000 }], keep_track_ids: Some(vec![]) }).unwrap();
        assert!(!cached.matches(&cut, "small", "cs"));
        let mut muted = changed.clone();
        muted.project.tracks[0].muted = true;
        assert!(!cached.matches(&muted, "small", "cs"));
        let mut moved = changed;
        moved.path = "other.json".into();
        assert!(!cached.matches(&moved, "small", "cs"));

        let view = serde_json::to_value(cached.view()).unwrap();
        assert_eq!(view["revision"], 3);
        assert_eq!(view["words"][0]["startUs"], 2_000_000);
        assert_eq!(view["words"][0]["text"], "Ahoj");
    }

    #[test]
    fn captions_keep_timeline_times_after_edits_and_replace_previous_track() {
        let mut editor = Editor::new(Project::new("Test"));
        let request = request(Some(3), Some(15));
        let segments = group_words(&transcript().words, request.grouping());
        editor.apply(EditCmd::RenameProject { name: "Edited during recognition".into() }, None).unwrap();
        let cmd = caption_edit(&editor.project, segments.clone(), request.style.clone());
        assert!(matches!(cmd, EditCmd::AddCaptions { .. }));
        editor.apply(cmd, None).unwrap();
        let cmd = caption_edit(&editor.project, segments, request.style);
        assert!(matches!(cmd, EditCmd::ReplaceCaptions { .. }));
        editor.apply(cmd, None).unwrap();
        let tracks: Vec<_> = editor.project.tracks.iter().filter(|t| t.name == "Captions").collect();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].clips.len(), 1);
        assert_eq!(tracks[0].clips[0].start_us, 2_000_000);
        assert_eq!(editor.revision, 3);
    }

    #[test]
    fn cancelled_transcription_stops_before_reading_media_or_models() {
        let cancel = AtomicBool::new(true);
        let absent = Path::new("does-not-exist");
        let result = transcribe_timeline(&Project::new("Test"), absent, absent, absent, "cs", &cancel, |_, _| panic!("Must stop first"));
        assert_eq!(result.unwrap_err().to_string(), "cancelled");
    }

    #[test]
    #[ignore = "requires tmp-test/talk.mp4 and downloaded small + Silero models"]
    fn real_timeline_transcription_groups_talk_into_reel_captions() -> anyhow::Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tmp-test");
        let out = root.join("fonts");
        std::fs::create_dir_all(&out)?;
        let asset = capopen_engine::media::probe(&root.join("talk.mp4"), "talk".into())?;
        // The same sound imported as an audio file is music, not speech.
        let song = Asset { id: "song".into(), kind: AssetKind::Audio, ..asset.clone() };
        let mut project = Project::new("Speech test");
        project.apply(EditCmd::AddAssets { assets: vec![asset, song] })?;
        project.apply(EditCmd::AddClip { asset_id: "talk".into(), start_us: None, track_id: None })?;
        // A leading gap proves the recogniser returns timeline rather than asset time.
        project.tracks[0].clips[0].start_us = 2_000_000;
        let cancel = AtomicBool::new(false);
        let cache = out.join("cache");
        let mixed = speech_audio(&project, &cache, &cancel)?;
        assert!(mixed[..32_000].iter().all(|sample| *sample == 0.0));
        let mut music = project.tracks[0].clone();
        music.id = "music".into();
        music.kind = TrackKind::Audio;
        if let ClipContent::Media { asset_id, .. } = &mut music.clips[0].content {
            *asset_id = "song".into();
        }
        project.tracks.push(music);
        assert_eq!(speech_audio(&project, &cache, &cancel)?, mixed);
        let models = root.join("xdg/data/capopen/models");
        let transcript = transcribe_timeline(&project, &cache, &models.join("ggml-small.bin"),
            &models.join(VAD_MODEL), "cs", &cancel, |p, phase| eprintln!("{phase}: {p:.0}"))?;
        std::fs::write(out.join("talk-transcript.json"), serde_json::to_vec_pretty(&transcript)?)?;
        assert!(!transcript.words.is_empty());
        // VAD padding and estimated token times can precede the first audible sample.
        assert!((1_800_000..2_500_000).contains(&transcript.words[0].start_us));
        assert!(transcript.words.iter().all(|word| word.end_us >= word.start_us));
        assert!(transcript.words.last().unwrap().end_us > 11_000_000);
        let captions = group_words(&transcript.words, request(Some(3), Some(15)).grouping());
        for caption in &captions {
            assert!((1..=3).contains(&caption.text.split_whitespace().count()));
            assert!(caption.text.chars().count() <= 15, "{}", caption.text);
            assert!(caption.end_us > caption.start_us);
            println!("{:.3}–{:.3}  {}", caption.start_us as f64 / 1e6, caption.end_us as f64 / 1e6, caption.text);
        }
        std::fs::write(out.join("talk-captions.json"), serde_json::to_vec_pretty(&captions)?)?;
        Ok(())
    }
}
