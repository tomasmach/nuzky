use std::path::Path;

use anyhow::{Context, Result, ensure};
use capopen_engine::model::Asset;
pub use capopen_engine::speech::Word;
use serde::{Deserialize, Serialize};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperVadContext, WhisperVadContextParams,
    WhisperVadParams,
};

use crate::{Range, audio::open_pcm};

const RATE: usize = 16_000;
const CS: usize = RATE / 100;
const GAP: usize = RATE / 5;
/// Audio Whisper encodes in one pass.
const WINDOW: usize = 30 * RATE;

pub enum AudioSource<'a> {
    Asset { asset: &'a Asset, cache: &'a Path },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start_us: i64,
    pub end_us: i64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// Detected ISO language, or the requested code. "auto" if no speech was found.
    pub language: String,
    pub words: Vec<Word>,
    pub segments: Vec<Segment>,
}

/// Local transcription, greedy decoding without random temperature fallback.
/// Silero VAD compaction is explicitly mapped back to source time. Token times are
/// Whisper estimates, not forced alignment; inspect before destructive editing.
pub fn transcribe_words(
    source: AudioSource<'_>,
    model_path: &Path,
    vad_model_path: &Path,
    language: &str,
) -> Result<Transcript> {
    transcribe_words_cancellable(source, model_path, vad_model_path, language, || false)
}

pub fn transcribe_words_cancellable(
    source: AudioSource<'_>,
    model_path: &Path,
    vad_model_path: &Path,
    language: &str,
    cancelled: impl Fn() -> bool,
) -> Result<Transcript> {
    let check = || {
        ensure!(!cancelled(), "CANCELLED: transcription cancelled");
        Ok::<_, anyhow::Error>(())
    };
    check()?;
    ensure!(
        language == "auto" || (!language.contains('\0') && whisper_rs::get_lang_id(language).is_some()),
        "Unknown language: {language}"
    );
    ensure!(model_path.is_file(), "Speech model not found: {}", model_path.display());
    let converted;
    let audio = match source {
        AudioSource::Asset { asset, cache } => {
            let pcm = open_pcm(asset, cache)?;
            // Average three 48 kHz stereo frames (six channel samples) into one 16 kHz mono sample.
            converted = pcm.samples().chunks(6).map(|s| s.iter().sum::<f32>() / s.len() as f32).collect::<Vec<_>>();
            &converted
        }
    };
    ensure!(audio.iter().all(|s| s.is_finite()), "Speech audio contains non-finite samples");
    let mut transcript = Transcript { language: language.into(), words: Vec::new(), segments: Vec::new() };
    if audio.is_empty() {
        return Ok(transcript);
    }
    check()?;
    let regions = voice_regions(audio, vad_model_path)?;
    check()?;
    if regions.is_empty() {
        return Ok(transcript);
    }
    let mut context_params = WhisperContextParameters::default();
    context_params.use_gpu(cfg!(feature = "gpu"));
    let context = match WhisperContext::new_with_params(model_path, context_params) {
        Ok(context) => context,
        Err(error) if cfg!(feature = "gpu") => {
            eprintln!("GPU speech context failed ({error}); retrying on CPU");
            let mut cpu_params = WhisperContextParameters::default();
            cpu_params.use_gpu(false);
            WhisperContext::new_with_params(model_path, cpu_params)
                .context("Loading speech model on CPU after GPU failure")?
        }
        Err(error) => return Err(error).context("Loading speech model"),
    };
    if transcript.language == "auto" {
        // Detect once on up to 30 s of speech: the first region alone may be a short
        // greeting, and its guess would then decide every later region.
        let enough = regions.iter().scan(0, |len, &(start, end)| {
            (*len < WINDOW).then(|| {
                *len += end - start + GAP;
                (start, end)
            })
        });
        let (speech, _) = compact_audio(audio, &enough.collect::<Vec<_>>());
        // Its own state: recognising on the detecting one marked the first region as no speech.
        let mut detect = context.create_state().context("Creating speech recognition state")?;
        let mut params = recognition_params("auto");
        params.set_detect_language(true);
        detect.full(params, &speech).context("Detecting the spoken language")?;
        transcript.language =
            whisper_rs::get_lang_str(detect.full_lang_id_from_state()).context("Missing detected language")?.into();
        check()?;
    }
    let mut state = context.create_state().context("Creating speech recognition state")?;
    // Independent decoding prevents tokens from drifting across compacted pauses.
    for region in regions {
        check()?;
        let (compact, mapping) = compact_audio(audio, &[region]);
        state.full(recognition_params(&transcript.language), &compact).context("Recognising speech region")?;
        check()?;
        transcript.language =
            whisper_rs::get_lang_str(state.full_lang_id_from_state()).context("Missing detected language")?.into();
        append_segments(&state, context.token_eot(), &mapping, &mut transcript, &cancelled)?;
    }
    Ok(transcript)
}

