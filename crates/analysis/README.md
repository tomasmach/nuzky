# nuzky-analysis

Local, synchronous analysis for editing proposals. All public results and parameter
structs implement serde. Times are integer microseconds relative to the media's
container origin; ranges are half-open.
Functions return `anyhow::Result` except the lexical `filler_words` helper.

```rust,ignore
loudness(asset: &Asset, cache: &Path, window_us: i64) -> Result<Vec<f32>>
program_loudness(asset: &Asset, cache: &Path) -> Result<ProgramLoudness>
silences(asset: &Asset, cache: &Path, params: SilenceParams) -> Result<Vec<Range>>
scene_cuts(asset: &Asset, params: SceneParams) -> Result<Vec<SceneCut>>
transcribe_words(source: AudioSource<'_>, model: &Path, vad: &Path, language: &str)
    -> Result<Transcript>
align_to_sound(words: &mut [Word], asset: &Asset, cache: &Path, aligner: Option<&Aligner>,
    cancelled: impl Fn() -> bool) -> Result<bool>
filler_words(transcript: &Transcript, language: &str) -> Vec<Range>
```

`AudioSource::Asset { asset, cache }` uses the engine's 48 kHz stereo PCM cache.
The engine's `pcm_path` includes the asset ID, source file size and modification
time (nanoseconds), so a size or mtime change generates a new cache path. A change
that preserves both size and mtime is not detected; this is not a content hash.
`Transcript` contains `language`, `words` (`start_us`, `end_us`, `text`,
`probability`) and `segments` (`start_us`, `end_us`, `text`). `SceneCut` contains
`time_us` and `score`, rather than just an integer, so callers can rank proposals.

## Behaviour and limits

- Loudness is non-overlapping RMS dBFS, including the final partial window. Channels
  contribute equally; silence is a finite -120 dBFS. `program_loudness` is ITU-R
  BS.1770-4 / EBU R128 from the engine's `loudness::Meter`, the one the Reels export levels
  with: `integrated_lufs` (K-weighted, gated; `None` when nothing passes the -70 LUFS gate)
  and `true_peak_dbtp` (4x oversampled; -120 for digital silence). It agrees with FFmpeg's
  `ebur128` within 0.1 LU and 0.1 dB.
- Silence uses 10 ms RMS windows. Default threshold is the 10th percentile +6 dB,
  capped at -35 dBFS to avoid treating a steady foreground signal as silence.
  `threshold_db: Some(dbfs)` overrides it. Runs must last 400 ms before subtracting
  120 ms at each end, including at file edges. A steady quiet music bed can be treated as
  background; loud or changing music can hide pauses. This is not speech VAD.
- Scenes compare every decoded frame at 64x36 RGB, with a minimum score of 0.18,
  an adaptive recent-motion threshold and 300 ms minimum separation. RGB catches
  equal-luma colour changes. Source orientation makes the score invariant to fixed
  rotation metadata; real PTS preserve VFR timing. Fades are excluded deliberately.
  Fast motion and flashes may yield false cuts, while visually similar cuts may
  be missed. Resolution changes within a stream depend on engine decoder support.
- Transcription uses local Whisper and Silero files; no downloads or network calls.
  Silero regions have 120 ms padding and a 300 ms minimum silence. Each region is
  compacted and decoded separately, then token ranges are mapped to source time.
  This prevents a token spanning a removed pause from being assigned to the wrong
  side. The context is loaded once, with four CPU threads, greedy decoding and no
  temperature fallback. Output is repeatable for a fixed model/build/hardware;
  cross-platform floating-point bit identity is not promised. Region isolation
  costs encoder runs and loses inter-region linguistic context. Decoding nearby
  regions together was measured 1.7-3x faster with fewer word errors, but its word
  boundaries were 3-5x further off, so regions stay separate. With `auto`, the
  language is detected once on up to 30 s of speech and used for every region.
