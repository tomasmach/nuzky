# Nuzky

Nuzky ([nuzky.app](https://nuzky.app)) is an open-source desktop video editor in the spirit of CapCut (GPL-3.0-or-later). A Rust engine (`crates/engine`: project model, edits and undo, FFmpeg decoding, wgpu compositor, audio mixing, export), local Whisper (`crates/analysis`), local face and subject models (`crates/vision`), the editing authority (`crates/session`), tools for AI agents (`crates/mcp`, `crates/cli`), a Tauri 2 desktop shell (`src-tauri`) and a React 19 + zustand UI (`src`). Status: prototype. Linux is verified; macOS and Windows only compile so far.

People edit videos from their phones and often let an AI agent do the cutting for them. A change rarely belongs to one surface only: the UI, the engine, the MCP tools, the CLI and the export share the same project.

This file is for development. `DESIGN.md` describes how the app looks, `docs/INTERACTION.md` how it behaves. `skills/nuzky-edit/` is the guide for an agent that edits videos in Nuzky, not for developing Nuzky. Setting up a checkout and sending a pull request are in `CONTRIBUTING.md`.

## Language

Everything in the repository and on GitHub is in English: code, comments, docs, commit messages, branch names, pull request titles and descriptions, issues and review replies. This holds even when your tools or the conversation default to another language.

## What we never sacrifice

Whatever you break, you break in someone's unfinished video.

1. **The user's media and projects.** Never overwrite or delete source media. Export to an existing file happens only with explicit consent and through a temporary `.nuzky-part-*`. Projects are saved atomically, and no edit is lost after a crash or a closed window. A project saved by an older version always opens, so a new model field has `#[serde(default)]`. A cache format change bumps the version in the file name (`PCM_VERSION`), so old data is never read as new.
2. **What you see is what you export.** Preview and export draw with the same renderer. Cuts land on the frame by real timestamps (VFR and HEVC from phones), orientation and mirroring from metadata match in the preview, thumbnails and export, and sound stays in sync with the picture. Compare decoding and rendering with an FFmpeg reference, not with your own estimate. Selfies from the front camera (EXIF 2, 4, 5, 7) were once rotated instead of mirrored.
3. **Files from outside are untrusted.** A `.nuzky` project or a media file can come from anyone. FFmpeg opens inputs only through `media::open_input`, which allows only the `file` protocol; a review found that a shared project with an `http://…` path made FFmpeg download from a foreign server. An invalid project must not crash the preview or the app (a panic on the colour `#€`). Local IPC stays 0700/0600 with a token, and nothing listens on the network. The network is used only to download models with a pinned SHA-256 and for the update check, which once a day reads only the version number from a fixed GitHub address and downloads or runs nothing.
4. **An AI agent works under the user's control.** Every change from the UI, an agent or the CLI goes through `ProjectSession` and `EditCmd` validation. Nobody writes project JSON directly. An agent run is visible live and undoes as one step. During a run the UI shows a lock with the reason and a Stop button. Stopping an agent or a job takes a few seconds at most, even in the middle of Whisper or analysis.
5. **Calm and speed.** The UI never waits for decoding, analysis or export. Slow work runs as a job with a percentage and can be cancelled. Response budgets are in `docs/INTERACTION.md` (Response time). Playback does not re-render the whole app on every frame.

If a rule here fights your task, do not break it quietly. Say so.

## Four ways to hurt yourself

1. **The user's real data.** The app reads projects, recent projects and models from `$XDG_DATA_HOME/nuzky`, the audio cache from `$XDG_CACHE_HOME/nuzky`, and keeps its IPC socket in `$XDG_RUNTIME_DIR/nuzky`. Never run the app, the CLI or `nuzky mcp` with the user's real directories: it would open their last project or attach to their running app. `scripts/repro.py` isolates everything itself; isolate a manual run the same way. On macOS the app ignores `XDG_DATA_HOME` and `XDG_CACHE_HOME` and reads `~/Library`, so isolate it through `HOME`. `~/.cache/nuzky/deps` belongs to the build scripts; do not delete it.
2. **Disk and parallel builds.** Every `CARGO_TARGET_DIR` is a full build with FFmpeg, wgpu and whisper.cpp. Seventeen `target-*` directories once took 235 GB. Use the `target/` of your checkout. In a new worktree, `scripts/setup-linux-deps.sh` starts it as a copy-on-write clone of another worktree's build, so the first build compiles only the Nuzky crates. Never point two checkouts at one target or build directory: cargo names the Nuzky crates' builds the same in every checkout, so one would run the other's code. Create a separate target directory only for a parallel build and delete it afterwards. Do not run two full test suites at once (`scripts/check.sh` waits for another one), and keep large test files out of `/tmp`, which lives in memory.
3. **Windows and processes on the desktop.** The app opens real windows, native dialogs and notifications. Click through it with `scripts/repro.py`: headless gamescope and D-Bus turned off. Vite runs on the fixed port 1420 and WebKitWebDriver on 4444. A busy port means another session; do not kill it, wait. Stop only your own processes, by PID or process group.
4. **GitHub Actions.** While the repository is private, Actions minutes are capped and macOS counts ten times, so `checks.yml` runs on a pull request only with the `ci` label. A job that ends within seconds without a log did not run because of the budget. Tell the maintainer, and do not change code because of it.

## How it works

- Rust owns the project. The UI sends an `EditCmd` and gets a new snapshot back. Agents and the CLI go through the same `ProjectSession` (`crates/session`), which also holds autosave, crash recovery and agent runs.
- A thread in `src-tauri/src/engine.rs` draws the preview with wgpu (`crates/engine/src/render.rs`, `gpu.rs`) and sends frames over a loopback WebSocket with a secret and an origin check. Export uses the same renderer.
- The sound of each file is decoded once into a 48 kHz cache (`crates/engine/src/audio.rs`). Playback, export and captions all mix from it. The waveform reads peaks written beside the cache while it decodes, and the timeline loads them in blocks around what is on screen. The cache belongs to the file (path, size, mtime), not just the asset ID, because an agent can put a different file under the same ID.
- Long work runs as a job: export, captions, audio preparation and models in `src-tauri/src/jobs.rs`, agent work in `crates/session/src/jobs.rs`. Every job reports progress and responds to cancellation.
- The access of every MCP tool is in one table, `crates/mcp/src/rules.rs`. The agent guide `skills/nuzky-edit/SKILL.md` is also `nuzky://guide`. The AI architecture is in `docs/AI-ARCHITECTURE.md`.
- Error codes are message prefixes (`RUN_ACTIVE`, `CANCELLED`, `READ_ONLY`, `OUTPUT_EXISTS`…) that the frontend reads with `startsWith`. Do not rename a code without updating every place that reads it.

## Reach every surface

The most common defect is a change that works only on the path you tried.

- A new model field (`crates/engine/src/model.rs`): `#[serde(default)]`, its type in `src/lib/types.ts` (generated by `NUZKY_WRITE_TYPES=1 cargo test -p nuzky-app typescript_types`; `cargo test` fails while it is out of date), validation in `crates/session/src/validate.rs`, the MCP schema and struct literals in every crate. Check the preview, thumbnails (`src-tauri/src/thumbs.rs`), export and the CLI `frame`.
- A new `EditCmd`: validation, undo, the MCP schema and guide, the frontend (`src/lib/api.ts`, `src/lib/store.ts`) and the behaviour in `docs/INTERACTION.md`.
- A new MCP tool: a row in `rules.rs`, a description in `crates/mcp/src/lib.rs`, the guide in `SKILL.md` and a stdio test in `crates/cli/tests/mcp_stdio.rs`.
- Write a change of behaviour or keys into `docs/INTERACTION.md` in the same PR, and visuals into `DESIGN.md`.
- A new control respects the AI lock (`useLockReason`, `lockedProps` in `src/components/ui.tsx`), has a keyboard path and shows why it is disabled.
- New media reading goes through `media::open_input` and has a test on phone media (rotation, VFR, HEVC) against an FFmpeg reference.
- Platforms: Linux is verified; CI only compiles macOS and Windows. Do not claim anything works there.

## Verification

- Reproduce a bug in exactly the flow where it happened. When the first fix does not work, stop guessing and find the state that triggers it.
- End-to-end tests are the main tests: a flow in the real app (`tests/e2e/<area>.py`, run by `python3 scripts/repro.py <flow>`, `--list` shows the flows) and an integration test on the real binary and media (`crates/cli/tests/`, `crates/engine/tests/qa_*.rs`).
- A new feature or a fix of the UI, a flow or a bug is not done without an end-to-end scenario that would fail without it. Add a new flow, or a check to an existing one. The scenario checks what the user will see: project state, the saved file, preview or export pixels. That nothing crashed is not enough.
- A check must be able to fail. Break the guarded behaviour once, or run the check before the fix, and show that it fails.
- Write a unit test only for pure logic that end-to-end tests cannot cover cheaply (microsecond times, boundary calculations, parsing), or for a defect they would not catch. Do not add one after the implementation as decoration. First write down how the system can fail.
- Put a screenshot from `tmp-test/repro/<flow>/` and the command with the revision from `result.json` into the PR.
- Verify the engine on real files against an FFmpeg reference. `crates/engine/tests/qa_*.rs` generate their media with `ffmpeg`.
- Run Rust tests narrowly by name, but over the whole workspace: `cargo test --workspace <name>`, with `--lib` or `--test <file>` to start fewer binaries. `-p <crate>` resolves other features, so it builds most dependencies again, whisper.cpp and the FFmpeg bindings included, and `scripts/check.sh` then builds them once more. Tests marked `#[ignore]` need media and models. Prepare them with `scripts/fixtures.sh` and run `XDG_DATA_HOME=$PWD/tmp-test/xdg/data cargo test --workspace <name> -- --ignored`. Always run them when you change speech, analysis, thumbnails or decoding.
- Run the full gate `scripts/check.sh` before a merge to `main` (the pull request section describes the exception for the website) and before a release. It runs fmt, lint, clippy, types, both builds, all tests including the ignored ones, the check that Nuzky starts and finds faces without AVX2, the dependency audits and every repro flow. It takes tens of minutes, so run it in the background.
- Frontend before a push: `npm run lint` (hooks rules) and `npm run build` (types and the Vite build).
- Performance: measure first, then change, and give the numbers before and after.

## Pull requests and releases

- The base is `main`. Branch from a fresh `origin/main` and rebase before opening the PR. Fill in the pull request template: problem, solution, verification.
- Before a merge to `main`, `scripts/check.sh` passes on the exact commit being merged. The exception is a pull request that changes only the website in `site/` and no dependencies (`package.json`, `package-lock.json`): the gate does not check the website, so do not run it. `tsc --noEmit` and `next build` in `site/` and clicking through the affected pages are enough. On pull requests GitHub runs only the faster part (`.github/workflows/checks.yml`) and scans for secrets.
- The pre-push hook (`.githooks/pre-push`, turned on by `git config core.hooksPath .githooks`) scans for secrets and blocks video and audio files. Do not bypass it with `--no-verify`. Test media are generated and do not belong in git, because they could be someone's private videos.
- Release: the same version in `src-tauri/tauri.conf.json`, `Cargo.toml` and `package.json`, then the tag `v<version>`. The tag runs the build and tests on Linux, macOS and Windows and prepares a draft release. Only a person publishes it, after checking it on each system as `docs/BUILDING.md` describes.
- Check bot comments against the code: fix real findings, reject false ones with a reason.
- Clean up only after yourself. Several sessions share `tmp-test/`.

## Taste

- `DESIGN.md` and `docs/INTERACTION.md` are the law. Extend the components in `src/components/ui.tsx`; do not copy them.
- The look is CapCut's layout finished the way Apple finishes its apps: dark graphite panes, the video as the brightest thing on screen, colour only where it means something, and glass only on floating bars and short overlays. `DESIGN.md` describes the direction, materials, tokens, components, the performance budget of effects and the known quirks of WebKitGTK. Before changing a screen, read the part of it the change touches, write new values there in the same PR and check the result on a `scripts/repro.py` screenshot at 1440 × 900 and at the smallest window, 1024 × 640.
- The UI is in English. Copy says in plain words what will happen. An error says what to do next, without internal codes.
- Every mouse action also has a keyboard path.
- The licence is GPL-3.0-or-later. Do not add a dependency or an FFmpeg build that is incompatible with it (`libfdk_aac`, "nonfree" variants). `cargo deny` checks the licences of Rust dependencies.

## Code Review Rules

These apply to automated PR review (Codex). Find what the user would actually feel, and stop after a few rounds.

- Report P0 and P1 findings: lost or overwritten media or projects, a project that no longer opens after the change, a preview that differs from the export, a cut or sound that drifts, a crash or hang on ordinary phone media, an untrusted file that reaches the network or foreign files, an AI agent that bypasses the lock or undo, a job that cannot be stopped, and a failure the code swallows without telling the user.
- Always watch speed: work on the UI thread, re-rendering the whole app per frame, an extra decoder or allocation per frame, a whole media file in memory. When the user notices it as stutter or waiting, it is P1.
- Every finding needs a reachable scenario and an impact. A missing test is not a defect on its own.
- Do not report theoretical races without a realistic scenario, exotic inputs without a documented occurrence, style, naming, or anything beyond the scope of the PR. `std::fs::rename` on Windows overwrites an existing target; that is not a finding.
- In a repeated review, stick to the commits since the last round and do not return to resolved threads.
