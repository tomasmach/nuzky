#!/usr/bin/env bash
# Local gate before every merge to main and every release. GitHub runs no tests on pull requests.
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

step "Own node_modules" own_node_modules
step "npm install from the lockfile" npm ci --no-audit --no-fund
step "Rust format" cargo fmt --all --check
step "Frontend types and build" npm run build
step "Clippy" cargo clippy --workspace --all-targets --locked -- -D warnings
step "Rust tests" cargo test --workspace --locked
step "Test media and models" scripts/fixtures.sh
step "Rust tests with media and models" env XDG_DATA_HOME="$PWD/tmp-test/xdg/data" \
  cargo test --workspace --locked -- --ignored
step "npm audit" npm audit --audit-level=high
step "Rust advisories and licences" cargo deny --locked check advisories licenses
step "UI flows" python3 scripts/repro.py --all

echo
echo "check passed in $((SECONDS / 60))m $((SECONDS % 60))s"
