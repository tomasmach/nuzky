# AI architecture

CapOpen is built around AI the user brings: any MCP-capable agent can edit a project, live in the open app, and a planned in-app AI panel will run the user's installed agent under their subscription. The panel and `crates/agent` are not built yet; Connect agent covers Claude Code and Codex. Research behind these decisions: [docs/research](research).

Decided on 6 Oct 2026 after two independent proposals and a cross-critique, and refined the same day after the headless MCP landed (two more proposals and a cross-critique).

## Principles

- **One editing authority.** Every change, from the UI, an agent or the CLI, goes through `ProjectSession` and the existing `EditCmd` validation. Agents never write the project JSON directly.
- **Live, reversible runs.** An agent's changes appear in the app as they happen. A whole run is one undo step. Review is available but never required to finish a run.
- **Deterministic primitives, creative agents.** Timing-critical work (ripple cuts, caption timing, transcripts) is done by engine tools; the agent decides what to do and checks rendered frames.
- **Local first.** Editing, analysis, transcription and export run on the machine. A cloud model only sees what CapOpen sends it: prompts, tool results, transcripts and frames.

## Processes

```
Claude Code / Codex / Gemini / Cursor ──stdio──▶ capopen-app mcp --project P ──IPC──▶ CapOpen app (Host for P)
                                                         │
                                                         └─ no app running: its own Host for P
```

- `crates/session` holds `Host`: the `ProjectSession`, the one job system and the transcript store. The app owns one shared Host; each connected MCP client has a separate access level and run ownership. A headless bridge creates its own Host. The desktop export/captions/transcript jobs still use `src-tauri/src/jobs.rs`; migrating those jobs is separate work. `ProjectSession` owns the live `Editor` and its history, revision, session epoch, the open run, the writer thread and crash recovery. It has no Tauri dependency.
- The bridge is `capopen-app mcp --project <path> [--allow-write]`, handled before Tauri starts, so agents run the installed app's own version; `capopen mcp` is the same code for development. The bridge serves the MCP catalog, guide and schema itself and sends each tool call to the app, or to its own Host when no app owns the project.
- IPC: a Unix domain socket (`$XDG_RUNTIME_DIR/capopen/<project-hash>.sock`, directory 0700, files 0600) on Unix. Windows currently always uses a headless Host; a user-restricted named pipe is planned. Newline-delimited JSON: a hello `{capopen: 1, token, client, access}` answered with the session epoch, then `{id, tool, args}` → `{id, result}`. The 32-byte random token is hex-encoded in `<project-hash>.token` next to the socket and rewritten on every open. The directory is 0700 and both files are 0600. Without XDG_RUNTIME_DIR, the directory is `<temp>/capopen-<uid>`. Nothing listens on the network.
- Access is per client: a read-only client never takes the lock and mutating tools return `READ_ONLY`.
- Ownership is an OS lock on `<project>.lock`. A writable headless bridge currently holds it for its lifetime. The bridge chooses app IPC or headless once at startup. If an attached app closes or switches project, later tool calls return `APP_CLOSED` and require restarting the agent's MCP server. Releasing an idle headless lock and retrying read-only app opens are planned.
- The session reports changes as events on a channel, never calling out while holding its lock. The app forwards them to the preview and the frontend, which ignores snapshots older than the one it has within the same session epoch. The revision changes only when the project does.

## Runs and undo

A run groups one agent turn. There is one linear history: a run is one entry in it, between the user's own edits.

- `begin_run(label)` seals the history, writes `<project>.checkpoint.json` and returns `run_id`. Mutations are only accepted from the run that owns the session. Manual edits, Undo and Redo in the UI are blocked with "AI is editing · Stop"; the toast offers "Stop and edit", so stopping the AI is always deliberate.
- `apply_edits(run_id, request_id, edits[])` applies a batch atomically to the live project under the run's coalesce key `run:<id>`, so the whole run merges into one undo entry. A repeated `request_id` with the same content returns the stored result; with different content it is an error. The result lists created, changed and removed clip ids and the real times after magnetic repacking, and is returned only once the project is on disk.
- `end_run(run_id, keep | discard)`: keep seals the entry; discard drops it without a redo entry. A run also ends with keep when its client disconnects or after 2 minutes without calls from that client; reads by the UI or other clients do not keep it open.
- Stop in the UI revokes the run first (late calls get `RUN_STOPPED`), cancels its jobs, then ends it with keep. Ctrl+Z then undoes the whole run and Ctrl+Shift+Z brings it back.
- `undo_run(run_id)` works only while that run is the last history entry.
- Every result carries `revision` and `session_epoch`; mutations may pass `expected_revision` to fail fast on stale state.
- User edits are saved shortly after they stop; run batches are saved before they are acknowledged.
- On open, a leftover checkpoint means a run did not finish: the app asks to keep the AI changes or restore the version before, as an undoable edit. Headless mode refuses writes until that is resolved.
- Agents never set the user's selection.

