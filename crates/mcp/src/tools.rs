use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, prelude::BASE64_STANDARD};
use nuzky_analysis::{SceneParams, SilenceParams};
use nuzky_engine::{
    Project,
    edit::{EditCmd, new_id},
    export::{ExportOptions, ExportPhase, Quality, check_options, export},
    media::probe,
};
use nuzky_session::{Expect, Mode, ProjectSession, SessionState, host::Host, jobs::check_cancel};
use rmcp::model::{CallToolResult, ContentBlock};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{activity, media, params::*, transcript};

const PREVIEW_CHARS: usize = 400;
/// How long activity waits for its job before handing it over to poll.
const ANSWER_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

struct PreparedImport {
    paths: Vec<String>,
    assets: Vec<nuzky_engine::model::Asset>,
    expect: Expect,
}

struct PreparedTranscriptEdit {
    arguments: Value,
    edits: Vec<EditCmd>,
    expect: Expect,
    response: Value,
}

#[derive(Clone, Copy, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    #[serde(rename = "read")]
    ReadOnly,
    Write,
}

pub struct Client {
    pub id: String,
    pub access: Access,
}

pub struct Backend {
    pub host: Arc<Host>,
    client: Client,
    runs: Mutex<HashSet<String>>,
    closed: AtomicBool,
    project_dir: PathBuf,
    project_path: PathBuf,
    export_queue: Arc<Mutex<()>>,
    transcript_requests: Mutex<HashMap<(String, String), PreparedTranscriptEdit>>,
    import_requests: Mutex<HashMap<(String, String), PreparedImport>>,
}

impl Backend {
    pub fn open(project: &Path, allow_write: bool, cache: PathBuf) -> Result<Self> {
        let session = ProjectSession::open(project, if allow_write { Mode::Write } else { Mode::ReadOnly }, None)?;
        Self::shared(
            Arc::new(Host::new(session, cache)?),
            project,
            Client { id: new_id(), access: if allow_write { Access::Write } else { Access::ReadOnly } },
        )
    }

    pub fn shared(host: Arc<Host>, project: &Path, client: Client) -> Result<Self> {
        let project_path = std::fs::canonicalize(project).context("PROJECT_MISSING: resolving project")?;
        let project_dir = project_path.parent().context("INVALID_PROJECT: no directory")?.to_path_buf();
        std::fs::create_dir_all(&host.cache_dir).context("CACHE_UNAVAILABLE: creating media cache")?;
        Ok(Self {
            host,
            client,
            runs: Mutex::default(),
            closed: AtomicBool::new(false),
            project_dir,
            project_path,
            export_queue: Arc::default(),
            transcript_requests: Mutex::default(),
            import_requests: Mutex::default(),
        })
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.host.jobs.cancel_owner(&self.client.id);
    }

    pub fn disconnect(&self) -> Result<()> {
        self.close();
        let runs = self.runs.lock().unwrap();
        if let Some(run) = self.host.session.state()?.open_run
            && runs.contains(&run.run_id)
        {
            self.host.session.end_run(&run.run_id, nuzky_session::EndAction::Keep)?;
        }
        Ok(())
    }

    pub fn call(&self, name: &str, arguments: Value) -> Result<CallToolResult> {
        self.call_inner(name, arguments).map(crate::limits::tool_result)
    }

    fn call_inner(&self, name: &str, arguments: Value) -> Result<CallToolResult> {
        let rules = crate::rules::find(name);
        let read = rules.is_some_and(|rules| rules.reads(&arguments));
        let mut runs = (!read).then(|| self.runs.lock().unwrap());
        ensure!(!self.closed.load(Ordering::Acquire), "CLIENT_CLOSED: client disconnected");
        // Reads never hold this client's run lock while the session is locked.
        let owned = match &runs {
            Some(runs) => self.host.session.touch_run(|run| runs.contains(run)),
            None => {
                let runs = self.runs.lock().unwrap().clone();
                self.host.session.touch_run(|run| runs.contains(run))
            }
        };
        owned?;
        let mut state = self.host.session.state()?;
        // Reads hold no run lock, so they look up whose run it is for the moment of this check.
        let mine = |run: &str| match &runs {
            Some(runs) => runs.contains(run),
            None => self.runs.lock().unwrap().contains(run),
        };
        if rules.is_some_and(|rules| rules.run_job) && state.open_run.as_ref().is_some_and(|run| !mine(&run.run_id)) {
            state.open_run = None;
        }
        ensure!(read || self.client.access == Access::Write, "READ_ONLY: this client cannot mutate");
        if let Some(run) = arguments.get("run_id").and_then(Value::as_str) {
            ensure!(mine(run), "INVALID_RUN: run belongs to another client");
            if name != "undo_run" {
                self.host.session.check_run(run)?;
            }
        }
        state.read_only |= self.client.access == Access::ReadOnly;
        if name == "inspect_frames" {
            return self.inspect(parse(arguments)?, &state);
        }
        let mut value = self.dispatch(name, arguments, &state)?;
        if name == "begin_run"
            && let Some(id) = value["run_id"].as_str()
            && let Some(runs) = &mut runs
        {
            runs.insert(id.to_owned());
        }
        let object = value.as_object_mut().context("Tool produced a non-object result")?;
        object.entry("revision").or_insert(json!(state.stamp.revision));
        object.entry("session_epoch").or_insert(json!(state.stamp.session_epoch));
        Ok(CallToolResult::structured(value))
    }

    fn dispatch(&self, name: &str, arguments: Value, state: &SessionState) -> Result<Value> {
        match name {
            "get_state" => self.get_state(parse(arguments)?, state),
            "begin_run" => {
                let a: Begin = parse(arguments)?;
                Ok(serde_json::to_value(self.host.session.begin_run(a.label)?)?)
            }
            "resolve_recovery" => {
                let a: Recovery = parse(arguments)?;
                let action = match a.action {
                    RecoveryAction::Keep => nuzky_session::RecoveryAction::Keep,
                    RecoveryAction::Restore => nuzky_session::RecoveryAction::Restore,
                };
                Ok(serde_json::to_value(self.host.session.resolve_recovery(action)?)?)
            }
            "apply_edits" => {
                let a: Apply = parse(arguments)?;
                check_new_assets(&a.edits)?;
                Ok(serde_json::to_value(self.host.session.apply_edits(
                    &a.run_id,
                    &a.request_id,
                    a.edits,
                    Expect { revision: a.expected_revision, speech_layout_key: a.expected_speech_layout_key },
                )?)?)
            }
            "end_run" => {
                let a: End = parse(arguments)?;
                let action = match a.action {
                    EndAction::Keep => nuzky_session::EndAction::Keep,
                    EndAction::Discard => nuzky_session::EndAction::Discard,
                };
                Ok(serde_json::to_value(self.host.session.end_run(&a.run_id, action)?)?)
            }
            "undo_run" => {
                let a: Undo = parse(arguments)?;
                Ok(serde_json::to_value(self.host.session.undo_run(&a.run_id)?)?)
            }
            "suggest_options" => {
                let a: SuggestOptions = parse(arguments)?;
                ensure!((2..=6).contains(&a.options.len()), "INVALID_ARGUMENTS: offer 2 to 6 options");
                let short = |text: &Option<String>| text.as_ref().is_none_or(|t| t.chars().count() <= 300);
                ensure!(short(&a.question), "INVALID_ARGUMENTS: keep the question under 300 characters");
                ensure!(
                    a.options.iter().all(|c| short(&c.detail)),
                    "INVALID_ARGUMENTS: keep each detail under 300 characters"
                );
                let mut seen = HashSet::new();
                for choice in &a.options {
                    let label = choice.label.trim();
                    ensure!(
                        !label.is_empty() && label.chars().count() <= 80,
                        "INVALID_ARGUMENTS: each label needs 1 to 80 characters"
                    );
                    ensure!(seen.insert(label.to_lowercase()), "INVALID_ARGUMENTS: labels must differ");
                }
                Ok(
                    json!({"shown": a.options.len(), "next": "End your turn now. The label the user picks arrives as their next message."}),
                )
            }
            "import_media" => self.import(parse(arguments)?, state),
            "activity" => self.activity(parse(arguments)?, state),
            "analyze" => self.analyze(parse(arguments)?, state),
            "segment_subject" => self.segment_subject(parse(arguments)?, state),
            "transcribe" => self.transcribe(parse(arguments)?, state),
            "get_transcript" => self.get_transcript(parse(arguments)?, state),
            "edit_transcript" => self.edit_transcript(parse(arguments)?, state),
            "correct_words" => self.correct_words(parse(arguments)?, state),
            "job" => {
                let a: Job = parse(arguments)?;
                self.host.jobs.get_for(Some(&self.client.id), &a.job_id, matches!(a.action, JobAction::Cancel))
            }
            "build_captions" => self.captions(parse(arguments)?, state),
            "apply_zooms" => self.apply_zooms(parse(arguments)?, state),
            "export_video" => self.export(parse(arguments)?, state),
            _ => anyhow::bail!("UNKNOWN_TOOL: {name}"),
        }
    }

