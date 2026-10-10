//! Stdio MCP catalog with local and app-connected backends.
mod activity;
pub mod bridge;
#[cfg(unix)]
pub mod ipc;
mod limits;
mod media;
pub mod model_download;
mod params;
mod rules;
mod tools;
pub mod transcript;
pub mod zooms;

use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::{ErrorData, RoleServer, ServerHandler, model::*, service::RequestContext};
use schemars::{JsonSchema, schema_for};
use serde_json::Value;

use bridge::Target;

const GUIDE: &str = include_str!("../../../skills/nuzky-edit/SKILL.md");
const STYLE_FIRST: &str = "This creator has their own editing style, nuzky://style, measured from their recordings and finished cuts. It is at the end of this guide. Its rules and numbers override the defaults here.\n\n";
/// The whole way from raw takes to a reel, as the guide describes it, so one prompt starts it.
const ROUGH_CUT: &str = "Make a rough cut of this whole project for Instagram Reels and TikTok, then export it, following the guide below.\n\n1. get_state. Transcribe whatever is untranscribed and poll job until done.\n2. get_transcript, read every sentence, then analyze(kind: \"retakes\"). Keep the last complete attempt of each restarted sentence and drop the fillers that start sentences; check every group against the text and decide each review pair yourself. Add other slips and false starts you find.\n3. Plan with edit_transcript dry_run, then in one run: edit_transcript with the deletions (long pauses shorten by default), build_captions with the Reel style, inspect_frames with safe_area around cuts and captions, end_run keep.\n4. export_video with preset \"reels\" to a new .mp4 in the user's Videos folder (an absolute path), named after the project, poll until done and report the path, the duration and anything you were unsure about.";

const STYLE_NOTE: &str = "\n\n# This creator's style\n\nThe creator's EDIT.md follows, also at nuzky://style. Where it gives a rule or a number, follow it instead of the defaults above: what to cut, pause length, caption limits, framing and zoom, including zoom the user did not ask for. Anything it does not cover keeps the defaults.\n\n";

