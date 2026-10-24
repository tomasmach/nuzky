# Building and distributing Nuzky

Use current stable Rust (minimum 1.94, which `candle-core` needs for its NEON code on Apple Silicon), Node.js 22.12+ and the committed Cargo/npm lockfiles. Pull requests run lint, types, the app and website builds, clippy, `cargo test --workspace --locked`, the dependency audits and the check without AVX2 on `ubuntu-24.04` (`.github/workflows/checks.yml`) for every pull request that is not a draft and every push to `main`. A release tag runs the tests and a release Tauri build on `ubuntu-24.04` (x86_64), `macos-14` (arm64) and `windows-2022` (x64). It needs no secrets; with the Apple secrets set, the macOS build is also signed with a Developer ID and notarized (macOS, Signing and notarization). This build pipeline is not evidence that editing, playback or export has been tested on all three systems.

## Worktree dependency builds

`scripts/seed-target.sh` seeds a new checkout's own `target/` from another worktree of this repository, preferring matching Cargo lockfiles and profiles. It uses copy-on-write clones (`cp --reflink=always` on Linux btrfs/XFS, `cp -c` on macOS APFS), holds Cargo's donor lock while copying and excludes every workspace-crate fingerprint. Each checkout still has its own target directory. Linux setup runs it automatically; on macOS run it after the exports below, and `scripts/check.sh` and `scripts/repro.py` also run it before building. Reuse requires the same toolchain, features and native build environment as the donor.

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

On Linux and Windows, Whisper runs on the GPU through Vulkan and falls back to the CPU when ggml finds no graphics card or no GPU context can be created. On Linux, building needs `glslc`, the Vulkan headers and the loader library (`libvulkan-dev glslc` on Ubuntu, `vulkan-loader-devel vulkan-headers glslc` on Fedora); without root, the setup script downloads a checksum-pinned LunarG SDK instead. The app uses Vulkan FP32 (`GGML_VK_DISABLE_F16=1`) because FP16 moved word times by up to 330 ms in our tests. On macOS, Whisper runs on the GPU through Metal, whose shaders whisper.cpp embeds in the binary. On an M4 Pro a 5:48 Czech recording took 45 s to recognise instead of 135 s on the CPU, with the same 522 words and word times within 35 ms. Windows has the Vulkan loader only with a graphics driver, so Nuzky's binaries load `vulkan-1.dll` on first use (`/DELAYLOAD`, `crates/analysis/build.rs`): without it, in a virtual machine or on Microsoft Basic Display Adapter, Nuzky still starts and recognises on the CPU. On a Core Ultra 7 155H with Intel Arc graphics the same recording took 95–104 s instead of 1317 s on the CPU. 514 of the 522 words came out the same and in the same order, the text differed in three places, and 95 % of word times were within 80 ms. The Mac's result is about as far from this laptop's CPU.

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
brew install ffmpeg@8 ffmpeg-full pkg-config cmake flock
export PKG_CONFIG_PATH="$(brew --prefix ffmpeg@8)/lib/pkgconfig"
export SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"
export PATH="$(brew --prefix ffmpeg-full)/bin:$PATH"
bash scripts/seed-target.sh
npm ci
npm run tauri dev
```

These exports match the Darwin environment used by `scripts/check.sh` and `scripts/repro.py`: link against `ffmpeg@8`, use the selected Xcode SDK, and run FFmpeg fixtures with `ffmpeg-full` for `drawtext`. Keep them in the terminal for manual workspace builds and tests.

Use the [versioned Homebrew formula](https://formulae.brew.sh/formula/ffmpeg@8) so a future `brew ffmpeg` upgrade does not silently change the FFmpeg major version. CI installs plain `ffmpeg`, FFmpeg 9.0 on 10 October 2026: the Homebrew metadata of the `macos-14` runner image predates `ffmpeg@8`. Current Homebrew has no bottles for macOS 14, so updating Homebrew on that runner would build FFmpeg and its dependencies from source. The engine tests in CI take `drawtext` from `ffmpeg-full`, as below. The [GitHub runner catalog](https://github.com/actions/runner-images) maps `macos-14` to arm64; CI also asserts `uname -m` is `arm64`.

If every C link fails with `ld: tapi error: malformed file … libSystem.tbd … unknown architecture arm64e.x1-macos`, the selected Xcode is older than the Command Line Tools SDK the compiler picks (Xcode 26 on macOS 27). Update Xcode, or give the toolchain its own SDK: `export SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"`. Switching to the Command Line Tools 27 instead builds in debug, but its `strip` with the macOS 27 SDK and the 14.0 deployment target writes proc-macro libraries that dyld rejects (`mis-aligned LINKEDIT string pool`), so release builds fail.