    fn inspect(&self, args: Inspect, state: &SessionState) -> Result<CallToolResult> {
        let project = self.media_project(&state.project);
        let mut info = json!({"revision": state.stamp.revision, "session_epoch": state.stamp.session_epoch,
            "labels": "seconds.microseconds"});
        let times = match args.sample {
            None => {
                ensure!(
                    args.range_us.is_none()
                        && args.cursor.is_none()
                        && args.max_frames.is_none()
                        && args.min_change.is_none(),
                    "INVALID_ARGUMENTS: range_us, cursor, max_frames and min_change go with sample \"changes\""
                );
                args.times_us.context("INVALID_ARGUMENTS: give times_us, or sample \"changes\"")?
            }
            Some(Sample::Changes) => {
                ensure!(args.times_us.is_none(), "INVALID_ARGUMENTS: give times_us or sample, not both");
                let max = args.max_frames.unwrap_or(media::MAX_FRAMES);
                ensure!((1..=media::MAX_FRAMES).contains(&max), "INVALID_ARGUMENTS: max_frames must be 1..=16");
                let cursor = match args.cursor {
                    Some(cursor) => {
                        ensure!(
                            args.range_us.is_none() && args.min_change.is_none(),
                            "INVALID_ARGUMENTS: the cursor carries range_us and min_change; leave them out"
                        );
                        activity::Cursor::decode(&cursor, &state.stamp.session_epoch, state.stamp.revision)?
                    }
                    None => {
                        let min_change = args.min_change.unwrap_or(activity::DEFAULT_MIN_CHANGE);
                        ensure!(min_change < 64, "INVALID_ARGUMENTS: min_change must be 0..=63");
                        let range = activity::timeline_range(&project, args.range_us)?;
                        activity::Cursor::new(&state.stamp.session_epoch, state.stamp.revision, range, min_change)
                    }
                };
                media::check_media(&project)?;
                let (range, every) = (cursor.range(), cursor.every());
                let found = activity::changes(&project, cursor, max, &|| self.closed.load(Ordering::Acquire))?;
                info["sample"] = json!("changes");
                info["range_us"] = json!([range.0, range.1]);
                info["candidate_every_us"] = json!(every);
                info["skipped"] = json!(found.skipped);
                info["next"] = json!(found.next);
                if found.times.is_empty() {
                    info["times_us"] = json!([]);
                    return Ok(CallToolResult::success(vec![ContentBlock::text(info.to_string())]));
                }
                found.times
            }
        };
        let bytes = media::contact_sheet(&project, &times, args.width, args.safe_area)?;
        info["times_us"] = json!(times);
        Ok(CallToolResult::success(vec![
            ContentBlock::text(info.to_string()),
            ContentBlock::image(BASE64_STANDARD.encode(bytes), "image/png"),
        ]))
    }

    /// Renders and mixes the range as a job, so a long range reports progress and can be cancelled,
    /// and answers in this call when the job finishes within a few seconds.
    fn activity(&self, args: Activity, state: &SessionState) -> Result<Value> {
        let points = args.points.unwrap_or(60);
        ensure!((8..=200).contains(&points), "INVALID_ARGUMENTS: points must be 8..=200");
        let project = self.media_project(&state.project);
        let range = activity::timeline_range(&project, args.range_us)?;
        media::check_media(&project)?;
        let cache = self.host.cache_dir.clone();
        // Everything the mix can play, music included, also sound that a transition carries over
        // the range's edge; the mix leaves out files whose sound is not prepared.
        let heard: HashSet<&str> = project
            .tracks
            .iter()
            .filter(|track| !track.muted && track.kind != nuzky_engine::model::TrackKind::Text)
            .flat_map(|track| &track.clips)
            .filter_map(|clip| match &clip.content {
                nuzky_engine::model::ClipContent::Media { asset_id, volume, .. } if *volume > 0.0 => {
                    Some(asset_id.as_str())
                }
                _ => None,
            })
            .collect();
        let unprepared: Vec<_> = project
            .assets
            .iter()
            .filter(|a| heard.contains(a.id.as_str()) && nuzky_engine::audio::has_audio(a))
            .filter(|a| !nuzky_engine::audio::pcm_path(&cache, a).exists())
            .cloned()
            .collect();
        let job = self.host.start_job(
            &self.client.id,
            state.open_run.as_ref().map(|run| run.run_id.as_str()),
            "activity",
            state.stamp.clone(),
            move |cancel, progress| {
                for (i, asset) in unprepared.iter().enumerate() {
                    progress.set("preparing_audio", Some(i as f32 / unprepared.len() as f32));
                    nuzky_engine::audio::ensure_pcm(&cache, asset, |_| check_cancel(&cancel))?;
                }
                let cancelled = || cancel.load(Ordering::Relaxed);
                activity::activity(&project, &cache, range, points, &cancelled, &|done| {
                    progress.set("looking", Some(done))
                })
            },
        )?;
        let id = job["job_id"].as_str().context("Job has no id")?.to_owned();
        let deadline = std::time::Instant::now() + ANSWER_WAIT;
        loop {
            let job = self.host.jobs.get(&id, false)?;
            match job["status"].as_str() {
                Some("done") => return Ok(job["result"].clone()),
                Some("failed") => anyhow::bail!("{}", job["error"].as_str().unwrap_or("JOB_FAILED")),
                Some("running") if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                _ => return Ok(job),
            }
        }
    }

    fn get_state(&self, args: State, state: &SessionState) -> Result<Value> {
        if let Some(range) = args.range {
            ensure!(
                range.start_us >= 0 && range.end_us > range.start_us,
                "INVALID_RANGE: invalid half-open timeline range"
            );
        }
        let mut project = state.project.clone();
        for track in &mut project.tracks {
            track.clips.retain(|clip| {
                args.range.is_none_or(|r| clip.start_us < r.end_us && clip.end_us() > r.start_us)
                    && args.clip_ids.as_ref().is_none_or(|ids| ids.contains(&clip.id))
            });
        }
        let mut value = serde_json::to_value(state)?;
        let object = value.as_object_mut().context("Invalid state")?;
        object.remove("project");
        object.insert("name".into(), json!(project.name));
        object.insert("canvas".into(), json!(project.canvas));
        object.insert("assets".into(), json!(project.assets));
        object.insert("tracks".into(), json!(project.tracks));
        object.insert("duration_us".into(), json!(state.project.duration_us()));
        object.insert("caption_stats".into(), caption_stats(&state.project));
        object.insert("filtered".into(), json!(args.range.is_some() || args.clip_ids.is_some()));
        Ok(value)
    }

