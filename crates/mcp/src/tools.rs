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
use capopen_session::{Expect, Mode, ProjectSession, SessionState};
use rmcp::model::{CallToolResult, ContentBlock};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    jobs::{Jobs, Output, check_cancel},
    media,
    params::*,
    transcript,
};

pub struct Backend {
    pub session: ProjectSession,
    pub jobs: Jobs,
    cache: PathBuf,
    project_dir: PathBuf,
    project_path: PathBuf,
    export_queue: Arc<Mutex<()>>,
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
            session,
            jobs: Jobs::default(),
            cache,
            project_dir,
            project_path,
            export_queue: Arc::new(Mutex::new(())),
        })
    }

    pub fn call(&self, name: &str, arguments: Value) -> Result<CallToolResult> {
        let state = self.session.state()?;
        if name == "inspect_frames" {
            let args: Inspect = parse(arguments)?;
            let bytes = media::contact_sheet(
                &self.media_project(&state.project),
                &args.times_us,
                args.width,
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
                Ok(serde_json::to_value(self.session.begin_run(a.label)?)?)
            }
            "resolve_recovery" => {
                let a: Recovery = parse(arguments)?;
                let action = match a.action {
                    RecoveryAction::Keep => capopen_session::RecoveryAction::Keep,
                    RecoveryAction::Restore => capopen_session::RecoveryAction::Restore,
                };
                Ok(serde_json::to_value(self.session.resolve_recovery(action)?)?)
            }
            "apply_edits" => {
                let a: Apply = parse(arguments)?;
                Ok(serde_json::to_value(self.session.apply_edits(
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
                    self.session.end_run(&a.run_id, action)?,
                )?)
            }
            "undo_run" => {
                let a: Undo = parse(arguments)?;
                Ok(serde_json::to_value(self.session.undo_run(&a.run_id)?)?)
            }
            "import_media" => self.import(parse(arguments)?, state),
            "analyze" => self.analyze(parse(arguments)?, state),
            "transcribe" => self.transcribe(parse(arguments)?, state),
            "job" => {
                let a: Job = parse(arguments)?;
                self.jobs
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
        let result = self.session.apply_edits(
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
            let record =
                self.jobs
                    .transcript(args.params.transcript_id.as_deref().context(
                        "Fillers require params.transcript_id from an asset transcription",
                    )?)?;
            ensure!(
                record.target == args.asset_id
                    && record.project.asset(&args.asset_id) == state.project.asset(&args.asset_id),
                "TRANSCRIPT_MISMATCH: fillers require the current asset transcript"
            );
            Some(record.transcript)
        } else {
            None
        };
        let cache = self.cache.clone();
        self.jobs.start("analysis", state.stamp.clone(), move |cancel, progress| {
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
            Ok(Output::Json(json!({"asset_id": asset.id, "time_basis": "source", "analysis": result})))
        })
    }

    fn transcribe(&self, args: Transcribe, state: &SessionState) -> Result<Value> {
        let (model, vad) = transcript::models(&args.model)?;
        let project = self.media_project(&state.project);
        let original = state.project.clone();
        let cache = self.cache.clone();
        self.jobs.start(
            "transcription",
            state.stamp.clone(),
            move |cancel, progress| {
                progress.set("transcribing", None);
                let mut record = transcript::transcribe(
                    project,
                    args.target,
                    &args.language,
                    &model,
                    &vad,
                    &cache,
                    &cancel,
                )?;
                record.project = original;
                Ok(Output::Transcript(record))
            },
        )
    }

    fn captions(&self, args: Captions, state: &SessionState) -> Result<Value> {
        owns_run(state, &args.run_id)?;
        let record = self.jobs.transcript(&args.transcript_id)?;
        let grouping = args.grouping();
        let style = args.style.unwrap_or_else(reel_style);
        let (edit, _) = transcript::caption_edit(&record, &state.project, style, grouping)?;
        let result = self.session.apply_edits(
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
        let cache = self.cache.clone();
        let options = ExportOptions {
            resolution: Some(args.resolution),
            fps: Some(args.fps),
            crf: args.quality.crf(),
            replace_existing: false,
            ..ExportOptions::default()
        };
        let queue = self.export_queue.clone();
        self.jobs
            .start("export", state.stamp.clone(), move |cancel, progress| {
                progress.set("waiting_for_export", None);
                let _export = queue.lock().unwrap();
                check_cancel(&cancel)?;
                export(&project, &cache, &out, &options, &cancel, |p| {
                    progress.set(
                        "exporting",
                        Some(p.frame as f32 / p.total_frames.max(1) as f32),
                    )
                })?;
                Ok(Output::Json(
                    json!({"path": out, "duration_us": project.duration_us()}),
                ))
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
    match backend.session.state() {
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
