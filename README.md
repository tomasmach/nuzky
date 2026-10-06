<img src="assets/icon.svg" width="64" height="64" alt="">

# CapOpen

An open-source desktop video editor in the spirit of CapCut, with a native Rust engine. Free, GPLv3, runs locally.

**Status: prototype.** Editing, playback, captions and export work on Linux. macOS and Windows builds are not tested yet.

## What it does

- Vertical-first canvas (9:16) with 16:9, 1:1 and 4:5 formats
- Import video, audio and images, including rotated and variable-frame-rate phone footage (HEVC/H.264)
- Magnetic main track, overlay video tracks, audio tracks and text tracks
- Move, trim, split and delete with snapping, undo and redo
- Text clips with outline, background box, size, colour, position, rotation and opacity
- Auto captions with Whisper running on your computer, with voice detection so music and silence stay uncaptioned
- Real-time preview with sound; the sound card clock keeps picture and audio in sync
- MP4 export (H.264 + AAC) using the same renderer as the preview, so the export matches what you saw
- Automatic saving after every edit

## Architecture

```
crates/engine   project model, edits + undo, FFmpeg decoding, wgpu compositor,
                text rendering, audio mixing, MP4 export
crates/analysis local speech recognition, caption grouping, silence and scene analysis
crates/session  editing authority, undo runs, autosave, crash recovery and transcripts
crates/mcp      agent tools, stdio bridge and live-app IPC
crates/cli      `capopen` headless CLI and MCP entry point
src-tauri       desktop shell: preview thread, audio output, background jobs, session events
src             React + TypeScript UI
```

- The engine renders every frame offscreen with wgpu. The preview streams those frames to the webview over a loopback WebSocket that only accepts the app's own origin and a per-launch secret. Export reuses the same renderer at full resolution.
- Each video clip decodes on its own thread with exact seeking on real timestamps, so cuts and variable frame rates stay frame accurate.
- Audio of each file is decoded once into a 48 kHz cache. Playback, waveforms, export and captions all mix from it.
- The Rust side owns the project. The UI sends edit commands and receives the new project back.

More detail: [docs/INTERACTION.md](docs/INTERACTION.md) (behaviour), [DESIGN.md](DESIGN.md) (visual tokens).

## Build and run

Use current stable Rust (at least 1.90), Node 22.12+, FFmpeg shared libraries with headers, clang (for bindgen) and cmake (for whisper.cpp). Linux also needs ALSA headers and WebKitGTK 4.1. CI builds Linux x86_64, macOS Apple Silicon and Windows x64. See [docs/BUILDING.md](docs/BUILDING.md) for exact dependencies, installers, runtime libraries and release limitations.

Fedora / Nobara:

```sh
sudo dnf install ffmpeg-free-devel alsa-lib-devel clang cmake webkit2gtk4.1-devel vulkan-loader-devel vulkan-headers glslc
```

Without root on Fedora/Nobara, `bash scripts/setup-linux-deps.sh` downloads the FFmpeg and ALSA headers into `~/.cache/capopen/deps` and writes a local `.cargo/config.toml`. The corresponding runtime libraries and WebKitGTK must already be installed. Install cmake with `pip install --user cmake` and put `~/.local/bin` on `PATH`.

```sh
npm install
npm run tauri dev
```

For macOS and Windows, install the platform dependencies in [BUILDING.md](docs/BUILDING.md) before running the same commands. CI artifacts contain installers; pushing a matching `v*` version tag prepares a draft GitHub Release. Builds are unsigned previews until verified on each target OS.

## CLI

```sh
cargo run -p capopen-cli -- probe clip.mov
cargo run -p capopen-cli -- new project.capopen a.mp4 b.mov
cargo run -p capopen-cli -- frame project.capopen 2.5 frame.png 540
cargo run -p capopen-cli -- render project.capopen out.mp4
```

Projects are JSON files, so the same project renders identically in the app and on a server.

## Tests

```sh
cargo test --workspace
npm run typecheck
```

## License

GPL-3.0-or-later. FFmpeg with x264 is GPL and compatible with this licence. The export uses FFmpeg's native AAC encoder because `libfdk_aac` is not GPL compatible.
