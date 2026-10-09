#!/usr/bin/env bash
# Creates the media and models that `cargo test -- --ignored` and scripts/repro.py read from tmp-test/.
# Media are synthetic (FFmpeg test patterns, espeak-ng speech), so no real video is needed or committed.
# Models are downloaded once and checked against the SHA-256 the app pins in src-tauri/src/jobs.rs.
# Existing files are kept; delete one to create it again.
set -euo pipefail
cd "$(dirname "$0")/.."

out=tmp-test
models=$out/xdg/data/nuzky/models
mkdir -p "$out/engine-evidence" "$models"
for tool in ffmpeg espeak-ng curl sha256sum; do
  command -v "$tool" >/dev/null || { echo "fixtures: $tool is required" >&2; exit 1; }
done

# Writes to a temporary name next to the target, so an interrupted run leaves no half file behind.
media() {
  local file=$out/$1
  shift
  [ -s "$file" ] && return
  echo "> $file"
  local part
  part=$(dirname "$file")/.part-$(basename "$file")
  "$@" "$part"
  mv "$part" "$file"
}
ff() { ffmpeg -v error -y "$@"; }

speech="Welcome back to the channel. Today we cut a short video on the laptop. First we import the clips, \
then we remove the pauses and add captions, so everyone can follow along without sound."

say() { espeak-ng -v en-us -s 150 -w "$1" "$speech"; }
media speech.wav say
media talk.mp4 ff -f lavfi -i testsrc2=s=1080x1920:r=30 -i "$out/speech.wav" -shortest \
  -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -b:a 128k
media wide.mp4 ff -f lavfi -i testsrc2=s=1920x1080:r=25:d=5 -f lavfi -i sine=frequency=440:duration=5 \
  -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -b:a 128k
media portrait.mp4 ff -f lavfi -i testsrc2=s=1080x1920:r=30:d=6 -f lavfi -i sine=frequency=660:duration=6 \
  -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -b:a 128k
# Phone-like HEVC with a variable frame rate: three of every five 60 fps frames, original timestamps.
media phone_hevc_vfr.mov ff -f lavfi -i testsrc2=s=1920x1080:r=60:d=4 -f lavfi -i sine=frequency=550:duration=4 \
  -vf "select='lt(mod(n\,5)\,3)'" -fps_mode vfr -c:v libx265 -x265-params log-level=error -tag:v hvc1 \
  -pix_fmt yuv420p -c:a aac -b:a 128k
media music.mp3 ff -f lavfi -i "aevalsrc=0.3*sin(2*PI*(330+110*floor(mod(t\,4)))*t):s=44100:d=20" \
  -c:a libmp3lame -b:a 128k
# Ten minutes with a keyframe every ten seconds, for filmstrip timing.
media filmstrip-long.mp4 ff -f lavfi -i testsrc=s=1920x1080:r=25:d=600 -c:v libx264 -preset ultrafast \
  -g 250 -pix_fmt yuv420p -an
media engine-evidence/identity.png ff -f lavfi -i testsrc2=s=540x960 -frames:v 1 -update 1

# The Czech talking head of the reel flow, spoken from tests/e2e/reel_takes.tsv: one file per take, each
# phrase followed by its pause. Quiet pink room tone, so pauses are not digital silence, and speech around
# -25 LUFS, so the export has to raise it to the Reels level. A changed list makes the takes again.
takes=tests/e2e/reel_takes.tsv
reel_take() {
  local take=$1 colour=$2 out=$3 dir n=0 inputs=()
  dir=$(mktemp -d)
  while IFS=$'\t' read -r number pause _ phrase; do
    [ "$number" = "$take" ] || continue
    espeak-ng -v cs -s 150 -w "$dir/$n.wav" "$phrase"
    ff -i "$dir/$n.wav" -af "aresample=48000,apad=pad_dur=$pause" -ac 1 "$dir/p$n.wav"
    inputs+=(-i "$dir/p$n.wav")
    n=$((n + 1))
  done < <(grep -v '^#' "$takes")
  ff "${inputs[@]}" -filter_complex "concat=n=$n:v=0:a=1,volume=-5dB" "$dir/speech.wav"
  local seconds
  seconds=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$dir/speech.wav")
  ff -i "$dir/speech.wav" -f lavfi -i "anoisesrc=color=pink:amplitude=0.004:seed=$take:sample_rate=48000:duration=$seconds" \
    -f lavfi -i "color=c=$colour:s=1080x1920:r=30:d=$seconds,noise=alls=10:allf=u" \
    -filter_complex "[0:a][1:a]amix=inputs=2:normalize=0,aformat=channel_layouts=mono[a]" -map 2:v -map "[a]" \
    -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -b:a 128k -ar 48000 -shortest "$out"
  rm -rf "$dir"
}
for take in 1 2 3; do
  [ "$takes" -nt "$out/reel-$take.mp4" ] && rm -f "$out/reel-$take.mp4"
