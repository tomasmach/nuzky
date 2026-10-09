//! Word boundaries found in the sound. Whisper's word times are estimates: next to a pause it
//! often stretches a word over the silence and into the next one, which in Czech moved a
//! sentence's first syllable by 100–300 ms in our recordings. A cut placed from such times clips
//! that syllable. Each boundary between two words moves into the quiet stretch of the recording
//! nearest to it, so cuts and their margins land where nothing is said.

use std::path::Path;

use anyhow::Result;
use nuzky_engine::{
    audio::{samples_to_us, us_to_samples},
    model::{Asset, CHANNELS},
};

use crate::{Word, align::Aligner, audio::open_pcm};

/// Level windows of 10 ms every 5 ms.
const WINDOW: usize = 480;
const HOP: usize = 240;
/// How far from Whisper's estimate a boundary may move, never past the middle of either word.
const REACH_US: i64 = 400_000;
/// A measured word's sound goes on while it stays within this of the word's own level. Its
/// edges follow the sound only that far: the word timing model hears a held vowel end early and
/// a soft onset late, but the room's echo after a word is not the word.
const WORD_DROP_DB: f32 = 20.0;
/// A quiet run this long is a pause between words; shorter ones may be the closure of a
/// consonant inside a word, so a pause nearby wins over them.
const PAUSE_US: i64 = 100_000;
const QUIET_RUN_US: i64 = 20_000;
/// Without any quiet run, the boundary moves to the quietest moment this close to the estimate.
const NUDGE_US: i64 = 60_000;
/// Speech must stand this far above the room for its pauses to be found.
const MIN_RANGE_DB: f32 = 12.0;
/// Digital silence, such as padding before late-starting sound, says nothing about the room.
const SILENT_DB: f32 = -100.0;

/// Moves word boundaries of one recording into its quiet stretches. `samples` is its 48 kHz
/// interleaved stereo sound, `words` its words in order, in the recording's own time. Words keep
/// their order and text; a boundary with no quiet stretch near it stays where Whisper put it at
/// the edges of speech, and moves to the quietest nearby moment between two words.
pub fn align_words(words: &mut [Word], samples: &[f32]) {
    refine_words(words, samples, &vec![false; words.len()]);
}

/// [`align_words`] after the word timing model measured the words marked in `measured`. Their
/// edges stay where the model heard them and only grow into the gap while their sound goes on;
/// between two measured words with no quiet moment, the boundary is the middle of the gap the
/// model left. Boundaries between unmeasured words move as [`align_words`] moves them.
pub(crate) fn refine_words(words: &mut [Word], samples: &[f32], measured: &[bool]) {
    let levels = Levels::new(samples);
    let Some(threshold) = levels.threshold() else { return };
    for i in 0..=words.len() {
        if (i > 0 && measured[i - 1]) || measured.get(i) == Some(&true) {
            levels.follow_sound(words, measured, i, threshold);
            continue;
        }
        let left = i.checked_sub(1).map(|j| (words[j].start_us, words[j].end_us));
        let right = words.get(i).map(|w| (w.start_us, w.end_us));
        let (from, to) = match (left, right) {
            (Some(l), Some(r)) => (l.1.min(r.0), l.1.max(r.0)),
            (Some(l), None) => (l.1, l.1),
            (None, Some(r)) => (r.0, r.0),
            (None, None) => return,
        };
        let middle = |(start, end): (i64, i64)| (start + end) / 2;
        let lo = left.map_or(from - REACH_US, middle).max(from - REACH_US).max(0);
        let hi = right.map_or(to + REACH_US, middle).min(to + REACH_US).min(levels.duration_us());
        if lo >= hi {
            continue;
        }
        // A quiet run counts when it reaches into the window and is measured whole, as far as the
        // two words allow, so a pause that starts before the window still ends the word on time.
        let outer = (left.map_or(lo, |l| l.0.min(lo)), right.map_or(hi, |r| r.1.max(hi)));
        let runs: Vec<_> =
            levels.quiet_runs(outer.0, outer.1, threshold).into_iter().filter(|r| r.1 > lo && r.0 < hi).collect();
        let distance = |&(a, b): &(i64, i64)| (from - b).max(a - to).max(0);
        // A pause wins; a shorter quiet run may be a consonant's closure, so only one right at
        // the estimate counts.
        let gap = runs
            .iter()
            .filter(|r| r.1 - r.0 >= PAUSE_US)
            .max_by_key(|r| (r.1 - r.0, -distance(r), -r.0))
            .or_else(|| {
                runs.iter()
                    .filter(|r| r.1 - r.0 >= QUIET_RUN_US && distance(r) <= NUDGE_US)
                    .min_by_key(|r| (distance(r), r.0))
            })
            .copied();
        let gap = match gap {
            Some(gap) => gap,
            // At the edges of speech there is nothing to share; keep the estimate.
            None if left.is_none() || right.is_none() => continue,
            None => {
                let t = levels.quietest((lo.max(from - NUDGE_US), hi.min(to + NUDGE_US)), (from + to) / 2);
                (t, t)
            }
        };
        if left.is_some() {
            words[i - 1].end_us = gap.0;
        }
        if right.is_some() {
            words[i].start_us = gap.1;
        }
    }
}

