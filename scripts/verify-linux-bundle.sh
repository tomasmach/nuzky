#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$repo/target}"
target="$(cd "$target" && pwd)"
binary="$target/release/capopen-app"
ldd "$binary" | tee "$target/release/ldd.txt"
if grep -q 'not found' "$target/release/ldd.txt"; then
  echo 'Unresolved runtime dependency' >&2
  exit 1
fi
shopt -s nullglob
images=("$target"/release/bundle/appimage/*.AppImage)
test "${#images[@]}" -eq 1
extract="$(mktemp -d)"
trap 'rm -rf "$extract"' EXIT
(cd "$extract" && "${images[0]}" --appimage-extract >/dev/null)
test -x "$extract/squashfs-root/AppRun"
app_binary="$extract/squashfs-root/usr/bin/capopen-app"
test -x "$app_binary"
libs="$(find "$extract/squashfs-root/usr" -type d -name 'lib*' -print | paste -sd :)"
LD_LIBRARY_PATH="$libs" ldd "$app_binary" | tee "$target/release/appimage-ldd.txt"
if grep -q 'not found' "$target/release/appimage-ldd.txt"; then
  echo 'Unresolved extracted AppImage dependency' >&2
  exit 1
fi
for lib in avcodec avformat avutil swscale swresample; do
  find "$extract/squashfs-root" -name "lib$lib.so.*" -print -quit | grep -q .
done
stat --printf='%n: %s bytes\n' "${images[0]}" "$binary"
