//! Speed that keeps the pitch (WSOLA). The sound plays in overlapping pieces of 21 ms at their own
//! rate. Speed decides roughly where in the file each piece starts; within 10.7 ms of there it
//! starts where it best continues the piece before, so the overlaps add up without beats. Sound
//! stays within half a piece of where speed puts it, which captions and the picture rely on.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::audio::{EDGE_FADE, QUIET_RMS};
use crate::model::CHANNELS;

const FRAME: usize = 1024;
/// Each piece overlaps half of the next.
const HOP: i64 = FRAME as i64 / 2;
/// How far a piece may move from where speed puts it: one period of the lowest voices.
const SEEK: i64 = 512;
/// The search first compares averages of 4 samples, then the samples around the best.
const COARSE: i64 = 4;
/// In every block of this many pieces (5.5 s at 1x) the quietest starts where speed puts it when it
/// is at least 10 dB below the loudest, as a pause between words in a noisy room is.
const BLOCK: i64 = 512;
const RESTART_BELOW_LOUDEST: f32 = 0.1;
/// Where no block of a span of this many (22 s at 1x) has such a dip, as in steady music, where a
/// join out of step can be heard, only the quietest piece of the span starts over. A piece is
/// found from at most two spans back.
const SPAN: i64 = 4;
/// Pieces remembered at most; any piece can be found again.
const REMEMBERED: usize = 1 << 16;

/// A periodic Hann window: the halves of two overlapping pieces add up to one.
static WINDOW: LazyLock<Vec<f32>> = LazyLock::new(|| {
    (0..FRAME).map(|n| (0.5 - 0.5 * (std::f64::consts::TAU * n as f64 / FRAME as f64).cos()) as f32).collect()
});

/// Where the pieces of one stretched sound start in its file. Piece `k` plays from `k * HOP`
/// frames after the anchor's place on the timeline. Each depends only on the file, the speed and
/// the anchor, so every buffer of playback and export hears the same pieces.
pub(crate) struct Stretch {
    anchor: i64,
    speed: f64,
    found: HashMap<i64, i64>,
    /// Each block's piece that starts over at a dip, if any, and its quietest piece with its loudness.
    blocks: HashMap<i64, (Option<i64>, (f32, i64))>,
    /// The pieces `reach` found last, from piece `first` on, for `sample` to read.
    first: i64,
    reached: Vec<i64>,
}

impl Stretch {
    /// `anchor` is the file frame speed puts at the anchor's place on the timeline.
    pub(crate) fn new(anchor: i64, speed: f64) -> Self {
        Self { anchor, speed, found: HashMap::new(), blocks: HashMap::new(), first: 0, reached: Vec::new() }
    }

    /// Where speed puts piece `k`: its middle on the straight line through the anchor.
    fn nominal(&self, k: i64) -> i64 {
        let middle = (k * HOP + HOP) as f64;
        self.anchor + (middle * self.speed - HOP as f64).round() as i64
    }

    /// Finds the pieces that sound from `from` to `to` frames after the anchor's place.
    pub(crate) fn reach(&mut self, samples: &[f32], from: i64, to: i64) {
        if self.found.len() > REMEMBERED {
            self.found.clear();
        }
        self.first = from.div_euclid(HOP) - 1;
        self.reached = (self.first..=(to - 1).div_euclid(HOP)).map(|k| self.piece(samples, k)).collect();
    }

    /// Piece `k`. Piece 0, pieces among quiet sound and the pieces that start over in a block or span
    /// start where speed puts them; the others continue their neighbour towards piece 0, found first.
    fn piece(&mut self, samples: &[f32], k: i64) -> i64 {
        let inwards = -k.signum();
        let mut j = k;
        let mut start = loop {
            if let Some(&start) = self.found.get(&j) {
                break start;
            }
            if j == 0 || self.free(samples, j) {
                break self.nominal(j);
            }
            j += inwards;
        };
        self.found.insert(j, start);
        while j != k {
            j -= inwards;
            start = best(samples, start - inwards * HOP, self.nominal(j));
            self.found.insert(j, start);
        }
        start
    }