/// Measures `words` in the sound of `asset`, from its PCM cache, which recognition has just used:
/// with the word timing model when there is one, then [`align_words`]. True when the model
/// measured at least one word.
pub fn align_to_sound(
    words: &mut [Word],
    asset: &Asset,
    cache: &Path,
    aligner: Option<&Aligner>,
    cancelled: impl Fn() -> bool,
) -> Result<bool> {
    let pcm = open_pcm(asset, cache, &cancelled)?;
    let placed = match aligner {
        Some(aligner) => {
            // Three 48 kHz stereo frames (six channel samples) into one 16 kHz mono sample, as recognition hears it.
            let audio: Vec<f32> = pcm.samples().chunks(6).map(|s| s.iter().sum::<f32>() / s.len() as f32).collect();
            aligner.align(words, &audio, &cancelled)?
        }
        None => vec![false; words.len()],
    };
    anyhow::ensure!(!cancelled(), "CANCELLED: transcription cancelled");
    refine_words(words, pcm.samples(), &placed);
    Ok(placed.contains(&true))
}

struct Levels {
    /// dBFS of each 10 ms window, starting every 5 ms.
    db: Vec<f32>,
    frames: usize,
}

impl Levels {
    fn new(samples: &[f32]) -> Self {
        let frames = samples.len() / CHANNELS;
        let db = (0..frames.div_ceil(HOP))
            .map(|k| {
                let window = &samples[k * HOP * CHANNELS..((k * HOP + WINDOW).min(frames)) * CHANNELS];
                let power =
                    window.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / window.len().max(1) as f64;
                (10.0 * power.max(1e-12).log10()) as f32
            })
            .collect();
        Self { db, frames }
    }

    fn duration_us(&self) -> i64 {
        samples_to_us(self.frames as i64)
    }

    /// A quarter of the way, at least 6 dB, from the room's level (10th percentile) to speech
    /// (90th percentile). None for a recording without sound, and for one whose quiet moments are
    /// not clearly quieter than its speech: there silence cannot be told from words.
    fn threshold(&self) -> Option<f32> {
        let mut heard: Vec<f32> = self.db.iter().copied().filter(|db| *db > SILENT_DB).collect();
        if heard.is_empty() {
            return None;
        }
        heard.sort_unstable_by(f32::total_cmp);
        let (room, speech) = (heard[(heard.len() - 1) / 10], heard[(heard.len() - 1) * 9 / 10]);
        (speech - room >= MIN_RANGE_DB).then(|| room + (0.25 * (speech - room)).max(6.0))
    }

    fn hop(us: i64) -> usize {
        (us_to_samples(us).max(0) as usize / HOP).min(usize::MAX / 2)
    }

    /// Stretches inside `lo..hi` where every window is below `threshold`, as time ranges.
    fn quiet_runs(&self, lo: i64, hi: i64, threshold: f32) -> Vec<(i64, i64)> {
        let (first, last) = (Self::hop(lo), Self::hop(hi).min(self.db.len()));
        let time = |hop: usize| samples_to_us((hop * HOP).min(self.frames) as i64);
        let mut runs: Vec<(i64, i64)> = Vec::new();
        let mut start = None;
        for k in first..=last {
            let quiet = k < last && self.db[k] < threshold;
            match (quiet, start) {
                (true, None) => start = Some(k),
                (false, Some(s)) => {
                    // The last quiet window ends 10 ms after it starts.
                    runs.push((time(s).max(lo), (time(k - 1) + samples_to_us(WINDOW as i64)).min(hi)));
                    start = None;
                }
                _ => {}
            }
        }
        runs
    }

