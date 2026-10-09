//! Word times measured in the sound. Whisper's word times are estimates, often 100–300 ms off in
//! connected Czech speech, where `align_words` finds no pause to correct them. A wav2vec2 CTC model
//! of the transcript's language hears which letter is spoken in every 20 ms frame; the Viterbi
//! path that spells Whisper's words in order through those frames gives each word where its
//! letters are heard. Words the model cannot spell (numbers, abbreviations, letters outside its
//! alphabet) take the stretch between their measured neighbours. Without a model for the
//! language, or where the model and the text disagree, the words keep Whisper's times.

use std::path::Path;

use anyhow::Result;
use nuzky_engine::speech::Word;
use unicode_normalization::UnicodeNormalization;

use crate::wav2vec2::{RECEPTIVE, STRIDE, Wav2Vec2};

const RATE: usize = 16_000;
const FRAME_US: i64 = (STRIDE * 1_000_000 / RATE) as i64;
/// Words this far apart in Whisper's times are aligned separately.
const SPAN_GAP_US: i64 = 1_000_000;
/// Sound around a span's words, for Whisper's estimates that start late or end early.
const SPAN_PAD_US: i64 = 500_000;
/// A span this long is split at its widest gap; the path's memory grows with frames × letters.
const MAX_SPAN_US: i64 = 120_000_000;
/// The model hears at most this much at once, with context on both sides of what it keeps.
const WINDOW: usize = 20 * RATE;
const CONTEXT: usize = 2 * RATE;
/// The forced path may cost this much more per frame than the model's own best guess before the
/// text and the sound are taken to disagree (a hallucinated sentence, music, another language).
const MAX_DISAGREEMENT: f32 = 1.0;
/// A word whose letters the model hears less surely than this keeps Whisper's estimate.
const MIN_WORD_PROBABILITY: f32 = 0.1;
/// Further than this from Whisper's estimate, the path found some other sound.
const MAX_SHIFT_US: i64 = 1_500_000;

/// A word timing model for one language, downloaded on first use.
pub struct AlignModel {
    pub language: &'static str,
    /// What transcripts record as their alignment.
    pub id: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// GGUF conversions of Apache-2.0 wav2vec2 CTC models: comodoro/wav2vec2-xls-r-300m-cs-250 (the
/// Czech model WhisperX aligns with), quantised to 8 bits, and facebook/wav2vec2-base-960h.
pub const ALIGN_MODELS: &[AlignModel] = &[
    AlignModel {
        language: "cs",
        id: "wav2vec2-xls-r-300m-cs-250-q8_0",
        file: "wav2vec2-xls-r-300m-cs-250-q8_0.gguf",
        url: "https://huggingface.co/cstr/wav2vec2-xls-r-300m-cs-250-GGUF/resolve/a264af811793d1f32a6ff3ce7de7ad9b2dcfdd40/wav2vec2-xls-r-300m-cs-250-q8_0.gguf",
        size: 373_328_224,
        sha256: "bde0e0d90ae14c60ffea627ccdd7ba263ea6d4a3d27882476fce32a257914eb8",
    },
    AlignModel {
        language: "en",
        id: "wav2vec2-base-960h-f16",
        file: "wav2vec2-base-960h.gguf",
        url: "https://huggingface.co/cstr/wav2vec2-base-960h-GGUF/resolve/7c025ea07cffc65b211b360e5865dfe59df7e8af/wav2vec2-base-960h.gguf",
        size: 206_939_264,
        sha256: "298d900e715936118c1476a5b246ceda08c1942411d350f38ccb02da1eac3cf7",
    },
];

/// The word timing model for `language`, if there is one.
pub fn align_model(language: &str) -> Option<&'static AlignModel> {
    ALIGN_MODELS.iter().find(|m| m.language == language)
}

pub struct Aligner {
    model: Wav2Vec2,
    alphabet: Alphabet,
}

impl Aligner {
    pub fn load(path: &Path) -> Result<Self> {
        let model = Wav2Vec2::load(path)?;
        let alphabet = Alphabet::new(&model.tokens, model.blank)?;
        Ok(Self { model, alphabet })
    }