done
media reel-1.mp4 reel_take 1 0x2b3a4a
media reel-2.mp4 reel_take 2 0x3a2b4a
media reel-3.mp4 reel_take 3 0x2b4a3a

# A talking head in a noisy room for the voice flow: four phrases with a second of pause after each, pink
# room noise around -40 dBFS and 50 Hz mains hum around -30 dBFS under them, the sound Clean voice is for.
noisy_talk() {
  local out=$1 dir n=0 inputs=() seconds
  dir=$(mktemp -d)
  for phrase in "Hello and welcome back to the channel." "The air conditioning in this room is really loud." \
    "Listen to the hum under my voice." "Clean voice should take most of it away."; do
    espeak-ng -v en-us -s 150 -w "$dir/$n.wav" "$phrase"
    ff -i "$dir/$n.wav" -af "aresample=48000,apad=pad_dur=1" -ac 1 "$dir/p$n.wav"
    inputs+=(-i "$dir/p$n.wav")
    n=$((n + 1))
  done
  ff "${inputs[@]}" -filter_complex "concat=n=$n:v=0:a=1,adelay=1000,volume=-6dB" "$dir/speech.wav"
  seconds=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$dir/speech.wav")
  ff -i "$dir/speech.wav" -f lavfi -i "anoisesrc=color=pink:amplitude=0.05:seed=11:sample_rate=48000:duration=$seconds" \
    -f lavfi -i "sine=frequency=50:sample_rate=48000:duration=$seconds" \
    -f lavfi -i "color=c=0x3a3a2b:s=1080x1920:r=30:d=$seconds,noise=alls=10:allf=u" \
    -filter_complex "[2:a]volume=-9dB[h];[0:a][1:a][h]amix=inputs=3:normalize=0,aformat=channel_layouts=mono[a]" \
    -map 3:v -map "[a]" -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -b:a 128k -ar 48000 -shortest "$out"
  rm -rf "$dir"
}
media voice.mp4 noisy_talk

# Compares digests directly: macOS ships a BSD sha256sum without GNU's --check from stdin.
sha256_is() { [ "$(sha256sum "$1" | cut -d' ' -f1)" = "$2" ]; }
model() {
  local file=$models/$1 sha=$2 url=$3
  if [ -f "$file" ] && sha256_is "$file" "$sha"; then return; fi
  echo "> $file"
  curl --fail --location --silent --show-error --output "$file.part" "$url"
  sha256_is "$file.part" "$sha" || { echo "fixtures: $file.part does not match its SHA-256" >&2; exit 1; }
  mv "$file.part" "$file"
}
model ggml-small.bin 1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
# The model the app picks for Czech when it is installed; the reel flow recognises Czech with it.
model ggml-large-v3-turbo-q5_0.bin 394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2 \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin
model ggml-silero-v5.1.2.bin 29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf \
  https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin
# The word timing models the app downloads for Czech and English transcripts (crates/analysis/src/align.rs).
model wav2vec2-xls-r-300m-cs-250-q8_0.gguf bde0e0d90ae14c60ffea627ccdd7ba263ea6d4a3d27882476fce32a257914eb8 \
  https://huggingface.co/cstr/wav2vec2-xls-r-300m-cs-250-GGUF/resolve/a264af811793d1f32a6ff3ce7de7ad9b2dcfdd40/wav2vec2-xls-r-300m-cs-250-q8_0.gguf
model wav2vec2-base-960h.gguf 298d900e715936118c1476a5b246ceda08c1942411d350f38ccb02da1eac3cf7 \
  https://huggingface.co/cstr/wav2vec2-base-960h-GGUF/resolve/7c025ea07cffc65b211b360e5865dfe59df7e8af/wav2vec2-base-960h.gguf

# Real Czech connected speech for the word timing test (crates/analysis/tests/alignment.rs): 25 s of chapter 2
# of Krysař by Viktor Dyk, read for LibriVox and released into the public domain. Its word boundaries were
# checked by hand in crates/analysis/tests/data/krysar-cs-boundaries.tsv; the cut must stay exactly this one.
krysar() {
  local mp3=$out/.krysar-02.mp3 sha=2ed724c0d782029b4b5219fd86c52682ac33acda1ca6dda3f0ebb8a95f88659b
  if ! { [ -f "$mp3" ] && sha256_is "$mp3" "$sha"; }; then
    curl --fail --location --silent --show-error --output "$mp3.part" \
      https://archive.org/download/krysar_2007_librivox/krysar_02_dyk_64kb.mp3
    sha256_is "$mp3.part" "$sha" || { echo "fixtures: $mp3.part does not match its SHA-256" >&2; exit 1; }
    mv "$mp3.part" "$mp3"
  fi
  ff -ss 97.5 -t 25.1 -i "$mp3" -ac 1 -c:a pcm_s16le -f wav "$1"
}
media krysar-cs.wav krysar
