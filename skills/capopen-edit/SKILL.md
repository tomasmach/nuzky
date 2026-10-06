---
name: capopen-edit
description: Edit local videos by words through CapOpen MCP, then build safe-area captions, inspect frames and export.
---

# Edit with CapOpen

Use MCP for every project change. Never write project JSON or overwrite source media. Media, recognition and export stay local; transcripts and inspected frames are visible to your model.

## Cut slips and pauses, then caption

1. `get_state`: check the canvas, assets, duration, speech_key and any open run. If recovery_checkpoint is present, ask whether to keep current edits or restore, then `resolve_recovery` with that choice.
2. `transcribe` with no arguments recognises all heard files lacking stored words. Language defaults to auto and model to the best installed. Poll `job` with pauses until done. Explicit asset_ids force recognition again; avoid this unless needed. Transcribe before opening a run.
3. `get_transcript`: read ALL sentences before choosing cuts. Words have zero-based `i`, timeline start_us/end_us, text and probability `p`. Sentences have inclusive `from`/`to` word indices; pauses show after_word and gap_us. Optional range_us filters without renumbering. Transcribe any untranscribed assets before continuing.
4. Mark retakes, slips, false starts, meaningless fillers and off-topic detours. Keep the LAST complete take when a sentence restarts. Preserve negations, the argument and its final point. Remove closing calls to action such as “napiš mi do komentáře…” unless the user asks for one. Do not invent a duration target or remove a whole topic just to reach one. Filler analysis is optional and reads stored words; dictionary matches need context.
5. `begin_run`, then `edit_transcript(run_id, speech_key, delete: [[from,to],...], dry_run: true)`; alternatively use keep ranges. Word ranges are INCLUSIVE. Use the key from get_transcript. The tool handles source mapping, tight cut padding, edge silence and internal pauses (default shorten_pauses_us=300000). Never calculate source-to-timeline offsets yourself. Check duration_us.before/after and preview_text. Apply the same request with dry_run=false and the ORIGINAL speech_key, not the predicted key from the preview. SPEECH_CHANGED means read the transcript and plan again. Omit delete/keep to shorten pauses only. One rippleDeleteRanges edit respects music tracks kept in place.
6. `build_captions(run_id)` uses current derived words and creates or replaces one Captions track. Default Reel style: size 95, white, regular, black stroke 7.5, no background, Inter; defaults max_words=2/max_chars=15. A single long word stays intact. Captions never span a clip cut; vertical canvases automatically use the safe wrap width. Prefer defaults unless asked otherwise.
7. `inspect_frames(times_us: [...], safe_area: true)` around cuts, caption changes and representative phrases. Shaded margins are outside the IG/TikTok safe area. Check words, readable captions, the face and the ending. Correct transcription mistakes without inventing speech. After manual caption edits inspect get_state.caption_stats; keep multiword captions within chosen limits. Never abbreviate speech just to fit. Do not add zoom unless asked.
8. `end_run(run_id, action: "keep")`, then `export_video(path: "reel.mp4", resolution: 1080, fps: 30, quality: "recommended")` unless the user specifies otherwise. The resolution is the short side. Poll until done and report the real output path, duration and any uncertainty. The output path must be new.

## Make it dynamic, only when asked

For “rozhýbej to, občas zoom, pomalý nájezd”, read the story first. A constant base zoom around 1.1 is optional. Use a 1.15–1.3 punch-in on key claims or punchlines; a slow push-in of 6–10% over one sentence when building up; a pull-out at a reveal or turn. Keep it subtle, not on every sentence. Use get_transcript timeline times to split at sentence boundaries, then updateClip transform or setKeyframes. Read capopen://schema for exact edit fields. A complete transform is {"x":0,"y":0,"scale":1.2,"rotation":0,"opacity":1}. Inspect frames with safe_area=true after changes.

## Runs and low-level edits

All timeline times are integer microseconds; time ranges are half-open [start,end). Word-index ranges above are inclusive. apply_edits takes camelCase EditCmd fields, a fresh request_id per batch, and optional expected_revision/expected_speech_key from get_state. edit_transcript instead uses get_transcript.speech_key, which also guards against re-recognition. Retry identical content with the same request_id after a save failure; the live edit may already be applied. The main track is magnetic. One run is one Undo; end_run(discard) restores its start. Runs auto-keep after two idle minutes, so plan before opening one. A stopped run returns RUN_STOPPED; never resume it. undo_run works only for the last history entry in this session. Never change the user's selection.
