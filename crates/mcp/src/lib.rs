//! Stdio MCP catalog with local and app-connected backends.
pub mod bridge;
#[cfg(unix)]
pub mod ipc;
mod limits;
mod media;
mod params;
mod tools;
pub mod transcript;

use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::{ErrorData, RoleServer, ServerHandler, model::*, service::RequestContext};
use schemars::{JsonSchema, schema_for};
use serde_json::Value;

use bridge::Target;

const GUIDE: &str = include_str!("../../../skills/capopen-edit/SKILL.md");

#[derive(Clone)]
struct Server {
    target: Arc<Target>,
    tools: Arc<Vec<Tool>>,
}

fn tool<T: JsonSchema>(name: &'static str, description: &'static str) -> Result<Tool> {
    let schema = schema_for!(T);
    let object = schema.as_object().context("Tool schema must be an object")?.clone();
    let read_only = matches!(name, "get_state" | "get_transcript" | "inspect_frames");
    let annotations = ToolAnnotations::new()
        .read_only(read_only)
        .destructive(matches!(
            name,
            "apply_edits" | "edit_transcript" | "end_run" | "undo_run" | "build_captions" | "resolve_recovery"
        ))
        .idempotent(matches!(name, "get_state" | "get_transcript" | "inspect_frames" | "apply_edits"))
        .open_world(false);
    Ok(Tool::new(name, description, object).with_annotations(annotations))
}

