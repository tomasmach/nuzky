#!/usr/bin/env bash
# Local gate before every merge to main and every release. GitHub runs no tests on pull requests.
# A pull request that changes only the website in site/ and no dependencies skips it (AGENTS.md).
# Stops at the first failing step and names it. Needs the tools listed in scripts/repro.py and cargo-deny.
set -uo pipefail
cd "$(dirname "$0")/.."

step() {
  local name=$1 started=$SECONDS
  shift
  echo
  echo "==> $name"
  if ! "$@"; then
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
  env XDG_DATA_HOME="$PWD/tmp-test/xdg/data" XDG_CACHE_HOME="$PWD/tmp-test/xdg/cache" XDG_RUNTIME_DIR="$runtime" "$@"
}

# ONNX Runtime is opened at run time, never linked, so Nuzky starts on CPUs without AVX2. Both programs
# must reach main under an emulated Nehalem and the face models must run there.
without_avx2() (
  # A program that dies under qemu would leave a core dump in the checkout.
  ulimit -c 0
  cargo build --locked -p nuzky-app -p nuzky-cli || return 1
  for program in nuzky-app nuzky; do
    # `mcp` without a project reaches main and refuses with INVALID_ARGUMENTS and exit code 1.
    local said
    said=$(qemu-x86_64-static -cpu Nehalem "target/debug/$program" mcp 2>&1)
    [[ $said == *INVALID_ARGUMENTS* ]] || {
      echo "$program does not start on a CPU without AVX2: $said" >&2
      return 1
    }
  done
  CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="qemu-x86_64-static -cpu Nehalem" \
    isolated cargo test --locked -p nuzky-vision --lib -- --include-ignored runtime:: the_face_models
)

step "Own node_modules" own_node_modules
step "npm install from the lockfile" npm ci --no-audit --no-fund
step "Rust format" cargo fmt --all --check
step "Frontend types and build" npm run build
step "Clippy" cargo clippy --workspace --all-targets --locked -- -D warnings
step "ONNX Runtime" node scripts/fetch-onnxruntime.mjs
step "Rust tests" isolated cargo test --workspace --locked
step "Test media and models" scripts/fixtures.sh
step "Rust tests with media and models" isolated cargo test --workspace --locked -- --ignored
step "Starts and finds faces without AVX2" without_avx2
step "npm audit" npm audit --audit-level=high
step "Rust advisories and licences" cargo deny --locked check advisories licenses
step "UI flows" python3 scripts/repro.py --all

echo
echo "check passed in $((SECONDS / 60))m $((SECONDS % 60))s"
