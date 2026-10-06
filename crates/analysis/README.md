# capopen-analysis

Local, synchronous analysis for editing proposals. All public results and parameter
structs implement serde. Times are integer microseconds relative to the media's
container origin; ranges are half-open.
Functions return `anyhow::Result` except the lexical `filler_words` helper.

```rust,ignore
loudness(asset: &Asset, cache: &Path, window_us: i64) -> Result<Vec<f32>>
integrated_lufs(asset: &Asset, cache: &Path) -> Result<f32>
silences(asset: &Asset, cache: &Path, params: SilenceParams) -> Result<Vec<Range>>
scene_cuts(asset: &Asset, params: SceneParams) -> Result<Vec<SceneCut>>
transcribe_words(source: AudioSource<'_>, model: &Path, vad: &Path, language: &str)
    -> Result<Transcript>
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
  contribute equally; silence is a finite -120 dBFS. `integrated_lufs` is only the
  ungated, unweighted stereo energy approximation `-0.691 + 10 log10(sum powers)`.
  It is not BS.1770 LUFS and must not drive delivery normalization.
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
  costs encoder runs and loses inter-region linguistic context.
- Subword bytes are joined before UTF-8 decoding, preserving Czech characters.
  Non-speech annotation spans and segments with high no-speech probability are
  omitted. Word probability is the mean of text-token probabilities. Token timing
  is an estimate, not forced alignment. Zero-duration token estimates are retained
  as words; they cannot be used directly as deletion ranges. The 48->16 kHz box
  filter follows the existing caption path, not a high-quality antialias resampler.
- Czech fillers are `ehm`, `hmm`, `eee`; English fillers are `um`, `uh`, `erm`.
  `jakoby`, `prostě`, `like`, and `you know` require >=150 ms pauses or commas on
  both sides (file edges count). Adjacent repeated words separated by <=250 ms
  propose deleting earlier copies, keeping the last. Sentence punctuation blocks
  repetition matching. This is conservative lexical detection, not semantic
  understanding: intentional repetition and ambiguous discourse words need review.
  Unsupported languages produce no filler proposals. `auto` uses transcript language.

## CLI

```sh
cargo run -p capopen-analysis --bin capopen-analyze -- clip.mp4 silences
cargo run -p capopen-analysis --bin capopen-analyze -- clip.mp4 words --lang cs \
  --model /path/ggml-small.bin --vad-model /path/ggml-silero-v5.1.2.bin
```

Commands: `loudness`, `silences`, `scenes`, `words`, `fillers`. Output is one compact
JSON value on stdout; errors and native diagnostics go to stderr. Loudness reports
100 ms windows plus the explicitly named `integrated_lufs_approx`. `--cache` selects
a cache directory; default is the OS temp directory's `capopen-analysis` folder.
CLI cache IDs depend on canonical path, file size and modification time. API callers
keep asset IDs stable; `pcm_path` performs the same size+mtime invalidation for them.

Model discovery checks `$XDG_DATA_HOME/capopen/models` (or
`~/.local/share/capopen/models`) and `tmp-test/xdg/data/capopen/models`. Explicit model
paths take priority. Silero is also looked up alongside the chosen Whisper model.

## Verification

`cargo test -p capopen-analysis` includes synthetic tones/noisy pauses, UTF-8 token
joining and time mapping, conservative filler/stutter cases, and FFmpeg-generated
hard cuts, slow fades, rotation and VFR. FFmpeg with libx264 must be available.
Fixtures stay under `tmp-test/analysis`. Real model checks are separate so ordinary
tests do not require hundreds of MB of model files.