## Transcripts

- A transcript belongs to a media file, not to a timeline: words with times in the file's own (source) time, stored once in `<data dir>/capopen/transcripts/<fingerprint>.json`. The fingerprint hashes the file size, its first and last MiB, and 32 evenly spaced 64 KiB chunks. The same file in several projects is recognised once and a moved file keeps its transcript. Version 2 records also store duration_us; older records and duration differences over 1 ms are treated as missing.
- Whisper's word times are estimates; next to a pause it often stretches a word over the silence into the next one (100–300 ms in Czech recordings). Right after recognition, `capopen_analysis::align_words` moves each boundary between two words into the quiet stretch of the sound nearest to it (a pause wins over a consonant's closure; without one, the quietest moment within 60 ms), and the record stores those times. Cut margins, pauses and captions all count from them.
- What the timeline says is always derived: `engine::speech::map_words` maps the words of every heard clip (unmuted track, volume above 0, a video asset with sound, detached sound included) through its source range and speed. Cuts, slivers, speed, undo and redo are therefore always right, and a transcript never goes out of date; the only note left is "N clips not transcribed".
- `get_transcript` and `edit_transcript` use `transcript_key`, which includes recognised words as well as their timeline layout. `get_state` exposes `speech_layout_key`; `apply_edits` accepts it as `expected_speech_layout_key`, a hash of exactly what `map_words` reads. The session rejects it with `SPEECH_CHANGED` when the speech moved in the meantime, and accepts it after unrelated edits such as a caption restyle.
- Tracks have `keep_in_place`; `RippleDeleteRanges` without `keep_track_ids` uses it, so agents cut the way the UI does and leave music alone.

## Tools

All times are integer microseconds on the timeline unless a field says `source`. Ranges are `[start, end)`.

| Tool | Purpose |
|---|---|
| `get_state(range?, clip_ids?)` | Project, tracks, clips with ids and times, the user's selection and playhead, revision, open run |
| `begin_run(label)` / `end_run(run_id, action)` / `undo_run(run_id)` | Run lifecycle |
| `apply_edits(run_id, request_id, edits[], expected_revision?)` | Atomic batch of `EditCmd` |
| `import_media(run_id, paths[], request_id?)` | Probe local files and add them as assets; a repeated `request_id` never adds them twice |
| `inspect_frames(times[], width?)` | Rendered frames as one image (contact sheet with timestamps); fails if media is missing |
| `analyze(kind, asset_id?, params)` | `silences`, `loudness`, `scenes`, `fillers` of one asset as a job, from `crates/analysis`. `retakes` reads the stored words of the whole timeline and answers at once, also to read-only clients: groups of attempts at one restarted sentence with the last complete one to keep, fillers that start a sentence, near-repeats that differ in a negation or number for review, and `suggested_delete` word ranges for `edit_transcript`. It fails with `TRANSCRIPT_MISSING` while a heard clip lacks words |
| `transcribe(asset_ids?)` | Job recognising the heard media that has no transcript yet |
| `get_transcript(range?)` | Numbered timeline words and sentences, `transcript_key`, untranscribed clips |
| `edit_transcript(run_id?, transcript_key, keep or delete word ranges, dry_run?)` | Cuts by word numbers with tight padding and shortened pauses; returns the new duration and text. A dry run needs no open run |
| `build_captions(run_id, style?, max_words?, max_chars?)` | Deterministic caption clips on one captions track, never across a cut; the Reel style by default |
| `export_video(path, preset? or resolution + fps, quality?)` | Job exporting a snapshot; `preset: "reels"` writes 1080x1920 at 30 fps with the sound levelled to -14 LUFS, true peak at most -1 dBTP |
| `job(job_id, get | cancel)` | Progress, result, cancel |

`RippleDeleteRanges { ranges, keep_track_ids? }` cuts the ranges out of every track except the kept ones and closes the gaps, so video, overlays, audio and captions stay in sync. `AddCaptions` never deletes other tracks.

Resources: `capopen://guide` (the editing skill), `capopen://schema` (project JSON schema) and, when the creator has one, `capopen://style` (their `EDIT.md`). Prompt: `edit_selected(goal)`.

## Creator style

