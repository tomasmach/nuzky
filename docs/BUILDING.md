# Building and distributing Nuzky

Use current stable Rust (minimum 1.94, which `candle-core` needs for its NEON code on Apple Silicon), Node.js 22.12+ and the committed Cargo/npm lockfiles. Pull requests run lint, types, the app and website builds, clippy and `cargo test --workspace --locked` on `ubuntu-24.04` (`.github/workflows/checks.yml`): every pull request that is not a draft once the repository is public, and only pull requests with the `ci` label while it is private. A release tag runs the tests and a release Tauri build on `ubuntu-24.04` (x86_64), `macos-14` (arm64) and `windows-2022` (x64). No signing secrets are required. This build pipeline is not evidence that editing, playback or export has been tested on all three systems.

## Linux

Ubuntu 24.04:

```sh
bash scripts/setup-ubuntu-deps.sh
npm ci
npm run tauri dev
```

The script installs the packages CI uses, so its list is the one known to work.

Ubuntu 24.04 ships [FFmpeg 6.1](https://packages.ubuntu.com/noble/libavcodec-dev). `ffmpeg-next 9.0.0` supports older FFmpeg releases, including 6.1, through build-time version detection; its crate version does not require FFmpeg 9. CI deliberately uses distribution packages here. [Upstream build notes](https://github.com/zmwangx/rust-ffmpeg/wiki/Notes-on-building) describe the native-library requirement. Local Fedora/Nobara development uses FFmpeg 8. Both routes must keep headers and runtime SONAMEs matched.

Fedora/Nobara with root:

```sh
sudo dnf install ffmpeg-free-devel alsa-lib-devel clang cmake webkit2gtk4.1-devel vulkan-loader-devel vulkan-headers glslc
npm install
npm run tauri dev
```

Without root, when FFmpeg, ALSA and WebKitGTK runtimes already exist:

```sh
bash scripts/setup-linux-deps.sh
pip install --user cmake
export PATH="$HOME/.local/bin:$PATH"
npm install
npm run tauri dev
```

The setup script extracts RPM headers and, if neither system Vulkan development packages nor `VULKAN_SDK` are available, downloads the SHA-256-pinned LunarG SDK 1.4.363.0 into `~/.cache/nuzky/deps/vulkan-sdk` (override the dependency root with `NUZKY_DEPS`). This download supports x86_64 Linux. It links against the existing system `libvulkan.so.1`; install a working Vulkan driver separately. It generates ignored, machine-specific `.cargo/config.toml` with `VULKAN_SDK`, SDK tools on `PATH`, and `LIBRARY_PATH` for linking. Rerun setup after changing your shell paths. No sudo or replacement of system libraries is involved. Do not copy that config onto another machine. Export requires an FFmpeg build with the `libx264` encoder; Fedora's available codecs depend on installed packages. Check `ffmpeg -hide_banner -encoders` for `libx264` and `aac`.

### Speech recognition on the GPU

On Linux, Whisper runs on the GPU through Vulkan and falls back to the CPU when no GPU context can be created. Building needs `glslc`, the Vulkan headers and the loader library (`libvulkan-dev glslc` on Ubuntu, `vulkan-loader-devel vulkan-headers glslc` on Fedora); without root, the setup script downloads a checksum-pinned LunarG SDK instead. The app uses Vulkan FP32 (`GGML_VK_DISABLE_F16=1`) because FP16 moved word times by up to 330 ms in our tests. On macOS, Whisper runs on the GPU through Metal, whose shaders whisper.cpp embeds in the binary. On an M4 Pro a 5:48 Czech recording took 45 s to recognise instead of 135 s on the CPU, with the same 522 words and word times within 35 ms. Windows still recognises speech on the CPU.

Build Ubuntu installers with:

```sh
node scripts/build-bundles.mjs
bash scripts/verify-linux-bundle.sh
```

This creates `target/release/bundle/appimage/*.AppImage` and `deb/*.deb`. The AppImage bundles FFmpeg and the native libraries collected by linuxdeploy. The `.deb` uses system FFmpeg, ALSA, GTK and WebKitGTK and declares the FFmpeg ABI package names from `pkg-config`. The script's Debian package names target Ubuntu 24.04; do not produce a distributable Ubuntu `.deb` on Fedora. Install the Ubuntu package with `sudo apt install ./Nuzky_*.deb` so apt resolves dependencies.

For a local Fedora/Nobara AppImage, or a separate build directory:

```sh
export PATH="$HOME/.local/bin:$PATH"
export CARGO_TARGET_DIR="$PWD/target-release"
APPIMAGE_EXTRACT_AND_RUN=1 npm run tauri build -- --bundles appimage
bash scripts/verify-linux-bundle.sh
```

`APPIMAGE_EXTRACT_AND_RUN=1` lets AppImage build tools run without FUSE. The verifier checks `ldd`, extracts the AppImage without launching the GUI, checks the embedded binary's libraries and asserts the five linked FFmpeg libraries are included. It is not a GUI startup test. A Nobara-built image inherits Nobara's glibc baseline and is not the release artifact for Ubuntu. Release images are built on Ubuntu 24.04; older distributions are not supported. Drivers/display services still come from the host.

## macOS, Apple Silicon

Install Xcode command-line tools (`xcode-select --install`), Rust and Node, then:

```sh
brew install ffmpeg@8 pkg-config cmake
export PKG_CONFIG_PATH="$(brew --prefix ffmpeg@8)/lib/pkgconfig"
export PATH="$(brew --prefix ffmpeg@8)/bin:$PATH"
npm ci
npm run tauri dev
```

Use the [versioned Homebrew formula](https://formulae.brew.sh/formula/ffmpeg@8) so a future `brew ffmpeg` upgrade does not silently change the FFmpeg major version. CI installs plain `ffmpeg`, FFmpeg 9.0 on 10 October 2026: the Homebrew metadata of the `macos-14` runner image predates `ffmpeg@8`. Current Homebrew has no bottles for macOS 14, so updating Homebrew on that runner would build FFmpeg and its dependencies from source. The engine tests in CI take `drawtext` from `ffmpeg-full`, as below. The [GitHub runner catalog](https://github.com/actions/runner-images) maps `macos-14` to arm64; CI also asserts `uname -m` is `arm64`.

If every C link fails with `ld: tapi error: malformed file … libSystem.tbd … unknown architecture arm64e.x1-macos`, the selected Xcode is older than the Command Line Tools SDK the compiler picks (Xcode 26 on macOS 27). Update Xcode, or give the toolchain its own SDK: `export SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"`. Switching to the Command Line Tools 27 instead builds in debug, but its `strip` with the macOS 27 SDK and the 14.0 deployment target writes proc-macro libraries that dyld rejects (`mis-aligned LINKEDIT string pool`), so release builds fail.

For tests and fixtures: `brew install espeak-ng` for `scripts/fixtures.sh`. The engine tests draw frame numbers with FFmpeg's `drawtext`, which Homebrew's `ffmpeg` and `ffmpeg@8` lack. `brew install ffmpeg-full` (keg-only) and put it first only for test runs: `PATH="$(brew --prefix ffmpeg-full)/bin:$PATH" cargo test --workspace`. Installing it upgrades shared Homebrew libraries such as x265; run `brew linkage --test` afterwards and `brew upgrade` any formula it reports, then rebuild Nuzky. On macOS the app keeps data in `~/Library/Application Support/nuzky` and cache in `~/Library/Caches/nuzky` regardless of `XDG_DATA_HOME`/`XDG_CACHE_HOME`; isolate a manual run with `HOME=<test dir>` plus `XDG_RUNTIME_DIR`. `scripts/check.sh` puts `ffmpeg-full` first and gives the tests their own home by itself.

```sh
node scripts/build-bundles.mjs
```

The script first builds `target/release/bundle/macos/Nuzky.app`. `bundle-macos-libs.py` walks the executable's `otool -L` dependencies recursively, copies non-system dylibs to `Contents/Frameworks`, rewrites references using `install_name_tool`, and ad-hoc signs the modified libraries and app. It rejects unresolved libraries and basename collisions. It checks `codesign --verify --deep --strict` and creates `dmg/Nuzky_<version>_aarch64.dmg`. Copy the app out of the mounted image into Applications. End users do not need Homebrew for these prepared bundles. A raw `npm run tauri build` does not run this relocation step and still requires the build machine's Homebrew libraries.

The deployment minimum is macOS 14. Ad-hoc signing is not Apple Developer ID signing or notarization. Gatekeeper can block downloaded preview builds; for a trusted download use System Settings → Privacy & Security → Open Anyway. Developer ID signing and notarization must be added before claiming frictionless public installation. Always sign after library relocation, never modify a signed/notarized release afterward.

## Windows x64

Install Rust with the MSVC toolchain, Node 22, Visual Studio 2022 Build Tools with **Desktop development with C++** and the Windows SDK. Install LLVM x64 and CMake if missing:

```powershell
winget install LLVM.LLVM
winget install Kitware.CMake
```

Open a fresh PowerShell terminal at the repository root:

```powershell
./scripts/setup-windows-deps.ps1
npm ci
cargo test --workspace --locked
npm run tauri dev
```

GitHub's Windows 2022 image already provides LLVM, CMake and MSVC. The setup script verifies LLVM/CMake, downloads the [BtbN month-end GPL shared build](https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-09-30-13-08), verifies its pinned SHA-256, and sets `FFMPEG_DIR`, `LIBCLANG_PATH` and `PATH`. It includes headers and MSVC import libraries. BtbN retains month-end assets for two years; renew this pin before September 2028. Do not replace it with a floating `latest` URL.

```powershell
node scripts/build-bundles.mjs
```

The script generates a Tauri resource map with each FFmpeg DLL at the installer resource root, next to `nuzky-app.exe`, and builds `target/release/bundle/nsis/*-setup.exe`. This lets the Windows loader resolve FFmpeg before Rust starts, without a runtime PATH change or a machine-wide FFmpeg installation. The FFmpeg licence and readme are copied when present. Tauri also bundles the Visual C++ runtime; its default WebView2 installer downloads the runtime if missing, so first installation may require internet. The installer is not Authenticode signed and can trigger SmartScreen.

## What ffmpeg-sys-next actually reads

Inspected `~/.cargo/registry/src/*/ffmpeg-sys-next-9.0.0/build.rs`, particularly `main()` and `link_to_libraries()`:

- `FFMPEG_DIR` selects a prebuilt prefix with `include/` and `lib/` (or `lib/amd64` for x86_64). It takes precedence over vcpkg and pkg-config. Use it on Windows.
- Otherwise the crate tries vcpkg, then probes `libavutil`, the enabled components and `libavcodec` through pkg-config. `PKG_CONFIG_PATH` works. `FFMPEG_PKG_CONFIG_PATH` is not read.
- Dynamic linking is the default. The Cargo `static` feature sets `CARGO_FEATURE_STATIC`; do not set that internal variable manually. The `build` feature invokes the crate's FFmpeg source build instead of these prebuilt paths. Nuzky enables neither.
- `LIBCLANG_PATH` locates libclang for bindgen. `BINDGEN_EXTRA_CLANG_ARGS` can supply missing compiler include paths (used by the no-root Linux setup). These are bindgen inputs, not FFmpeg runtime paths.
- Build-time discovery does not arrange runtime loading. AppImage collection, macOS relocation and Windows DLL placement above are separate required steps.

## ONNX Runtime

Covers (`crates/vision`) run their models with ONNX Runtime, which Nuzky opens at run time and never links (`ort` with `load-dynamic`). `ort`'s default, pyke's prebuilt static library, needs AVX2: linked into `nuzky-app` it stopped the app before `main` on CPUs without AVX2, checked under `qemu-x86_64-static -cpu Nehalem`. Microsoft's official build picks its CPU kernels at run time and runs there, so that is what ships.

`node scripts/fetch-onnxruntime.mjs` downloads the pinned ONNX Runtime 1.28.3 release archive for the platform from [microsoft/onnxruntime](https://github.com/microsoft/onnxruntime/releases/tag/v1.28.3), checks its SHA-256 and keeps only the library, `LICENSE` and `ThirdPartyNotices.txt` in `<deps>/onnxruntime-1.28.3/` (`~/.cache/nuzky/deps`, `%LOCALAPPDATA%\Nuzky\build-deps` on Windows, or `NUZKY_DEPS`). The Linux and Windows setup scripts, `scripts/check.sh`, `scripts/repro.py`, CI and `build-bundles.mjs` run it; on macOS and Ubuntu run it once after `npm ci`. Debug builds load the library from there, release builds only from their own install:

| Install | Library |
|---|---|
| AppImage and deb | `usr/lib/Nuzky/libonnxruntime.so.1`, Tauri's resource directory, named after `productName` |
| .app | `Contents/Frameworks/libonnxruntime.1.dylib`, copied and signed by `bundle-macos-libs.py` |
| NSIS | `onnxruntime.dll` beside `nuzky-app.exe`, next to the VC++ runtime Tauri already bundles |

`NUZKY_ONNXRUNTIME=<file>` replaces the lookup, for packagers and release builds run outside an install. When the library cannot load, the app works as before and the cover tools answer `VISION_UNAVAILABLE` with the reason, before any model is downloaded. Telemetry, which Microsoft's Windows build turns on by default, is switched off as it loads. The library adds about 4 MB to the NSIS installer, 7–9 MB to the AppImage and deb and 11 MB to the dmg (compressed sizes of the library); the executables do not grow.

`scripts/check.sh` starts `nuzky-app` and `nuzky` under an emulated Nehalem and runs the face models there, so a change that brings back an AVX2 requirement fails the gate. Only CI or real hardware shows the macOS relocation and signing with the extra library, the NSIS layout and Windows on a CPU without AVX.

## CI, release and remaining platform checks

GitHub scans every pull request for secrets (`secrets.yml`) and runs the Linux checks in `checks.yml`. While the repository is private, Actions minutes are capped and macOS counts ten times, so `checks.yml` runs only on pull requests with the `ci` label. `scripts/check.sh` adds the tests with media and models, the audits and the UI flows, and runs locally before every merge. `ci.yml` runs on request (`gh workflow run ci.yml --ref <branch>`) and from `release.yml`. It caches npm and Cargo, tests all three systems, then builds and uploads installers and build-info files for 14 days. A `v*` tag runs the same build/test matrix; all three jobs must pass before a draft release is created and receives all installers plus SHA-256 checksums. The tag must equal `v` plus `src-tauri/tauri.conf.json`'s version. Set all package versions consistently before tagging. The release job uses the repository's automatic `GITHUB_TOKEN` with `contents: write`; no personal token is needed. A rerun can replace draft assets but refuses to modify a published release.

Each release also carries `latest.json` with its version. Installed copies read it from `https://github.com/tomasmach/nuzky/releases/latest/download/latest.json` at most once a day and show "Update available" on the home screen. GitHub serves that address from the newest published release that is not a prerelease, so publishing the draft is what announces a version; drafts and prereleases are never announced. Anonymous requests to a private repository get 404, so the repository must be public before the first public release, or installed copies never hear of an update. After a release is published, `update-notice.yml` reads the address like an installed copy and fails, which GitHub reports by email, when it does not show the published tag. To take back a bad release, turn it back into a draft or mark it as a prerelease; the address then points at the previous one. Nuzky only notifies: people download the new installer from the release page, and nothing installs itself. The file keeps the shape of the Tauri updater's static JSON, so a later in-app installer can add its signed `platforms` to the same file without silencing older copies, which read only `version`. A package manager that updates Nuzky itself can set `NUZKY_NO_UPDATE_CHECK=1`; Nuzky then never checks.

GitHub must still establish compilation and test success on Ubuntu/Apple Silicon/Windows, NSIS DLL placement, macOS relocation/signature correctness and successful installation on clean hosts. Before publishing, test a video import, preview with sound, H.264/AAC export and Whisper captions on each OS. Validate the macOS app with Homebrew absent and Windows with FFmpeg absent from PATH. Linux's `ldd` and extraction check cannot substitute for those tests.

No Rust/TypeScript changes are part of this build work. Current audio output uses `cpal::default_host()` and does not need a speculative Windows ALSA cfg. If a target compiler exposes a platform-specific API issue, the owning engineer must fix it before release; CI does not skip failing platforms. No runtime FFmpeg path code should be needed with the prepared bundle layout. CLI commands and tests that spawn the external `ffmpeg` executable still need it on PATH; only its libraries are shipped to end users.

## Redistribution

Nuzky is GPL-3.0-or-later. FFmpeg with x264 is a GPL combination; use GPL shared builds, never BtbN's `nonfree` variant or a build with `libfdk_aac`. See [FFmpeg's licensing guidance](https://ffmpeg.org/legal.html). Patent permissions are separate from copyright licences.

Before publishing a draft, preserve the exact FFmpeg and transitive library versions, their licence/copyright notices, and complete corresponding source including patches and build scripts. Attach a source archive beside the binary downloads; a link to upstream's latest branch or a list of versions is insufficient. `build-info-*.txt`, the Windows pin and macOS `Contents/Resources/bundled-libraries.txt` identify build inputs, but the workflows do not assemble a complete third-party source archive. This is a required release-maintainer step, not an automated compliance claim.

For Ubuntu images, obtain the matching distribution sources with `apt-get source ffmpeg x264` after enabling `deb-src`, and do the same for other redistributed packages. For BtbN preserve the exact FFmpeg revision, the month-end build recipes/patches and the source versions of every statically included dependency. For Homebrew preserve formula revisions, source tarballs/patches, and licences for every copied dylib. Package those inputs and notices, check they correspond to the actual build, then attach them before publishing. The `.deb` relies on distribution FFmpeg and does not itself redistribute those FFmpeg libraries.

ONNX Runtime is MIT, © Microsoft; the installers carry its `LICENSE` and `ThirdPartyNotices.txt` in `licenses/onnxruntime/`. Add the ONNX Runtime 1.28.3 source (commit `0d68ff6b3b72b04aac578decd6c4c45d322bb962`, with the dependencies its `cmake/deps.txt` lists) to the release source archive next to FFmpeg's.

### Downloaded models

Models are not bundled. The app and `nuzky vision-models` download them on request from pinned commits or releases, and a file is used only when its size and SHA-256 match (`crates/vision/src/models.rs`). They can be redistributed under these licences:

| File | Model | Licence | Source |
|---|---|---|---|
| `yunet-2026may.onnx` | YuNet face detector, 2026may export | MIT, © Shiqi Yu | [opencv_zoo @ 26cc381](https://github.com/opencv/opencv_zoo/tree/26cc381e4d2594bb9f47a26eb8fd96c94a13660d/models/face_detection_yunet) |
| `face-landmarks-v2.onnx` | MediaPipe Face Mesh V2 | Apache-2.0, © Google | converted from `face_landmarker.task` float16/1 by `scripts/convert-mediapipe.py`, published with the licence and a NOTICE of the changes at [tomasmach/nuzky-models @ a4f7e3c](https://github.com/tomasmach/nuzky-models/tree/a4f7e3c99e0b1b971d69595a1c3efb64fbc18730) |
| `face-blendshapes-v2.onnx` | MediaPipe Blendshape V2 | Apache-2.0, © Google | as above |
| `selfie-segmenter.onnx` | MediaPipe Selfie Segmenter, 256×256 | Apache-2.0, © Google | as above, from `selfie_segmenter.tflite` float16/1 |
| `birefnet-lite.onnx` | BiRefNet_lite with native `DeformConv` | MIT, © ZhengPeng | exported from [ZhengPeng7/BiRefNet_lite @ aa62cd8](https://huggingface.co/ZhengPeng7/BiRefNet_lite/tree/aa62cd87eafb9cc43056d08ef3615a14628b831d) by `scripts/convert-birefnet.py`, published with the licence and NOTICE as the release asset [nuzky-models `birefnet-lite-1`](https://github.com/tomasmach/nuzky-models/releases/tag/birefnet-lite-1) |

The model cards state the MediaPipe licences: [Face Mesh V2](https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf), [Blendshape V2](https://storage.googleapis.com/mediapipe-assets/Model%20Card%20Blendshape%20V2.pdf), [Selfie Segmentation](https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Selfie%20Segmentation.pdf). Models under non-commercial, research-only or AGPL terms are not used, among them RMBG-1.4 and RMBG-2.0, SCRFD and the other InsightFace models, dlib's 68-point predictor and the pyiqa quality models.
