use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, ensure};
use base64::{Engine as _, prelude::BASE64_STANDARD};
use capopen_analysis::{SceneParams, SilenceParams};
use capopen_engine::{
    Project,
    edit::{EditCmd, new_id},
    export::{ExportOptions, export},
    media::probe,
};
use capopen_session::{Expect, Mode, ProjectSession, SessionState, host::Host, jobs::check_cancel, transcripts::{Record, Segment, VERSION}};
use rmcp::model::{CallToolResult, ContentBlock};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    media,
    params::*,
    transcript,
};

const PREVIEW_CHARS: usize = 400;

struct PreparedTranscriptEdit {
    arguments: Value,
    edit: EditCmd,
    expect: Expect,
    response: Value,
}

pub struct Backend {
    pub host: Host,
    client_id: String,
    project_dir: PathBuf,
    project_path: PathBuf,
    export_queue: Arc<Mutex<()>>,
    transcript_requests: Mutex<HashMap<(String, String), PreparedTranscriptEdit>>,
}

impl Backend {
    pub fn open(project: &Path, allow_write: bool, cache: PathBuf) -> Result<Self> {
        let project_path = std::fs::canonicalize(project).context("Resolving project")?;
        let project_dir = project_path
            .parent()
            .context("Project has no directory")?
            .to_path_buf();
        let session = ProjectSession::open(
            project,
            if allow_write {
                Mode::Write
            } else {
                Mode::ReadOnly
            },
            None,
        )?;
        std::fs::create_dir_all(&cache).context("Creating media cache")?;
        Ok(Self {
            host: Host::new(session, cache)?,
            client_id: new_id(),
            project_dir,
            project_path,
            export_queue: Arc::new(Mutex::new(())),
            transcript_requests: Mutex::default(),
        })
    }

