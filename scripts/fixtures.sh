#!/usr/bin/env bash
# Creates the media and models that `cargo test -- --ignored` and scripts/repro.py read from tmp-test/.
# Media are synthetic (FFmpeg test patterns, espeak-ng speech) or public-domain recordings pinned by SHA-256;
# nothing is committed.
# Models are downloaded and media made once per machine, in ~/.cache/nuzky/deps/fixtures (or $NUZKY_DEPS/fixtures),
# and every checkout takes a copy-on-write clone. Models are checked against the SHA-256 the app pins in
# src-tauri/src/jobs.rs. Media are made again when this script, the reel takes or the FFmpeg or espeak-ng version
# change. Otherwise existing files are kept; delete one to create it again.
set -euo pipefail
cd "$(dirname "$0")/.."

out=tmp-test
models=$out/xdg/data/nuzky/models
shared=${NUZKY_DEPS:-$HOME/.cache/nuzky/deps}/fixtures
mkdir -p "$out/engine-evidence" "$models" "$shared"
for tool in ffmpeg espeak-ng curl sha256sum; do
  command -v "$tool" >/dev/null || { echo "fixtures: $tool is required" >&2; exit 1; }
done

# A clone shares disk blocks until one side changes; a hard link where the file system cannot clone.
# Placed under a temporary name and renamed, so an interrupted copy is never taken for a whole file and a hard
# link never writes through into the shared copy.
place() {
  local part=$2.part.$$
  mkdir -p "$(dirname "$2")"
  rm -f "$part"
  cp --reflink=always "$1" "$part" 2>/dev/null || ln "$1" "$part" 2>/dev/null || cp "$1" "$part"
  mv -f "$part" "$2"
}