fn recognition_params(language: &str) -> FullParams<'_, '_> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(4);
    params.set_language((language != "auto").then_some(language));
    params.set_token_timestamps(true);
    params.set_split_on_word(true);
    params.set_no_context(true);
    params.set_temperature(0.0);
    params.set_temperature_inc(0.0);
    params.set_suppress_nst(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params
}

fn append_segments(
    state: &whisper_rs::WhisperState,
    end_token: i32,
    mapping: &[Mapping],
    transcript: &mut Transcript,
    cancelled: &impl Fn() -> bool,
) -> Result<()> {
    for segment in state.as_iter() {
        ensure!(!cancelled(), "CANCELLED: transcription cancelled");
        let text = segment.to_str_lossy().context("Reading transcript segment")?;
        if is_annotation(&text) || segment.no_speech_probability() > 0.6 {
            continue;
        }
        let mut words = WordBuilder::default();
        for index in 0..segment.n_tokens() {
            let token = segment.get_token(index).context("Missing speech token")?;
            if token.token_id() >= end_token {
                continue;
            }
            let data = token.token_data();
            let start = data.t0.max(0).saturating_mul(10_000);
            let end = data.t1.max(data.t0).max(0).saturating_mul(10_000);
            words.push(token.to_bytes().context("Reading speech token")?, start, end, data.p);
        }
        let compact_words = words.finish();
        let mut segment_words = Vec::new();
        for mut word in compact_words {
            if let Some(range) = original_range(mapping, word.start_us, word.end_us) {
                word.start_us = range.start_us;
                word.end_us = range.end_us;
                segment_words.push(word);
            }
        }
        // Segment boundaries must not reintroduce the silence removed by VAD.
        for group in segment_words.chunk_by(|a, b| b.start_us - a.end_us < 300_000) {
            if let (Some(first), Some(last)) = (group.first(), group.last()) {
                transcript.segments.push(Segment {
                    start_us: first.start_us,
                    end_us: last.end_us,
                    text: group.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
                });
            }
        }
        transcript.words.extend(segment_words);
    }
    Ok(())
}

fn voice_regions(audio: &[f32], model: &Path) -> Result<Vec<(usize, usize)>> {
    let model = model.to_str().context("VAD model path must be UTF-8")?;
    ensure!(!model.contains('\0'), "VAD model path contains a NUL byte");
    let mut context_params = WhisperVadContextParams::new();
    context_params.set_n_threads(4);
    context_params.set_use_gpu(false);
    let mut context = WhisperVadContext::new(model, context_params).context("Loading voice detector")?;
    let mut params = WhisperVadParams::new();
    params.set_speech_pad(120);
    params.set_min_silence_duration(300);
    let segments = context.segments_from_samples(params, audio).context("Detecting speech")?;
    let mut regions: Vec<(usize, usize)> = Vec::new();
    for segment in segments {
        let start = ((segment.start.max(0.0) as f64 * CS as f64).round() as usize).min(audio.len());
        let end = ((segment.end.max(0.0) as f64 * CS as f64).round() as usize).min(audio.len());
        if end <= start {
            continue;
        }
        match regions.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => regions.push((start, end)),
        }
    }
    Ok(regions)
}

struct Mapping {
    compact_start: usize,
    source_start: usize,
    len: usize,
}

fn compact_audio(audio: &[f32], regions: &[(usize, usize)]) -> (Vec<f32>, Vec<Mapping>) {
    let mut joined = Vec::new();
    let mut mapping = Vec::new();
    for &(start, end) in regions {
        if !joined.is_empty() {
            joined.resize(joined.len() + GAP, 0.0);
        }
        mapping.push(Mapping { compact_start: joined.len(), source_start: start, len: end - start });
        joined.extend_from_slice(&audio[start..end]);
    }
    // Whisper rejects very short buffers; this tail is deliberately absent from the map.
    joined.resize(joined.len().max(RATE), 0.0);
    (joined, mapping)
}

fn sample_us(sample: usize) -> i64 {
    (sample as i128 * 1_000_000 / RATE as i128) as i64
}