/// The creator's EDIT.md, read anew each time so edits to it apply to the next request.
fn style() -> Option<String> {
    std::fs::read_to_string(nuzky_analysis::style::style_path()).ok().filter(|text| !text.trim().is_empty())
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
            "Read compact project assets, tracks and clips, selection (empty headless), playhead (0 headless), timeline-layout speech_layout_key (for apply_edits), revision, session_epoch, open_run and recovery_checkpoint. Times are integer microseconds; ranges are [start,end). Optional range and clip_ids filter clips only; duration_us and caption_stats (count, max_chars, max_words) always describe the full timeline. Use caption_stats after manual text corrections. Media source out = sourceInUs + durationUs * speed. The main track is magnetic: edits pack clips back-to-back from zero. Read nuzky://guide before editing.",
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
            "Atomically apply EditCmd JSON (camelCase fields and type tags). All times are integer microseconds. Supply a unique request_id within this run; retry identical content with the same id while the run is open for the stored result. Ended runs reject retries with INVALID_RUN; runs stopped by the user return RUN_STOPPED. expected_revision rejects stale edits; expected_speech_layout_key rejects moved speech with SPEECH_CHANGED while allowing unrelated caption restyles. Main track is magnetic and repacks after edits. rippleDeleteRanges removes the union of half-open timeline ranges from every track except keepTrackIds, then closes gaps; omit keepTrackIds to leave the tracks marked keepInPlace (music) alone; ranges refer to the timeline BEFORE this command. A generated caption loses the words heard inside the ranges: with words left on both sides it becomes two captions, with none left it goes. Source analysis times must first be mapped through sourceInUs, startUs and speed. addCaptions creates a new track; replaceCaptions replaces only the named captions track (a text track named Captions). Other text never lands on a captions track automatically. Rejected commands or validation leave the project and history unchanged. Successful batches are saved before returning. A save failure reports an error but keeps the live edit; retry the identical request to retry saving without applying it twice. Result contains created/changed/removed ids and actual resulting clip times.",
        )?,
        tool::<params::End>(
            "end_run",
            "End the owning run: keep saves all its edits as ONE undo entry; discard drops the run entry and saves its previous state without a redo entry. The run id is then revoked. A clean disconnect keeps changes too. Keep leaves revision unchanged; discard increments it only if the project changes.",
        )?,
        tool::<params::Undo>(
            "undo_run",
            "Undo this whole run only if it is the LAST entry in the shared user/run history and no run is open. Saves the restored project. The UI can redo the whole run. History belongs to this session epoch. Revision increments only if the project changes.",
        )?,
        tool::<params::ListHistory>(
            "list_history",
            "List the kept versions of the project, newest first: index (stays with the version), hash (first 12 hex digits of the SHA-256 of the project's compact JSON), label, at_ms (Unix time in ms) and run_id for a version an agent's run made (none for the user's own steps). A version is the project after each kept run, each of the user's steps, each undo, redo and restore, and as it was opened. The user's step still in progress (a drag) leads the list under the index it will keep. current_hash is the project now, so the version with that hash is the current one, and older counts versions kept but not listed. Versions outlive restarts, up to the newest 200. Read-only; works while a run is open, but an open run becomes a version only when it ends.",
        )?,
        tool::<params::UndoTo>(
            "undo_to",
            "Restore a kept version from list_history by index or hash prefix (4+ hex digits) as ONE new undo step, so Undo in the app or undo_to the version before takes it back; versions after it stay listed, nothing is lost. Requires --allow-write and no open run (RUN_ACTIVE). The project then has exactly that version's hash. Saves before returning. changed is false and revision stays when the project already is that version. UNKNOWN_VERSION when no kept version matches or a prefix matches several.",
        )?,
        tool::<params::Import>(
            "import_media",
            "Probe existing local file paths and add assets through the owning run. Relative paths resolve beside the project. Optional request_id: reuse it with the same paths after a save failure to finish the original import without adding the assets twice. Returns asset_ids; follow with addClip edits to place them. No uploads, downloads or automatic insertion.",
        )?,
        tool::<params::Inspect>(
            "inspect_frames",
            "Render 1–16 timeline times_us as ONE PNG contact sheet with timestamp labels and revision. Or, instead of times_us, sample: \"changes\" picks the frames itself: in range_us=[start,end) (whole timeline by default) it looks at a candidate every 0.25 s (at most 120, further apart in a longer range; candidate_every_us), compares their perceptual hashes and keeps a frame only if it differs from each of the last four kept by more than min_change of 64 bits (default 16; gestures within one shot mostly stay below it, a cut to other footage is 30 or more), at most max_frames (1-16, default 16) per page. It returns times_us of the kept frames, skipped (candidates that looked like a kept frame) and next: pass it as cursor, with sample and without range_us or min_change, for the next page without repeats; null when the range is done. A static talking head gives one frame per shot. width is per-frame pixels (96–1280; default 320). safe_area=true overlays translucent unsafe margins on vertical canvases to check captions and faces. Missing media or times outside the timeline are errors.",
        )?,
        tool::<params::Activity>(
            "activity",
            "Read, as text and cheaply, where the picture and sound change, before looking at frames. range_us=[start,end) (whole timeline by default) is split into points steps (8-200, default 60) of step_us; point i covers [start + i*step_us, start + (i+1)*step_us). change: the biggest jump between consecutive looks at the picture within the step (a look every look_every_us at a tiny render of the timeline as exported); motion: how much the picture moves within 0.1 s, averaged over the step; both are the mean colour difference in percent, the measure of analyze scenes, where 18 or more is a hard cut. loudness_db: RMS dBFS of the timeline mix over the step, -120 is silence. peaks lists the strongest local maxima of each curve, {t_us, value} in time order; a change peak's t_us is the look where the new picture showed. A longer range is looked at more sparsely rather than for longer, so a fast pan can score like a cut and a very short insert can fall between looks: confirm with inspect_frames before cutting. AUDIO_NOT_READY names a job that prepares the sound of the files; poll it, then call again. Missing media is an error.",
        )?,
        tool::<params::Analyze>(
            "analyze",
            "Local analysis. retakes needs no asset_id and answers at once: it reads the stored words of the whole timeline, numbered exactly as get_transcript (same transcript_key), and fails with TRANSCRIPT_MISSING while a heard clip is untranscribed. It returns groups of attempts at one restarted sentence, also across clips: sentences {from,to,start_us,end_us,text,complete}, keep (index of the last complete attempt, the recommended one) and delete (the other attempts); fillers that start a sentence {from,to,text}, covering only the filler words; unfinished {from,to,text}: words that trail off (recognised with an ellipsis) before a new sentence starts; restarts inside one sentence without a full stop are groups too; review: consecutive near-repeats that differ in a negation, a number or a content word, with a reason, never deleted for you; suggested_delete: all group deletes, fillers and unfinished attempts, sorted and merged, INCLUSIVE word ranges ready for edit_transcript delete. Same words give the same result. Read the groups against the text before deleting. emphasis also needs no asset_id and answers at once, with the same word numbering and transcript_key: zooms {from,to,start_us,end_us,text,score,scale} are sentences for a punch-in, scored by how much louder they are than the median sentence (measured in the files' sound over the words), an exclamation and the opening hook; only sentences of 1.2-8 s with three words or more, at least 5 s apart and about one per 12 s of timeline; scale 1.15-1.3 by score. Same words and sound give the same result. AUDIO_NOT_READY names a job that prepares the sound of the files; poll it, then analyze again. Pass the chosen zooms to apply_zooms. The other kinds need asset_id, start a job (poll job(get)) and use source microseconds. silences: threshold_db, min_silence_us (400000), pad_us (120000); loudness: window_us (100000, wider for media over ten hours; at most 360000 values), RMS dBFS per window plus integrated_lufs (ITU-R BS.1770, null for silence) and true_peak_dbtp of the whole file; scenes: threshold (0.18), min_gap_us (300000); fillers: reads stored source words, transcribe first. Review filler suggestions in context. thumbnail_frames needs no asset_id and runs as a job (phases looking, scoring): it picks frames of the whole timeline worth a cover or thumbnail, looking at the picture as exported (colours, zooms, overlays) without text tracks. A look every 0.25 s (sharpness, exposure, a face) shortlists about 40 moments away from cuts, transitions and clip animations; a closer look scores those. Result candidates (6-8, best first, at least 2 s apart, fewer in a short timeline) {time_us, score, parts, faces, subject_box, crops}: parts are 0..1, higher is better: sharpness, exposure, and with a face eyes_open, mouth (not caught mid-word), facing (eyes and head toward the camera), smile, framing_9x16 (face size, inside the Reels/TikTok safe area and the 3:4 profile grid) and framing_16x9 (size, headroom, clear of YouTube's duration badge); params.format \"9:16\" or \"16:9\" decides which framing counts (the canvas shape by default). faces [{box,confidence}] and subject_box (the person) are [x,y,w,h] canvas pixels, crops the 9:16 and 16:9 cuts of the canvas around the face. Look at the top candidates with inspect_frames before choosing. Needs the face models; MODEL_MISSING says how to install them, nothing is downloaded here. VISION_UNAVAILABLE means covers cannot run on this computer; tell the user, editing works as before.",
        )?,
        tool::<params::SegmentSubject>(
            "segment_subject",
            "Mask the subject of the timeline frame at time_us as a job (phases rendering, waiting_for_other_mask, loading_model, segmenting; masks are made one at a time): the person when there is one, otherwise the most prominent object, for placing text behind them on a cover. The frame is rendered as exported without text tracks. Result: mask_path (8-bit grayscale PNG in Nuzky's cache at canvas resolution, white = subject), width, height, subject_box [x,y,w,h] canvas pixels (null when nothing stands out), subject_share (share of the canvas the subject covers, 0..1), person (a face is in the frame) and cached. The same frame, edit and model reuse the cached mask. Takes a few seconds on the CPU. Needs the mask model; MODEL_MISSING says how to install it, nothing is downloaded here; VISION_UNAVAILABLE as for thumbnail_frames.",
        )?,
        tool::<params::Transcribe>(
            "transcribe",
            "Recognise source files locally and store reusable word timestamps. Omit asset_ids to recognise every heard asset lacking a stored transcript (includes detached sound). Supply asset_ids to re-recognise them. language defaults to auto. model defaults to installed large-v3-turbo-q5_0, else small; an absolute local model path is accepted. Whisper models are never downloaded here; the word timing model of the recognised language (Czech, English) is downloaded once, then word times are measured in the sound; without it they are Whisper's estimates aligned to pauses. Returns job_id; poll job with pauses (phases transcribing, downloading_word_timing_model, measuring_word_times). Result assets lists asset_id, words count, language and word_times: measured or estimated. Then use get_transcript.",
        )?,
        tool::<params::GetTranscript>(
            "get_transcript",
            "Read derived timeline speech: transcript_key, revision, words {i,start_us,end_us,text,p}, sentences {from,to,start_us,end_us,text}, pauses {after_word,gap_us}, untranscribed asset ids. i/from/to are global zero-based INCLUSIVE word indices valid for this transcript_key; p is recognition probability. Sentences split at phrase punctuation or gaps >=600000 us; pauses include gaps >=300000 us. Optional range_us=[start,end) filters results without renumbering. Use word indices with edit_transcript; never map source times by hand.",
        )?,
        tool::<params::EditTranscript>(
            "edit_transcript",
            "Cut by INCLUSIVE zero-based word ranges [[from,to],...]. Requires the current get_transcript transcript_key (includes recognition contents) and run_id unless dry_run. Optional request_id: reuse it with identical arguments after a save failure to finish the original save without cutting again; omitted ids are generated. Supply delete OR keep; omit both to shorten pauses only. shorten_pauses_us defaults to 300000. Removed passages take surrounding silence, kept passages retain at most 80 ms before and 120 ms after a boundary word, counted from where the word is heard: recognition moves word boundaries into the silence between words, so cuts do not clip syllables, and every cut crossfades over 20 ms through quiet sound; internal long pauses lose their middle. No boundary lands inside a word. One ripple edit respects tracks kept in place. dry_run=true simulates without changing project/history and needs no open run, so plan before begin_run. Returns duration_us {before,after}, removed_us, ranges (engine startUs/endUs), preview_text (first 400 characters), predicted/new transcript_key, revision, dry_run. Apply using the ORIGINAL transcript_key after checking the dry run. SPEECH_CHANGED rejects stale speech; overlapping speech may be rejected.",
        )?,
        tool::<params::CorrectWords>(
            "correct_words",
            "Correct misrecognised words, e.g. \"oka\" written for a spoken \"okna\": corrections [{i, text}] with i from get_transcript and the text the word should read, punctuation included. Requires run_id and the current get_transcript transcript_key (SPEECH_CHANGED when stale). The correction belongs to the project and to that recognised word, so get_transcript, analyze retakes, build_captions and the app's transcript all read it, also after rebuilding captions; it changes no timing and cuts nothing. Caption clips on the Captions track that show the word get it replaced in the same step, keeping style and timing. A word heard in several clips is corrected everywhere. Text equal to the recognised word removes the correction. If the file is recognised again differently, the correction no longer applies. Optional request_id: reuse it with identical arguments after a save failure to finish the original save. Returns words {i, before, after} for every place a word is heard, captions_changed (clip ids) and the new transcript_key.",
        )?,
        tool::<params::Job>(
            "job",
            "Read job progress/result or request cancellation. Includes owner, kind, snapshot revision and session_epoch. Poll with pauses. Cancellation is cooperative after the current engine operation; completed assets remain stored when a later asset is cancelled.",
        )?,
        tool::<params::Captions>(
            "build_captions",
            "Build captions from stored words mapped through every heard clip, including detached audio and speed changes. No caption spans a clip cut. Requires run_id. Defaults max_words=2, max_chars=15; a single longer word stays intact. Adds or replaces ONE Captions track, preserving other text. Multiple Captions tracks require explicit replaceCaptions. Default Reel style: size 95, white, regular, black stroke 7.5, no background, Inter. style_preset picks a preset by name instead (reel, outline, yellow, box, clean, karaoke, green_box); karaoke is Reel with the word being spoken in yellow, green_box bold white on a dark box with the spoken word green. Karaoke captions store each word's time and highlight only while a word is spoken; updateClip text with as many words keeps that timing, adding or removing words turns the caption's highlight off. Give style or style_preset, not both. Vertical canvases automatically wrap to the IG/TikTok safe width. Inspect frames with safe_area=true.",
        )?,
        tool::<params::ApplyZooms>(
            "apply_zooms",
            "Punch in on INCLUSIVE word ranges as one edit of the run: zooms [{from,to,scale}] such as analyze(kind: \"emphasis\").zooms, with the current transcript_key (SPEECH_CHANGED when stale). Each zoom becomes a main-track timeline range from midway into the silence before its first word to midway into the silence after its last, at most 150 ms out, or on to the cut when only silence lies between; main-track clips are split there and the pieces inside get their scale multiplied by scale, keeping position and rotation. An edge that would leave a piece under 0.3 s, or cut a transition, animation or fade short, moves to the clip's edge. Sound plays on unchanged. Clips with keyframes are left alone and listed in skipped. Zooms must not overlap; scale 0.25-4. Requires run_id. Optional request_id: reuse it with identical arguments after a save failure to finish the original save without zooming twice. Returns ranges (startUs, endUs, scale), skipped clip ids, the new transcript_key and revision.",
        )?,
        tool::<params::ApplyMotion>(
            "apply_motion",
            "A slow camera move as one edit of the run, without working out keyframes: kind pushIn zooms in, pullOut starts zoomed in and ends on the clip's own framing, kenBurns zooms in while drifting right; strength is how far, 0.06 subtle, 0.1 or 0.15 strong (0.02-0.5). clip_id moves one video or image clip on any track, over the part of it inside range_us or its whole length; without clip_id, range_us moves every main-track video and image clip under it as one continuous motion, e.g. one sentence from get_transcript start_us of its first word to end_us of its last. Each clip gets two keyframes on a smooth curve (ease \"smooth\": it starts and ends gently) from its own transform, editable afterwards with setKeyframes. The zoom centres on the middle of the IG/TikTok safe area on vertical canvases, else on the canvas centre, so a face framed there stays put. Clips that already have keyframes are left alone and listed in skipped; remove their keyframes first to replace them. Requires run_id. Optional request_id: reuse it with identical arguments after a save failure. Returns revision, changed clip ids and skipped.",
        )?,
        tool::<params::Export>(
            "export_video",
            "Start a LOCAL H.264/AAC export of an immutable snapshot; job reports its revision. For Instagram Reels or TikTok pass preset=\"reels\": 1080x1920, 30 fps, H.264 High, AAC 48 kHz stereo, sound levelled to -14 LUFS with true peak <= -1 dBTP in the file (the project and preview keep their levels); needs a 9:16 canvas, resolution/fps may be omitted or must be 1080/30. Without preset resolution (the SHORT side in pixels, 1080 gives 1080x1920 on a portrait canvas) and fps=1..240 are required. quality high/recommended/small, default recommended. path must be new; relative paths resolve beside the project. Requires --allow-write. Job phases: measuring_loudness (preset only), exporting. Poll job(get) until done before reporting success.",
        )?,
        tool::<params::SuggestOptions>(
            "suggest_options",
            "Only when you run as Nuzky's AI panel (your instructions say so): offer the user 2-6 choices when a decision is theirs to make, e.g. which take to keep, cut tight or loose, which caption style. They show as buttons under your message, and the label picked arrives as the user's next message. Labels are short and distinct, at most 80 characters, in the user's language; detail is one optional line on what each choice does. Call it as the last thing in your turn, end the turn right after, and do not repeat the options in text. Anywhere else, such as a terminal, nobody sees the buttons: ask in plain text instead. Never offer choices for something you can decide from the guide or the user's style.",
        )?,
    ])
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().enable_prompts().build())
            .with_server_info(Implementation::new("nuzky", env!("CARGO_PKG_VERSION")))
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
            Resource::new("nuzky://guide", "Editing guide").with_mime_type("text/markdown"),
            Resource::new("nuzky://schema", "Project JSON schema").with_mime_type("application/schema+json"),
        ];
        if style().is_some() {
            resources
                .push(Resource::new("nuzky://style", "This creator's editing style").with_mime_type("text/markdown"));
        }
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let text = match request.uri.as_str() {
            "nuzky://guide" => guide(),
            "nuzky://schema" => schema_for!(nuzky_engine::Project).to_value().to_string(),
            "nuzky://style" => style().ok_or_else(|| {
                ErrorData::invalid_params("No EDIT.md yet: nuzky style learn writes one from finished cuts", None)
            })?,
            _ => return Err(ErrorData::invalid_params("Unknown Nuzky resource", None)),
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