    pub fn call(&self, name: &str, arguments: Value) -> Result<CallToolResult> {
        let state = self.host.session.state()?;
        if name == "inspect_frames" {
            let args: Inspect = parse(arguments)?;
            let bytes = media::contact_sheet(
                &self.media_project(&state.project),
                &args.times_us,
                args.width,
                args.safe_area,
            )?;
            return Ok(CallToolResult::success(vec![ContentBlock::text(json!({"revision": state.stamp.revision, "session_epoch": state.stamp.session_epoch, "times_us": args.times_us, "labels": "seconds.microseconds"}).to_string()), ContentBlock::image(BASE64_STANDARD.encode(bytes), "image/png")]));
        }
        let mut value = self.dispatch(name, arguments, &state)?;
        let object = value
            .as_object_mut()
            .context("Tool produced a non-object result")?;
        object
            .entry("revision")
            .or_insert(json!(state.stamp.revision));
        object
            .entry("session_epoch")
            .or_insert(json!(state.stamp.session_epoch));
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
                    RecoveryAction::Keep => capopen_session::RecoveryAction::Keep,
                    RecoveryAction::Restore => capopen_session::RecoveryAction::Restore,
                };
                Ok(serde_json::to_value(self.host.session.resolve_recovery(action)?)?)
            }
            "apply_edits" => {
                let a: Apply = parse(arguments)?;
                Ok(serde_json::to_value(self.host.session.apply_edits(
                    &a.run_id,
                    &a.request_id,
                    a.edits,
                    Expect { revision: a.expected_revision, speech_key: a.expected_speech_key },
                )?)?)
            }
            "end_run" => {
                let a: End = parse(arguments)?;
                let action = match a.action {
                    EndAction::Keep => capopen_session::EndAction::Keep,
                    EndAction::Discard => capopen_session::EndAction::Discard,
                };
                Ok(serde_json::to_value(
                    self.host.session.end_run(&a.run_id, action)?,
                )?)
            }
            "undo_run" => {
                let a: Undo = parse(arguments)?;
                Ok(serde_json::to_value(self.host.session.undo_run(&a.run_id)?)?)
            }
            "import_media" => self.import(parse(arguments)?, state),
            "analyze" => self.analyze(parse(arguments)?, state),
            "transcribe" => self.transcribe(parse(arguments)?, state),
            "get_transcript" => self.get_transcript(parse(arguments)?, state),
            "edit_transcript" => self.edit_transcript(parse(arguments)?, state),
            "job" => {
                let a: Job = parse(arguments)?;
                self.host.jobs
                    .get(&a.job_id, matches!(a.action, JobAction::Cancel))
            }
            "build_captions" => self.captions(parse(arguments)?, state),
            "export_video" => self.export(parse(arguments)?, state),
            _ => anyhow::bail!("UNKNOWN_TOOL: {name}"),
        }
    }

    fn get_state(&self, args: State, state: &SessionState) -> Result<Value> {
        if let Some(range) = args.range {
            ensure!(
                range.start_us >= 0 && range.end_us > range.start_us,
                "Invalid half-open timeline range"
            );
        }
        let mut project = state.project.clone();
        for track in &mut project.tracks {
            track.clips.retain(|clip| {
                args.range
                    .is_none_or(|r| clip.start_us < r.end_us && clip.end_us() > r.start_us)
                    && args
                        .clip_ids
                        .as_ref()
                        .is_none_or(|ids| ids.contains(&clip.id))
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
        object.insert(
            "filtered".into(),
            json!(args.range.is_some() || args.clip_ids.is_some()),
        );
        Ok(value)
    }

    fn import(&self, args: Import, state: &SessionState) -> Result<Value> {
        owns_run(state, &args.run_id)?;
        ensure!(!args.paths.is_empty(), "No paths to import");
        let assets = args
            .paths
            .iter()
            .map(|p| {
                let path = self.resolve(p);
                ensure!(path.is_file(), "MEDIA_MISSING: {}", path.display());
                probe(
                    &std::fs::canonicalize(&path).context("Resolving media path")?,
                    new_id(),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let ids: Vec<_> = assets.iter().map(|a| a.id.clone()).collect();
        let result = self.host.session.apply_edits(
            &args.run_id,
            &new_id(),
            vec![EditCmd::AddAssets { assets }],
            Expect { revision: Some(state.stamp.revision), speech_key: None },
        )?;
        Ok(
            json!({"revision": result.stamp.revision, "session_epoch": result.stamp.session_epoch, "asset_ids": ids}),
        )
    }

    fn analyze(&self, args: Analyze, state: &SessionState) -> Result<Value> {
        let mut asset = state
            .project
            .asset(&args.asset_id)
            .context("UNKNOWN_ASSET")?
            .clone();
        asset.path = self.resolve(&asset.path).to_string_lossy().into_owned();
        ensure!(
            Path::new(&asset.path).is_file(),
            "MEDIA_MISSING: {}",
            asset.path
        );
        let transcript = if matches!(args.kind, AnalysisKind::Fillers) {
            let record = self.host.transcripts.get(&asset)?.context("TRANSCRIPT_MISSING: transcribe this asset first")?;
            Some(capopen_analysis::Transcript {
                language: record.language, words: record.words, segments: vec![],
            })
        } else {
            None
        };
        let cache = self.host.cache_dir.clone();
        self.host.jobs.start(&self.client_id, "analysis", state.stamp.clone(), move |cancel, progress| {
            check_cancel(&cancel)?;
            progress.set("analyzing", None);
            let p = args.params;
            let result = match args.kind {
                AnalysisKind::Silences => json!({"ranges": capopen_analysis::silences(&asset, &cache, SilenceParams { threshold_db: p.threshold_db, min_silence_us: p.min_silence_us.unwrap_or(400_000), pad_us: p.pad_us.unwrap_or(120_000) })?}),
                AnalysisKind::Loudness => { let window = p.window_us.unwrap_or(100_000); json!({"window_us": window, "dbfs": capopen_analysis::loudness(&asset, &cache, window)?}) },
                AnalysisKind::Scenes => json!({"cuts": capopen_analysis::scene_cuts(&asset, SceneParams { threshold: p.threshold.unwrap_or(0.18), min_gap_us: p.min_gap_us.unwrap_or(300_000) })?}),
                AnalysisKind::Fillers => { let t = transcript.context("Missing filler transcript")?; json!({"ranges": capopen_analysis::filler_words(&t, &t.language)}) },
            };
            check_cancel(&cancel)?;
            Ok(json!({"asset_id": asset.id, "time_basis": "source", "analysis": result}))
        })
    }

    fn transcribe(&self, args: Transcribe, state: &SessionState) -> Result<Value> {
        let project = self.media_project(&state.project);
        let mut assets = Vec::new();
        if let Some(ids) = args.asset_ids {
            for id in ids {
                let asset = project.asset(&id).context("UNKNOWN_ASSET: transcription source")?;
                if !assets.iter().any(|a: &capopen_engine::model::Asset| a.id == id) {
                    assets.push(asset.clone());
                }
            }
        } else {
            let heard = transcript::heard_assets(&project);
            for asset in project.assets.iter().filter(|a| heard.contains(&a.id)) {
                if self.host.transcripts.get(asset)?.is_none() { assets.push(asset.clone()); }
            }
        }
        let name = args.model.unwrap_or_else(|| transcript::best_model().into());
        let model_paths = if assets.is_empty() { None } else { Some(transcript::models(&name)?) };
        let language = args.language.unwrap_or_else(|| "auto".into());
        let store = self.host.transcripts.clone();
        let cache = self.host.cache_dir.clone();
        self.host.jobs.start(&self.client_id, "transcription", state.stamp.clone(), move |cancel, progress| {
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
                let (model, vad) = model_paths.as_ref().context("MODEL_MISSING: transcription model")?;
                let result = capopen_analysis::transcribe_words(
                    capopen_analysis::AudioSource::Asset { asset: &asset, cache: &cache }, model, vad, &language,
                )?;
                check_cancel(&cancel)?;
                let record = Record { version: VERSION, fingerprint, duration_us: asset.duration_us, model: name.clone(), language: result.language,
                    words: result.words, segments: result.segments.into_iter().map(|s| Segment {
                        start_us: s.start_us, end_us: s.end_us, text: s.text,
                    }).collect() };
                store.put(&asset, &record)?;
                recognised.push(json!({"asset_id":asset.id,"words":record.words.len(),"language":record.language}));
            }
            Ok(json!({"assets":recognised}))
        })
    }

    fn get_transcript(&self, args: GetTranscript, state: &SessionState) -> Result<Value> {
        let derived = transcript::derive(&self.media_project(&state.project), &self.host.transcripts)?;
        let mut result = transcript::summary(&derived, args.range_us)?;
        result["speech_key"] = json!(transcript::word_key(&state.project, &derived.words));
        Ok(result)
    }

    fn edit_transcript(&self, args: EditTranscript, state: &SessionState) -> Result<Value> {
        owns_run(state, &args.run_id)?;
        let request_id = args.request_id.clone().unwrap_or_else(new_id);
        let key = (args.run_id.clone(), request_id.clone());
        let mut requests = self.transcript_requests.lock().unwrap();
        requests.retain(|(run, _), _| run == &args.run_id);
        if !args.dry_run && let Some(prepared) = requests.get(&key) {
            ensure!(prepared.arguments == serde_json::to_value(&args)?,
                "REQUEST_CONFLICT: request_id was used with different transcript arguments");
            return self.apply_transcript_edit(&args.run_id, &request_id, prepared);
        }
        let prepared = self.prepare_transcript_edit(&args, state)?;
        if args.dry_run { return Ok(prepared.response); }
        // Retain the original ranges and expectations even if the live edit's save fails.
        let prepared = requests.entry(key).or_insert(prepared);
        self.apply_transcript_edit(&args.run_id, &request_id, prepared)
    }

    fn apply_transcript_edit(&self, run_id: &str, request_id: &str, prepared: &PreparedTranscriptEdit) -> Result<Value> {
        let applied = self.host.session.apply_edits(run_id, request_id,
            vec![prepared.edit.clone()], prepared.expect.clone())?;
        let mut response = prepared.response.clone();
        response["revision"] = json!(applied.stamp.revision);
        response["session_epoch"] = json!(applied.stamp.session_epoch);
        Ok(response)
    }

    fn prepare_transcript_edit(&self, args: &EditTranscript, state: &SessionState) -> Result<PreparedTranscriptEdit> {
        let derived = transcript::derive(&self.media_project(&state.project), &self.host.transcripts)?;
        ensure!(derived.untranscribed.is_empty(), "TRANSCRIPT_MISSING: transcribe all heard assets before cutting");
        ensure!(args.speech_key == transcript::word_key(&state.project, &derived.words),
            "SPEECH_CHANGED: timeline speech or recognised words changed");
        let before = state.project.duration_us();
        let ranges = transcript::edit_ranges(&state.project, &derived, args.delete.as_deref(),
            args.keep.as_deref(), args.shorten_pauses_us.unwrap_or(transcript::DEFAULT_PAUSE_US))?;
        let edit = EditCmd::RippleDeleteRanges { ranges: ranges.clone(), keep_track_ids: None };
        let mut preview = state.project.clone();
        preview.apply(edit.clone()).context("EDIT_REJECTED: preview failed")?;
        capopen_session::validate(&preview)?;
        let words = capopen_engine::speech::map_words(&preview, &derived.sources);
        let identities = |words: &[capopen_engine::speech::TimelineWord]| {
            let mut ids: Vec<_> = words.iter().map(|w| (w.asset_id.clone(), w.source_start_us, w.text.clone())).collect();
            ids.sort();
            ids
        };
        let retained: Vec<_> = derived.words.iter().filter(|w| !ranges.iter()
            .any(|r| r.start_us <= w.start_us && r.end_us >= w.end_us && w.start_us < r.end_us)).cloned().collect();
        ensure!(identities(&words) == identities(&retained),
            "UNSAFE_CUT: kept tracks or sub-frame fragments would change the selected words");
        let preview_text: String = words.iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ").chars().take(PREVIEW_CHARS).collect();
        Ok(PreparedTranscriptEdit {
            arguments: serde_json::to_value(args)?,
            edit,
            expect: Expect { revision: Some(state.stamp.revision), speech_key: Some(state.speech_key.clone()) },
            response: json!({"duration_us":{"before":before,"after":preview.duration_us()},
                "removed_us":before-preview.duration_us(),"ranges":ranges,"preview_text":preview_text,
                "speech_key":transcript::word_key(&preview, &words),"revision":state.stamp.revision,"dry_run":args.dry_run}),
        })
    }

    fn captions(&self, args: Captions, state: &SessionState) -> Result<Value> {
        owns_run(state, &args.run_id)?;
        let derived = transcript::derive(&self.media_project(&state.project), &self.host.transcripts)?;
        ensure!(derived.untranscribed.is_empty(), "TRANSCRIPT_MISSING: transcribe all heard assets before captions");
        let grouping = args.grouping();
        let style = args.style.unwrap_or_else(reel_style);
        let (edit, _) = transcript::caption_edit(&derived.words, &state.project, style, grouping)?;
        let result = self.host.session.apply_edits(
            &args.run_id,
            &new_id(),
            vec![edit],
            Expect { revision: Some(state.stamp.revision), speech_key: None },
        )?;
        Ok(
            json!({"revision": result.stamp.revision, "session_epoch": result.stamp.session_epoch, "caption_count": result.outcome.created.len(), "created": result.outcome.created, "removed": result.outcome.removed}),
        )
    }

    fn export(&self, args: Export, state: &SessionState) -> Result<Value> {
        ensure!(
            !state.read_only,
            "READ_ONLY: --allow-write is required to write an export"
        );
        ensure!(
            (2..=7680).contains(&args.resolution) && (1..=240).contains(&args.fps),
            "Invalid export resolution or fps"
        );
        let out = self.resolve(&args.path);
        ensure!(!out.exists(), "OUTPUT_EXISTS: choose a new export path");
        let parent = std::fs::canonicalize(out.parent().context("Output has no parent")?)
            .context("Export directory must exist")?;
        let out = parent.join(out.file_name().context("Output needs a filename")?);
        ensure!(
            out != self.project_path
                && !out
                    .as_os_str()
                    .to_string_lossy()
                    .starts_with(&format!("{}.", self.project_path.display())),
            "Export cannot overwrite the project or its sidecars"
        );
        let project = self.media_project(&state.project);
        media::check_media(&project)?;
        let cache = self.host.cache_dir.clone();
        let options = ExportOptions {
            resolution: Some(args.resolution),
            fps: Some(args.fps),
            crf: args.quality.crf(),
            replace_existing: false,
            ..ExportOptions::default()
        };
        let queue = self.export_queue.clone();
        self.host.jobs
            .start(&self.client_id, "export", state.stamp.clone(), move |cancel, progress| {
                progress.set("waiting_for_export", None);
                let _export = queue.lock().unwrap();
                check_cancel(&cancel)?;
                export(&project, &cache, &out, &options, &cancel, |p| {
                    progress.set(
                        "exporting",
                        Some(p.frame as f32 / p.total_frames.max(1) as f32),
                    )
                })?;
                Ok(json!({"path": out, "duration_us": project.duration_us()}))
            })
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            self.project_dir.join(path)
        }
    }

    fn media_project(&self, project: &Project) -> Project {
        let mut project = project.clone();
        for asset in &mut project.assets {
            asset.path = self.resolve(&asset.path).to_string_lossy().into_owned();
        }
        project
    }
}

fn owns_run(state: &SessionState, id: &str) -> Result<()> {
    ensure!(!state.read_only, "READ_ONLY: restart with --allow-write");
    ensure!(
        state.open_run.as_ref().is_some_and(|r| r.run_id == id),
        "INVALID_RUN: begin a run first"
    );
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
        Err(error) => tool_error(&errors, format!("Tool worker failed: {error}")),
    }
}

fn tool_error(backend: &Backend, message: String) -> CallToolResult {
    match backend.host.session.state() {
        Ok(state) => CallToolResult::structured_error(json!({
            "error": message, "revision": state.stamp.revision, "session_epoch": state.stamp.session_epoch,
        })),
        Err(_) => CallToolResult::error(vec![ContentBlock::text(message)]),
    }
}

fn caption_stats(project: &Project) -> Value {
    let captions = project
        .tracks
        .iter()
        .filter(|track| track.name == "Captions")
        .flat_map(|track| &track.clips)
        .filter_map(|clip| match &clip.content {
            capopen_engine::model::ClipContent::Text { text, .. } => Some(text),
            _ => None,
        });
    let (count, max_chars, max_words) = captions.fold((0, 0, 0), |(count, chars, words), text| {
        (
            count + 1,
            chars.max(text.chars().count()),
            words.max(text.split_whitespace().count()),
        )
    });
    json!({"count": count, "max_chars": max_chars, "max_words": max_words})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caption_stats_count_unicode_characters_and_exclude_titles() {
        let mut project = Project::new("stats");
        let style = capopen_engine::model::TextStyle {
            font_family: None,
            font_size: 64.0,
            color: "#fff".into(),
            bold: false,
            stroke_width: 0.0,
            stroke_color: "#000".into(),
            background: None,
            max_width: None,
        };
        project
            .apply(EditCmd::AddText {
                start_us: 0,
                text: "A very long unrelated title".into(),
                style: style.clone(),
            })
            .unwrap();
        project
            .apply(EditCmd::AddCaptions {
                segments: vec![capopen_engine::edit::CaptionSegment {
                    start_us: 0,
                    end_us: 1_000_000,
                    text: "Příliš žluťoučký".into(),
                }],
                style,
            })
            .unwrap();
        assert_eq!(
            caption_stats(&project),
            json!({"count":1,"max_chars":16,"max_words":2})
        );
    }
}

#[cfg(test)]
mod transcript_tests {
    use super::*;

    fn fixture() -> (PathBuf, Backend, Project) {
        let dir = std::env::temp_dir().join(format!("transcript-tools-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut project, sources) = transcript::tests::fixture();
        let asset_path = dir.join("talk.mov");
        std::fs::write(&asset_path, b"fixture content").unwrap();
        project.assets[0].path = asset_path.to_string_lossy().into();
        let clip = project.tracks[0].clips[0].id.clone();
        project.apply(EditCmd::SplitClip { clip_id: clip, at_us: 5_000_000 }).unwrap();
        let path = dir.join("project.capopen");
        std::fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
        let store = capopen_session::transcripts::TranscriptStore::at(dir.join("transcripts")).unwrap();
        store.put(&project.assets[0], &Record { version: VERSION, duration_us: project.assets[0].duration_us,
            fingerprint: store.fingerprint(&project.assets[0]).unwrap(), model: "fixture".into(), language: "en".into(),
            words: sources["talk"].clone(), segments: vec![],
        }).unwrap();
        let backend = Backend {
            host: Host { session: ProjectSession::open(&path, Mode::Write, None).unwrap(),
                jobs: Default::default(), transcripts: store, cache_dir: dir.join("cache") },
            client_id: "test".into(), project_dir: dir.clone(), project_path: path,
            export_queue: Arc::default(), transcript_requests: Mutex::default(),
        };
        (dir, backend, project)
    }

    #[test]
    fn dry_run_stale_key_and_multiple_clip_slips_are_one_undo() {
        let (dir, backend, project) = fixture();
        let run = backend.host.session.begin_run("remove slips".into()).unwrap();
        let state = backend.host.session.state().unwrap();
        let initial = backend.get_transcript(GetTranscript { range_us: None }, &state).unwrap();
        let args = json!({"run_id":run.run_id,"speech_key":initial["speech_key"],"delete":[[1,2],[6,6]],"dry_run":true});
        let preview = backend.dispatch("edit_transcript", args.clone(), &state).unwrap();
        assert_eq!(backend.host.session.state().unwrap().project, project);
        assert_eq!(backend.host.session.state().unwrap().stamp.revision, state.stamp.revision);
        let mut apply = args;
        apply["dry_run"] = json!(false);
        let result = backend.dispatch("edit_transcript", apply.clone(), &state).unwrap();
        assert_eq!(preview["duration_us"], result["duration_us"]);
        assert_eq!(preview["preview_text"], result["preview_text"]);
        let changed = backend.host.session.state().unwrap();
        assert_eq!(result["speech_key"], backend.get_transcript(GetTranscript { range_us: None }, &changed).unwrap()["speech_key"]);
        assert!(backend.dispatch("edit_transcript", apply, &changed).unwrap_err().to_string().contains("SPEECH_CHANGED"));
        let transcript = backend.get_transcript(GetTranscript { range_us: None }, &changed).unwrap();
        assert_eq!(transcript["words"].as_array().unwrap().len(), 5);
        backend.host.session.end_run(&run.run_id, capopen_session::EndAction::Keep).unwrap();
        backend.host.session.undo_run(&run.run_id).unwrap();
        assert_eq!(backend.host.session.state().unwrap().project, project);
        let mut record = backend.host.transcripts.get(&project.assets[0]).unwrap().unwrap();
        record.words.remove(0);
        backend.host.transcripts.put(&project.assets[0], &record).unwrap();
        let new_run = backend.host.session.begin_run("stale recognition".into()).unwrap();
        let current = backend.host.session.state().unwrap();
        let stale = json!({"run_id":new_run.run_id,"speech_key":initial["speech_key"],"delete":[[0,0]],"dry_run":true});
        assert!(backend.dispatch("edit_transcript", stale, &current).unwrap_err().to_string().contains("SPEECH_CHANGED"));
        drop(backend);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn transcript_request_id_retries_failed_save_without_cutting_again() {
        let (dir, backend, before) = fixture();
        let run = backend.host.session.begin_run("retry transcript".into()).unwrap();
        let initial = backend.host.session.state().unwrap();
        let transcript = backend.get_transcript(GetTranscript { range_us: None }, &initial).unwrap();
        let args = json!({"run_id":run.run_id,"request_id":"cut-once","speech_key":transcript["speech_key"],"delete":[[1,2]]});
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
        conflict["delete"] = json!([[0,0]]);
        assert!(backend.dispatch("edit_transcript", conflict, &live).unwrap_err().to_string().contains("REQUEST_CONFLICT"));
        backend.host.session.end_run(&run.run_id, capopen_session::EndAction::Keep).unwrap();
        backend.host.session.undo_run(&run.run_id).unwrap();
        assert_eq!(backend.host.session.state().unwrap().project, before);
        drop(backend);
        std::fs::remove_dir_all(dir).unwrap();
    }

}