fn original_range(mapping: &[Mapping], start_us: i64, end_us: i64) -> Option<Range> {
    let mut best = None;
    let mut overlap = 0;
    for entry in mapping {
        let start = start_us.max(sample_us(entry.compact_start));
        let end = end_us.min(sample_us(entry.compact_start + entry.len));
        if start_us == end_us
            && start_us >= sample_us(entry.compact_start)
            && start_us < sample_us(entry.compact_start + entry.len)
        {
            let time = start_us + sample_us(entry.source_start) - sample_us(entry.compact_start);
            return Some(Range { start_us: time, end_us: time });
        }
        if end - start > overlap {
            overlap = end - start;
            let offset = sample_us(entry.source_start) - sample_us(entry.compact_start);
            best = Some(Range { start_us: start + offset, end_us: end + offset });
        }
    }
    best
}

#[derive(Default)]
struct WordBuilder {
    bytes: Vec<u8>,
    start: i64,
    end: i64,
    probability: f64,
    tokens: usize,
    words: Vec<Word>,
}

impl WordBuilder {
    fn push(&mut self, bytes: &[u8], start: i64, end: i64, probability: f32) {
        for part in bytes.split_inclusive(u8::is_ascii_whitespace) {
            if part.first().is_some_and(u8::is_ascii_whitespace) {
                self.flush();
            }
            let content = part.trim_ascii();
            if !content.is_empty() {
                if self.bytes.is_empty() {
                    self.start = start;
                }
                self.end = end;
                self.bytes.extend_from_slice(content);
                self.probability += if probability.is_finite() { probability.clamp(0.0, 1.0) as f64 } else { 0.0 };
                self.tokens += 1;
            }
            if part.last().is_some_and(u8::is_ascii_whitespace) {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        // Tokens can split a Czech UTF-8 character; decode only after joining bytes.
        let text = String::from_utf8_lossy(&self.bytes).into_owned();
        self.words.push(Word {
            start_us: self.start,
            end_us: self.end,
            text,
            probability: (self.probability / self.tokens.max(1) as f64) as f32,
        });
        self.bytes.clear();
        self.probability = 0.0;
        self.tokens = 0;
    }

    fn finish(mut self) -> Vec<Word> {
        self.flush();
        let mut closing = None;
        self.words.retain(|word| {
            let text = word.text.trim_matches('.');
            if closing.is_none() {
                closing = match text.chars().next() {
                    Some('[') => Some(']'),
                    Some('(') => Some(')'),
                    Some('*') => Some('*'),
                    _ => None,
                };
            }
            if let Some(end) = closing {
                if text.ends_with(end) {
                    closing = None;
                }
                return false;
            }
            !is_annotation(text) && text.chars().any(char::is_alphanumeric)
        });
        self.words
    }
}

fn is_annotation(text: &str) -> bool {
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '.');
    [('[', ']'), ('(', ')'), ('*', '*')].iter().any(|&(a, b)| text.starts_with(a) && text.ends_with(b))
        || text.chars().all(|c| c == '♪' || c == '♫' || c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_maps_boundaries_without_spanning_removed_pause() {
        let audio = vec![0.5; 160_000];
        let (_, mapping) = compact_audio(&audio, &[(16_000, 32_000), (112_000, 128_000)]);
        assert_eq!(original_range(&mapping, 200_000, 400_000), Some(Range { start_us: 1_200_000, end_us: 1_400_000 }));
        assert_eq!(
            original_range(&mapping, 1_200_000, 1_500_000),
            Some(Range { start_us: 7_000_000, end_us: 7_300_000 })
        );
        assert_eq!(original_range(&mapping, 1_000_000, 1_200_000), None);
        assert_eq!(
            original_range(&mapping, 900_000, 1_250_000),
            Some(Range { start_us: 1_900_000, end_us: 2_000_000 })
        );
    }

    #[test]
    fn joins_utf8_subwords_and_drops_annotations() {
        let mut builder = WordBuilder::default();
        builder.push(b" P\xc5", 0, 100_000, 0.8);
        builder.push(b"\x99\xc3\xad", 100_000, 200_000, 0.9);
        builder.push(b"klad", 200_000, 300_000, 1.0);
        builder.push(b". [Music playing] Ahoj", 300_000, 500_000, 0.9);
        let words = builder.finish();
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "Příklad.");
        assert!((words[0].probability - 0.9).abs() < 1e-6);
        assert_eq!(words[1].text, "Ahoj");
    }
}