    /// Moves `words` of 16 kHz mono `audio` to where the model hears them. Returns, for each word,
    /// whether its times were measured; the others keep Whisper's estimate between measured
    /// neighbours. Text and order never change.
    pub fn align(&self, words: &mut [Word], audio: &[f32], cancelled: &dyn Fn() -> bool) -> Result<Vec<bool>> {
        let mut placed = vec![false; words.len()];
        let duration = sample_us(audio.len());
        for span in spans(words) {
            if cancelled() {
                anyhow::bail!("CANCELLED: transcription cancelled");
            }
            let from =
                if span.start == 0 { 0 } else { (words[span.start - 1].end_us + words[span.start].start_us) / 2 };
            let to = words.get(span.end).map_or(duration, |next| (words[span.end - 1].end_us + next.start_us) / 2);
            let lo = (words[span.start].start_us - SPAN_PAD_US).max(from).max(0);
            let hi = (words[span.end - 1].end_us + SPAN_PAD_US).min(to).min(duration);
            let (first, last) = (us_sample(lo), us_sample(hi).min(audio.len()));
            if last <= first + RECEPTIVE {
                continue;
            }
            let emissions = self.emissions(&audio[first..last], cancelled)?;
            let local = &mut placed[span.clone()];
            let words = &mut words[span];
            for (i, time) in self.place(words, &emissions, sample_us(first)).into_iter().enumerate() {
                if let Some((start, end)) = time
                    && (start - words[i].start_us).abs() <= MAX_SHIFT_US
                {
                    (words[i].start_us, words[i].end_us) = (start, end);
                    local[i] = true;
                }
            }
            fill_between(words, local);
        }
        keep_order(words, duration);
        Ok(placed)
    }

    /// Log-probabilities, frames × tokens, of `audio`, computed in windows with context so a long
    /// stretch costs no more memory than one window.
    fn emissions(&self, audio: &[f32], cancelled: &dyn Fn() -> bool) -> Result<Emissions> {
        let tokens = self.model.tokens.len();
        if audio.len() <= WINDOW {
            let data = self.model.log_probs(audio, cancelled)?;
            return Ok(Emissions { frames: data.len() / tokens, tokens, data });
        }
        let total = self.model.frames(audio.len());
        let keep = (WINDOW - 2 * CONTEXT) / STRIDE;
        let mut data = Vec::with_capacity(total * tokens);
        let mut frame = 0;
        while frame < total {
            // Window starts stay on the frame grid, so its frames continue the ones before.
            let start = (frame * STRIDE).saturating_sub(CONTEXT) / STRIDE * STRIDE;
            let end = ((frame + keep) * STRIDE + CONTEXT + RECEPTIVE).min(audio.len());
            let window = self.model.log_probs(&audio[start..end], cancelled)?;
            let offset = frame - start / STRIDE;
            let take = keep.min(total - frame).min((window.len() / tokens).saturating_sub(offset));
            anyhow::ensure!(take > 0, "Word timing window ended early");
            data.extend_from_slice(&window[offset * tokens..(offset + take) * tokens]);
            frame += take;
        }
        Ok(Emissions { frames: data.len() / tokens, tokens, data })
    }

    /// Start and end of each word in the stretch whose frames begin at `origin_us`, where the path
    /// measured it. None for words it could not spell or did not hear clearly, and for all words
    /// when the text and the sound disagree.
    fn place(&self, words: &[Word], emissions: &Emissions, origin_us: i64) -> Vec<Option<(i64, i64)>> {
        let mut labels = Vec::new();
        let mut owner = Vec::new();
        let mut spellable = Vec::with_capacity(words.len());
        for (i, word) in words.iter().enumerate() {
            let spelling = self.alphabet.spell(&word.text);
            spellable.push(matches!(spelling, Spelling::Letters(_)));
            let letters = match spelling {
                Spelling::Letters(ids) => ids.into_iter().map(Label::Token).collect::<Vec<_>>(),
                Spelling::Unknown(count) => vec![Label::Any; count],
                Spelling::Silent => continue,
            };
            if !labels.is_empty() {
                labels.push(Label::Token(self.alphabet.separator));
                owner.push(None);
            }
            owner.extend(std::iter::repeat_n(Some(i), letters.len()));
            labels.extend(letters);
        }
        let mut times = vec![None; words.len()];
        let Some(path) = viterbi(emissions, &labels, self.alphabet.blank) else { return times };
        if path.disagreement > MAX_DISAGREEMENT {
            return times;
        }
        for i in 0..words.len() {
            let ks: Vec<usize> = (0..labels.len()).filter(|&k| owner[k] == Some(i)).collect();
            let (Some(&a), Some(&b)) = (ks.first(), ks.last()) else { continue };
            if !spellable[i] {
                continue;
            }
            let probability = ks.iter().map(|&k| path.probability[k]).sum::<f32>() / ks.len() as f32;
            if probability < MIN_WORD_PROBABILITY {
                continue;
            }
            let start = origin_us + path.spans[a].0 as i64 * FRAME_US;
            let end = origin_us + (path.spans[b].1 as i64 + 1) * FRAME_US;
            times[i] = Some((start, end));
        }
        times
    }
}