- `capopen style learn <recording> <cut>...` turns pairs of a raw recording and the creator's finished cut into `<data dir>/capopen/EDIT.md`: what gets cut (restarted sentences, repeats, slips, fillers, dropped passages, calls to action), pauses, pace, cuts, captions and zoom, each rule in numbers and shown on at least three moments of the recordings. Rarer things are listed as seen, not as rules. The same input always gives the same file, and an existing file is only replaced with `--replace`, since the creator may have edited it.
- Where the cut came from is found by sound (`crates/analysis/src/style/align.rs`): the spectral shape of 20 ms windows, which a clip's own volume does not change, matched by a Viterbi path over candidate places in the recording. Retakes repeat the same words, so text alone cannot place a cut. A recording word counts as kept when its middle lies in a kept piece, or when the cut's own recognition heard the same word within a second of it: recognition times can be that far off next to pauses, where cuts are.
- Captions are found in the cut's picture as white pixels next to black ones, in the band where they are most common, and their words are counted from the gaps between them. Zoom and framing compare each moment of the cut with the same moment of the recording; scale, x and y are in CapOpen transform terms.
- The MCP server reads `EDIT.md` on each request. With it, the instructions start with a pointer to the style, and the instructions, `capopen://guide` and `edit_selected` end with it; `capopen://style` serves it alone. Its rules override the guide's defaults. Without it, everything is exactly as before.
- `capopen style compare <recording> <cut> <project>` scores a project by the recording's words: recall is the share of the creator's kept words the project plays, precision the share of the project's words the creator kept. It also lists the passages that differ.
- Recordings, transcripts and `EDIT.md` stay on the computer. An agent connected through MCP reads `EDIT.md`, which quotes the recordings, the same way it reads transcripts.

## Agent setup

- The app ships `skills/capopen-edit/SKILL.md` and an `AGENTS.md` template: units, magnetic main track, ripple behaviour, analyse → edit → inspect frames → export, and never editing the JSON directly.
- Connect agent (top bar) writes one `capopen` MCP server into Claude Code's user settings (`~/.claude.json`, or `$CLAUDE_CONFIG_DIR/.claude.json`, under `mcpServers`) and Codex's (`~/.codex/config.toml` or `$CODEX_HOME/config.toml`, as `[mcp_servers.capopen]`): this app's executable (the AppImage itself when it runs from one) with `mcp --current --allow-write`. The file is copied to `<file>.capopen-backup-<unix time>` first and replaced in one step; a symlinked config is written where the link points, every other key keeps its exact text (Codex comments included), an entry that already runs this app is left alone, and a file that does not parse is not touched. Code: `src-tauri/src/connect.rs`.
- `--current` attaches to the project open in the app: each app listener writes the open project's path to `current` next to its socket (0600 in the 0700 endpoint directory) and removes it when the project closes. With no project open the bridge refuses to start (`APP_NOT_RUNNING`) instead of editing headless, so one configuration serves every project; the agent's MCP server is restarted after switching projects.
- Planned: the same for Gemini CLI, Cursor and Claude Desktop.
- Planned diagnostics will check, in order: executable found, config parsed, MCP handshake, project attached, `get_state` and one `inspect_frames` call.

## In-app AI panel (planned, not built)

- Planned v1 will run the user's installed agent over ACP: Codex (`codex-acp`), Claude Code (the official ACP adapter around the unmodified Claude Code, logged in by Claude Code itself) and Gemini CLI (`gemini --acp`). CapOpen passes its own MCP server for the session and disables the agent's shell and file-writing tools; each adapter must prove that in a test.
- The planned ACP client will live in Rust (`crates/agent`, not created yet); React will render normalised events: message, tool started, tool finished, job progress, error.
- A prompt automatically attaches the selection, playhead, selected range and optionally a frame; the attachment is shown before sending and frozen at send time.
- States the panel distinguishes: agent not installed, not logged in, usage limit reached, CapOpen error. The typed prompt is never lost.
- Later: "Continue with ChatGPT" directly through OpenAI's open-source sign-in, then API keys, OpenRouter and Ollama.

## Phases

1. Headless foundation (done): `crates/session` and `crates/mcp` hosting their own session.
2. One authority: keyed undo steps in the engine, `keep_in_place`, `engine::speech`; `ProjectSession` on the live `Editor` with user edits, events, the writer thread and per-client access; the app on `Host` with its lock, read-only state and recovery dialog; one job system.
3. Transcripts per media file, `get_transcript`, `edit_transcript` and captions from the derived words, in the app and in MCP; the frontend's own transcript bookkeeping goes away.
4. IPC and the `capopen-app mcp` bridge, then the run UI. Gate: Claude Code in a project folder cuts silences, adds captions and exports, live in the open app, with one Undo and a Stop mid-run.
5. Panel (planned, not built): `crates/agent` (ACP) and the React panel. Gate: the same task from the panel through Codex and Claude Code, including Stop and Undo.
6. Direct ChatGPT sign-in and other providers.

## Risks

- Retiming after cuts is the hardest correctness problem; `map_words` needs fixtures with speed changes, detached audio, slivers, kept music, transitions and duplicated clips.
- Recognising a whole long recording once costs time up front; if that hurts, store which ranges a transcript covers.
- Blocking manual edits for a whole run may feel heavy; revisit after trying it by hand.
- On Windows the app is a GUI-subsystem executable; stdio for `capopen-app mcp` must be tested there.
- Provider adapters and subscription terms change; keep them behind `crates/agent` and pin tested versions.
- ACP is not a sandbox; restrictions on the agent's own tools must be verified per adapter.