    /// The boundary between words `i - 1` and `i`, one of them measured by the word timing model.
    fn follow_sound(&self, words: &mut [Word], measured: &[bool], i: usize, room: f32) {
        let floor = |word: &Word| self.loudness(word.start_us, word.end_us).map(|db| (db - WORD_DROP_DB).max(room));
        let time = |hop: usize| samples_to_us((hop * HOP).min(self.frames) as i64);
        let duration = self.duration_us();
        let (left, right) = (i.checked_sub(1), (i < words.len()).then_some(i));
        let (end, start) = (left.map(|j| words[j].end_us), right.map(|j| words[j].start_us));
        let mut new_end = end;
        let mut quiet = true;
        if let (Some(j), Some(end)) = (left, end)
            && measured[j]
            && let Some(floor) = floor(&words[j])
        {
            let limit = start.unwrap_or((end + REACH_US).min(duration)).max(end);
            let (mut k, last) = (Self::hop(end), Self::hop(limit).min(self.db.len()));
            while k < last && self.db[k] >= floor {
                k += 1;
            }
            quiet = k < last;
            new_end = Some(time(k).clamp(end, limit));
        }
        let mut new_start = start;
        if let (Some(j), Some(start)) = (right, start)
            && measured[j]
            && let Some(floor) = floor(&words[j])
        {
            let limit = new_end.unwrap_or((start - REACH_US).max(0)).min(start);
            let (first, mut k) = (Self::hop(limit), Self::hop(start).min(self.db.len()));
            while k > first && self.db[k - 1] >= floor {
                k -= 1;
            }
            new_start = Some(time(k).clamp(limit, start));
        }
        if let (Some(end), Some(start)) = (end, start) {
            let both = left.is_some_and(|j| measured[j]) && right.is_some_and(|j| measured[j]);
            if both && !quiet {
                // Connected speech: the model's gap is the boundary.
                (new_end, new_start) = (Some((end + start) / 2), Some((end + start) / 2));
            }
            let (e, s) = (new_end.unwrap_or(end), new_start.unwrap_or(start));
            // An unmeasured neighbour gives way to the measured word's sound.
            new_end = Some(e.min(s.max(e.min(start))));
            new_start = Some(s.max(new_end.unwrap_or(e)));
        }
        if let (Some(j), Some(end)) = (left, new_end) {
            words[j].end_us = end.max(words[j].start_us);
        }
        if let (Some(j), Some(start)) = (right, new_start) {
            words[j].start_us = start.min(words[j].end_us);
        }
    }

    /// The level a word is spoken at: the 90th percentile of its windows.
    fn loudness(&self, start: i64, end: i64) -> Option<f32> {
        let (first, last) = (Self::hop(start), Self::hop(end).max(Self::hop(start) + 1).min(self.db.len()));
        let mut heard: Vec<f32> = self.db.get(first..last)?.to_vec();
        if heard.is_empty() {
            return None;
        }
        heard.sort_unstable_by(f32::total_cmp);
        Some(heard[(heard.len() - 1) * 9 / 10])
    }