fn catalog() -> Result<Vec<Tool>> {
    Ok(vec![
        tool::<params::State>(
            "get_state",
            "Read compact project assets, tracks and clips, selection (empty headless), playhead (0 headless), timeline-layout speech_layout_key (for apply_edits), revision, session_epoch, open_run and recovery_checkpoint. Times are integer microseconds; ranges are [start,end). Optional range and clip_ids filter clips only; duration_us and caption_stats (count, max_chars, max_words) always describe the full timeline. Use caption_stats after manual text corrections. Media source out = sourceInUs + durationUs * speed. The main track is magnetic: edits pack clips back-to-back from zero. Read capopen://guide before editing.",
        )?,
        tool::<params::Begin>(
            "begin_run",
            "Begin one reversible editing run and persist its checkpoint. Requires --allow-write. Only this run may mutate the project. End with keep or discard. Two minutes without tool calls auto-keeps the run; plan or transcribe before opening a run. Every response includes revision and session_epoch. Revision changes only when the project changes; beginning a run leaves it unchanged.",
        )?,
        tool::<params::Recovery>(
            "resolve_recovery",
            "Resolve get_state.recovery_checkpoint after a crash. Ask the user before choosing: keep preserves the current project file; restore validates and saves the checkpoint project as an undoable edit. Both remove the checkpoint. Requires --allow-write and no open run. Revision changes only if the project changes.",
        )?,
        tool::<params::Apply>(
            "apply_edits",
            "Atomically apply EditCmd JSON (camelCase fields and type tags). All times are integer microseconds. Supply a unique request_id within this run; retry identical content with the same id while the run is open for the stored result. Ended runs reject retries with INVALID_RUN; runs stopped by the user return RUN_STOPPED. expected_revision rejects stale edits; expected_speech_layout_key rejects moved speech with SPEECH_CHANGED while allowing unrelated caption restyles. Main track is magnetic and repacks after edits. rippleDeleteRanges removes the union of half-open timeline ranges from every track except keepTrackIds, then closes gaps; omit keepTrackIds to leave the tracks marked keepInPlace (music) alone; ranges refer to the timeline BEFORE this command. Source analysis times must first be mapped through sourceInUs, startUs and speed. addCaptions creates a new track; replaceCaptions replaces only the named caption track. Rejected commands or validation leave the project and history unchanged. Successful batches are saved before returning. A save failure reports an error but keeps the live edit; retry the identical request to retry saving without applying it twice. Result contains created/changed/removed ids and actual resulting clip times.",
        )?,
        tool::<params::End>(
            "end_run",
            "End the owning run: keep saves all its edits as ONE undo entry; discard drops the run entry and saves its previous state without a redo entry. The run id is then revoked. A clean disconnect keeps changes too. Keep leaves revision unchanged; discard increments it only if the project changes.",
        )?,
        tool::<params::Undo>(
            "undo_run",
            "Undo this whole run only if it is the LAST entry in the shared user/run history and no run is open. Saves the restored project. The UI can redo the whole run. History belongs to this session epoch. Revision increments only if the project changes.",
        )?,
        tool::<params::Import>(
            "import_media",
            "Probe existing local file paths and add assets through the owning run. Relative paths resolve beside the project. Returns asset_ids; follow with addClip edits to place them. No uploads, downloads or automatic insertion.",
        )?,
        tool::<params::Inspect>(
            "inspect_frames",
            "Render 1–16 timeline times_us as ONE PNG contact sheet with timestamp labels and revision. width is per-frame pixels (96–1280; default 320). safe_area=true overlays translucent unsafe margins on vertical canvases to check captions and faces. Missing media or times outside the timeline are errors.",
        )?,
        tool::<params::Analyze>(
            "analyze",
            "Start local asset analysis. Poll job(get). Source microseconds. silences: threshold_db, min_silence_us (400000), pad_us (120000); loudness: window_us (100000), RMS dBFS; scenes: threshold (0.18), min_gap_us (300000); fillers: reads stored source words, transcribe first. Review filler suggestions in context.",
        )?,
        tool::<params::Transcribe>(
            "transcribe",
            "Recognise source files locally and store reusable word timestamps. Omit asset_ids to recognise every heard asset lacking a stored transcript (includes detached sound). Supply asset_ids to re-recognise them. language defaults to auto. model defaults to installed large-v3-turbo-q5_0, else small; an absolute local model path is accepted. No model download. Returns job_id; poll job with pauses. Result assets lists asset_id, words count, language. Then use get_transcript.",
        )?,
        tool::<params::GetTranscript>(
            "get_transcript",
            "Read derived timeline speech: transcript_key, revision, words {i,start_us,end_us,text,p}, sentences {from,to,start_us,end_us,text}, pauses {after_word,gap_us}, untranscribed asset ids. i/from/to are global zero-based INCLUSIVE word indices valid for this transcript_key; p is recognition probability. Sentences split at phrase punctuation or gaps >=600000 us; pauses include gaps >=300000 us. Optional range_us=[start,end) filters results without renumbering. Use word indices with edit_transcript; never map source times by hand.",
        )?,
        tool::<params::EditTranscript>(
            "edit_transcript",
            "Cut by INCLUSIVE zero-based word ranges [[from,to],...]. Requires run_id and the current get_transcript transcript_key (includes recognition contents). Optional request_id: reuse it with identical arguments after a save failure to finish the original save without cutting again; omitted ids are generated. Supply delete OR keep; omit both to shorten pauses only. shorten_pauses_us defaults to 300000. Removed passages take surrounding silence, kept passages retain at most 80 ms before and 120 ms after a boundary word; internal long pauses lose their middle. No boundary lands inside a word. One ripple edit respects tracks kept in place. dry_run=true simulates without changing project/history. Returns duration_us {before,after}, removed_us, ranges (engine startUs/endUs), preview_text (first 400 characters), predicted/new transcript_key, revision, dry_run. Apply using the ORIGINAL transcript_key after checking the dry run. SPEECH_CHANGED rejects stale speech; overlapping speech may be rejected.",
        )?,
        tool::<params::Job>(
            "job",
            "Read job progress/result or request cancellation. Includes owner, kind, snapshot revision and session_epoch. Poll with pauses. Cancellation is cooperative after the current engine operation; completed assets remain stored when a later asset is cancelled.",
        )?,
        tool::<params::Captions>(
            "build_captions",
            "Build captions from stored words mapped through every heard clip, including detached audio and speed changes. No caption spans a clip cut. Requires run_id. Defaults max_words=2, max_chars=15; a single longer word stays intact. Adds or replaces ONE Captions track, preserving other text. Multiple Captions tracks require explicit replaceCaptions. Default Reel style: size 95, white, regular, black stroke 7.5, no background, Inter. Vertical canvases automatically wrap to the IG/TikTok safe width. Inspect frames with safe_area=true.",
        )?,
        tool::<params::Export>(
            "export_video",
            "Start a LOCAL H.264/AAC export of an immutable snapshot; job reports its revision. resolution is the SHORT side in pixels (1080 gives 1080x1920 on a portrait canvas), fps=1..240, quality high/recommended/small. path must be new; relative paths resolve beside the project. Requires --allow-write. Poll job(get) until done before reporting success.",
        )?,
    ])
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().enable_prompts().build())
            .with_server_info(Implementation::new("capopen", env!("CARGO_PKG_VERSION")))
            .with_instructions(GUIDE)
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools.as_ref().clone()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|t| t.name == name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        Ok(self
            .target
            .clone()
            .call(request.name.to_string(), Value::Object(request.arguments.unwrap_or_default()))
            .await
            .into())
    }

    async fn list_resources(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new("capopen://guide", "Editing guide").with_mime_type("text/markdown"),
            Resource::new("capopen://schema", "Project JSON schema").with_mime_type("application/schema+json"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let text = match request.uri.as_str() {
            "capopen://guide" => GUIDE.into(),
            "capopen://schema" => schema_for!(capopen_engine::Project).to_value().to_string(),
            _ => return Err(ErrorData::invalid_params("Unknown CapOpen resource", None)),
        };
        Ok(ReadResourceResult::new(vec![ResourceContents::text(text, request.uri)]).into())
    }

    async fn list_prompts(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
            "edit_selected",
            Some("Edit the selected clips toward a goal"),
            Some(vec![PromptArgument::new("goal").with_required(true)]),
        )]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        if request.name != "edit_selected" {
            return Err(ErrorData::invalid_params("Unknown prompt", None));
        }
        let goal = request
            .arguments
            .as_ref()
            .and_then(|a| a.get("goal"))
            .and_then(Value::as_str)
            .ok_or_else(|| ErrorData::invalid_params("goal is required", None))?;
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(Role::User, format!("{goal}\n\nRead get_state and use its selection. Headless selection is empty: use the whole timeline.\n\n{GUIDE}"))]).into())
    }
}
