//! Finds where every moment of a finished cut came from in its recording, by sound alone.
//! Words recognised twice can differ, retakes repeat the same words, and a cut can drop
//! half a sentence, so text cannot place a cut reliably; the sound of the same moment can.

use std::path::Path;

use anyhow::{Result, ensure};
use nuzky_engine::model::Asset;
use serde::Serialize;

use crate::{Range, Word, audio::open_pcm};

/// Feature frames per second: one every 10 ms.
const FPS: i64 = 100;
const FRAME_US: i64 = 1_000_000 / FPS;
/// 16 kHz mono samples per frame.
const HOP: usize = 160;
/// Band centres in Hz. Each frame keeps the log energy of these bands.
const BANDS: [f32; 8] = [200.0, 400.0, 700.0, 1100.0, 1700.0, 2600.0, 4000.0, 6000.0];
/// Bands this far (log10) below a frame's loudest band count as equally quiet.
const RANGE: f32 = 3.0;
/// Frames are speech when their loudest band is within this (log10) of the file's speech level.
const SPEECH: f32 = -1.2;
/// A search window of 0.3 s of the cut, moved by 0.15 s: short enough for a spliced word or two.
const CHUNK: usize = 30;
const CHUNK_HOP: usize = 15;
/// Cost of jumping to another place in the recording, in frame costs.
const SWITCH: f32 = 4.0;
/// Per-frame cost of cut sound found nowhere in the recording, such as added music.
const UNMATCHED: f32 = 0.7;
/// How far outside a piece a kept word's middle may be.
const EDGE_US: i64 = 50_000;
/// How far apart two recognitions of the same spoken word can place it.
const RECOGNITION_US: i64 = 1_000_000;
/// Frames this far (log10) below the file's speech level are silence.
const QUIET: f32 = -1.6;
/// Silences shorter than this are gaps inside words.
const MIN_PAUSE: usize = 8;
/// Most candidate places a cut is matched against.
const MAX_PLACES: usize = 400;
/// Candidate places closer than this are the same place.
const SAME_PLACE: i64 = 3;

/// A 20 ms window: the shape of its spectrum, which a clip's own volume in the cut does not
/// change, and how loud it is against the file's speech.
#[derive(Clone, Copy, Debug)]
struct Frame {
    shape: [f32; BANDS.len()],
    loud: f32,
}

/// A continuous piece of the recording in the cut: cut time `start_us..end_us` plays recording
/// time `start_us + offset_us..end_us + offset_us`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Piece {
    pub start_us: i64,
    pub end_us: i64,
    pub offset_us: i64,
}

