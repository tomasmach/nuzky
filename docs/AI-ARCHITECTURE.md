# AI architecture

CapOpen is built around AI the user brings: any MCP-capable agent can edit a project, live in the open app, and the app's own AI panel runs the user's installed agent under their subscription. Research behind these decisions: [docs/research](research).

Decided on 6 Oct 2026 after two independent proposals and a cross-critique.

## Principles

- **One editing authority.** Every change, from the UI, an agent or the CLI, goes through `ProjectSession` and the existing `EditCmd` validation. Agents never write the project JSON directly.
- **Live, reversible runs.** An agent's changes appear in the app as they happen. A whole run is one undo step. Review is available but never required to finish a run.
- **Deterministic primitives, creative agents.** Timing-critical work (ripple cuts, caption timing, transcripts) is done by engine tools; the agent decides what to do and checks rendered frames.
- **Local first.** Editing, analysis, transcription and export run on the machine. A cloud model only sees what CapOpen sends it: prompts, tool results, transcripts and frames.

## Processes

```
Claude Code / Codex / Gemini / Cursor ──stdio──▶ capopen mcp --project P ──IPC──▶ CapOpen app (ProjectSession for P)
                                                         │
                                                         └─ no app running: hosts ProjectSession for P itself
```

- `crates/session` holds `ProjectSession`: editor and history, revision, session epoch, the open run, the single write queue (replaces today's autosave) and crash recovery. It has no Tauri dependency; the app and the bridge both use it.
- `crates/mcp` is the stdio MCP server (`capopen mcp --project <path>`). If the app owns the project it forwards every tool call to the app over IPC; otherwise it hosts the session in process. Headless writes need `--allow-write`. A second headless bridge for the same project gets `PROJECT_BUSY`.
- IPC: a Unix domain socket (`$XDG_RUNTIME_DIR/capopen/<project-hash>.sock`, owner-only) or a Windows named pipe restricted to the current user. Versioned JSON-RPC; the bridge authenticates with a random token from an owner-only file next to the socket. Nothing listens on the network.
- Ownership of a project is an OS lock on `<project>.lock`. The app opening a project held by a headless bridge says so and opens it read-only.

## Runs

A run groups one agent turn.

- `begin_run(label)` records a checkpoint (the project before the run) in memory and in `<project>.checkpoint.json`, and returns `run_id`. Mutations are only accepted from the run that owns the session; manual edits in the UI are blocked with "AI is editing · Stop".
- `apply_edits(run_id, request_id, edits[])` applies a batch atomically to the live project: all or nothing. A repeated `request_id` with the same content returns the stored result; with different content it is an error. The result lists created, changed and removed clip ids and the real times after magnetic repacking.
- `end_run(run_id, keep | discard)`: keep pushes the checkpoint as one undo entry; discard restores it. A run also ends with keep when its client disconnects or after 2 minutes without calls, so a crashed agent never leaves the editor locked.
- Stop in the UI revokes the run first (late calls are rejected), cancels its jobs, then ends it with keep. The user can still press Undo.
- `undo_run(run_id)` works only while that run is the last history entry.
- Every result carries `revision` and `session_epoch`; mutations may pass `expected_revision` to fail fast on stale state.
- Exports started during a run render an immutable snapshot taken at the call and report its revision.
- On open, a leftover checkpoint file means a run did not finish: the app offers to restore the state before it.

## Tools

All times are integer microseconds on the timeline unless a field says `source`. Ranges are `[start, end)`.

| Tool | Purpose |
|---|---|
| `get_state(range?, clip_ids?)` | Project, tracks, clips with ids and times, selection, playhead, revision, open run |
| `begin_run(label)` / `end_run(run_id, action)` / `undo_run(run_id)` | Run lifecycle |
| `apply_edits(run_id, request_id, edits[], expected_revision?)` | Atomic batch of `EditCmd` |
| `import_media(run_id, paths[])` | Probe allowed files and add them as assets |
| `inspect_frames(times[], width?)` | Rendered frames as one image (contact sheet with timestamps); fails if media is missing |
| `analyze(kind, target, params)` | `silences`, `loudness`, `scenes`, `fillers` from `crates/analysis` |
| `transcribe(target, language, model)` | Job producing a transcript with word timestamps, bound to the revision it was made from |
| `build_captions(run_id, transcript_id, style, words_per_caption)` | Deterministic caption clips on one captions track |
| `export_video(path, resolution, fps, quality)` | Job exporting a snapshot |
| `job(job_id, get | cancel)` | Progress, result, cancel |

New edit commands: `RippleDeleteRanges { ranges, keep_track_ids }` cuts the ranges out of every track except the kept ones and closes the gaps, so video, overlays, audio and captions stay in sync. `AddCaptions` takes an optional target track and never deletes other tracks.

Resources: `capopen://guide` (the editing skill), `capopen://schema` (project JSON schema). Prompt: `edit_selected(goal)`.

## Agent setup

- The app ships `skills/capopen-edit/SKILL.md` and an `AGENTS.md` template: units, magnetic main track, ripple behaviour, analyse → edit → inspect frames → export, and never editing the JSON directly.
- Settings → Connect your agent shows the exact config change for Claude Code (`.mcp.json`), Codex (`config.toml`), Gemini CLI, Cursor and Claude Desktop, writes only the `capopen` entry after confirmation and keeps a backup.
- Diagnostics check, in order: executable found, config parsed, MCP handshake, project attached, `get_state` and one `inspect_frames` call.

## In-app AI panel

- v1 runs the user's installed agent over ACP: Codex (`codex-acp`), Claude Code (the official ACP adapter around the unmodified Claude Code, logged in by Claude Code itself) and Gemini CLI (`gemini --acp`). CapOpen passes its own MCP server for the session and disables the agent's shell and file-writing tools; each adapter must prove that in a test.
- The ACP client lives in Rust (`crates/agent`); React renders normalised events: message, tool started, tool finished, job progress, error.
- A prompt automatically attaches the selection, playhead, selected range and optionally a frame; the attachment is shown before sending and frozen at send time.
- States the panel distinguishes: agent not installed, not logged in, usage limit reached, CapOpen error. The typed prompt is never lost.
- Later: "Continue with ChatGPT" directly through OpenAI's open-source sign-in, then API keys, OpenRouter and Ollama.

## Phases

1. Foundation: `crates/session` (runs, revisions, write queue, recovery, validation) and the new edit commands; the app switches to it.
2. MCP: `crates/mcp` with IPC to the app and headless mode; analysis, transcription, captions and frame tools; skill and Connect flow. Gate: Claude Code in a project folder cuts silences, adds captions and exports, live in the open app, with one Undo.
3. Panel: `crates/agent` (ACP) and the React panel. Gate: the same task from the panel through Codex and Claude Code, including Stop and Undo.
4. Direct ChatGPT sign-in and other providers.

## Risks

- Retiming after cuts is the hardest correctness problem; it needs fixtures with speed changes, detached audio, keyframes, transitions and captions.
- Provider adapters and subscription terms change; keep them behind `crates/agent` and pin tested versions.
- ACP is not a sandbox; restrictions on the agent's own tools must be verified per adapter.