For tests and fixtures: `brew install espeak-ng` for `scripts/fixtures.sh`. The engine tests draw frame numbers with FFmpeg's `drawtext`, which Homebrew's `ffmpeg` and `ffmpeg@8` lack. `brew install ffmpeg-full` (keg-only) and put it first only for test runs: `PATH="$(brew --prefix ffmpeg-full)/bin:$PATH" cargo test --workspace`. Installing it upgrades shared Homebrew libraries such as x265; run `brew linkage --test` afterwards and `brew upgrade` any formula it reports, then rebuild Nuzky. On macOS the app keeps data in `~/Library/Application Support/nuzky` and cache in `~/Library/Caches/nuzky` regardless of `XDG_DATA_HOME`/`XDG_CACHE_HOME`; isolate a manual run with `HOME=<test dir>` plus `XDG_RUNTIME_DIR`. `scripts/check.sh` puts `ffmpeg-full` first and gives the tests their own home through `scripts/isolated.sh`.

```sh
node scripts/build-bundles.mjs
```

The script first builds `target/release/bundle/macos/Nuzky.app`. `bundle-macos-libs.py` walks the executable's `otool -L` dependencies recursively, copies non-system dylibs to `Contents/Frameworks`, rewrites references using `install_name_tool`, and ad-hoc signs the modified libraries and app. It rejects unresolved libraries and basename collisions. It checks `codesign --verify --deep --strict` and creates `dmg/Nuzky_<version>_aarch64.dmg`. Finder lays out the dmg's window, the background from `src-tauri/dmg` with Nuzky and an Applications link beside it, so the build needs a logged-in macOS session and asks once for permission to control Finder; it stops while another disk named Nuzky is mounted, because Finder would lay out that one instead. Copy the app out of the mounted image into Applications. End users do not need Homebrew for these prepared bundles. A raw `npm run tauri build` does not run this relocation step and still requires the build machine's Homebrew libraries.

The deployment minimum is macOS 14.

### Signing and notarization

`build-bundles.mjs` signs ad-hoc and never notarizes; its `build-info-darwin.txt` says `macOS signature: ad-hoc, not notarized`. Gatekeeper blocks such a download until the user clicks Open Anyway in System Settings → Privacy & Security, as the release notes then say.

```sh
node scripts/sign-macos.mjs
```

With the Apple variables below, `sign-macos.mjs` takes the app out of the dmg that `build-bundles.mjs` made and signs it again: every library in `Contents/Frameworks` and the app, with the Developer ID, the hardened runtime and a secure timestamp. Then it makes the dmg again, signs it, sends it to Apple's notary service with `notarytool`, waits for the answer, staples the ticket to the app and dmg, validates both with `stapler validate` and checks the dmg with `spctl`. It then packages the app as `macos/Nuzky_<version>_aarch64.app.tar.gz` for the updater. When Apple does not accept the dmg, the log shows Apple's reasons and the script fails. It writes the result on the `macOS signature:` line of `build-info-darwin.txt`. Without the variables, it changes nothing and says that the dmg is not notarized, as a warning in GitHub Actions. Never change a signed or notarized app afterwards.

