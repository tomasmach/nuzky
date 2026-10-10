#!/usr/bin/env bash
# Local gate before every merge to main and every release. GitHub runs the faster part of it on every pull
# request (.github/workflows/checks.yml); the tests with media and models, the check without AVX2, the audits and
# the UI flows run only here. A pull request that changes only the website in site/ and no dependencies skips it
# (AGENTS.md). On macOS it runs everything except the UI flows and the check without AVX2, which need Linux.
# Stops at the first failing step and names it. Needs the tools listed in scripts/repro.py and cargo-deny.
set -uo pipefail
cd "$(dirname "$0")/.."
os=$(uname -s)

# On macOS the gate builds against Homebrew's FFmpeg as docs/BUILDING.md describes and runs the ffmpeg command of
# ffmpeg-full, whose drawtext the engine tests draw frame numbers with.
if [ "$os" = Darwin ]; then
  export PKG_CONFIG_PATH="${PKG_CONFIG_PATH:-$(brew --prefix ffmpeg@8)/lib/pkgconfig}"
  export SDKROOT="${SDKROOT:-$(xcrun --sdk macosx --show-sdk-path)}"
  PATH="$(brew --prefix ffmpeg-full)/bin:$PATH"
fi

# One gate at a time per machine, whichever checkout it runs in: two at once take longer than one after the
# other, fail on timing checks and collide on the UI flows' ports. A second gate waits here. The lock sits with
# the build dependencies, not in XDG_RUNTIME_DIR, which a terminal running the app may have changed.
command -v flock >/dev/null || { echo "check needs flock; on macOS: brew install flock" >&2; exit 1; }
mkdir -p "$HOME/.cache/nuzky/deps"
exec 9>"$HOME/.cache/nuzky/deps/check.lock"
if ! flock -n 9; then
  echo "Another scripts/check.sh runs on this machine; this one starts when it ends."
  flock 9
fi
# The time reported at the end leaves out the wait.
SECONDS=0

step() {
  local name=$1 started=$SECONDS
  shift
  echo
  echo "==> $name"
  # The steps do not inherit the lock, so a process one of them leaves behind cannot hold it.
  if ! "$@" 9>&-; then
    echo
    echo "check FAILED at: $name ($((SECONDS - started))s)" >&2
    exit 1
  fi
  echo "<== $name passed ($((SECONDS - started))s)"
}

# npm ci empties node_modules, which must not be another checkout's directory behind a symlink.
own_node_modules() {
  if [ -L node_modules ]; then
    echo "node_modules is a symlink to another checkout. Remove the link and run the check again." >&2
    return 1
  fi
}

# Tests must not touch the user's Nuzky transcripts, caches or the socket of their running app.
runtime=$(mktemp -d "${TMPDIR:-/tmp}/nuzky-check.XXXXXX")
trap 'rm -rf "$runtime"' EXIT
isolated() {
  local data=$PWD/tmp-test/xdg/data cache=$PWD/tmp-test/xdg/cache
  if [ "$os" = Darwin ]; then
    # macOS ignores the XDG variables and keeps Nuzky's data and cache under ~/Library, so the tests get a home
    # of their own whose Library folders lead to the same directories. Cargo, rustup and the build dependencies
    # stay in the real home.
    local home=$PWD/tmp-test/xdg/home
    mkdir -p "$home/Library/Application Support" "$home/Library/Caches" "$data/nuzky" "$cache/nuzky"
    ln -sfn "$data/nuzky" "$home/Library/Application Support/nuzky"
    ln -sfn "$cache/nuzky" "$home/Library/Caches/nuzky"
    set -- HOME="$home" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
      NUZKY_DEPS="${NUZKY_DEPS:-$HOME/.cache/nuzky/deps}" "$@"
  fi
  env XDG_DATA_HOME="$data" XDG_CACHE_HOME="$cache" XDG_RUNTIME_DIR="$runtime" "$@"
}

# ONNX Runtime is opened at run time, never linked, so Nuzky starts on CPUs without AVX2. Both programs
# must reach main under an emulated Nehalem and the face models must run there.
without_avx2() (
  # A program that dies under qemu would leave a core dump in the checkout.
  ulimit -c 0
  # The same build as scripts/repro.py: every target of the workspace resolves the features `cargo test` does,
  # so it adds only the app's binary instead of building the Nuzky crates again.
  cargo build --locked --workspace --all-targets || return 1
  for program in nuzky-app nuzky; do
    # `mcp` without a project reaches main and refuses with INVALID_ARGUMENTS and exit code 1.
    local said
    said=$(qemu-x86_64-static -cpu Nehalem "target/debug/$program" mcp 2>&1)
    [[ $said == *INVALID_ARGUMENTS* ]] || {
      echo "$program does not start on a CPU without AVX2: $said" >&2
      return 1
    }
  done
  # Selecting the whole workspace reuses the test build; `-p nuzky-vision` alone resolves other features and
  # builds the engine and its dependencies a second time. The filters match tests in nuzky-vision only.
  CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="qemu-x86_64-static -cpu Nehalem" \
    isolated cargo test --locked --workspace --lib -- --include-ignored runtime:: the_face_models
)

# The Rust tests with media and the UI flows spend their time waiting for speech recognition on the GPU, the app
# and real time, each with about a tenth of the CPU, so they run side by side. The flows' report follows the tests'.
media_tests_and_flows() {
  local flows=tmp-test/ui-flows.log tests_failed=0 flows_failed=0
  python3 scripts/repro.py --all >"$flows" 2>&1 &
  local pid=$!
  # A job in the background ignores Ctrl+C, so a stopped gate stops its UI flows too and waits for their cleanup.
  trap 'kill -TERM "$pid" 2>/dev/null; wait "$pid"; exit 130' INT
  trap 'kill -TERM "$pid" 2>/dev/null; wait "$pid"; exit 143' TERM
  isolated cargo test --workspace --locked -- --ignored || tests_failed=1
  wait "$pid" || flows_failed=1
  trap - INT TERM
  echo
  cat "$flows"
  [ "$tests_failed" = 0 ] || echo "Rust tests with media and models FAILED" >&2
  [ "$flows_failed" = 0 ] || echo "UI flows FAILED" >&2
  [ "$tests_failed$flows_failed" = 00 ]
}

step "Own node_modules" own_node_modules
step "npm install from the lockfile" npm ci --no-audit --no-fund
step "Rust format" cargo fmt --all --check
step "Frontend lint" npm run lint
step "Frontend types and build" npm run build
step "Website build" npm run site:build
step "Clippy" cargo clippy --workspace --all-targets --locked -- -D warnings
step "ONNX Runtime" node scripts/fetch-onnxruntime.mjs
step "Rust tests" isolated cargo test --workspace --locked
step "Test media and models" scripts/fixtures.sh
if [ "$os" != Darwin ]; then
  step "Starts and finds faces without AVX2" without_avx2
fi
step "npm audit" npm audit --audit-level=high
step "Rust advisories and licences" cargo deny --locked check advisories licenses
if [ "$os" = Darwin ]; then
  step "Rust tests with media and models" isolated cargo test --workspace --locked -- --ignored
else
  step "Rust tests with media and models, and the UI flows" media_tests_and_flows
fi

echo
echo "check passed in $((SECONDS / 60))m $((SECONDS % 60))s"
if [ "$os" = Darwin ]; then
  echo "On macOS it left out the UI flows and the check without AVX2; they need Linux."
fi