    fn import(&self, args: Import, state: &SessionState) -> Result<Value> {
        owns_run(state, &args.run_id)?;
        ensure!(!args.paths.is_empty(), "INVALID_ARGUMENTS: no paths to import");
        let request_id = args.request_id.clone().unwrap_or_else(new_id);
        let key = (args.run_id.clone(), request_id.clone());
        let mut requests = self.import_requests.lock().unwrap();
        requests.retain(|(run, _), _| run == &args.run_id);
        // A retry after a failed save reuses the probed assets, so it never adds them twice.
        let prepared = match requests.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                ensure!(entry.get().paths == args.paths, "REQUEST_CONFLICT: request_id was used with different paths");
                entry.into_mut()
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let assets = args
                    .paths
                    .iter()
                    .map(|p| {
                        let path = self.resolve(p);
                        ensure!(path.is_file(), "MEDIA_MISSING: {}", path.display());
                        probe(&std::fs::canonicalize(&path).context("Resolving media path")?, new_id())
                    })
                    .collect::<Result<Vec<_>>>()?;
                entry.insert(PreparedImport {
                    paths: args.paths,
                    assets,
                    expect: Expect { revision: Some(state.stamp.revision), speech_layout_key: None },
                })
            }
        };
        let ids: Vec<_> = prepared.assets.iter().map(|a| a.id.clone()).collect();
        let result = self.host.session.apply_edits(
            &args.run_id,
            &request_id,
            vec![EditCmd::AddAssets { assets: prepared.assets.clone() }],
            prepared.expect.clone(),
        )?;
        Ok(json!({"revision": result.stamp.revision, "session_epoch": result.stamp.session_epoch, "asset_ids": ids}))
    }

    fn analyze(&self, args: Analyze, state: &SessionState) -> Result<Value> {
        if matches!(args.kind, AnalysisKind::Retakes) {
            // Pure computation over stored words, so it answers at once instead of as a job.
            ensure!(args.asset_id.is_none(), "INVALID_ARGUMENTS: retakes reads the whole timeline; omit asset_id");
            let derived = transcript::derive(&self.media_project(&state.project), &self.host.transcripts)?;
            ensure!(
                derived.untranscribed.is_empty(),
                "TRANSCRIPT_MISSING: transcribe every heard asset first, untranscribed: {}",
                derived.untranscribed.join(", ")
            );
            let mut result = serde_json::to_value(nuzky_analysis::retakes(&derived.words))?;
            result["time_basis"] = json!("timeline");
            result["transcript_key"] = json!(transcript::word_key(&state.project, &derived.words));
            return Ok(result);
        }
        if matches!(args.kind, AnalysisKind::Emphasis) {
            // Reads stored words and the prepared sound of their files, so it answers at once too.
            ensure!(args.asset_id.is_none(), "INVALID_ARGUMENTS: emphasis reads the whole timeline; omit asset_id");
            let project = self.media_project(&state.project);
            let derived = transcript::derive(&project, &self.host.transcripts)?;
            if derived.untranscribed.is_empty() {
                self.prepare_sound(&project, &derived, state)?;
            }
            let zooms = crate::zooms::suggest(&project, &derived, &self.host.cache_dir)?;
            return Ok(json!({"zooms": zooms, "time_basis": "timeline",
                "transcript_key": transcript::word_key(&state.project, &derived.words)}));
        }
        if matches!(args.kind, AnalysisKind::ThumbnailFrames) {
            return self.thumbnail_frames(args, state);
        }
        let asset_id = args.asset_id.context("INVALID_ARGUMENTS: asset_id is required for this kind")?;
        let mut asset = state.project.asset(&asset_id).with_context(|| format!("UNKNOWN_ASSET: {asset_id}"))?.clone();
        asset.path = self.resolve(&asset.path).to_string_lossy().into_owned();
        ensure!(Path::new(&asset.path).is_file(), "MEDIA_MISSING: {}", asset.path);
        let window = matches!(args.kind, AnalysisKind::Loudness)
            .then(|| loudness_window(asset.duration_us, args.params.window_us))
            .transpose()?
            .unwrap_or_default();
        let transcript = if matches!(args.kind, AnalysisKind::Fillers) {
            let record =
                self.host.transcripts.get(&asset)?.context("TRANSCRIPT_MISSING: transcribe this asset first")?;
            Some(nuzky_analysis::Transcript { language: record.language, words: record.words, segments: vec![] })
        } else {
            None
        };
        let cache = self.host.cache_dir.clone();
        self.host.start_job(&self.client.id, state.open_run.as_ref().map(|run| run.run_id.as_str()), "analysis", state.stamp.clone(), move |cancel, progress| {
            check_cancel(&cancel)?;
            progress.set("analyzing", None);
            let p = args.params;
            let result = match args.kind {
                AnalysisKind::Silences => json!({"ranges": nuzky_analysis::silences_cancellable(&asset, &cache, SilenceParams { threshold_db: p.threshold_db, min_silence_us: p.min_silence_us.unwrap_or(400_000), pad_us: p.pad_us.unwrap_or(120_000) }, || cancel.load(Ordering::Relaxed))?}),
                AnalysisKind::Loudness => {
                    let cancelled = || cancel.load(Ordering::Relaxed);
                    let program = nuzky_analysis::program_loudness_cancellable(&asset, &cache, cancelled)?;
                    json!({"window_us": window, "dbfs": nuzky_analysis::loudness_cancellable(&asset, &cache, window, cancelled)?,
                        "integrated_lufs": program.integrated_lufs, "true_peak_dbtp": program.true_peak_dbtp})
                }
                AnalysisKind::Scenes => json!({"cuts": nuzky_analysis::scene_cuts_cancellable(&asset, SceneParams { threshold: p.threshold.unwrap_or(0.18), min_gap_us: p.min_gap_us.unwrap_or(300_000) }, || cancel.load(Ordering::Relaxed))?}),
                AnalysisKind::Fillers => { let t = transcript.context("Missing filler transcript")?; json!({"ranges": nuzky_analysis::filler_words(&t, &t.language)}) },
                AnalysisKind::Retakes | AnalysisKind::Emphasis | AnalysisKind::ThumbnailFrames => {
                    anyhow::bail!("This kind is not an asset analysis")
                }
            };
            check_cancel(&cancel)?;
            Ok(json!({"asset_id": asset.id, "time_basis": "source", "analysis": result}))
        })
    }

    /// Renders the timeline without text and scores frames for a cover, as a job. The face models
    /// must be installed; this never downloads them.
    fn thumbnail_frames(&self, args: Analyze, state: &SessionState) -> Result<Value> {
        ensure!(args.asset_id.is_none(), "INVALID_ARGUMENTS: thumbnail_frames reads the whole timeline; omit asset_id");
        let format = match args.params.format.as_deref() {
            None => None,
            Some("9:16") => Some(nuzky_vision::Format::Vertical),
            Some("16:9") => Some(nuzky_vision::Format::Wide),
            Some(other) => anyhow::bail!("INVALID_ARGUMENTS: format is \"9:16\" or \"16:9\", not {other:?}"),
        };
        let project = self.media_project(&state.project);
        activity::timeline_range(&project, None)?;
        media::check_media(&project)?;
        let models = nuzky_analysis::models_dir();
        nuzky_vision::models::require(nuzky_vision::models::FRAMES, &models)?;
        let format = format.unwrap_or(nuzky_vision::Format::of(&project.canvas));
        self.host.start_job(
            &self.client.id,
            state.open_run.as_ref().map(|run| run.run_id.as_str()),
            "analysis",
            state.stamp.clone(),
            move |cancel, progress| {
                let candidates = nuzky_vision::thumbnail_frames(&project, &models, Some(format), &cancel, &mut |phase, done| {
                    let phase = match phase {
                        nuzky_vision::thumbnails::Phase::Looking => "looking",
                        nuzky_vision::thumbnails::Phase::Scoring => "scoring",
                    };
                    progress.set(phase, Some(done))
                })?;
                Ok(json!({"kind": "thumbnail_frames", "time_basis": "timeline", "format": format, "candidates": candidates}))
            },
        )
    }

    /// Masks the subject of one timeline frame as a job and keeps the mask in the cache.
    fn segment_subject(&self, args: SegmentSubject, state: &SessionState) -> Result<Value> {
        let project = self.media_project(&state.project);
        let (start, end) = activity::timeline_range(&project, None)?;
        ensure!((start..end).contains(&args.time_us), "INVALID_RANGE: time_us must be inside 0..{end}");
        media::check_media(&project)?;
        let models = nuzky_analysis::models_dir();
        nuzky_vision::models::require(nuzky_vision::models::MASK, &models)?;
        let cache = self.host.cache_dir.clone();
        let t = args.time_us;
        self.host.start_job(
            &self.client.id,
            state.open_run.as_ref().map(|run| run.run_id.as_str()),
            "segment",
            state.stamp.clone(),
            move |cancel, progress| {
                let mask = nuzky_vision::segment_subject(&project, t, &models, &cache, &cancel, &mut |phase| {
                    progress.set(
                        match phase {
                            nuzky_vision::mask::Phase::Rendering => "rendering",
                            nuzky_vision::mask::Phase::Waiting => "waiting_for_other_mask",
                            nuzky_vision::mask::Phase::Loading => "loading_model",
                            nuzky_vision::mask::Phase::Segmenting => "segmenting",
                        },
                        None,
                    )
                })?;
                Ok(json!({"time_us": t, "mask_path": mask.path, "width": mask.width, "height": mask.height,
                    "subject_box": mask.subject_box, "subject_share": mask.subject_share, "person": mask.person,
                    "cached": mask.cached}))
            },
        )
    }

    /// Without the app nothing prepares the sound of files whose words were stored earlier, and
    /// decoding a whole file inside a tool call could not be stopped, so it becomes a job.
    fn prepare_sound(&self, project: &Project, derived: &transcript::Derived, state: &SessionState) -> Result<()> {
        let cache = self.host.cache_dir.clone();
        let missing: Vec<_> = project
            .assets
            .iter()
            .filter(|a| derived.sources.contains_key(&a.id))
            .filter(|a| nuzky_engine::audio::has_audio(a) && !nuzky_engine::audio::pcm_path(&cache, a).exists())
            .cloned()
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let names = missing.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
        let job = self.host.start_job(
            &self.client.id,
            state.open_run.as_ref().map(|run| run.run_id.as_str()),
            "analysis",
            state.stamp.clone(),
            move |cancel, progress| {
                for (i, asset) in missing.iter().enumerate() {
                    progress.set("preparing_audio", Some(i as f32 / missing.len() as f32));
                    nuzky_engine::audio::ensure_pcm(&cache, asset, |_| check_cancel(&cancel))?;
                }
                Ok(json!({"prepared": missing.iter().map(|a| &a.id).collect::<Vec<_>>()}))
            },
        )?;
        anyhow::bail!(
            "AUDIO_NOT_READY: preparing the sound of {names} as job {}; poll job until done, then analyze again",
            job["job_id"].as_str().unwrap_or_default()
        )
    }

    fn transcribe(&self, args: Transcribe, state: &SessionState) -> Result<Value> {
        let project = self.media_project(&state.project);
        let mut assets = Vec::new();
        if let Some(ids) = args.asset_ids {
            for id in ids {
                let asset = project.asset(&id).with_context(|| format!("UNKNOWN_ASSET: {id}"))?;
                if !assets.iter().any(|a: &nuzky_engine::model::Asset| a.id == id) {
                    assets.push(asset.clone());
                }
            }
        } else {
            let heard = transcript::heard_assets(&project);
            for asset in project.assets.iter().filter(|a| heard.contains(&a.id)) {
                if self.host.transcripts.get(asset)?.is_none() {
                    assets.push(asset.clone());
                }
            }
        }
        let name = args.model.unwrap_or_else(|| transcript::best_model().into());
        let model_paths = if assets.is_empty() { None } else { Some(transcript::models(&name)?) };
        let language = args.language.unwrap_or_else(|| "auto".into());
        let store = self.host.transcripts.clone();
        let cache = self.host.cache_dir.clone();
        self.host.start_job(
            &self.client.id,
            state.open_run.as_ref().map(|run| run.run_id.as_str()),
            "transcription",
            state.stamp.clone(),
            move |cancel, progress| {
                let mut recognised = Vec::new();
                let count = assets.len();
                let mut recognised_files = std::collections::HashSet::new();
                for (i, asset) in assets.into_iter().enumerate() {
                    check_cancel(&cancel)?;
                    progress.set("transcribing", Some(i as f32 / count as f32));
                    let fingerprint = store.fingerprint(&asset)?;
                    if !recognised_files.insert(fingerprint.clone()) {
                        recognised.push(json!({"asset_id":asset.id,"reused":true}));
                        continue;
                    }
                    let models = model_paths.as_ref().context("MODEL_MISSING: transcription model")?;
                    let fraction = Some(i as f32 / count as f32);
                    let stage = |stage| match stage {
                        transcript::Stage::Waiting => progress.set("waiting_for_other_transcription", fraction),
                        transcript::Stage::Recognising => progress.set("transcribing", fraction),
                        transcript::Stage::DownloadingAligner(done) => {
                            progress.set("downloading_word_timing_model", Some(done))
                        }
                        transcript::Stage::Aligning => progress.set("measuring_word_times", fraction),
                    };
                    let record =
                        transcript::recognise(&store, &asset, &cache, &name, models, &language, &cancel, stage)?;
                    recognised.push(json!({
                        "asset_id": asset.id,
                        "words": record.words.len(),
                        "language": record.language,
                        "word_times": if record.alignment.is_some() { "measured" } else { "estimated" },
                    }));
                }
                Ok(json!({"assets":recognised}))
            },
        )
    }

    fn get_transcript(&self, args: GetTranscript, state: &SessionState) -> Result<Value> {
        let derived = transcript::derive(&self.media_project(&state.project), &self.host.transcripts)?;
        let mut result = transcript::summary(&derived, args.range_us)?;
        result["transcript_key"] = json!(transcript::word_key(&state.project, &derived.words));
        Ok(result)
    }

    fn edit_transcript(&self, args: EditTranscript, state: &SessionState) -> Result<Value> {
        // A dry run plans the cut before a run is open and changes nothing.
        if args.dry_run {
            return Ok(self.prepare_transcript_edit(&args, state)?.response);
        }
        let run_id = args.run_id.clone().context("INVALID_ARGUMENTS: run_id is required unless dry_run is true")?;
        owns_run(state, &run_id)?;
        let request_id = args.request_id.clone().unwrap_or_else(new_id);
        let key = (run_id.clone(), request_id.clone());
        let mut requests = self.transcript_requests.lock().unwrap();
        requests.retain(|(run, _), _| run == &run_id);
        if let Some(prepared) = requests.get(&key) {
            ensure!(
                prepared.arguments == serde_json::to_value(&args)?,
                "REQUEST_CONFLICT: request_id was used with different transcript arguments"
            );
            return self.apply_transcript_edit(&run_id, &request_id, prepared);
        }
        let prepared = self.prepare_transcript_edit(&args, state)?;
        // Retain the original ranges and expectations even if the live edit's save fails.
        let prepared = requests.entry(key).or_insert(prepared);
        self.apply_transcript_edit(&run_id, &request_id, prepared)
    }

    fn apply_transcript_edit(
        &self,
        run_id: &str,
        request_id: &str,
        prepared: &PreparedTranscriptEdit,
    ) -> Result<Value> {
        let applied =
            self.host.session.apply_edits(run_id, request_id, prepared.edits.clone(), prepared.expect.clone())?;
        let mut response = prepared.response.clone();
        response["revision"] = json!(applied.stamp.revision);
        response["session_epoch"] = json!(applied.stamp.session_epoch);
        Ok(response)
    }

    fn prepare_transcript_edit(&self, args: &EditTranscript, state: &SessionState) -> Result<PreparedTranscriptEdit> {
        let derived = transcript::derive(&self.media_project(&state.project), &self.host.transcripts)?;
        transcript::check_key(&state.project, &derived, &args.transcript_key)?;
        let before = state.project.duration_us();
        let ranges = transcript::edit_ranges(
            &state.project,
            &derived,
            args.delete.as_deref(),
            args.keep.as_deref(),
            Some(args.shorten_pauses_us.unwrap_or(transcript::DEFAULT_PAUSE_US)),
        )?;
        let cut = transcript::plan_cut(&state.project, &derived, ranges)?;
        let after = cut.preview.duration_us();
        let preview_text: String =
            cut.words.iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ").chars().take(PREVIEW_CHARS).collect();
        Ok(PreparedTranscriptEdit {
            arguments: serde_json::to_value(args)?,
            expect: Expect {
                revision: Some(state.stamp.revision),
                speech_layout_key: Some(state.speech_layout_key.clone()),
            },
            response: json!({"duration_us":{"before":before,"after":after},
                "removed_us":before-after,"ranges":cut.ranges,"preview_text":preview_text,
                "transcript_key":transcript::word_key(&cut.preview, &cut.words),"revision":state.stamp.revision,"dry_run":args.dry_run}),
            edits: vec![cut.edit],
        })
    }

    fn correct_words(&self, args: CorrectWords, state: &SessionState) -> Result<Value> {
        owns_run(state, &args.run_id)?;
        let request_id = args.request_id.clone().unwrap_or_else(new_id);
        let key = (args.run_id.clone(), request_id.clone());
        let arguments = json!({"correct_words": args});
        let mut requests = self.transcript_requests.lock().unwrap();
        requests.retain(|(run, _), _| run == &args.run_id);
        // A retry after a failed save applies what was planned then: the key no longer matches.
        if let Some(prepared) = requests.get(&key) {
            ensure!(
                prepared.arguments == arguments,
                "REQUEST_CONFLICT: request_id was used with different transcript arguments"
            );
            return self.apply_transcript_edit(&args.run_id, &request_id, prepared);
        }
        let project = self.media_project(&state.project);
        let derived = transcript::derive(&project, &self.host.transcripts)?;
        transcript::check_word_key(&state.project, &derived, &args.transcript_key)?;
        let fixes: Vec<_> = args.corrections.iter().map(|c| (c.i, c.text.clone())).collect();
        let plan = transcript::plan_correction(&state.project, &derived, &fixes)?;
        let mut preview = project;
        for edit in plan.edits.clone() {
            preview.apply(edit).context("EDIT_REJECTED: preview failed")?;
        }
        nuzky_session::validate(&preview)?;
        let after = transcript::derive(&preview, &self.host.transcripts)?;
        let words: Vec<_> =
            plan.words.iter().map(|(i, from, to)| json!({"i": i, "before": from, "after": to})).collect();
        let prepared = PreparedTranscriptEdit {
            arguments,
            expect: Expect { revision: Some(state.stamp.revision), speech_layout_key: None },
            response: json!({"words": words, "captions_changed": plan.captions,
                "transcript_key": transcript::word_key(&preview, &after.words)}),
            edits: plan.edits,
        };
        let prepared = requests.entry(key).or_insert(prepared);
        self.apply_transcript_edit(&args.run_id, &request_id, prepared)
    }

    fn captions(&self, args: Captions, state: &SessionState) -> Result<Value> {
        owns_run(state, &args.run_id)?;
        let derived = transcript::derive(&self.media_project(&state.project), &self.host.transcripts)?;
        ensure!(derived.untranscribed.is_empty(), "TRANSCRIPT_MISSING: transcribe all heard assets before captions");
        let grouping = args.grouping();
        let style = args.style()?;
        let (edit, _) = transcript::caption_edit(&derived.words, &state.project, style, grouping)?;
        let result = self.host.session.apply_edits(
            &args.run_id,
            &new_id(),
            vec![edit],
            Expect { revision: Some(state.stamp.revision), speech_layout_key: None },
        )?;
        Ok(
            json!({"revision": result.stamp.revision, "session_epoch": result.stamp.session_epoch, "caption_count": result.outcome.created.len(), "created": result.outcome.created, "removed": result.outcome.removed}),
        )
    }

    /// Punch-ins on word ranges as one edit of the run. Splitting clips moves the speech layout and
    /// so the transcript key; a retry with the same request_id applies the zoom planned the first
    /// time instead of planning it again.
    fn apply_zooms(&self, args: ApplyZooms, state: &SessionState) -> Result<Value> {
        owns_run(state, &args.run_id)?;
        let request_id = args.request_id.clone().unwrap_or_else(new_id);
        let key = (args.run_id.clone(), request_id.clone());
        let arguments = json!({ "apply_zooms": &args });
        let mut requests = self.transcript_requests.lock().unwrap();
        requests.retain(|(run, _), _| run == &args.run_id);
        if let Some(prepared) = requests.get(&key) {
            ensure!(prepared.arguments == arguments, "REQUEST_CONFLICT: request_id was used with different arguments");
            return self.apply_transcript_edit(&args.run_id, &request_id, prepared);
        }
        let derived = transcript::derive(&self.media_project(&state.project), &self.host.transcripts)?;
        transcript::check_key(&state.project, &derived, &args.transcript_key)?;
        let ranges = crate::zooms::ranges(&state.project, &derived.words, &args.zooms)?;
        let edit = EditCmd::ZoomRanges { ranges: ranges.clone() };
        let mut preview = state.project.clone();
        let outcome = preview.apply(edit.clone()).map_err(|e| anyhow!("EDIT_REJECTED: {e:#}"))?;
        let words = nuzky_engine::speech::map_words(&preview, &derived.sources);
        let prepared = requests.entry(key).or_insert(PreparedTranscriptEdit {
            arguments,
            edits: vec![edit],
            expect: Expect {
                revision: Some(state.stamp.revision),
                speech_layout_key: Some(state.speech_layout_key.clone()),
            },
            response: json!({"ranges": ranges, "skipped": outcome.skipped,
                "transcript_key": transcript::word_key(&preview, &words)}),
        });
        self.apply_transcript_edit(&args.run_id, &request_id, prepared)
    }

    fn export(&self, args: Export, state: &SessionState) -> Result<Value> {
        ensure!(!state.read_only, "READ_ONLY: --allow-write is required to write an export");
        ensure!(
            args.preset.is_some() || (args.resolution.is_some() && args.fps.is_some()),
            "INVALID_ARGUMENTS: give resolution and fps, or preset \"reels\""
        );
        ensure!(
            args.resolution.is_none_or(|r| (2..=7680).contains(&r)) && args.fps.is_none_or(|f| (1..=240).contains(&f)),
            "Invalid export resolution or fps"
        );
        let out = self.resolve(&args.path);
        ensure!(!out.exists(), "OUTPUT_EXISTS: choose a new export path");
        let parent = std::fs::canonicalize(out.parent().context("Output has no parent")?)
            .context("Export directory must exist")?;
        let out = parent.join(out.file_name().context("Output needs a filename")?);
        ensure!(
            out != self.project_path
                && !out.as_os_str().to_string_lossy().starts_with(&format!("{}.", self.project_path.display())),
            "Export cannot overwrite the project or its sidecars"
        );
        let project = self.media_project(&state.project);
        let options = ExportOptions {
            resolution: args.resolution,
            fps: args.fps,
            crf: args.quality.unwrap_or(Quality::Recommended).crf(),
            replace_existing: false,
            delivery: args.preset,
            ..ExportOptions::default()
        };
        check_options(&project, &options).map_err(|e| anyhow!("INVALID_ARGUMENTS: {e}"))?;
        media::check_media(&project)?;
        let cache = self.host.cache_dir.clone();
        let queue = self.export_queue.clone();
        self.host.start_job(
            &self.client.id,
            state.open_run.as_ref().map(|run| run.run_id.as_str()),
            "export",
            state.stamp.clone(),
            move |cancel, progress| {
                progress.set("waiting_for_export", None);
                let _export = queue.lock().unwrap();
                check_cancel(&cancel)?;
                export(&project, &cache, &out, &options, &cancel, |p| {
                    let phase = match p.phase {
                        ExportPhase::Loudness => "measuring_loudness",
                        ExportPhase::Rendering => "exporting",
                    };
                    progress.set(phase, Some(p.fraction))
                })?;
                Ok(json!({"path": out, "duration_us": project.duration_us(), "preset": options.delivery}))
            },
        )
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let path = PathBuf::from(path);
        if path.is_absolute() { path } else { self.project_dir.join(path) }
    }

    fn media_project(&self, project: &Project) -> Project {
        let mut project = project.clone();
        for asset in &mut project.assets {
            asset.path = self.resolve(&asset.path).to_string_lossy().into_owned();
        }
        project
    }
}

