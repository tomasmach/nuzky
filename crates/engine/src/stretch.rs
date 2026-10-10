//! Speed that keeps the pitch (WSOLA). The sound plays in overlapping pieces of 21 ms at their own
//! rate. Speed decides roughly where in the file each piece starts; within 10.7 ms of there it
//! starts where it best continues the piece before, so the overlaps add up without beats. Sound
//! stays within half a piece of where speed puts it, which captions and the picture rely on.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::audio::QUIET_RMS;
use crate::model::CHANNELS;

const FRAME: usize = 1024;
/// Each piece overlaps half of the next.
const HOP: i64 = FRAME as i64 / 2;
/// How far a piece may move from where speed puts it: one period of the lowest voices.
const SEEK: i64 = 512;
/// The search first compares averages of 4 samples, then the samples around the best.
const COARSE: i64 = 4;
/// In every block of this many pieces (5.5 s at 1x) the quietest starts where speed puts it, so a
/// piece is found from at most two blocks back, also in sound without a pause.
const BLOCK: i64 = 512;

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
    /// The quietest piece of each block.
    quietest: HashMap<i64, i64>,
    /// The pieces `reach` found last, from piece `first` on, for `sample` to read.
    first: i64,
    reached: Vec<i64>,
}

impl Stretch {
    /// `anchor` is the file frame speed puts at the anchor's place on the timeline.
    pub(crate) fn new(anchor: i64, speed: f64) -> Self {
        Self { anchor, speed, found: HashMap::new(), quietest: HashMap::new(), first: 0, reached: Vec::new() }
    }

    /// Where speed puts piece `k`: its middle on the straight line through the anchor.
    fn nominal(&self, k: i64) -> i64 {
        let middle = (k * HOP + HOP) as f64;
        self.anchor + (middle * self.speed - HOP as f64).round() as i64
    }

    /// Finds the pieces that sound from `from` to `to` frames after the anchor's place.
    pub(crate) fn reach(&mut self, samples: &[f32], from: i64, to: i64) {
        self.first = from.div_euclid(HOP) - 1;
        self.reached = (self.first..=(to - 1).div_euclid(HOP)).map(|k| self.piece(samples, k)).collect();
    }

    /// Piece `k`. Piece 0, pieces among quiet sound and the quietest piece of each block start where
    /// speed puts them; the others continue their neighbour towards piece 0, found first.
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
        let level = |k: i64| loudness(samples, self.nominal(k));
        if level(k) <= QUIET_RMS * QUIET_RMS {
            return true;
        }
        let block = k.div_euclid(BLOCK);
        if let Some(&quietest) = self.quietest.get(&block) {
            return quietest == k;
        }
        let pieces = (block * BLOCK..(block + 1) * BLOCK).map(|j| (level(j), j));
        let quietest = pieces.min_by(|a, b| a.0.total_cmp(&b.0)).unwrap().1;
        self.quietest.insert(block, quietest);
        quietest == k
    }

    /// The sound `at` frames after the anchor's place; `reach` must have found its pieces.
    pub(crate) fn sample(&self, samples: &[f32], at: i64, channel: usize) -> f32 {
        let (k, n) = (at.div_euclid(HOP), at.rem_euclid(HOP));
        let read = |frame: i64| {
            usize::try_from(frame).ok().and_then(|f| samples.get(f * CHANNELS + channel)).copied().unwrap_or(0.0)
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

    /// Pieces before the anchor, which a transition plays, keep a tone as steady as those after it, and each
    /// piece is the same whichever buffer asks for it first.
    #[test]
    fn pieces_before_the_anchor_join_like_those_after_it() {
        let tone: Vec<f32> = (0..96_000)
            .flat_map(|i| [(std::f64::consts::TAU * 440.0 * i as f64 / 48_000.0).sin() as f32 * 0.25; 2])
            .collect();
        let play = |buffers: &[(i64, i64)]| {
            let mut stretch = Stretch::new(48_000, 1.25);
            let mut out = vec![0.0; 48_000];
            for &(from, to) in buffers {
                stretch.reach(&tone, from, to);
                for at in from..to {
                    out[(at + 24_000) as usize] = stretch.sample(&tone, at, 0);
                }
            }
            out
        };
        let whole = play(&[(-24_000, 24_000)]);
        let backwards: Vec<_> = (-24..24).rev().map(|i| (i * 1000, (i + 1) * 1000)).collect();
        assert!(play(&backwards) == whole, "the pieces depend on the order buffers ask for them");
        // 2400 samples are 22 whole periods of 440 Hz.
        for (index, window) in whole.chunks(2400).enumerate() {
            let rms = (window.iter().map(|s| s * s).sum::<f32>() / window.len() as f32).sqrt();
            assert!((rms / (0.25 / 2f32.sqrt()) - 1.0).abs() < 0.02, "window {index} is at {rms}");
        }
    }
}