The app needs no entitlements. WebKit runs JavaScript in its own processes, Nuzky records no sound or video and sends no Apple Events, its keychain items are in the login keychain, and every library it loads carries the same signature. The hardened runtime lets the app load only libraries signed by the same team, so `NUZKY_ONNXRUNTIME` cannot point a signed app at another ONNX Runtime.

The maintainer adds these repository secrets in GitHub under Settings → Secrets and variables → Actions:

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | The Developer ID Application certificate with its private key as a `.p12` file, encoded with `base64 -i certificate.p12 \| pbcopy`. Only the Account Holder can create the certificate: Xcode → Settings → Accounts → Manage Certificates → + → Developer ID Application. Export it from Keychain Access, My Certificates, with Export → Personal Information Exchange (.p12). |
| `APPLE_CERTIFICATE_PASSWORD` | The password you gave the `.p12` file. |
| `APPLE_SIGNING_IDENTITY` | The certificate's name, `Developer ID Application: <name> (<team ID>)`, as `security find-identity -v -p codesigning` prints it. |
| `APPLE_ID` | The email address of the Apple Account in the developer team. |
| `APPLE_TEAM_ID` | The 10-character team ID from the Membership details at developer.apple.com/account. |
| `APPLE_APP_PASSWORD` | An app-specific password from account.apple.com → Sign-In and Security → App-Specific Passwords. |

`APPLE_CERTIFICATE` and `APPLE_CERTIFICATE_PASSWORD` go together, and so do `APPLE_ID`, `APPLE_TEAM_ID` and `APPLE_APP_PASSWORD`. Both groups need `APPLE_SIGNING_IDENTITY`. When a group is incomplete, the script stops and names the missing secret. Without the notarization group, the dmg is signed but not notarized, and Gatekeeper still blocks it. Locally, export the variables and run the script after `build-bundles.mjs`. Without `APPLE_CERTIFICATE`, it uses the identity from your own keychain.

Sign the builds you use yourself with the Developer ID too. Nuzky keeps the Sign in with ChatGPT tokens in the login keychain, and macOS lets an app read an item without asking only when it has the signature of an app the item already allows. An ad-hoc signature is the hash of one build, so every new ad-hoc build asks for the keychain password the first time it reads the tokens. A Developer ID signature names the team, so the next Developer ID build reads them without asking. The first Developer ID build after an ad-hoc one asks once; Always Allow ends that.

```sh
node scripts/build-bundles.mjs
APPLE_SIGNING_IDENTITY="Developer ID Application: <name> (<team ID>)" node scripts/sign-macos.mjs
```

`codesign` takes the key from the login keychain and asks once for access to it; Always Allow ends that too. Without the notarization variables the dmg stays not notarized, which does not matter for your own copy: macOS checks notarization only for downloaded files. Quit Nuzky, move the old copy in Applications to the Trash and copy the new one from the dmg. `codesign -dv /Applications/Nuzky.app` shows `TeamIdentifier=<team ID>`.

`ci.yml` runs `sign-macos.mjs` in the `sign-macos` job, on a fresh machine after the build job, and passes the secrets only to its signing step; `release.yml` hands them to `ci.yml`. The job downloads the macOS artifact, runs no `npm ci`, cargo or other dependency code, and replaces the artifact with the signed dmg and its build-info. The build job runs the build code of every Rust and npm dependency. Such code could read the environment of another process of the same user, or leave behind a process, a changed `PATH` or a changed script for a later step, so the secrets never reach the build's machine. Keep them out of `build-bundles.mjs` for a second reason: the Tauri CLI reads the same names and would sign and notarize the app itself before the libraries are relocated. `sign-macos.mjs` imports the certificate into a temporary keychain, adds it to the user's keychain search list while it signs, because `codesign` on macOS 14 finds no identity outside that list, and afterwards restores the list and deletes the keychain, also after a failure. With the secrets set, every macOS build is signed and notarized, including the build on each push to `main`. A wrong certificate or an expired password then fails `main` before it can fail a release.

To check a notarized dmg on a Mac:

```sh
xcrun stapler validate Nuzky_<version>_aarch64.dmg
spctl -a -vv -t open --context context:primary-signature Nuzky_<version>_aarch64.dmg
hdiutil attach Nuzky_<version>_aarch64.dmg
spctl -a -vv /Volumes/Nuzky/Nuzky.app
codesign --verify --deep --strict --verbose=2 /Volumes/Nuzky/Nuzky.app
hdiutil detach /Volumes/Nuzky
```

`stapler` prints `The validate action worked!`, and both `spctl` commands print `accepted` and `source=Notarized Developer ID`. An ad-hoc build prints `rejected` instead. `build-info-darwin.txt` names the result on its `macOS signature:` line.

`release.yml` reads that line. When it says `Developer ID, notarized and stapled`, the draft's notes leave out the macOS line about Open Anyway. A rerun writes the draft's notes again and drops edits made on GitHub, so change the text in `scripts/release-notes.md`. `node --test 'scripts/*.test.mjs'` fails when `scripts/release-notes.md` no longer has exactly one such line. The README is not generated and says the dmg is notarized, so a release whose dmg is not notarized also changes what the README says about macOS.

## Windows x64

Install Rust with the MSVC toolchain, Node 22, Visual Studio 2022 Build Tools with **Desktop development with C++** and the Windows SDK. Install LLVM x64 and CMake if missing, FFmpeg's command-line tools, which the tests and UI flows run, and OpenSSL, whose command makes the RSA key the ChatGPT sign-in tests sign with; put `C:\Program Files\OpenSSL-Win64\bin` on your `PATH` if its installer did not:

```powershell
winget install LLVM.LLVM
winget install Kitware.CMake
winget install Gyan.FFmpeg
winget install ShiningLight.OpenSSL.Light
```

Open a fresh PowerShell terminal at the repository root:

```powershell
./scripts/setup-windows-deps.ps1
npm ci
cargo test --workspace --locked
npm run tauri dev
```

Every PowerShell that builds or runs Nuzky needs the variables the setup script sets, so run it again in a new terminal. GitHub's Windows 2022 image already provides LLVM, CMake and MSVC. The setup script verifies LLVM/CMake, downloads the [BtbN month-end GPL shared build](https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-09-30-13-08), verifies its pinned SHA-256, and sets `FFMPEG_DIR`, `LIBCLANG_PATH` and `PATH`. It includes headers and MSVC import libraries. For Whisper on the GPU it downloads the LunarG Vulkan SDK 1.4.363.0 installer, verifies its pinned SHA-256 and installs it with `copy_only=1` into the build dependencies, where it sets no environment variables and registers no layers; Windows lists it under the user's installed apps as Vulkan SDK 1.4.363.0. The script sets `VULKAN_SDK` to it, and `CMAKE_PROJECT_INCLUDE` to `scripts/whisper-paths.cmake`, which keeps ggml's nested shader build within the 260 characters MSBuild can write even with long paths enabled. Outside CI it also fetches the Microsoft Edge WebDriver of the installed WebView2 version into the build dependencies and checks Microsoft's signature on it.

The UI flows run on Windows too, with Python 3, `python -m pip install pillow numpy`, eSpeak NG (`winget install eSpeak-NG.eSpeak-NG`, its folder on `PATH`), which the style flows speak with, and the media from `scripts/fixtures.sh` (run it in Git Bash, or copy `tmp-test` from another machine): `python scripts/repro.py <flow>` in the PowerShell where the setup script ran. Edge WebDriver drives the app's WebView2, and the app's window opens on the desktop, so run them where nobody works. Each flow gives the app its own home folder (`USERPROFILE`) with its own `AppData`, so its data, cache and webview storage are never yours. BtbN retains month-end assets for two years; renew this pin before September 2028. Do not replace it with a floating `latest` URL.

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

`.github/workflows/checks.yml` starts `nuzky-app` and `nuzky` under an emulated Nehalem and runs the face models there, so a change that brings back an AVX2 requirement fails the required `Checks` run. Only CI or real hardware shows the macOS relocation and signing with the extra library, the NSIS layout and Windows on a CPU without AVX.