/// An hour in 10 ms windows; longer media needs wider windows so results stay bounded.
const MAX_LOUDNESS_VALUES: i64 = 360_000;
const MAX_LOUDNESS_WINDOW_US: i64 = 3_600_000_000;

fn loudness_window(duration_us: i64, window_us: Option<i64>) -> Result<i64> {
    let min = (duration_us / MAX_LOUDNESS_VALUES + 1).max(10_000);
    let window = window_us.unwrap_or(min.max(100_000));
    ensure!(
        (min..=MAX_LOUDNESS_WINDOW_US).contains(&window),
        "INVALID_ARGUMENTS: loudness window_us must be {min}..={MAX_LOUDNESS_WINDOW_US} for this asset"
    );
    Ok(window)
}

/// Agents add media as existing local files, as import_media does; projects may keep missing ones.
fn check_new_assets(edits: &[EditCmd]) -> Result<()> {
    for edit in edits {
        if let EditCmd::AddAssets { assets } = edit {
            for asset in assets {
                nuzky_session::local_media_path(&asset.path)?;
                ensure!(Path::new(&asset.path).is_file(), "MEDIA_MISSING: {}", asset.path);
            }
        }
    }
    Ok(())
}

fn owns_run(state: &SessionState, id: &str) -> Result<()> {
    ensure!(!state.read_only, "READ_ONLY: restart with --allow-write");
    ensure!(state.open_run.as_ref().is_some_and(|r| r.run_id == id), "INVALID_RUN: begin a run first");
    Ok(())
}