    /// Whether piece `k` starts where speed puts it, whatever comes before it.
    fn free(&mut self, samples: &[f32], k: i64) -> bool {
        if loudness(samples, self.nominal(k)) <= QUIET_RMS * QUIET_RMS {
            return true;
        }
        let block = k.div_euclid(BLOCK);
        let (restart, _) = self.block(samples, block);
        if restart.is_some() {
            return restart == Some(k);
        }
        let span = block.div_euclid(SPAN) * SPAN;
        let blocks: Vec<_> = (span..span + SPAN).map(|b| self.block(samples, b)).collect();
        let quietest = blocks.iter().map(|b| b.1).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
        blocks.iter().all(|b| b.0.is_none()) && quietest.1 == k
    }

    /// The piece of `block` that starts over at a dip, if any, and its quietest piece with its loudness.
    fn block(&mut self, samples: &[f32], block: i64) -> (Option<i64>, (f32, i64)) {
        if let Some(&found) = self.blocks.get(&block) {
            return found;
        }
        let pieces = block * BLOCK..(block + 1) * BLOCK;
        let levels: Vec<_> = pieces.map(|j| (loudness(samples, self.nominal(j)), j)).collect();
        let loudest = levels.iter().map(|l| l.0).fold(0.0, f32::max);
        let quietest = levels.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
        let found = ((quietest.0 <= loudest * RESTART_BELOW_LOUDEST).then_some(quietest.1), quietest);
        self.blocks.insert(block, found);
        found
    }

    /// The sound `at` frames after the anchor's place; `reach` must have found its pieces. Pieces
    /// reach past where speed puts them, so they read the file only in `kept`, the frames the clip
    /// plays, coming in and going out at its edges as a clip edge does.
    pub(crate) fn sample(&self, samples: &[f32], at: i64, channel: usize, kept: (i64, i64)) -> f32 {
        let (k, n) = (at.div_euclid(HOP), at.rem_euclid(HOP));
        let read = |frame: i64| {
            let Some(&sample) = usize::try_from(frame).ok().and_then(|f| samples.get(f * CHANNELS + channel)) else {
                return 0.0;
            };
            match (frame - kept.0).min(kept.1 - 1 - frame) {
                inside if inside >= EDGE_FADE => sample,
                inside => sample * inside.max(0) as f32 / EDGE_FADE as f32,
            }
        };
        let (before, piece) = (self.reached[(k - 1 - self.first) as usize], self.reached[(k - self.first) as usize]);
        WINDOW[(n + HOP) as usize] * read(before + n + HOP) + WINDOW[n as usize] * read(piece + n)
    }
}

/// Mean square of the sound around a piece that starts at `start`, wherever the search could move
/// it. Nobody hears how a piece among quiet sound joins, so it starts where speed puts it, and the
/// pieces after it depend on nothing before it: playback started late in a long clip finds its
/// pieces from the last pause, not from the start of the clip.
fn loudness(samples: &[f32], start: i64) -> f32 {
    let (from, to) = ((start - SEEK).max(0) as usize, (start + FRAME as i64 + SEEK).max(0) as usize);
    let part = samples.get(from * CHANNELS..(to * CHANNELS).min(samples.len())).unwrap_or(&[]);
    dot(part, part) / ((to - from).max(1) * CHANNELS) as f32
}