## CI, release and remaining platform checks

GitHub scans every pull request for secrets (`secrets.yml`) and runs the Linux checks, the dependency audits and the check without AVX2 in `checks.yml`. `scripts/check.sh` adds the tests with media and models and the UI flows, and passes locally on the head of the version pull request before a release. `ci.yml` builds macOS on every push to `main` and runs on request (`gh workflow run ci.yml --ref <branch>`) and from `release.yml`. It caches npm and Cargo, tests all three systems, then builds and uploads installers and build-info files for 14 days; the `sign-macos` job then signs and notarizes the dmg when the Apple secrets are set. A `v*` tag runs the same build/test matrix; all three jobs must pass before the version draft receives the four installers, the macOS updater package and `latest.json`. A separate draft pre-release, `third-party-source-<version>`, targets the same commit and receives the build-info files; its tag does not start with `v`, so it does not trigger another build. The tag must equal `v` plus `src-tauri/tauri.conf.json`'s version, and `cargo test` fails when that version differs from `Cargo.toml`'s, `package.json`'s or `package-lock.json`'s (`every_package_has_the_same_version` in `src-tauri/src/updates.rs`). The release job uses the repository's automatic `GITHUB_TOKEN` with `contents: write`; no personal token is needed. A rerun replaces both drafts' assets and notes but refuses to modify either if one has been published.

The version release carries `latest.json` with its version, update package URLs and signatures. The release job still signs the packages and reads the resulting `.sig` files into the `signature` fields; it does not attach those files separately. Installed copies read it from `https://github.com/nuzkyapp/nuzky/releases/latest/download/latest.json` at most once a day and show "Update available" on the home screen. GitHub serves that address from the newest published release that is not a prerelease, so publishing the draft is what announces a version; drafts and prereleases are never announced. Anonymous requests to a private repository get 404, so the repository must be public before the first public release, or installed copies never hear of an update. After a release is published, `update-notice.yml` reads the address like an installed copy and fails, which GitHub reports by email, when it does not show the published tag. To take back a bad release, turn it back into a draft or mark it as a prerelease; the address then points at the previous one. Nothing installs by itself: Update in the app downloads the package `latest.json` names for that kind of install (`darwin-aarch64-app`, `windows-x86_64-nsis`, `linux-x86_64-appimage` or `linux-x86_64-deb`; `src-tauri/src/updates.rs`), only from an address under `https://github.com/nuzkyapp/nuzky/releases/download/`, and keeps it only when its signature matches the public key in `src-tauri/tauri.conf.json` and names the version `latest.json` announces (`requireSignedVersion`, which is why the release job signs with `--app-version`). It installs the package and restarts only when the user chooses Restart to update, never while an export or other work runs. The file uses the Tauri updater's static JSON with signed `platforms`; older copies read only `version`. A package manager that updates Nuzky itself can set `NUZKY_NO_UPDATE_CHECK=1`; Nuzky then never checks.

GitHub must still establish compilation and test success on Ubuntu/Apple Silicon/Windows, NSIS DLL placement, macOS relocation/signature correctness and successful installation on clean hosts. Linux's `ldd` and extraction check cannot substitute for testing the installers (step 3 below); a system nobody tested is named as untested in the release notes.

No Rust/TypeScript changes are part of this build work. Current audio output uses `cpal::default_host()` and does not need a speculative Windows ALSA cfg. If a target compiler exposes a platform-specific API issue, the owning engineer must fix it before release; CI does not skip failing platforms. No runtime FFmpeg path code should be needed with the prepared bundle layout. CLI commands and tests that spawn the external `ffmpeg` executable still need it on PATH; only its libraries are shipped to end users.

## Releasing a version

Only the maintainer tags and publishes. A release, `v0.1.0-alpha` included, goes in this order:

1. **Version and notes.** A pull request sets the version in `src-tauri/tauri.conf.json` and `Cargo.toml` (then `cargo update --workspace`) and runs `npm version <version> --no-git-tag-version` for `package.json` and its lockfile; `cargo test` fails when one is missed. It rewrites `scripts/release-notes.md`, which becomes the draft's text. Its required checks pass, and `scripts/check.sh` passes on its head before the merge.
2. **Tag.** The maintainer runs `git tag v<version> <merge commit>` and `git push origin v<version>`. `release.yml` stops unless the tag is `v` plus the app version, builds and tests on all three systems and creates both drafts with the files below.
3. **Installers.** The maintainer installs the draft's installers on a clean machine of each system they can test on and tests importing a phone video, preview with sound, H.264/AAC export and captions, the macOS app with Homebrew absent and Windows with no FFmpeg on PATH. On macOS, a notarized app opens after one question whether to open an app from the internet, and an app that is not notarized opens with Open Anyway. On Windows, More info → Run anyway starts it. Both match the draft's notes, and the foot of the home screen's sidebar shows the new version. The notes say how far each system was tested.
4. **Third-party source.** The maintainer collects it and attaches it to `third-party-source-<version>`, as Redistribution below describes.
5. **Publish** the draft as a regular release, with "Set as a pre-release" off and "Set as the latest release" on. Installed copies and `update-notice.yml` read `releases/latest`, which GitHub never points at a prerelease, so an alpha published as a prerelease would never be announced. The `-alpha` in the version already says what it is. At the same time, publish `third-party-source-<version>` with "Set as a pre-release" on; it must never become the latest release.
6. **Announcement.** On publishing, `update-notice.yml` reads `latest.json` as an installed copy does and fails, which GitHub reports by email, unless it shows the new tag.