fn parse<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).context("INVALID_ARGUMENTS")
}

pub async fn call(backend: Arc<Backend>, name: String, arguments: Value) -> CallToolResult {
    let errors = backend.clone();
    match tokio::task::spawn_blocking(move || backend.call(&name, arguments)).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => tool_error(&errors, format!("{error:#}")),
        Err(error) => tool_error(&errors, format!("TOOL_FAILED: worker failed: {error}")),
    }
}

pub(crate) fn error_message(message: String) -> String {
    if message
        .split_once(": ")
        .is_some_and(|(code, _)| !code.is_empty() && code.bytes().all(|c| c.is_ascii_uppercase() || c == b'_'))
    {
        message
    } else {
        format!("TOOL_FAILED: {message}")
    }
}

pub(crate) fn tool_error(backend: &Backend, message: String) -> CallToolResult {
    let message = error_message(message);
    let stamp = backend.host.session.stamp();
    CallToolResult::structured_error(json!({
        "error": message, "revision": stamp.revision, "session_epoch": stamp.session_epoch,
    }))
}

fn caption_stats(project: &Project) -> Value {
    let captions =
        project.tracks.iter().filter(|track| track.is_captions()).flat_map(|track| &track.clips).filter_map(|clip| {
            match &clip.content {
                nuzky_engine::model::ClipContent::Text { text, .. } => Some(text),
                _ => None,
            }
        });
    let (count, max_chars, max_words) = captions.fold((0, 0, 0), |(count, chars, words), text| {
        (count + 1, chars.max(text.chars().count()), words.max(text.split_whitespace().count()))
    });
    json!({"count": count, "max_chars": max_chars, "max_words": max_words})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspect_frames_reports_deleted_image_name() {
        let dir = std::env::temp_dir().join(format!("inspect-missing-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("deleted.ppm");
        std::fs::write(&image, b"P6\n2 2\n255\nabcdefghijkl").unwrap();
        let mut project = Project::new("missing image");
        project.apply(EditCmd::AddAssets { assets: vec![probe(&image, "image".into()).unwrap()] }).unwrap();
        project.apply(EditCmd::AddClip { asset_id: "image".into(), start_us: None, track_id: None }).unwrap();
        let path = dir.join("project.nuzky");
        std::fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
        std::fs::remove_file(image).unwrap();
        let backend = Backend::open(&path, false, dir.join("cache")).unwrap();
        let error = backend.call("inspect_frames", json!({"times_us": [0], "width": 64})).unwrap_err();
        assert!(format!("{error:#}").contains("deleted.ppm"), "{error:#}");
        drop(backend);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agent_assets_must_be_existing_local_files() {
        let dir = std::env::temp_dir().join(format!("add-assets-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("still.ppm");
        std::fs::write(&image, b"P6\n2 2\n255\nabcdefghijkl").unwrap();
        let path = dir.join("project.nuzky");
        std::fs::write(&path, serde_json::to_vec(&Project::new("assets")).unwrap()).unwrap();
        let backend = Backend::open(&path, true, dir.join("cache")).unwrap();
        let run = backend.call("begin_run", json!({"label":"assets"})).unwrap().structured_content.unwrap();
        let mut asset = probe(&image, "still".into()).unwrap();
        let add = |asset: &nuzky_engine::model::Asset, request: &str| {
            backend.call(
                "apply_edits",
                json!({"run_id":run["run_id"],"request_id":request,"edits":[{"type":"addAssets","assets":[asset]}]}),
            )
        };
        for (path, code) in [
            ("http://127.0.0.1:9/x.mp4", "INVALID_ASSET_PATH"),
            ("concat:/a.mp4|/b.mp4", "INVALID_ASSET_PATH"),
            ("still.ppm", "INVALID_ASSET_PATH"),
            ("/nonexistent-nuzky-asset.mp4", "MEDIA_MISSING"),
        ] {
            let mut bad = asset.clone();
            bad.path = path.into();
            let error = format!("{:#}", add(&bad, path).unwrap_err());
            assert!(error.starts_with(code), "{path}: {error}");
        }
        assert!(backend.host.session.state().unwrap().project.assets.is_empty());
        asset.path = image.to_string_lossy().into();
        add(&asset, "ok").unwrap();
        assert_eq!(backend.host.session.state().unwrap().project.assets.len(), 1);
        drop(backend);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn other_clients_reads_do_not_keep_an_idle_run_open() {
        let dir = std::env::temp_dir().join(format!("idle-clients-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.nuzky");
        std::fs::write(&path, serde_json::to_vec(&Project::new("idle")).unwrap()).unwrap();
        let timeout = std::time::Duration::from_millis(300);
        let session = ProjectSession::open_with_idle_timeout(&path, Mode::Write, timeout, None).unwrap();
        let host = Arc::new(Host::new(session, dir.join("cache")).unwrap());
        let client = |access| Backend::shared(host.clone(), &path, Client { id: new_id(), access }).unwrap();
        let (agent, viewer) = (client(Access::Write), client(Access::ReadOnly));
        agent.call("begin_run", json!({"label":"abandoned"})).unwrap();
        let began = std::time::Instant::now();
        while host.session.state().unwrap().open_run.is_some() {
            assert!(began.elapsed() < timeout * 5, "another client's reads kept the run open");
            viewer.call("get_state", json!({})).unwrap();
            let _ = viewer.call("job", json!({"job_id":"missing","action":"get"}));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        agent.call("begin_run", json!({"label":"active"})).unwrap();
        let began = std::time::Instant::now();
        while began.elapsed() < timeout * 3 {
            agent.call("get_state", json!({})).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(host.session.state().unwrap().open_run.is_some());
        drop((agent, viewer));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_only_clients_cancel_their_own_jobs_only() {
        let dir = std::env::temp_dir().join(format!("job-owners-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.nuzky");
        std::fs::write(&path, serde_json::to_vec(&Project::new("jobs")).unwrap()).unwrap();
        let session = ProjectSession::open(&path, Mode::Write, None).unwrap();
        let host = Arc::new(Host::new(session, dir.join("cache")).unwrap());
        let client = |access| Backend::shared(host.clone(), &path, Client { id: new_id(), access }).unwrap();
        let (writer, reader) = (client(Access::Write), client(Access::ReadOnly));
        let stamp = host.session.stamp();
        let job = |owner: &Backend| {
            let started = host.start_job(&owner.client.id, None, "analysis", stamp.clone(), |cancel, _| {
                while !cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                anyhow::bail!("CANCELLED: test")
            });
            started.unwrap()["job_id"].as_str().unwrap().to_owned()
        };
        let (theirs, mine) = (job(&writer), job(&reader));
        let refused = format!("{:#}", reader.call("job", json!({"job_id": theirs, "action": "cancel"})).unwrap_err());
        assert!(refused.starts_with("UNAUTHORIZED"), "{refused}");
        let cancel =
            reader.call("job", json!({"job_id": mine, "action": "cancel"})).unwrap().structured_content.unwrap();
        assert_eq!(cancel["cancel_requested"], true);
        host.jobs.get(&theirs, true).unwrap();
        drop((writer, reader));
        host.jobs.shutdown();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn import_request_id_retries_a_failed_save_without_adding_assets_twice() {
        let dir = std::env::temp_dir().join(format!("import-retry-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("still.ppm");
        std::fs::write(&image, b"P6\n2 2\n255\nabcdefghijkl").unwrap();
        let path = dir.join("project.nuzky");
        std::fs::write(&path, serde_json::to_vec(&Project::new("import")).unwrap()).unwrap();
        let backend = Backend::open(&path, true, dir.join("cache")).unwrap();
        let run = backend.call("begin_run", json!({"label":"import"})).unwrap().structured_content.unwrap();
        let args = json!({"run_id":run["run_id"],"paths":["still.ppm"],"request_id":"import-once"});
        let backup = dir.join("backup");
        std::fs::rename(&path, &backup).unwrap();
        std::fs::create_dir(&path).unwrap();
        let error = format!("{:#}", backend.call("import_media", args.clone()).unwrap_err());
        assert!(error.contains("SAVE_FAILED"), "{error}");
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&backup, &path).unwrap();
        let first = backend.call("import_media", args.clone()).unwrap().structured_content.unwrap();
        let again = backend.call("import_media", args.clone()).unwrap().structured_content.unwrap();
        assert_eq!(first["asset_ids"], again["asset_ids"]);
        let disk: Project = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(disk.assets.len(), 1);
        assert_eq!(backend.host.session.state().unwrap().project.assets.len(), 1);
        let mut other = args;
        other["paths"] = json!(["still.ppm", "still.ppm"]);
        let conflict = format!("{:#}", backend.call("import_media", other).unwrap_err());
        assert!(conflict.starts_with("REQUEST_CONFLICT"), "{conflict}");
        drop(backend);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_video_takes_the_reels_preset_or_explicit_settings() {
        let dir = std::env::temp_dir().join(format!("export-args-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.nuzky");
        let mut wide = Project::new("wide");
        (wide.canvas.width, wide.canvas.height) = (1920, 1080);
        std::fs::write(&path, serde_json::to_vec(&wide).unwrap()).unwrap();
        let backend = Backend::open(&path, true, dir.join("cache")).unwrap();
        let error = |args: Value| format!("{:#}", backend.call("export_video", args).unwrap_err());
        let missing = error(json!({"path": "out.mp4", "quality": "high"}));
        assert!(missing.starts_with("INVALID_ARGUMENTS"), "{missing}");
        let wrong_format = error(json!({"path": "out.mp4", "preset": "reels"}));
        assert!(
            wrong_format.starts_with("INVALID_ARGUMENTS: Reels & TikTok needs a 9:16 video and this one is 16:9"),
            "{wrong_format}"
        );
        assert!(error(json!({"path": "out.mp4", "preset": "youtube"})).contains("unknown variant"));
        drop(backend);
        std::fs::write(&path, serde_json::to_vec(&Project::new("tall")).unwrap()).unwrap();
        let backend = Backend::open(&path, true, dir.join("cache")).unwrap();
        let other = format!(
            "{:#}",
            backend.call("export_video", json!({"path": "out.mp4", "preset": "reels", "fps": 60})).unwrap_err()
        );
        assert!(other.starts_with("INVALID_ARGUMENTS: Reels & TikTok exports at fps 30"), "{other}");
        assert!(!dir.join("out.mp4").exists());
        drop(backend);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn loudness_windows_stay_bounded_for_long_media() {
        let hour = 3_600_000_000;
        assert_eq!(loudness_window(hour, None).unwrap(), 100_000);
        assert_eq!(loudness_window(hour, Some(10_001)).unwrap(), 10_001);
        assert!(loudness_window(hour, Some(1)).unwrap_err().to_string().starts_with("INVALID_ARGUMENTS"));
        assert!(loudness_window(hour, Some(MAX_LOUDNESS_WINDOW_US + 1)).is_err());
        let day = 24 * hour;
        let wide = loudness_window(day, None).unwrap();
        assert!(wide > 100_000 && day / wide <= MAX_LOUDNESS_VALUES);
        assert!(loudness_window(day, Some(100_000)).is_err());
    }

    #[test]
    fn caption_stats_count_unicode_characters_and_exclude_titles() {
        let mut project = Project::new("stats");
        let style = nuzky_engine::model::TextStyle {
            font_family: None,
            font_size: 64.0,
            color: "#fff".into(),
            bold: false,
            stroke_width: 0.0,
            stroke_color: "#000".into(),
            background: None,
            max_width: None,
            highlight: None,
        };
        project
            .apply(EditCmd::AddText { start_us: 0, text: "A very long unrelated title".into(), style: style.clone() })
            .unwrap();
        project
            .apply(EditCmd::AddCaptions {
                segments: vec![nuzky_engine::edit::CaptionSegment {
                    start_us: 0,
                    end_us: 1_000_000,
                    text: "Příliš žluťoučký".into(),
                    words: Vec::new(),
                }],
                style,
            })
            .unwrap();
        assert_eq!(caption_stats(&project), json!({"count":1,"max_chars":16,"max_words":2}));
    }
}

#[cfg(test)]
mod transcript_tests {
    use super::*;
    use nuzky_session::transcripts::{Record, VERSION};

    fn fixture() -> (PathBuf, Backend, Project) {
        let dir = std::env::temp_dir().join(format!("transcript-tools-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut project, sources) = transcript::tests::fixture();
        let asset_path = dir.join("talk.mov");
        std::fs::write(&asset_path, b"fixture content").unwrap();
        project.assets[0].path = asset_path.to_string_lossy().into();
        let clip = project.tracks[0].clips[0].id.clone();
        project.apply(EditCmd::SplitClip { clip_id: clip, at_us: 5_000_000 }).unwrap();
        let path = dir.join("project.nuzky");
        std::fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
        let store = nuzky_session::transcripts::TranscriptStore::at(dir.join("transcripts")).unwrap();
        store
            .put(
                &project.assets[0],
                &Record {
                    version: VERSION,
                    duration_us: project.assets[0].duration_us,
                    fingerprint: store.fingerprint(&project.assets[0]).unwrap(),
                    model: "fixture".into(),
                    language: "en".into(),
                    words: sources["talk"].clone(),
                    segments: vec![],
                    alignment: None,
                },
            )
            .unwrap();
        let backend = Backend {
            host: Arc::new(Host {
                session: ProjectSession::open(&path, Mode::Write, None).unwrap(),
                jobs: Default::default(),
                transcripts: store,
                cache_dir: dir.join("cache"),
            }),
            client: Client { id: "test".into(), access: Access::Write },
            runs: Mutex::default(),
            closed: AtomicBool::new(false),
            project_dir: dir.clone(),
            project_path: path,
            export_queue: Arc::default(),
            transcript_requests: Mutex::default(),
            import_requests: Mutex::default(),
        };
        (dir, backend, project)
    }

    #[test]
    fn dry_run_stale_key_and_multiple_clip_slips_are_one_undo() {
        let (dir, backend, project) = fixture();
        let run = backend.host.session.begin_run("remove slips".into()).unwrap();
        let state = backend.host.session.state().unwrap();
        let initial = backend.get_transcript(GetTranscript { range_us: None }, &state).unwrap();
        let args = json!({"run_id":run.run_id,"transcript_key":initial["transcript_key"],"delete":[[1,2],[6,6]],"dry_run":true});
        let preview = backend.dispatch("edit_transcript", args.clone(), &state).unwrap();
        assert_eq!(backend.host.session.state().unwrap().project, project);
        assert_eq!(backend.host.session.state().unwrap().stamp.revision, state.stamp.revision);
        let mut apply = args;
        apply["dry_run"] = json!(false);
        let result = backend.dispatch("edit_transcript", apply.clone(), &state).unwrap();
        assert_eq!(preview["duration_us"], result["duration_us"]);
        assert_eq!(preview["preview_text"], result["preview_text"]);
        let changed = backend.host.session.state().unwrap();
        assert_eq!(
            result["transcript_key"],
            backend.get_transcript(GetTranscript { range_us: None }, &changed).unwrap()["transcript_key"]
        );
        assert!(
            backend.dispatch("edit_transcript", apply, &changed).unwrap_err().to_string().contains("SPEECH_CHANGED")
        );
        let transcript = backend.get_transcript(GetTranscript { range_us: None }, &changed).unwrap();
        assert_eq!(transcript["words"].as_array().unwrap().len(), 5);
        backend.host.session.end_run(&run.run_id, nuzky_session::EndAction::Keep).unwrap();
        backend.host.session.undo_run(&run.run_id).unwrap();
        assert_eq!(backend.host.session.state().unwrap().project, project);
        let mut record = backend.host.transcripts.get(&project.assets[0]).unwrap().unwrap();
        record.words.remove(0);
        backend.host.transcripts.put(&project.assets[0], &record).unwrap();
        let new_run = backend.host.session.begin_run("stale recognition".into()).unwrap();
        let current = backend.host.session.state().unwrap();
        let stale =
            json!({"run_id":new_run.run_id,"transcript_key":initial["transcript_key"],"delete":[[0,0]],"dry_run":true});
        assert!(
            backend.dispatch("edit_transcript", stale, &current).unwrap_err().to_string().contains("SPEECH_CHANGED")
        );
        drop(backend);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn transcript_request_id_retries_failed_save_without_cutting_again() {
        let (dir, backend, before) = fixture();
        let run = backend.host.session.begin_run("retry transcript".into()).unwrap();
        let initial = backend.host.session.state().unwrap();
        let transcript = backend.get_transcript(GetTranscript { range_us: None }, &initial).unwrap();
        let args = json!({"run_id":run.run_id,"request_id":"cut-once","transcript_key":transcript["transcript_key"],"delete":[[1,2]]});
        std::fs::remove_file(&backend.project_path).unwrap();
        std::fs::create_dir(&backend.project_path).unwrap();
        let error = backend.dispatch("edit_transcript", args.clone(), &initial).unwrap_err();
        assert!(format!("{error:#}").contains("SAVE_FAILED"));
        let live = backend.host.session.state().unwrap();
        assert_ne!(live.project, before);
        assert_eq!(live.stamp.revision, initial.stamp.revision + 1);
        std::fs::remove_dir(&backend.project_path).unwrap();
        let saved = backend.dispatch("edit_transcript", args.clone(), &live).unwrap();
        let disk: Project = serde_json::from_slice(&std::fs::read(&backend.project_path).unwrap()).unwrap();
        assert_eq!(disk, live.project);
        assert_eq!(backend.host.session.state().unwrap().project, live.project);
        assert_eq!(saved["revision"], live.stamp.revision);
        assert_eq!(backend.dispatch("edit_transcript", args.clone(), &live).unwrap(), saved);
        let mut conflict = args;
        conflict["delete"] = json!([[0, 0]]);
        assert!(
            backend.dispatch("edit_transcript", conflict, &live).unwrap_err().to_string().contains("REQUEST_CONFLICT")
        );
        backend.host.session.end_run(&run.run_id, nuzky_session::EndAction::Keep).unwrap();
        backend.host.session.undo_run(&run.run_id).unwrap();
        assert_eq!(backend.host.session.state().unwrap().project, before);
        drop(backend);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dry_run_plans_before_a_run_even_for_read_only_clients() {
        let (dir, backend, before) = fixture();
        let state = backend.host.session.state().unwrap();
        let key = backend.get_transcript(GetTranscript { range_us: None }, &state).unwrap()["transcript_key"].clone();
        let plan_args = json!({"transcript_key":key,"delete":[[1,2]],"dry_run":true});
        let plan = backend.call("edit_transcript", plan_args.clone()).unwrap().structured_content.unwrap();
        assert_eq!(plan["dry_run"], true);
        let viewer = Backend::shared(
            backend.host.clone(),
            &backend.project_path,
            Client { id: "viewer".into(), access: Access::ReadOnly },
        )
        .unwrap();
        let viewed = viewer.call("edit_transcript", plan_args).unwrap().structured_content.unwrap();
        assert_eq!(viewed["duration_us"], plan["duration_us"]);
        assert_eq!(backend.host.session.state().unwrap().project, before);
        let missing_run = format!(
            "{:#}",
            backend.call("edit_transcript", json!({"transcript_key":key,"delete":[[1,2]]})).unwrap_err()
        );
        assert!(missing_run.starts_with("INVALID_ARGUMENTS"), "{missing_run}");
        let run = backend.call("begin_run", json!({"label":"cut"})).unwrap().structured_content.unwrap();
        let applied = backend
            .call("edit_transcript", json!({"run_id":run["run_id"],"transcript_key":key,"delete":[[1,2]]}))
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(applied["duration_us"], plan["duration_us"]);
        assert_ne!(backend.host.session.state().unwrap().project, before);
        drop((viewer, backend));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn correct_words_changes_captions_survives_rebuilding_retries_and_undoes() {
        let (dir, backend, before) = fixture();
        let run = backend.call("begin_run", json!({"label":"fix words"})).unwrap().structured_content.unwrap();
        let run_id = run["run_id"].as_str().unwrap().to_owned();
        let call = |name: &str, args: Value| backend.call(name, args).map(|r| r.structured_content.unwrap());
        call("build_captions", json!({"run_id": run_id, "max_words": 3, "max_chars": 100})).unwrap();
        let transcript = call("get_transcript", json!({})).unwrap();
        let key = transcript["transcript_key"].clone();
        let caption = |backend: &Backend| {
            let project = backend.host.session.state().unwrap().project;
            project
                .tracks
                .iter()
                .filter(|t| t.is_captions())
                .flat_map(|t| &t.clips)
                .map(|c| match &c.content {
                    nuzky_engine::model::ClipContent::Text { text, .. } => text.clone(),
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };
        assert!(caption(&backend).contains("word3"));
        let args = json!({"run_id": run_id, "request_id": "fix-once", "transcript_key": key,
            "corrections": [{"i": 3, "text": "fixed"}]});
        // A failed save keeps the live edit; the retry finishes it without correcting twice.
        std::fs::remove_file(&backend.project_path).unwrap();
        std::fs::create_dir(&backend.project_path).unwrap();
        let error = format!("{:#}", backend.call("correct_words", args.clone()).unwrap_err());
        assert!(error.contains("SAVE_FAILED"), "{error}");
        std::fs::remove_dir(&backend.project_path).unwrap();
        let fixed = call("correct_words", args.clone()).unwrap();
        assert_eq!(fixed["words"], json!([{"i": 3, "before": "word3", "after": "fixed"}]));
        assert_eq!(fixed["captions_changed"].as_array().unwrap().len(), 1);
        assert_eq!(call("correct_words", args.clone()).unwrap(), fixed);
        let mut conflict = args.clone();
        conflict["corrections"] = json!([{"i": 3, "text": "other"}]);
        assert!(format!("{:#}", backend.call("correct_words", conflict).unwrap_err()).starts_with("REQUEST_CONFLICT"));
        let disk: Project = serde_json::from_slice(&std::fs::read(&backend.project_path).unwrap()).unwrap();
        assert_eq!(disk.word_corrections.len(), 1);
        let shown = caption(&backend);
        assert!(shown.contains("fixed") && !shown.contains("word3"), "{shown}");
        let after = call("get_transcript", json!({})).unwrap();
        assert_eq!(after["words"][3]["text"], "fixed");
        assert_eq!(after["transcript_key"], fixed["transcript_key"]);
        // The old key numbers words that read differently now.
        let stale = json!({"run_id": run_id, "transcript_key": key, "corrections": [{"i": 4, "text": "x"}]});
        assert!(format!("{:#}", backend.call("correct_words", stale).unwrap_err()).starts_with("SPEECH_CHANGED"));
        // Rebuilding the captions keeps the correction, and retakes read it too.
        call("build_captions", json!({"run_id": run_id, "max_words": 3, "max_chars": 100})).unwrap();
        assert_eq!(caption(&backend), shown);
        let retakes = call("analyze", json!({"kind": "retakes"})).unwrap();
        assert_eq!(retakes["transcript_key"], fixed["transcript_key"]);
        call("end_run", json!({"run_id": run_id, "action": "keep"})).unwrap();
        call("undo_run", json!({"run_id": run_id})).unwrap();
        assert_eq!(backend.host.session.state().unwrap().project, before);
        drop(backend);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wrong_transcript_key_explains_which_tool_to_use() {
        let (dir, backend, before) = fixture();
        let run = backend.host.session.begin_run("wrong key".into()).unwrap();
        let state = backend.host.session.state().unwrap();
        let args = json!({"run_id":run.run_id,"transcript_key":state.speech_layout_key,"dry_run":true});
        let error = backend.dispatch("edit_transcript", args, &state).unwrap_err().to_string();
        assert!(error.contains("SPEECH_CHANGED"));
        assert!(error.contains("use get_transcript's transcript_key"));
        assert_eq!(backend.host.session.state().unwrap().project, before);
        drop(backend);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