/// The start within `SEEK` of `nominal` whose sound is most like the file at `target`, the sound
/// that plays on after the neighbouring piece. Silence keeps `nominal`.
fn best(samples: &[f32], target: i64, nominal: i64) -> i64 {
    const SPAN: usize = FRAME + 2 * SEEK as usize + COARSE as usize;
    let mono = |frame: i64| {
        usize::try_from(frame)
            .ok()
            .and_then(|f| samples.get(f * CHANNELS..f * CHANNELS + 2))
            .map_or(0.0, |s| (s[0] + s[1]) * 0.5)
    };
    let base = nominal - SEEK;
    let wanted: [f32; FRAME] = std::array::from_fn(|n| mono(target + n as i64));
    let around: [f32; SPAN] = std::array::from_fn(|n| mono(base + n as i64));
    let score = |candidate: &[f32], like: &[f32]| dot(candidate, like) / (dot(candidate, candidate) + 1e-9).sqrt();

    let coarse =
        |x: &[f32]| x.as_chunks::<{ COARSE as usize }>().0.iter().map(|c| c.iter().sum()).collect::<Vec<f32>>();
    let (wanted_coarse, around_coarse) = (coarse(&wanted), coarse(&around));
    let mut at = SEEK / COARSE;
    let mut top = score(&around_coarse[at as usize..][..wanted_coarse.len()], &wanted_coarse);
    for q in 0..=2 * SEEK / COARSE {
        let s = score(&around_coarse[q as usize..][..wanted_coarse.len()], &wanted_coarse);
        if s > top {
            (at, top) = (q, s);
        }
    }

    let mut at = at * COARSE;
    let mut top = score(&around[at as usize..][..FRAME], &wanted);
    for offset in (at - COARSE + 1).max(0)..=(at + COARSE - 1).min(2 * SEEK) {
        let s = score(&around[offset as usize..][..FRAME], &wanted);
        if s > top {
            (at, top) = (offset, s);
        }
    }
    base + at
}

/// In eight lanes, so it compiles to vector instructions.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0f32; 8];
    for (a, b) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
        lanes = std::array::from_fn(|i| lanes[i] + a[i] * b[i]);
    }
    lanes.iter().sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A steady tone at 1.25x stays steady over 12 s, past two blocks: pieces before the anchor, which a
    /// transition plays, join like those after it, nothing starts over out of step, and each piece is the same
    /// whichever buffer asks for it first. Late in the tone, finding a piece takes bounded work.
    #[test]
    fn a_steady_tone_stays_steady_on_both_sides_of_the_anchor_and_across_blocks() {
        let tone: Vec<f32> = (0..100 * 48_000)
            .flat_map(|i| [(std::f64::consts::TAU * 440.0 * i as f64 / 48_000.0).sin() as f32 * 0.25; 2])
            .collect();
        let (before, after) = (24_000, 12 * 48_000);
        let play = |buffers: &[(i64, i64)]| {
            let mut stretch = Stretch::new(48_000, 1.25);
            let mut out = vec![0.0; (before + after) as usize];
            for &(from, to) in buffers {
                stretch.reach(&tone, from, to);
                for at in from..to {
                    out[(at + before) as usize] = stretch.sample(&tone, at, 0, (0, 100 * 48_000));
                }
            }
            out
        };
        let whole = play(&[(-before, after)]);
        let backwards: Vec<_> = (-before / 1000..after / 1000).rev().map(|i| (i * 1000, (i + 1) * 1000)).collect();
        assert!(play(&backwards) == whole, "the pieces depend on the order buffers ask for them");
        // 2400 samples are 22 whole periods of 440 Hz.
        for (index, window) in whole.chunks(2400).enumerate() {
            let rms = (window.iter().map(|s| s * s).sum::<f32>() / window.len() as f32).sqrt();
            assert!((rms / (0.25 / 2f32.sqrt()) - 1.0).abs() < 0.02, "window {index} is at {rms}");
        }
        // Playback started 70 s into the tone finds its pieces from at most two spans back.
        let mut stretch = Stretch::new(48_000, 1.25);
        stretch.reach(&tone, 70 * 48_000, 70 * 48_000 + 1024);
        assert!(stretch.found.len() as i64 <= 2 * SPAN * BLOCK + 4, "{} pieces found", stretch.found.len());
    }
}