takes=tests/e2e/reel_takes.tsv
key=$({ cat scripts/fixtures.sh "$takes"; ffmpeg -version | head -1; espeak-ng --version; } | sha256sum | cut -c1-16)
made=$shared/media-$key
current=$([ "$(cat "$out/.media-key" 2>/dev/null)" = "$key" ] && echo yes || echo no)
# Writes to a temporary name next to the target, so an interrupted run leaves no half file behind.
media() {
  local file=$out/$1 cached=$made/$1
  shift
  [ "$current" = yes ] && [ -s "$file" ] && return
  if [ -s "$cached" ]; then
    place "$cached" "$file"
    return
  fi
  echo "> $file"
  local part
  part=$(dirname "$file")/.part-$(basename "$file")
  "$@" "$part"
  mv "$part" "$file"
  place "$file" "$cached"
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
# An hour of speech-like sound with pauses, for loading the waveform of a long recording (tests/e2e/waveform.py).
media hour.m4a ff -f lavfi \
  -i "aevalsrc=0.5*sin(2*PI*180*t)*(0.55+0.45*sin(2*PI*0.37*t))*gt(sin(2*PI*0.11*t)+0.3\,0):s=48000:c=stereo:d=3600" \
  -c:a aac -b:a 128k
media engine-evidence/identity.png ff -f lavfi -i testsrc2=s=540x960 -frames:v 1 -update 1

# The Czech talking head of the reel flow, spoken from tests/e2e/reel_takes.tsv: one file per take, each
# phrase followed by its pause. Quiet pink room tone, so pauses are not digital silence, and speech around
# -25 LUFS, so the export has to raise it to the Reels level. A changed list makes the takes again.
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
# Every download is kept once per machine under its SHA-256, so a new checkout clones the gigabytes of models
# instead of fetching them again. fetch <file> <sha256> <curl arguments ending with the URL>
fetch() {
  local file=$1 sha=$2 stored=$shared/$2
  shift 2
  if ! { [ -f "$stored" ] && sha256_is "$stored" "$sha"; }; then
    local part
    part=$(mktemp "$stored.part.XXXXXX")
    curl --fail --location --silent --show-error --output "$part" "$@"
    sha256_is "$part" "$sha" || { rm -f "$part"; echo "fixtures: $file does not match its SHA-256" >&2; exit 1; }
    mv "$part" "$stored"
  fi
  place "$stored" "$file"
}
model() {
  local file=$models/$1 sha=$2 url=$3
  if [ -f "$file" ] && sha256_is "$file" "$sha"; then return; fi
  echo "> $file"
  fetch "$file" "$sha" "$url"
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
# The face and subject models of cover frames and masks, pinned like the app pins them (crates/vision/src/models.rs).
model yunet-2026may.onnx ebafce4e3c118d6554634be5c27ab333b4c047a9a8c3faf1d7cf93101c22f0f0 \
  https://media.githubusercontent.com/media/opencv/opencv_zoo/26cc381e4d2594bb9f47a26eb8fd96c94a13660d/models/face_detection_yunet/face_detection_yunet_2026may.onnx
vision_models=https://raw.githubusercontent.com/tomasmach/nuzky-models/a4f7e3c99e0b1b971d69595a1c3efb64fbc18730
model face-landmarks-v2.onnx 6fc4bae7e3e2c5c0870e8d1b764839c7dcefee7e96c874b454da8be514e19f47 \
  $vision_models/face-landmarks-v2.onnx
model face-blendshapes-v2.onnx 74029fcef4076695dd1129d9c45a3d766532745868bd1eb3cc1deff87e7d6d66 \
  $vision_models/face-blendshapes-v2.onnx
model selfie-segmenter.onnx 7759b1df460279b03bb39813ee1cec7bc3a1a4be38006e98ddd48294c92bd9f4 \
  $vision_models/selfie-segmenter.onnx
model birefnet-lite.onnx 8fd304fd859a8dc999a4a93f1fb58f4c9a6bf575de64d95c2a78027e7964d7be \
  https://github.com/tomasmach/nuzky-models/releases/download/birefnet-lite-1/birefnet-lite.onnx

# Real Czech connected speech for the word timing test (crates/analysis/tests/alignment.rs): 25 s of chapter 2
# of Krysař by Viktor Dyk, read for LibriVox and released into the public domain. Its word boundaries were
# checked by hand in crates/analysis/tests/data/krysar-cs-boundaries.tsv; the cut must stay exactly this one.
krysar() {
  local mp3=$out/.krysar-02.mp3 sha=2ed724c0d782029b4b5219fd86c52682ac33acda1ca6dda3f0ebb8a95f88659b
  if ! { [ -f "$mp3" ] && sha256_is "$mp3" "$sha"; }; then
    fetch "$mp3" "$sha" https://archive.org/download/krysar_2007_librivox/krysar_02_dyk_64kb.mp3
  fi
  ff -ss 97.5 -t 25.1 -i "$mp3" -ac 1 -c:a pcm_s16le -f wav "$1"
}
media krysar-cs.wav krysar

# A real face for thumbnail frame choice and person masking (crates/vision/tests): the first 10 MiB (13 s) of a
# NASA live interview with astronaut Jeanette Epps, 4 October 2019, a US government work in the public domain
# (https://images.nasa.gov/details/iss061m2627771232_Live_Interviews_Jeanette_Epps_191004). The S3 object is
# versioned and has not changed since 2019, so the byte range is pinned like a whole file. Each still is a 9:16
# crop below the black top rows, scaled to 1080x1920: at 8.992 s she looks into the camera, at 9.159 s both eyes
# are shut mid-blink. Every -ss sits half a 59.94 fps frame before its frame. The outline of the person in
# face-open.png, crates/vision/tests/data/face-open-person.json, was drawn by hand on this exact crop.
epps() {
  local mp4=$out/.epps-2019-10-04.mp4 sha=0ceeb3bc5a834bbbc470b876ef959cf364da57d36a34b2b288704914202af105
  local id=iss061m2627771232_Live_Interviews_Jeanette_Epps_191004
  if ! { [ -f "$mp4" ] && sha256_is "$mp4" "$sha"; }; then
    fetch "$mp4" "$sha" --range 0-10485759 --max-filesize 10485760 "https://images-assets.nasa.gov/video/$id/$id~large.mp4"
  fi
  ff -ss "$1" -i "$mp4" -frames:v 1 -vf "crop=396:704:447:8,scale=1080:1920:flags=lanczos,format=rgb24" "$2"
}
media face-open.png epps 8.984
media face-blink.png epps 9.151
# Twelve seconds at 30 fps in 3 s segments the tests rely on: [0,3) open eyes under a strong blur, [3,6) the
# blink, [6,12) sharp open eyes (two segments of identical frames). A keyframe starts every segment.
face_thumb() {
  ff -loop 1 -framerate 30 -t 3 -i "$out/face-open.png" -loop 1 -framerate 30 -t 3 -i "$out/face-blink.png" \
    -loop 1 -framerate 30 -t 6 -i "$out/face-open.png" \
    -filter_complex "[0:v]gblur=sigma=8[blur];[blur][1:v][2:v]concat=n=3:v=1:a=0,format=yuv420p" \
    -c:v libx264 -preset veryfast -crf 18 -r 30 -force_key_frames 0,3,6,9 -an "$1"
}
media face-thumb.mp4 face_thumb
echo "$key" >"$out/.media-key"
