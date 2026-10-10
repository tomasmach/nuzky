#!/usr/bin/env bash
# Starts the build of a new checkout from the build of another worktree of this repository, so its first build
# compiles only the Nuzky crates instead of every dependency (whisper.cpp, the FFmpeg bindings, wgpu, Tauri).
# scripts/setup-linux-deps.sh runs it. It does nothing when target/ already holds most of a build, no other
# worktree has one, or the file system cannot clone files (btrfs and XFS can; ext4 cannot).
#
# The copy shares disk blocks with the original until either changes. Nothing of the workspace's own crates is
# reused: cargo names their builds the same in every checkout, so with a copied fingerprint it would run the
# other worktree's code. Their fingerprints are removed before anything is copied, and none is copied over.
set -euo pipefail
cd "$(dirname "$0")/.."

fingerprints() { find "$1/target/debug/.fingerprint" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l; }
same_inputs() { cat "$1/Cargo.lock" "$1/Cargo.toml" 2>/dev/null | sha256sum | cut -d' ' -f1; }
here=$(pwd -P)
ours=$(same_inputs .)

# The most recently built worktree, preferring one with the same Cargo.lock and profiles.
donor='' best=''
while read -r key path; do
  [ "$key" = worktree ] && [ -d "$path/target/debug/.fingerprint" ] || continue
  [ "$(cd "$path" && pwd -P)" = "$here" ] && continue
  rank="$([ "$(same_inputs "$path")" = "$ours" ] && echo 1 || echo 0)$(stat -c %Y "$path/target/debug/deps")"
  if [[ $rank > $best ]]; then best=$rank donor=$path; fi
done < <(git worktree list --porcelain)
[ -n "$donor" ] || exit 0
[ "$(fingerprints .)" -lt $(($(fingerprints "$donor") / 2)) ] || exit 0

mkdir -p target/debug/.fingerprint
exec 8>target/debug/.cargo-build-lock
flock -n 8 || { echo "cargo is building in this checkout; not starting from another build" >&2; exit 0; }
probe=target/debug/.reflink-probe
if ! cp --reflink=always Cargo.lock "$probe" 2>/dev/null; then
  echo "This file system cannot clone files; the first build starts from scratch." >&2
  exit 0
fi
rm -f "$probe"

echo "Starting target/ from the build in $donor"
workspace=$(cargo metadata --format-version 1 --no-deps --offline |
  python3 -c 'import json, sys; print("\n".join(p["name"] for p in json.load(sys.stdin)["packages"]))')
own() { while read -r name; do find "$1" -mindepth 1 -maxdepth 1 -name "$name-*"; done <<<"$workspace"; }
own target/debug/.fingerprint | xargs -r rm -rf
from=$donor/target/debug
# Cargo holds this lock while it builds there, so nothing is copied half written.
flock -s -w 300 "$from/.cargo-build-lock" bash -c '
  set -e
  cp -a --reflink=always "$1/build" "$1/deps" target/debug/
  find "$1/.fingerprint" -mindepth 1 -maxdepth 1 -printf "%f\0" | { grep -zvE "^($2)-" || true; } |
    (cd "$1/.fingerprint" && xargs -0 -r cp -a --reflink=always -t "$3")' \
  _ "$from" "$(paste -sd'|' <<<"$workspace")" "$PWD/target/debug/.fingerprint"
left=$(own target/debug/.fingerprint)
[ -z "$left" ] || { echo "seed-target: a workspace fingerprint was copied: $left" >&2; rm -rf $left; exit 1; }