impl Piece {
    pub fn source(&self) -> Range {
        Range { start_us: self.start_us + self.offset_us, end_us: self.end_us + self.offset_us }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Alignment {
    /// In cut order.
    pub pieces: Vec<Piece>,
    /// Share of the cut's speech found in the recording, 0..=1.
    pub matched: f32,
    pub cut_duration_us: i64,
    /// Silences between speech, in the cut's time and in the recording's.
    pub cut_pauses: Vec<Range>,
    pub recording_pauses: Vec<Range>,
}

/// Where a recording word plays: the piece, and its start and end in the cut.
pub type Place = Option<(usize, i64, i64)>;

impl Alignment {
    /// Where a recording word plays in the cut, if its middle was kept. A cut found by sound can
    /// land a few frames off, so the middle may be that close outside the piece.
    fn by_sound(&self, word: &Word) -> Place {
        let middle = (word.start_us + word.end_us) / 2;
        self.pieces.iter().enumerate().find_map(|(i, p)| {
            let source = p.source();
            (source.start_us - EDGE_US <= middle && middle < source.end_us + EDGE_US).then(|| {
                let start = (word.start_us - p.offset_us).clamp(p.start_us, p.end_us);
                (i, start, (word.end_us - p.offset_us).clamp(start, p.end_us))
            })
        })
    }

    /// Where each recording word plays in the cut. Recognition times can be most of a second off
    /// next to a pause, which is exactly where cuts are; so a word the sound does not place still
    /// counts as kept when the cut's own recognition heard the same word that close to it.
    pub fn places(&self, words: &[Word], cut_words: &[Word]) -> Vec<Place> {
        let mut places: Vec<Place> = words.iter().map(|w| self.by_sound(w)).collect();
        // Each cut word heard inside a piece: its token, piece, recording time and the word itself.
        let heard: Vec<(String, usize, i64, &Word)> = cut_words
            .iter()
            .filter_map(|w| {
                let middle = (w.start_us + w.end_us) / 2;
                let piece = self.pieces.iter().position(|p| p.start_us <= middle && middle < p.end_us)?;
                Some((super::token(&w.text), piece, w.start_us + self.pieces[piece].offset_us, w))
            })
            .collect();
        let mut claimed = vec![false; heard.len()];
        let claim = |word: &Word, claimed: &mut Vec<bool>| {
            let token = super::token(&word.text);
            let best = (0..heard.len())
                .filter(|&k| !claimed[k] && heard[k].0 == token && (heard[k].2 - word.start_us).abs() <= RECOGNITION_US)
                .min_by_key(|&k| ((heard[k].2 - word.start_us).abs(), k))?;
            claimed[best] = true;
            Some(best)
        };
        for (word, place) in words.iter().zip(&places) {
            if place.is_some() {
                claim(word, &mut claimed);
            }
        }
        for (i, word) in words.iter().enumerate() {
            if places[i].is_none()
                && let Some(k) = claim(word, &mut claimed)
            {
                let (piece, cut) = (heard[k].1, heard[k].3);
                places[i] = Some((piece, cut.start_us, cut.end_us.max(cut.start_us)));
            }
        }
        places
    }
}

/// Places every part of `cut` in `recording`. Order may change: a hook moved to the front is found too.
pub fn align(recording: &Asset, cut: &Asset, cache: &Path, cancelled: &dyn Fn() -> bool) -> Result<Alignment> {
    let source = features(recording, cache, cancelled)?;
    let target = features(cut, cache, cancelled)?;
    ensure!(!source.is_empty() && !target.is_empty(), "NO_AUDIO: the recording or the cut has no sound");
    let offsets = candidates(&source, &target, cancelled)?;
    let states = viterbi(&source, &target, &offsets);
    let speech: Vec<bool> = target.iter().map(is_speech).collect();
    let (mut heard, mut found) = (0usize, 0usize);
    for (t, state) in states.iter().enumerate() {
        if speech[t] {
            heard += 1;
            found += usize::from(state.is_some());
        }
    }
    let mut pieces: Vec<Piece> = Vec::new();
    for (t, state) in states.iter().enumerate() {
        let Some(k) = *state else { continue };
        let offset_us = offsets[k] * FRAME_US;
        match pieces.last_mut() {
            Some(last) if last.offset_us == offset_us && last.end_us == t as i64 * FRAME_US => last.end_us += FRAME_US,
            _ => pieces.push(Piece { start_us: t as i64 * FRAME_US, end_us: (t as i64 + 1) * FRAME_US, offset_us }),
        }
    }
    // A piece with less speech than a short word is silence or breath that matched by chance.
    pieces.retain(|p| (p.start_us / FRAME_US..p.end_us / FRAME_US).filter(|&t| speech[t as usize]).count() >= 10);
    Ok(Alignment {
        pieces,
        matched: if heard == 0 { 0.0 } else { found as f32 / heard as f32 },
        cut_duration_us: cut.duration_us,
        cut_pauses: pauses(&target),
        recording_pauses: pauses(&source),
    })
}

/// Quiet stretches with speech on both sides.
fn pauses(frames: &[Frame]) -> Vec<Range> {
    let mut out = Vec::new();
    let mut quiet_from = None;
    let mut spoken = false;
    for (t, frame) in frames.iter().enumerate() {
        if frame.loud < QUIET {
            quiet_from.get_or_insert(t);
        } else {
            if let Some(from) = quiet_from.take()
                && spoken
                && t - from >= MIN_PAUSE
            {
                out.push(Range { start_us: from as i64 * FRAME_US, end_us: t as i64 * FRAME_US });
            }
            spoken |= is_speech(frame);
        }
    }
    out
}

fn is_speech(frame: &Frame) -> bool {
    frame.loud > SPEECH
}

fn distance(a: &Frame, b: &Frame) -> f32 {
    a.shape.iter().zip(&b.shape).map(|(a, b)| (a - b).abs()).sum::<f32>() / BANDS.len() as f32
}

/// Log band energies of 20 ms windows every 10 ms, relative to the file's own speech level, so
/// equalising and a different sample rate in the cut do not matter.
fn features(asset: &Asset, cache: &Path, cancelled: &dyn Fn() -> bool) -> Result<Vec<Frame>> {
    let pcm = open_pcm(asset, cache, cancelled)?;
    // Three 48 kHz stereo frames become one 16 kHz mono sample, as for speech recognition.
    let mono: Vec<f32> = pcm.samples().chunks(6).map(|s| s.iter().sum::<f32>() / s.len() as f32).collect();
    let frames = mono.len() / HOP;
    let mut energy = vec![[0f32; BANDS.len()]; frames];
    for (band, &centre) in BANDS.iter().enumerate() {
        ensure!(!cancelled(), "CANCELLED: style analysis cancelled");
        let mut filter = Biquad::band_pass(centre, 16_000.0, 1.2);
        for (frame, samples) in energy.iter_mut().zip(mono.as_chunks::<HOP>().0) {
            frame[band] = samples.iter().map(|&s| filter.run(s).powi(2)).sum::<f32>();
        }
    }
    let mut levels: Vec<[f32; BANDS.len()]> = (0..frames)
        .map(|t| {
            let next = energy.get(t + 1).unwrap_or(&energy[t]);
            std::array::from_fn(|b| ((energy[t][b] + next[b]) / (2 * HOP) as f32 + 1e-12).log10())
        })
        .collect();
    for band in 0..BANDS.len() {
        let mut values: Vec<f32> = levels.iter().map(|f| f[band]).collect();
        values.sort_unstable_by(f32::total_cmp);
        let speech = values.get(values.len() * 9 / 10).copied().unwrap_or(0.0);
        levels.iter_mut().for_each(|frame| frame[band] -= speech);
    }
    Ok(levels
        .into_iter()
        .map(|level| {
            let loud = level.iter().copied().fold(f32::MIN, f32::max);
            let clamped = level.map(|v| v.max(loud - RANGE));
            let mean = clamped.iter().sum::<f32>() / BANDS.len() as f32;
            Frame { shape: clamped.map(|v| v - mean), loud }
        })
        .collect())
}

/// Where each speech window of the cut fits best in the recording, as frame offsets.
fn candidates(source: &[Frame], target: &[Frame], cancelled: &dyn Fn() -> bool) -> Result<Vec<i64>> {
    let mut found: Vec<(i64, f32)> = Vec::new();
    let mut start = 0;
    while start + CHUNK <= target.len().max(CHUNK) {
        ensure!(!cancelled(), "CANCELLED: style analysis cancelled");
        let chunk = &target[start..(start + CHUNK).min(target.len())];
        if chunk.len() > source.len() {
            break;
        }
        if chunk.iter().filter(|f| is_speech(f)).count() * 3 >= chunk.len() {
            // Every other frame and offset first, then the best offset to the frame.
            let cost = |offset: usize, step: usize| {
                chunk
                    .iter()
                    .step_by(step)
                    .enumerate()
                    .map(|(i, f)| distance(f, &source[offset + i * step]))
                    .sum::<f32>()
            };
            let last = source.len().saturating_sub(chunk.len());
            let coarse = (0..=last).step_by(2).map(|o| (o, cost(o, 2))).min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((best, _)) = coarse {
                let fine = (best.saturating_sub(2)..=(best + 2).min(last))
                    .map(|o| (o, cost(o, 1) / chunk.len() as f32))
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((offset, cost)) = fine {
                    found.push((offset as i64 - start as i64, cost));
                }
            }
        }
        start += CHUNK_HOP;
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut offsets: Vec<(i64, f32)> = Vec::new();
    for (offset, cost) in found {
        match offsets.last_mut() {
            Some(last) if offset - last.0 <= SAME_PLACE => {
                if cost < last.1 {
                    *last = (offset, cost);
                }
            }
            _ => offsets.push((offset, cost)),
        }
    }
    // The best places are enough; a long cut of a long recording would otherwise find thousands.
    offsets.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    offsets.truncate(MAX_PLACES);
    offsets.sort_by_key(|&(offset, _)| offset);
    Ok(offsets.into_iter().map(|(offset, _)| offset).collect())
}

/// The cheapest path through the candidate places, frame by frame. `None` is sound not in the recording.
fn viterbi(source: &[Frame], target: &[Frame], offsets: &[i64]) -> Vec<Option<usize>> {
    let states = offsets.len() + 1;
    let none = offsets.len();
    let emit = |t: usize, k: usize| {
        if k == none {
            return UNMATCHED;
        }
        let at = t as i64 + offsets[k];
        if at < 0 || at >= source.len() as i64 {
            return 2.0 * UNMATCHED;
        }
        distance(&target[t], &source[at as usize])
    };
    let mut cost: Vec<f32> = (0..states).map(|k| emit(0, k)).collect();
    let mut from = vec![0u16; target.len() * states];
    for t in 1..target.len() {
        let (best, best_cost) =
            cost.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1)).map(|(k, c)| (k, *c)).unwrap_or((none, 0.0));
        for k in 0..states {
            let (previous, stay) =
                if cost[k] <= best_cost + SWITCH { (k, cost[k]) } else { (best, best_cost + SWITCH) };
            from[t * states + k] = previous as u16;
            cost[k] = stay + emit(t, k);
        }
        // Costs only compare with each other; keeping them small keeps f32 exact enough.
        let least = cost.iter().copied().fold(f32::INFINITY, f32::min);
        cost.iter_mut().for_each(|c| *c -= least);
    }
    let mut state = cost.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1)).map_or(none, |(k, _)| k);
    let mut path = vec![None; target.len()];
    for t in (0..target.len()).rev() {
        path[t] = (state != none).then_some(state);
        state = from[t * states + state] as usize;
    }
    path
}