/// Runs of words close together in Whisper's times, as index ranges.
fn spans(words: &[Word]) -> Vec<std::ops::Range<usize>> {
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    for i in 0..words.len() {
        match spans.last_mut() {
            Some(span)
                if words[i].start_us - words[i - 1].end_us < SPAN_GAP_US
                    && words[i].end_us - words[span.start].start_us < MAX_SPAN_US =>
            {
                span.end = i + 1
            }
            _ => spans.push(i..i + 1),
        }
    }
    spans
}

/// Words of one span without a measured time share the stretch between their measured neighbours
/// in proportion to Whisper's durations, so cutting them removes exactly what lies between. At the
/// span's edges they keep Whisper's time, kept off their measured neighbour.
fn fill_between(words: &mut [Word], placed: &[bool]) {
    let mut i = 0;
    while i < words.len() {
        if placed[i] {
            i += 1;
            continue;
        }
        let run = i..(i..words.len()).find(|&j| placed[j]).unwrap_or(words.len());
        i = run.end;
        let left = run.start.checked_sub(1).map(|j| words[j].end_us);
        let right = words.get(run.end).map(|w| w.start_us);
        match (left, right) {
            (Some(from), Some(to)) => {
                let to = to.max(from);
                let lengths: Vec<i64> = words[run.clone()].iter().map(|w| (w.end_us - w.start_us).max(1)).collect();
                let total: i64 = lengths.iter().sum();
                let mut at = from;
                let mut sum = 0;
                for (k, j) in run.enumerate() {
                    sum += lengths[k];
                    let end = from + (to - from) * sum / total;
                    (words[j].start_us, words[j].end_us) = (at, end);
                    at = end;
                }
            }
            (Some(from), None) => {
                for word in &mut words[run] {
                    word.start_us = word.start_us.max(from);
                    word.end_us = word.end_us.max(word.start_us);
                }
            }
            (None, Some(to)) => {
                for word in &mut words[run] {
                    word.end_us = word.end_us.min(to);
                    word.start_us = word.start_us.min(word.end_us);
                }
            }
            (None, None) => {}
        }
    }
}

/// In order, without overlap, inside the recording.
fn keep_order(words: &mut [Word], duration: i64) {
    let mut floor = 0;
    for word in words {
        word.start_us = word.start_us.clamp(floor, duration.max(floor));
        word.end_us = word.end_us.clamp(word.start_us, duration.max(word.start_us));
        floor = word.end_us;
    }
}

fn sample_us(sample: usize) -> i64 {
    (sample as i128 * 1_000_000 / RATE as i128) as i64
}

fn us_sample(us: i64) -> usize {
    (us.max(0) as i128 * RATE as i128 / 1_000_000) as usize
}

struct Emissions {
    frames: usize,
    tokens: usize,
    data: Vec<f32>,
}

