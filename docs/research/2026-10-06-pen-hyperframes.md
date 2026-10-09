# Research: how pen.dev and HyperFrames integrate AI (6 Oct 2026)

Desk research by GPT-6.1 Sol from public docs and source; neither product was run. Claims as reported with sources.

## Lessons for Nuzky

- Keep the `.nuzky` JSON as the shared document; publish a versioned schema, examples, microsecond timing rules and asset path conventions.
- One editing backend with CLI and MCP entry points: `get_state`, `apply_edits`, `analyze_audio`, `transcribe`, `frame`, `render`, job status and cancel, all through existing `EditCmd` validation. A stdio MCP bridge talks to the running app when it is open and works headless otherwise (the pen.dev pattern).
- Ship the workflows, not just access: speech/silence intervals and word transcripts so "cut silences and add captions" works end to end; today the CLI exposes neither.
- Agent changes are visible and reversible: one undoable transaction per request, affected regions shown, a checkpoint kept. External JSON writes need validation, revision checks and coordinated reload so autosave never overwrites them.
- Installation and instructions ship together: "Connect Claude Code / Codex", connection diagnostics, copyable config, a project `AGENTS.md`/`CLAUDE.md` and an editing skill teaching analyse → edit → validate → inspect frames → export.
- Visual and selection context: frame images and contact sheets as MCP image results; `get_state` includes selected clips, playhead and selected range.
- In-app chat drives the user's local agent (Claude Code, Codex) with their existing login; show provider, connection state, progress, Stop, review and Undo; attach the selection automatically; keep the prompt on login failures and tell subscription limits apart from app errors.

## pen.dev (formerly Pencil)

Proprietary design canvas by High Agency (EULA, free until plans launch; planned tiers meter external agent use too). Desktop on macOS, Windows, Linux plus IDE extensions.

- External agents: local MCP over stdio; the server reaches the running editor through a Unix socket or Windows named pipe. Settings → MCP writes the client config entry for Claude Code, Codex, Gemini CLI, Claude Desktop and others ([docs](https://docs.pen.dev/getting-started/ai-integration)).
- In-app AI: reuse Claude Code/Codex settings, sign in with Claude or ChatGPT plans, or API keys; the CLI depends on the Claude Agent SDK and Codex SDK ([auth](https://docs.pen.dev/getting-started/authentication)).
- Document: `.pen` is JSON with a published schema, but edits are meant to go through tools: `get_app_state`, `read_skill`, `get_style`, `execute` (which can also take screenshots). Connected edits appear live ([format](https://docs.pen.dev/for-developers/the-pen-format)).
- Good: automatic selection context, reference images, visible tool activity, model/effort choice, Stop, Undo, side-by-side alternatives. Weak: account required, metering of your own agent, no request-wide rollback guarantee.

## HyperFrames (HeyGen)

Apache-2.0 HTML-based video framework: compositions are HTML/CSS/media with seekable JS animation; headless Chrome captures frames and FFmpeg encodes ([repo](https://github.com/heygen-com/hyperframes)). Studio is React/TS/Vite.

- External coding agents: install skills (`npx skills add heygen-com/hyperframes`) or plugins; Claude Code, Codex, Cursor, Gemini CLI write the project files and call the CLI with their own auth; no local MCP needed. Repo ships `AGENTS.md` and task skills for captions, motion, audio and verification.
- Desktop chat runs the local Claude Code or Codex under the user's account (guided Claude sign-in in app; Codex needs terminal steps).
- Separate hosted MCP behind HeyGen OAuth for Claude, ChatGPT and Grok connectors; renders on HeyGen infrastructure.
- Live state: agent, CLI and Studio edit the same files; preview hot-reloads; Studio detects conflicting external writes; `preview --context --json` returns selection, time, source location and a thumbnail; skills require timestamped snapshots for visual checks.
- Good: select-and-prompt, drawings and comments pinned to moments, batched pending edits, queue/steer, Undo per agent run. Weak: HeyGen login for desktop requests, clunky Codex onboarding, unrestricted Claude access by default.

## Comparison

| Pattern | pen.dev | HyperFrames |
|---|---|---|
| Editable source | JSON, edits via tools | HTML/CSS/JS, direct authoring |
| External AI | Local stdio MCP into the editor | Skills + CLI; separate hosted MCP |
| In-app subscription AI | Claude/ChatGPT plans or keys | Local Claude Code/Codex accounts |
| Strongest lesson | Agent access to live editor state | Shared source, visual verification, per-run Undo |
| For Nuzky | MCP over Rust edit commands | Skills + CLI + coordinated live reload |