- Subword bytes are joined before UTF-8 decoding, preserving Czech characters.
  Non-speech annotation spans and segments with high no-speech probability are
  omitted. Word probability is the mean of text-token probabilities. Token timing
  is an estimate, not forced alignment. Zero-duration token estimates are retained
  as words; they cannot be used directly as deletion ranges. The 48->16 kHz box
  filter follows the existing caption path, not a high-quality antialias resampler.
- `align_to_sound` measures word times in the sound after recognition. With an `Aligner`
  (`align_model(language)`: Czech `wav2vec2-xls-r-300m-cs-250` quantised to 8 bits, 373 MB,
  and English `wav2vec2-base-960h`, 207 MB, both Apache-2.0 GGUF files run with candle on
  the CPU), a wav2vec2 CTC model hears which letter is spoken in every 20 ms frame and a
  Viterbi path spells the words through those frames. Words a second apart in Whisper's
  times are aligned separately; a stretch over 20 s runs in windows with 2 s of context. A
  word the model cannot spell (digits, acronyms, `např.`, letters outside its alphabet) is
  one placeholder per character, and with words the model hears too faintly it shares the
  gap between measured neighbours in proportion to Whisper's durations. A stretch whose
  path costs over 1 nat per frame more than the model's own guess (other speech, music,
  a hallucinated sentence) keeps Whisper's times. Measured edges then follow the sound
  into the gap while it stays within 20 dB of the word's own level, so a held vowel before
  a pause ends where it ends and the room's echo is not the word; between two measured
  words with no quiet moment the cut is the middle of the gap the model left. Without a
  model the words keep `align_words`: Whisper's estimates moved into the nearest pause.
  On 25 s of real Czech connected speech (`tests/alignment.rs`) the model's boundaries
  land on average 4 ms and at most 30 ms past the hand-checked ranges; `align_words`
  alone missed by 85 ms on average and up to 270 ms. The Czech model measured those 25 s in
  5.7 s on a 16-thread Ryzen 7 5800X3D; a stop is noticed between its layers.
- Czech fillers are `ehm`, `hmm`, `eee`; English fillers are `um`, `uh`, `erm`.
  `jakoby`, `prostě`, `like`, and `you know` require >=150 ms pauses or commas on
  both sides (file edges count). Adjacent repeated words separated by <=250 ms
  propose deleting earlier copies, keeping the last. Sentence punctuation blocks
  repetition matching. This is conservative lexical detection, not semantic
  understanding: intentional repetition and ambiguous discourse words need review.
  Unsupported languages produce no filler proposals. `auto` uses transcript language.

## CLI

```sh
cargo run -p nuzky-analysis --bin nuzky-analyze -- clip.mp4 silences
cargo run -p nuzky-analysis --bin nuzky-analyze -- clip.mp4 words --lang cs \
  --model /path/ggml-small.bin --vad-model /path/ggml-silero-v5.1.2.bin
```

Commands: `loudness`, `silences`, `scenes`, `words`, `fillers`. `words` prints Whisper's
own estimates, before `align_to_sound`. Output is one compact
JSON value on stdout; errors and native diagnostics go to stderr. Loudness reports
100 ms RMS windows plus `integrated_lufs` (null for silence) and `true_peak_dbtp`. `--cache` selects
a cache directory; default is the OS temp directory's `nuzky-analysis` folder.
CLI cache IDs depend on canonical path, file size and modification time. API callers
keep asset IDs stable; `pcm_path` performs the same size+mtime invalidation for them.

Model discovery checks `$XDG_DATA_HOME/nuzky/models` (or
`~/.local/share/nuzky/models`) and `tmp-test/xdg/data/nuzky/models`. Explicit model
paths take priority. Silero is also looked up alongside the chosen Whisper model.

## Verification

`cargo test -p nuzky-analysis` includes synthetic tones/noisy pauses, UTF-8 token
joining and time mapping, conservative filler/stutter cases, and FFmpeg-generated
hard cuts, slow fades, rotation and VFR. FFmpeg with libx264 must be available.
Fixtures stay under `tmp-test/analysis`. Real model checks are separate so ordinary
tests do not require hundreds of MB of model files.