impl Emissions {
    fn row(&self, t: usize) -> &[f32] {
        &self.data[t * self.tokens..(t + 1) * self.tokens]
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Label {
    Token(usize),
    /// Any sound but silence: a letter of a word the model cannot spell.
    Any,
}

struct Alphabet {
    letters: std::collections::HashMap<char, usize>,
    blank: usize,
    separator: usize,
    upper: bool,
}

#[derive(Debug, PartialEq)]
enum Spelling {
    Letters(Vec<usize>),
    /// A word the model cannot spell, with its number of letters.
    Unknown(usize),
    /// Punctuation only.
    Silent,
}

/// Abbreviations read out as a longer word; spelling them would put the word's end in the middle.
const ABBREVIATIONS: &[&str] = &[
    "např", "tzv", "atd", "apod", "tj", "resp", "mj", "cca", "př", "kč", "str", "č", "mr", "mrs", "dr", "st", "etc",
    "vs", "jr", "sr", "km", "kg", "cm", "mm", "ml",
];

impl Alphabet {
    fn new(tokens: &[String], blank: usize) -> Result<Self> {
        let mut letters = std::collections::HashMap::new();
        let mut separator = None;
        for (id, token) in tokens.iter().enumerate() {
            let mut chars = token.chars();
            match (chars.next(), chars.next()) {
                (Some('|'), None) => separator = Some(id),
                (Some(c), None) if id != blank && (c.is_alphabetic() || c == '\'') => {
                    letters.insert(c, id);
                }
                _ => {}
            }
        }
        let separator = separator.ok_or_else(|| anyhow::anyhow!("Word timing model has no word separator"))?;
        let upper = letters.keys().any(|c| c.is_uppercase());
        Ok(Self { letters, blank, separator, upper })
    }

    fn spell(&self, word: &str) -> Spelling {
        let text: String = word.nfc().collect();
        let bare: String = text.chars().filter(|c| c.is_alphanumeric()).collect();
        if bare.is_empty() {
            return Spelling::Silent;
        }
        let lower = bare.to_lowercase();
        let acronym = bare.chars().count() >= 2 && bare.chars().all(|c| c.is_uppercase());
        let abbreviation =
            ABBREVIATIONS.contains(&lower.as_str()) && (text.ends_with('.') || lower == "kč" || lower == "cca");
        if acronym || abbreviation {
            return Spelling::Unknown(bare.chars().count());
        }
        let mut ids = Vec::new();
        for c in text.chars() {
            let c =
                if self.upper { c.to_uppercase().next().unwrap_or(c) } else { c.to_lowercase().next().unwrap_or(c) };
            if let Some(&id) = self.letters.get(&c) {
                ids.push(id);
            } else if c.is_alphanumeric() {
                // A letter outside the alphabet as its base letter: ä as a, ł stays unknown.
                match c.to_string().nfd().next().and_then(|base| self.letters.get(&base)) {
                    Some(&id) => ids.push(id),
                    None => return Spelling::Unknown(bare.chars().count()),
                }
            }
        }
        if ids.is_empty() { Spelling::Silent } else { Spelling::Letters(ids) }
    }
}

struct BestPath {
    /// First and last frame of each label.
    spans: Vec<(usize, usize)>,
    /// Mean probability of each label over its frames.
    probability: Vec<f32>,
    /// How much worse per frame the path is than the model's own best guess, in nats.
    disagreement: f32,
}

/// The most likely CTC path that spells `labels` in order through every frame: blanks may sit
/// between labels, a label may last several frames, and equal neighbours need a blank between.
/// None when there are too few frames.
fn viterbi(emissions: &Emissions, labels: &[Label], blank: usize) -> Option<BestPath> {
    let (frames, n) = (emissions.frames, labels.len());
    if n == 0 || frames == 0 {
        return None;
    }
    let needed = n + labels.windows(2).filter(|p| p[0] == p[1] && p[0] != Label::Any).count();
    if frames < needed {
        return None;
    }
    // Any sound but silence, for letters of words the model cannot spell.
    let any: Vec<f32> = (0..frames)
        .map(|t| {
            let row = emissions.row(t);
            row.iter().enumerate().filter(|&(k, _)| k != blank).map(|(_, &p)| p).fold(f32::NEG_INFINITY, f32::max)
        })
        .collect();
    let states = 2 * n + 1;
    let score = |t: usize, s: usize| -> f32 {
        if s.is_multiple_of(2) {
            emissions.row(t)[blank]
        } else {
            match labels[s / 2] {
                Label::Token(id) => emissions.row(t)[id],
                Label::Any => any[t],
            }
        }
    };
    let skip = |s: usize| s >= 2 && s % 2 == 1 && (labels[s / 2] != labels[s / 2 - 1] || labels[s / 2] == Label::Any);
    let mut back = vec![0u8; frames * states];
    let mut prev = vec![f32::NEG_INFINITY; states];
    prev[0] = score(0, 0);
    prev[1] = score(0, 1);
    let mut cur = vec![f32::NEG_INFINITY; states];
    for t in 1..frames {
        // States that cannot be reached yet or can no longer finish stay impossible.
        let lo = (states - 1).saturating_sub(2 * (frames - t));
        let hi = (2 * t + 1).min(states - 1);
        cur.fill(f32::NEG_INFINITY);
        for s in lo..=hi {
            let (mut best, mut from) = (prev[s], 0u8);
            if s >= 1 && prev[s - 1] > best {
                (best, from) = (prev[s - 1], 1);
            }
            if skip(s) && prev[s - 2] > best {
                (best, from) = (prev[s - 2], 2);
            }
            if best > f32::NEG_INFINITY {
                cur[s] = best + score(t, s);
                back[t * states + s] = from;
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let mut s = if prev[states - 1] >= prev[states - 2] { states - 1 } else { states - 2 };
    let total = prev[s];
    if !total.is_finite() {
        return None;
    }
    let mut spans = vec![(usize::MAX, 0); n];
    let mut sum = vec![0f32; n];
    let mut count = vec![0u32; n];
    for t in (0..frames).rev() {
        if s % 2 == 1 {
            let k = s / 2;
            if spans[k].0 == usize::MAX {
                spans[k].1 = t;
            }
            spans[k].0 = t;
            sum[k] += score(t, s).exp();
            count[k] += 1;
        }
        s -= back[t * states + s] as usize;
    }
    let best: f32 = (0..frames).map(|t| emissions.row(t).iter().copied().fold(f32::NEG_INFINITY, f32::max)).sum();
    Some(BestPath {
        spans,
        probability: sum.iter().zip(&count).map(|(s, &c)| s / c.max(1) as f32).collect(),
        disagreement: (best - total) / frames as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start: f64, end: f64, text: &str) -> Word {
        Word { start_us: (start * 1e6) as i64, end_us: (end * 1e6) as i64, text: text.into(), probability: 0.9 }
    }

    /// The Czech model's alphabet: separator, letters, unknown, blank.
    fn czech() -> Alphabet {
        let mut tokens = vec!["|".to_owned()];
        tokens.extend("abcdefghijklmnopqrstuvwxyzáéíóúýčďěňřšťůž".chars().map(String::from));
        tokens.extend(["[UNK]".to_owned(), "[PAD]".to_owned()]);
        Alphabet::new(&tokens, tokens.len() - 1).unwrap()
    }

    #[test]
    fn spells_words_as_the_model_writes_them() {
        let alphabet = czech();
        let letters = |text: &str| match alphabet.spell(text) {
            Spelling::Letters(ids) => {
                ids.iter().map(|&id| alphabet.letters.iter().find(|(_, v)| **v == id).unwrap().0).collect::<String>()
            }
            other => panic!("{text}: {other:?}"),
        };
        assert_eq!(letters("Příklad,"), "příklad");
        // Decomposed diacritics, as some files store them.
        assert_eq!(letters("Pr\u{030C}i\u{0301}klad"), "příklad");
        assert_eq!(letters("e-mail"), "email");
        assert_eq!(letters("Müller"), "muller");
        assert_eq!(alphabet.spell("10"), Spelling::Unknown(2));
        assert_eq!(alphabet.spell("2026."), Spelling::Unknown(4));
        assert_eq!(alphabet.spell("např."), Spelling::Unknown(4));
        assert_eq!(alphabet.spell("ČR"), Spelling::Unknown(2));
        assert_eq!(alphabet.spell("Łódź"), Spelling::Unknown(4));
        assert_eq!(alphabet.spell("–"), Spelling::Silent);
    }

    /// Emissions where `heard[t]` is the token that frame hears, almost surely.
    fn emissions(heard: &[usize], tokens: usize) -> Emissions {
        let mut data = vec![(0.01f32 / tokens as f32).ln(); heard.len() * tokens];
        for (t, &k) in heard.iter().enumerate() {
            data[t * tokens + k] = 0.99f32.ln();
        }
        Emissions { frames: heard.len(), tokens, data }
    }

    #[test]
    fn the_path_places_each_letter_where_it_is_heard() {
        // Tokens: 0 blank, 1 separator, 2 a, 3 b.
        let heard = [0, 0, 2, 2, 0, 1, 0, 3, 0, 0];
        let path = viterbi(&emissions(&heard, 4), &[Label::Token(2), Label::Token(1), Label::Token(3)], 0).unwrap();
        assert_eq!(path.spans, [(2, 3), (5, 5), (7, 7)]);
        assert!(path.probability.iter().all(|&p| p > 0.9), "{:?}", path.probability);
        assert!(path.disagreement < 0.01);
        // Text the sound does not say costs far more than the model's own guess.
        let wrong = viterbi(&emissions(&heard, 4), &[Label::Token(3), Label::Token(1), Label::Token(2)], 0).unwrap();
        assert!(wrong.disagreement > MAX_DISAGREEMENT, "{}", wrong.disagreement);
    }

    #[test]
    fn a_repeated_letter_needs_a_blank_between_and_too_few_frames_give_no_path() {
        let a = Label::Token(2);
        assert!(viterbi(&emissions(&[2, 2], 4), &[a, a], 0).is_none());
        let path = viterbi(&emissions(&[2, 0, 2], 4), &[a, a], 0).unwrap();
        assert_eq!(path.spans, [(0, 0), (2, 2)]);
        assert!(viterbi(&emissions(&[], 4), &[a], 0).is_none());
    }

    #[test]
    fn an_unspellable_word_takes_the_sound_between_its_neighbours() {
        // "a 10 b": the number sounds as letters the text does not have (token 4, 5).
        let heard = [0, 2, 0, 1, 0, 4, 5, 4, 5, 0, 1, 0, 3, 0];
        let labels = [Label::Token(2), Label::Token(1), Label::Any, Label::Any, Label::Token(1), Label::Token(3)];
        let path = viterbi(&emissions(&heard, 6), &labels, 0).unwrap();
        assert_eq!(path.spans[0], (1, 1));
        assert_eq!(path.spans[5], (12, 12));
        assert!(path.spans[2].0 >= 5 && path.spans[3].1 <= 8, "{:?}", path.spans);
    }

    #[test]
    fn unmeasured_words_share_the_gap_between_measured_neighbours() {
        let mut words = vec![word(0.0, 1.0, "za"), word(1.1, 1.3, "10"), word(1.3, 1.9, "minut"), word(2.0, 2.5, "a")];
        fill_between(&mut words, &[true, false, false, true]);
        assert_eq!((words[1].start_us, words[1].end_us), (1_000_000, 1_250_000));
        assert_eq!((words[2].start_us, words[2].end_us), (1_250_000, 2_000_000));
        // At the edge of a stretch, Whisper's time stays, kept off the measured word.
        let mut words = vec![word(0.0, 0.5, "10"), word(0.3, 0.8, "minut"), word(0.9, 1.4, "pak")];
        fill_between(&mut words, &[false, true, false]);
        assert_eq!((words[0].start_us, words[0].end_us), (0, 300_000));
        assert_eq!((words[2].start_us, words[2].end_us), (900_000, 1_400_000));
        // Nothing measured: nothing moves.
        let original = vec![word(0.0, 0.5, "a"), word(0.4, 0.9, "b")];
        let mut words = original.clone();
        fill_between(&mut words, &[false, false]);
        assert_eq!(words, original);
    }

    #[test]
    fn words_a_second_apart_are_aligned_separately() {
        let words = [word(0.0, 0.4, "a"), word(0.5, 0.9, "b"), word(2.0, 2.4, "c"), word(2.5, 2.6, "d")];
        assert_eq!(spans(&words), [0..2, 2..4]);
        let mut order = vec![word(0.0, 0.6, "a"), word(0.5, 0.9, "b"), word(0.8, 3.0, "c")];
        keep_order(&mut order, 2_000_000);
        assert!(order.windows(2).all(|p| p[0].end_us <= p[1].start_us));
        assert_eq!(order[2].end_us, 2_000_000);
    }
}