    /// The middle of the quietest window in `range`, the one nearest `near` on a tie.
    fn quietest(&self, (lo, hi): (i64, i64), near: i64) -> i64 {
        let half = samples_to_us(WINDOW as i64 / 2);
        let (first, last) =
            (Self::hop((lo - half).max(0)), Self::hop((hi - half).max(0)).min(self.db.len().saturating_sub(1)));
        (first..=last)
            .map(|k| (k, samples_to_us((k * HOP) as i64) + half))
            .filter(|&(_, t)| (lo..=hi).contains(&t))
            .min_by(|a, b| self.db[a.0].total_cmp(&self.db[b.0]).then(((a.1 - near).abs()).cmp(&(b.1 - near).abs())))
            .map_or(near, |(_, t)| t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: usize = 48_000;

    /// Room tone at about -60 dBFS with "speech" bursts at -20 dBFS in the given second ranges;
    /// each burst has a 40 ms closure in its middle, as a stop consonant has.
    fn recording(seconds: f64, speech: &[(f64, f64)]) -> Vec<f32> {
        let frames = (seconds * RATE as f64) as usize;
        let mut noise = 0x2545_f491_u32;
        let mut samples = Vec::with_capacity(frames * CHANNELS);
        for n in 0..frames {
            let t = n as f64 / RATE as f64;
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let room = (noise as f32 / u32::MAX as f32 - 0.5) * 0.003;
            let voiced = speech.iter().any(|&(a, b)| {
                let closure = (a + b) / 2.0;
                t >= a && t < b && !(t >= closure - 0.02 && t < closure + 0.02)
            });
            let value = room + if voiced { 0.14 * (t * 2.0 * std::f64::consts::PI * 180.0).sin() as f32 } else { 0.0 };
            samples.extend([value; CHANNELS]);
        }
        samples
    }

    fn word(start: f64, end: f64, text: &str) -> Word {
        Word { start_us: (start * 1e6) as i64, end_us: (end * 1e6) as i64, text: text.into(), probability: 0.9 }
    }

    fn near(us: i64, seconds: f64) -> bool {
        (us - (seconds * 1e6) as i64).abs() <= 12_000
    }

    #[test]
    fn a_word_stretched_over_a_pause_ends_where_its_sound_ends() {
        // "Jakoby, potom": the filler really ends at 0.52 s and the next word starts at 0.76 s, but
        // Whisper stretches the filler to 0.86 s and starts the next word there.
        let samples = recording(2.0, &[(0.1, 0.52), (0.76, 1.3), (1.3, 1.8)]);
        let mut words = vec![word(0.1, 0.86, "Jakoby,"), word(0.86, 1.3, "potom"), word(1.3, 1.8, "řešíte")];
        align_words(&mut words, &samples);
        assert!(near(words[0].end_us, 0.52), "{words:?}");
        assert!(near(words[1].start_us, 0.76), "{words:?}");
        // Connected words share one boundary at the quietest moment near Whisper's estimate.
        assert_eq!(words[1].end_us, words[2].start_us);
        assert!((words[1].end_us - 1_300_000).abs() <= NUDGE_US, "{words:?}");
    }

    #[test]
    fn an_early_estimate_moves_to_where_the_word_starts() {
        // Whisper starts the second word 270 ms before it is heard.
        let samples = recording(2.0, &[(0.0, 0.3), (0.49, 1.2)]);
        let mut words = vec![word(0.0, 0.28, "Ehm,"), word(0.22, 1.2, "nejdřív")];
        align_words(&mut words, &samples);
        assert!(near(words[0].end_us, 0.3) && near(words[1].start_us, 0.49), "{words:?}");
    }

    #[test]
    fn a_pause_wins_over_a_closure_inside_a_word() {
        // The estimate sits on the closure inside the second word; the real pause is 150 ms away.
        let samples = recording(2.0, &[(0.1, 0.5), (0.7, 1.3)]);
        let mut words = vec![word(0.1, 1.0, "první"), word(1.0, 1.3, "druhé")];
        align_words(&mut words, &samples);
        assert!(near(words[0].end_us, 0.5) && near(words[1].start_us, 0.7), "{words:?}");
    }

    #[test]
    fn edges_of_speech_follow_the_sound_and_stay_put_without_a_pause() {
        let samples = recording(3.0, &[(0.4, 1.0), (2.0, 3.0)]);
        let mut words = vec![word(0.25, 1.15, "a"), word(2.1, 3.0, "b")];
        align_words(&mut words, &samples);
        assert!(near(words[0].start_us, 0.4) && near(words[0].end_us, 1.0), "{words:?}");
        assert!(near(words[1].start_us, 2.0), "{words:?}");
        // Sound runs to the end of the file: the last word keeps Whisper's end.
        assert_eq!(words[1].end_us, 3_000_000);
    }

    #[test]
    fn order_and_lengths_survive_and_the_same_input_gives_the_same_words() {
        let samples = recording(4.0, &[(0.2, 0.9), (1.0, 1.6), (2.4, 3.9)]);
        let original = vec![
            word(0.0, 1.2, "a"),
            word(1.2, 1.2, "b"),
            word(1.1, 2.6, "c"),
            word(2.6, 2.7, "d"),
            word(3.0, 4.0, "e"),
        ];
        let mut first = original.clone();
        align_words(&mut first, &samples);
        let mut second = original.clone();
        align_words(&mut second, &samples);
        assert_eq!(first, second);
        assert!(first.iter().all(|w| w.end_us >= w.start_us), "{first:?}");
        assert!(first.windows(2).all(|p| p[0].end_us <= p[1].start_us), "{first:?}");
        assert_eq!(first.iter().map(|w| &w.text).collect::<Vec<_>>(), ["a", "b", "c", "d", "e"]);
    }

    /// Speech over loud steady noise, or speech without a pause: nothing is quiet enough to be a
    /// pause, so the words keep Whisper's times instead of collapsing into "silence".
    #[test]
    fn without_clear_pauses_words_keep_their_times() {
        let mut state = 0x1234_5678_u32;
        let samples: Vec<f32> = (0..RATE * 3)
            .flat_map(|n| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                // About -23 dBFS, swaying by a couple of dB.
                let sway = 1.0 + 0.15 * (n as f32 / RATE as f32 * 3.0).sin();
                [(state as f32 / u32::MAX as f32 - 0.5) * 0.24 * sway; CHANNELS]
            })
            .collect();
        let original = vec![word(0.2, 0.9, "a"), word(0.9, 1.7, "b"), word(1.8, 2.6, "c")];
        let mut words = original.clone();
        align_words(&mut words, &samples);
        assert_eq!(words, original);
    }

    #[test]
    fn silence_and_digital_padding_leave_words_alone() {
        let mut words = vec![word(0.1, 0.5, "a")];
        align_words(&mut words, &vec![0.0; RATE * CHANNELS]);
        assert_eq!(words, vec![word(0.1, 0.5, "a")]);
        align_words(&mut words, &[]);
        assert_eq!(words, vec![word(0.1, 0.5, "a")]);
    }

    #[test]
    fn a_measured_word_that_ends_early_before_a_pause_ends_with_its_sound() {
        // The model hears a held vowel end at 0.6 s; it sounds until 0.9 s. The next word, heard
        // 50 ms late, starts at 1.4 s.
        let samples = recording(2.5, &[(0.1, 0.9), (1.4, 2.0)]);
        let mut words = vec![word(0.1, 0.6, "dnů."), word(1.45, 2.0, "Byla")];
        refine_words(&mut words, &samples, &[true, true]);
        assert!(near(words[0].end_us, 0.9) && near(words[1].start_us, 1.4), "{words:?}");
        assert!(near(words[0].start_us, 0.1) && near(words[1].end_us, 2.0), "{words:?}");
    }

    #[test]
    fn connected_measured_words_keep_the_models_boundary() {
        // One stretch of speech; the second word's stop closure sits 60 ms after the boundary the
        // model heard. Pause alignment would move the cut into that closure, into the word.
        let samples = recording(2.0, &[(0.1, 1.5)]);
        let mut words = vec![word(0.1, 0.7, "jako"), word(0.74, 1.5, "noční")];
        let mut pauses = words.clone();
        refine_words(&mut words, &samples, &[true, true]);
        assert_eq!((words[0].end_us, words[1].start_us), (720_000, 720_000));
        align_words(&mut pauses, &samples);
        assert!(near(pauses[1].start_us, 0.82), "{pauses:?}");
    }

    #[test]
    fn the_rooms_echo_after_a_word_is_not_the_word() {
        // Speech near -20 dBFS to 0.6 s, then an echo 25 dB lower to 0.8 s, then the room.
        let samples: Vec<f32> = (0..RATE * 2)
            .flat_map(|n| {
                let t = n as f64 / RATE as f64;
                let tone = (t * 2.0 * std::f64::consts::PI * 180.0).sin() as f32;
                let level = if (0.1..0.6).contains(&t) {
                    0.14
                } else if (0.6..0.8).contains(&t) {
                    0.008
                } else {
                    0.0003
                };
                [tone * level; CHANNELS]
            })
            .collect();
        let mut words = vec![word(0.1, 0.5, "krysy."), word(1.5, 1.9, "Byla")];
        refine_words(&mut words, &samples, &[true, false]);
        assert!(near(words[0].end_us, 0.6), "{words:?}");
    }
}