/// RBJ band-pass with 0 dB peak gain.
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    fn band_pass(centre: f32, rate: f32, q: f32) -> Self {
        let w = std::f32::consts::TAU * centre / rate;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b: [alpha / a0, 0.0, -alpha / a0],
            a: [-2.0 * w.cos() / a0, (1.0 - alpha) / a0],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_just_past_a_piece_plays_at_its_end() {
        let alignment = Alignment {
            pieces: vec![Piece { start_us: 0, end_us: 10_000_000, offset_us: 5_000_000 }],
            matched: 1.0,
            cut_duration_us: 10_000_000,
            cut_pauses: Vec::new(),
            recording_pauses: Vec::new(),
        };
        let word = Word { start_us: 15_010_000, end_us: 15_070_000, text: "ok".into(), probability: 1.0 };
        assert_eq!(alignment.places(&[word], &[]), [Some((0, 10_000_000, 10_000_000))]);
    }

    #[test]
    fn a_word_the_cut_heard_keeps_the_times_the_cut_heard_it_at() {
        let alignment = Alignment {
            pieces: vec![Piece { start_us: 1_000_000, end_us: 3_000_000, offset_us: 9_000_000 }],
            matched: 1.0,
            cut_duration_us: 3_000_000,
            cut_pauses: Vec::new(),
            recording_pauses: Vec::new(),
        };
        let word = |start_us: i64, text: &str| Word {
            start_us,
            end_us: start_us + 200_000,
            text: text.into(),
            probability: 1.0,
        };
        // "intro" plays before the piece, so the cut's recognition of "tak" is its second word.
        let cut = [word(100_000, "intro"), word(1_500_000, "tak")];
        // Recognition placed "tak" 0.8 s early in the recording, in the pause before the piece.
        assert_eq!(alignment.places(&[word(9_700_000, "tak")], &cut), [Some((0, 1_500_000, 1_700_000))]);
    }

    fn tone_frames(pattern: &[u8]) -> Vec<Frame> {
        // Each symbol is a distinct "syllable" of 15 frames; 0 is silence.
        pattern
            .iter()
            .flat_map(|&s| {
                let frame = Frame {
                    shape: std::array::from_fn(
                        |b| if s == 0 { 0.0 } else { ((b as f32 + s as f32 * 3.0) % 7.0) / 4.0 },
                    ),
                    loud: if s == 0 { -3.0 } else { 0.0 },
                };
                std::iter::repeat_n(frame, 15)
            })
            .collect()
    }

    #[test]
    fn finds_kept_takes_in_a_retake_heavy_recording() {
        // Recording: take 1 of a sentence, pause, take 2 of it, pause, the next sentence.
        let source = tone_frames(&[1, 2, 3, 0, 0, 1, 2, 4, 5, 0, 0, 0, 6, 7, 8, 9, 0]);
        // The cut keeps take 2 and the next sentence, with a short pause between.
        let target = tone_frames(&[1, 2, 4, 5, 0, 6, 7, 8, 9]);
        let offsets = candidates(&source, &target, &|| false).unwrap();
        let path = viterbi(&source, &target, &offsets);
        let used: Vec<i64> = path.iter().flatten().map(|&k| offsets[k]).collect::<Vec<_>>();
        // Take 2 starts at symbol 5 in the recording and symbol 0 in the cut: 75 frames later.
        assert_eq!(used[0], 75);
        assert_eq!(used[100], 75 + 30, "the next sentence comes after a 3-symbol pause, cut to one");
        assert!(path.iter().all(Option::is_some));
    }
}