The updater uses `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Only the release job uses these secrets. The maintainer keeps a backup outside GitHub: losing the key means installed copies can never update themselves again and need a manual reinstall. The public key is in `src-tauri/tauri.conf.json`.

The version draft carries these six files. Their names change only with the version, so the website can link to them:

| File | What |
|---|---|
| `Nuzky_<version>_aarch64.dmg` | macOS 14 or newer, Apple Silicon |
| `Nuzky_<version>_aarch64.app.tar.gz` | the notarized app the macOS updater installs |
| `Nuzky_<version>_x64-setup.exe` | Windows x64 installer |
| `Nuzky_<version>_amd64.AppImage` | Linux x86_64 |
| `Nuzky_<version>_amd64.deb` | Ubuntu 24.04 or newer |
| `latest.json` | the version and update packages with their signatures |

GitHub shows each file's SHA-256 on the release page and provides Nuzky's own source as the tag's Source code archives. The separate source draft carries `build-info-darwin.txt`, `build-info-linux.txt` and `build-info-win32.txt`; the maintainer adds the third-party source archives before publishing both releases.

Installed copies offer a version when it is newer by semver precedence, so the versions after `0.1.0-alpha` are `0.1.0-alpha.2`, `0.1.0-beta.1` and so on. The `.deb` carries the same version, and Debian reads what follows the `-` as a package revision. apt orders the alphas and betas the same way, but takes a plain `0.1.0` for older than `0.1.0-alpha` and installs it over an alpha only with `--allow-downgrades`. Number the first release without a suffix `0.2.0` or later.

## Redistribution

Nuzky is GPL-3.0-or-later. FFmpeg with x264 is a GPL combination; use GPL shared builds, never BtbN's `nonfree` variant or a build with `libfdk_aac`. See [FFmpeg's licensing guidance](https://ffmpeg.org/legal.html). Patent permissions are separate from copyright licences.

Before publishing a draft, preserve the exact FFmpeg and transitive library versions, their licence/copyright notices, and complete corresponding source including patches and build scripts. Attach it to `third-party-source-<version>`; a link to upstream's latest branch or a list of versions is insufficient. The release job attaches the build-info files there and fills in `scripts/third-party-source-notes.md` with the version. Nuzky's own source is the Source code archive of the version tag. Each `build-info-<platform>.txt` names the exact FFmpeg its installer carries with a link to that source, and the ONNX Runtime archive with its source commit. Collecting the third-party source itself is a required release-maintainer step, not an automated compliance claim. Check that it corresponds to the build, then attach it to the source draft with `gh release upload third-party-source-<version> <file>` as one archive per system, `Nuzky_<version>_third-party-source-linux.tar.gz`, `-macos.tar.gz` and `-windows.tar.gz`, and ONNX Runtime's, which all three carry, as `Nuzky_<version>_third-party-source-onnxruntime.tar.gz`:

- **Linux.** The AppImage carries Ubuntu 24.04's FFmpeg and every other library linuxdeploy collected; the `.deb` relies on the system's and redistributes none of them. `build-info-linux.txt` lists the Ubuntu source package and version of every library in the AppImage, FFmpeg's among them (about 140). In an empty directory on Ubuntu 24.04 with `ubuntu-dev-tools` installed, for example in `podman run -it ubuntu:24.04`, this downloads each from Launchpad, which keeps older versions too:

  ```sh
  awk '/^Ubuntu source packages/{f=1;next} f&&/\t/{exit} f' build-info-linux.txt |
    while read -r package version; do pull-lp-source --download-only "$package" "$version"; done
  ```

- **macOS.** `build-info-darwin.txt` lists every copied dylib with its Homebrew Cellar path, which names the formula and its version, and the FFmpeg release the formula builds. For each formula add the source tarball and patches, which `brew fetch --build-from-source <formula>` downloads while Homebrew still offers that version, the formula file and its licences.
- **Windows.** `build-info-win32.txt` names the BtbN build, the FFmpeg commit it was built from and the FFmpeg-Builds tag whose `scripts.d` pin the source of every library in it. Add FFmpeg at that commit, the build scripts at that tag and each source they pin.
- **ONNX Runtime** is MIT, © Microsoft; the installers carry its `LICENSE` and `ThirdPartyNotices.txt` in `licenses/onnxruntime/`. Add the 1.28.3 source (commit `0d68ff6b3b72b04aac578decd6c4c45d322bb962`) with the dependencies its `cmake/deps.txt` lists.

### Bundled sounds

The sound effects and songs in `assets/sounds/` are built into every binary. Each is CC0 1.0 (the legal code is `assets/sounds/CC0-1.0.txt`); `assets/sounds/manifest.json` names its kind (effect or music), author, source page, the pinned download it came from and what was changed (silence trimmed at the edges, levelled to a −2 dBFS peak, encoded as Ogg Opus, a few shortened). The sources are Kenney's audio packs, OpenGameArt, BigSoundBank and Freesound, each page stating CC0, and for the song "Dancing" by HoliznaCC0, Free Music Archive. `scripts/build-sounds.py` makes the pack again from those pinned sources; `python3 -I scripts/build-sounds.py <id>` builds only that sound, since another FFmpeg encodes the others to other bytes. CC0 needs no notice; the manifest keeps the provenance. Sounds that people find online in the app are downloaded on their computer, never shipped.

### Bundled sample video

The tour for new users opens `assets/sample/spotlight-effect.mp4`, which is built into every binary: a minute of "Stop Performing and Unlock Confidence, Creativity & Calm (Solo Table Theory)" by Dianne Glavaš on YouTube, under CC BY 3.0 (the legal code is `assets/sample/CC-BY-3.0.txt`). `assets/sample/manifest.json` names the source, the pinned download, the attribution line and what was changed (cut to one minute, re-encoded at 1080p), and `assets/sample/ATTRIBUTION.txt` holds that line. The installers carry both files in `licenses/sample/`, and the video's project credits its author, so Copy credits and the credits beside an export name her. `python3 -I scripts/build-sample.py` makes the video again from its pinned source with yt-dlp and FFmpeg. `assets/sample/transcript.json` is what Nuzky's transcribe tool recognised in exactly these bytes, so the tour cuts and captions without a speech model; it is made with the models, not by the script, and a Rust test fails while it does not match the video.

### Bundled emoji and stickers

The emoji and stickers in `assets/stickers/` are built into every binary: `art.pack` holds their SVG drawings, `catalog.json` their names, search words and where each drawing sits in the pack, and `sources` in it names every source with its pinned revision, licence and what was changed. The emoji are Google's [Noto Emoji](https://github.com/googlefonts/noto-emoji) drawings under Apache-2.0 (`LICENSE-Apache-2.0.txt`) and its waved flags, which are public domain (`LICENSE-region-flags.txt`). Their order, groups and skin tones come from Google's [emoji-metadata](https://github.com/googlefonts/emoji-metadata) (Apache-2.0), their English names and search words from [CLDR](https://github.com/unicode-org/cldr-json) and the list of emoji from Unicode's `emoji-test.txt` (Unicode License v3, `LICENSE-Unicode-3.0.txt`). Nuzky's own stickers in `assets/stickers/own/` are CC0 1.0 (`CC0-1.0.txt`). `python3 -I scripts/build-stickers.py` makes the pack again from those pinned sources: it copies a gradient's inherited `gradientTransform` onto it, which usvg 0.48.1 would otherwise drop, and minifies each drawing with usvg through `scripts/sticker-pack`, which also packs them into independent zstd frames. The same sources give the same bytes. The installers carry the licence texts in `licenses/stickers/`. A placed emoji is drawn into a PNG on the user's computer; Apache-2.0 and CC0 ask nothing of the videos made with them.

### Downloaded models

Models are not bundled. The app and `nuzky vision-models` download them on request from pinned commits or releases, and a file is used only when its size and SHA-256 match (`crates/vision/src/models.rs`). They can be redistributed under these licences:

| File | Model | Licence | Source |
|---|---|---|---|
| `yunet-2026may.onnx` | YuNet face detector, 2026may export | MIT, © Shiqi Yu | [opencv_zoo @ 26cc381](https://github.com/opencv/opencv_zoo/tree/26cc381e4d2594bb9f47a26eb8fd96c94a13660d/models/face_detection_yunet) |
| `face-landmarks-v2.onnx` | MediaPipe Face Mesh V2 | Apache-2.0, © Google | converted from `face_landmarker.task` float16/1 by `scripts/convert-mediapipe.py`, published with the licence and a NOTICE of the changes at [tomasmach/nuzky-models @ a4f7e3c](https://github.com/tomasmach/nuzky-models/tree/a4f7e3c99e0b1b971d69595a1c3efb64fbc18730) |
| `face-blendshapes-v2.onnx` | MediaPipe Blendshape V2 | Apache-2.0, © Google | as above |
| `selfie-segmenter.onnx` | MediaPipe Selfie Segmenter, 256×256; also finds the person in every frame for clip backgrounds | Apache-2.0, © Google | as above, from `selfie_segmenter.tflite` float16/1 |
| `birefnet-lite.onnx` | BiRefNet_lite with native `DeformConv` | MIT, © ZhengPeng | exported from [ZhengPeng7/BiRefNet_lite @ aa62cd8](https://huggingface.co/ZhengPeng7/BiRefNet_lite/tree/aa62cd87eafb9cc43056d08ef3615a14628b831d) by `scripts/convert-birefnet.py`, published with the licence and NOTICE as the release asset [nuzky-models `birefnet-lite-1`](https://github.com/tomasmach/nuzky-models/releases/tag/birefnet-lite-1) |

The model cards state the MediaPipe licences: [Face Mesh V2](https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf), [Blendshape V2](https://storage.googleapis.com/mediapipe-assets/Model%20Card%20Blendshape%20V2.pdf), [Selfie Segmentation](https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Selfie%20Segmentation.pdf). Models under non-commercial, research-only or AGPL terms are not used, among them RMBG-1.4 and RMBG-2.0, SCRFD and the other InsightFace models, dlib's 68-point predictor and the pyiqa quality models.
