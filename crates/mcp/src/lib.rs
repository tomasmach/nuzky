//! Stdio MCP catalog with local and app-connected backends.
pub mod bridge;
#[cfg(unix)]
pub mod ipc;
mod limits;
mod media;
mod params;
mod rules;
mod tools;
pub mod transcript;

use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::{ErrorData, RoleServer, ServerHandler, model::*, service::RequestContext};
use schemars::{JsonSchema, schema_for};
use serde_json::Value;

use bridge::Target;

const GUIDE: &str = include_str!("../../../skills/capopen-edit/SKILL.md");
const STYLE_FIRST: &str = "This creator has their own editing style, capopen://style, measured from their recordings and finished cuts. It is at the end of this guide. Its rules and numbers override the defaults here.\n\n";
/// The whole way from raw takes to a reel, as the guide describes it, so one prompt starts it.
const ROUGH_CUT: &str = "Make a rough cut of this whole project for Instagram Reels and TikTok, then export it, following the guide below.\n\n1. get_state. Transcribe whatever is untranscribed and poll job until done.\n2. get_transcript, read every sentence, then analyze(kind: \"retakes\"). Keep the last complete attempt of each restarted sentence and drop the fillers that start sentences; check every group against the text and decide each review pair yourself. Add other slips and false starts you find.\n3. Plan with edit_transcript dry_run, then in one run: edit_transcript with the deletions (long pauses shorten by default), build_captions with the Reel style, inspect_frames with safe_area around cuts and captions, end_run keep.\n4. export_video with preset \"reels\" to a new .mp4 in the user's Videos folder (an absolute path), named after the project, poll until done and report the path, the duration and anything you were unsure about.";

const STYLE_NOTE: &str = "\n\n# This creator's style\n\nThe creator's EDIT.md follows, also at capopen://style. Where it gives a rule or a number, follow it instead of the defaults above: what to cut, pause length, caption limits, framing and zoom, including zoom the user did not ask for. Anything it does not cover keeps the defaults.\n\n";

/// The creator's EDIT.md, read anew each time so edits to it apply to the next request.
fn style() -> Option<String> {
    std::fs::read_to_string(capopen_analysis::style::style_path()).ok().filter(|text| !text.trim().is_empty())
}

/// The guide with the creator's style, pointed to first so a client that shortens long
/// instructions still sees it. Without EDIT.md this is the guide alone.
fn guide() -> String {
    match style() {
        Some(style) => format!("{STYLE_FIRST}{GUIDE}{STYLE_NOTE}{style}"),
        None => GUIDE.to_owned(),
    }
}

#[derive(Clone)]
struct Server {
    target: Arc<Target>,
    tools: Arc<Vec<Tool>>,
}

fn tool<T: JsonSchema>(name: &'static str, description: &'static str) -> Result<Tool> {
    let schema = schema_for!(T);
    let object = schema.as_object().context("Tool schema must be an object")?.clone();
    let rules = rules::find(name).with_context(|| format!("Tool {name} has no access rules"))?;
    let annotations = ToolAnnotations::new()
        .read_only(matches!(rules.reads, rules::Reads::Always))
        .destructive(rules.destructive)
        .idempotent(rules.idempotent)
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
            "Begin one reversible editing run and persist its checkpoint. Requires --allow-write. Only this run may mutate the project. End with keep or discard. Two minutes without tool calls from this client auto-keeps the run; plan or transcribe before opening a run. Every response includes revision and session_epoch. Revision changes only when the project changes; beginning a run leaves it unchanged.",
        )?,
        tool::<params::Recovery>(
            "resolve_recovery",
            "Resolve get_state.recovery_checkpoint after a crash. Ask the user before choosing: keep preserves the current project file; restore validates and saves the checkpoint project as an undoable edit. Both remove the checkpoint. Requires --allow-write and no open run. Revision changes only if the project changes.",
        )?,
        tool::<params::Apply>(
            "apply_edits",
            "Atomically apply EditCmd JSON (camelCase fields and type tags). All times are integer microseconds. Supply a unique request_id within this run; retry identical content with the same id while the run is open for the stored result. Ended runs reject retries with INVALID_RUN; runs stopped by the user return RUN_STOPPED. expected_revision rejects stale edits; expected_speech_layout_key rejects moved speech with SPEECH_CHANGED while allowing unrelated caption restyles. Main track is magnetic and repacks after edits. rippleDeleteRanges removes the union of half-open timeline ranges from every track except keepTrackIds, then closes gaps; omit keepTrackIds to leave the tracks marked keepInPlace (music) alone; ranges refer to the timeline BEFORE this command. Source analysis times must first be mapped through sourceInUs, startUs and speed. addCaptions creates a new track; replaceCaptions replaces only the named captions track (a text track named Captions). Other text never lands on a captions track automatically. Rejected commands or validation leave the project and history unchanged. Successful batches are saved before returning. A save failure reports an error but keeps the live edit; retry the identical request to retry saving without applying it twice. Result contains created/changed/removed ids and actual resulting clip times.",
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
            "Probe existing local file paths and add assets through the owning run. Relative paths resolve beside the project. Optional request_id: reuse it with the same paths after a save failure to finish the original import without adding the assets twice. Returns asset_ids; follow with addClip edits to place them. No uploads, downloads or automatic insertion.",
        )?,
        tool::<params::Inspect>(
            "inspect_frames",
            "Render 1–16 timeline times_us as ONE PNG contact sheet with timestamp labels and revision. width is per-frame pixels (96–1280; default 320). safe_area=true overlays translucent unsafe margins on vertical canvases to check captions and faces. Missing media or times outside the timeline are errors.",
        )?,
        tool::<params::Analyze>(
            "analyze",
            "Local analysis. retakes needs no asset_id and answers at once: it reads the stored words of the whole timeline, numbered exactly as get_transcript (same transcript_key), and fails with TRANSCRIPT_MISSING while a heard clip is untranscribed. It returns groups of attempts at one restarted sentence, also across clips: sentences {from,to,start_us,end_us,text,complete}, keep (index of the last complete attempt, the recommended one) and delete (the other attempts); fillers that start a sentence {from,to,text}, covering only the filler words; review: consecutive near-repeats that differ in a negation, a number or a content word, with a reason, never deleted for you; suggested_delete: all group deletes and fillers, sorted and merged, INCLUSIVE word ranges ready for edit_transcript delete. Same words give the same result. Read the groups against the text before deleting. The other kinds need asset_id, start a job (poll job(get)) and use source microseconds. silences: threshold_db, min_silence_us (400000), pad_us (120000); loudness: window_us (100000, wider for media over ten hours; at most 360000 values), RMS dBFS per window plus integrated_lufs (ITU-R BS.1770, null for silence) and true_peak_dbtp of the whole file; scenes: threshold (0.18), min_gap_us (300000); fillers: reads stored source words, transcribe first. Review filler suggestions in context.",
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
            "Cut by INCLUSIVE zero-based word ranges [[from,to],...]. Requires the current get_transcript transcript_key (includes recognition contents) and run_id unless dry_run. Optional request_id: reuse it with identical arguments after a save failure to finish the original save without cutting again; omitted ids are generated. Supply delete OR keep; omit both to shorten pauses only. shorten_pauses_us defaults to 300000. Removed passages take surrounding silence, kept passages retain at most 80 ms before and 120 ms after a boundary word, counted from where the word is heard: recognition moves word boundaries into the silence between words, so cuts do not clip syllables, and every cut crossfades over 20 ms through quiet sound; internal long pauses lose their middle. No boundary lands inside a word. One ripple edit respects tracks kept in place. dry_run=true simulates without changing project/history and needs no open run, so plan before begin_run. Returns duration_us {before,after}, removed_us, ranges (engine startUs/endUs), preview_text (first 400 characters), predicted/new transcript_key, revision, dry_run. Apply using the ORIGINAL transcript_key after checking the dry run. SPEECH_CHANGED rejects stale speech; overlapping speech may be rejected.",
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
            "Start a LOCAL H.264/AAC export of an immutable snapshot; job reports its revision. For Instagram Reels or TikTok pass preset=\"reels\": 1080x1920, 30 fps, H.264 High, AAC 48 kHz stereo, sound levelled to -14 LUFS with true peak <= -1 dBTP in the file (the project and preview keep their levels); needs a 9:16 canvas, resolution/fps may be omitted or must be 1080/30. Without preset resolution (the SHORT side in pixels, 1080 gives 1080x1920 on a portrait canvas) and fps=1..240 are required. quality high/recommended/small, default recommended. path must be new; relative paths resolve beside the project. Requires --allow-write. Job phases: measuring_loudness (preset only), exporting. Poll job(get) until done before reporting success.",
        )?,
    ])
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().enable_prompts().build())
            .with_server_info(Implementation::new("capopen", env!("CARGO_PKG_VERSION")))
            .with_instructions(guide())
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
        let mut resources = vec![
            Resource::new("capopen://guide", "Editing guide").with_mime_type("text/markdown"),
            Resource::new("capopen://schema", "Project JSON schema").with_mime_type("application/schema+json"),
        ];
        if style().is_some() {
            resources
                .push(Resource::new("capopen://style", "This creator's editing style").with_mime_type("text/markdown"));
        }
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let text = match request.uri.as_str() {
            "capopen://guide" => guide(),
            "capopen://schema" => schema_for!(capopen_engine::Project).to_value().to_string(),
            "capopen://style" => style().ok_or_else(|| {
                ErrorData::invalid_params("No EDIT.md yet: capopen style learn writes one from finished cuts", None)
            })?,
            _ => return Err(ErrorData::invalid_params("Unknown CapOpen resource", None)),
        };
        Ok(ReadResourceResult::new(vec![ResourceContents::text(text, request.uri)]).into())
    }

    async fn list_prompts(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(vec![
            Prompt::new(
                "edit_selected",
                Some("Edit the selected clips toward a goal"),
                Some(vec![PromptArgument::new("goal").with_required(true)]),
            ),
            Prompt::new(
                "rough_cut",
                Some("Rough-cut the whole project into a Reels and TikTok video with captions and export it"),
                Some(vec![
                    PromptArgument::new("wishes")
                        .with_description("Anything to do differently, e.g. keep the closing call to action")
                        .with_required(false),
                ]),
            ),
        ]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let argument = |name: &str| request.arguments.as_ref().and_then(|a| a.get(name)).and_then(Value::as_str);
        if request.name == "rough_cut" {
            let wishes = argument("wishes").filter(|w| !w.trim().is_empty()).unwrap_or("none");
            return Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                Role::User,
                format!("{ROUGH_CUT}\n\nThe user's wishes, which win over the steps: {wishes}\n\n{}", guide()),
            )])
            .into());
        }
        if request.name != "edit_selected" {
            return Err(ErrorData::invalid_params("Unknown prompt", None));
        }
        let goal = request
            .arguments
            .as_ref()
            .and_then(|a| a.get("goal"))
            .and_then(Value::as_str)
            .ok_or_else(|| ErrorData::invalid_params("goal is required", None))?;
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(Role::User, format!("{goal}\n\nRead get_state and use its selection. Headless selection is empty: use the whole timeline.\n\n{}", guide()))]).into())
    }
}
