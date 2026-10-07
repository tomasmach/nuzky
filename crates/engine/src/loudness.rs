//! Loudness and true peak after ITU-R BS.1770-4 (EBU R128), and the true-peak limiter the
//! Reels & TikTok export levels its sound with. Everything runs on the 48 kHz stereo of the
//! PCM cache and the mixer.

use std::collections::VecDeque;

use crate::model::{CHANNELS, SAMPLE_RATE};

/// 400 ms gating blocks that start every 100 ms (75 % overlap).
const STEP: usize = SAMPLE_RATE as usize / 10;
const BLOCK: usize = STEP * 4;
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
const RELATIVE_GATE_LU: f64 = -10.0;

/// Loudness of a mean square summed over channels (all weights 1.0 for left and right).
fn lufs(power: f64) -> f64 {
    -0.691 + 10.0 * power.log10()
}

pub fn db_to_gain(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

pub fn gain_to_db(gain: f32) -> f64 {
    20.0 * (gain as f64).log10()
}

#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// K-weighting at 48 kHz, BS.1770-4 Tables 1 and 2: the head's high shelf, then the RLB high-pass.
const SHELF: Biquad = Biquad {
    b: [1.53512485958697, -2.69169618940638, 1.19839281085285],
    a: [-1.69065929318241, 0.73248077421585],
    z: [0.0; 2],
};
const HIGH_PASS: Biquad = Biquad { b: [1.0, -2.0, 1.0], a: [-1.99004745483398, 0.99007225036621], z: [0.0; 2] };

/// Integrated loudness of 400 ms block powers with the absolute and the relative gate, or `None`
/// when no block is louder than -70 LUFS.
fn gated(blocks: &[f64]) -> Option<f64> {
    let mean = |blocks: &mut dyn Iterator<Item = f64>| {
        let (sum, count) = blocks.fold((0.0, 0usize), |(s, n), z| (s + z, n + 1));
        (count > 0).then(|| sum / count as f64)
    };
    let loud = || blocks.iter().copied().filter(|&z| z > 0.0 && lufs(z) > ABSOLUTE_GATE_LUFS);
    let relative = lufs(mean(&mut loud())?) + RELATIVE_GATE_LU;
    mean(&mut loud().filter(|&z| lufs(z) > relative)).map(lufs)
}

/// What a meter read from a whole programme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    /// Integrated loudness in LUFS; `None` when nothing is louder than -70 LUFS, as in silence.
    pub integrated: Option<f64>,
    /// Largest magnitude of the 4x oversampled signal; 1.0 is 0 dBTP.
    pub true_peak: f32,
}

impl Loudness {
    /// True peak in dBTP, -120 for digital silence so it stays a finite number.
    pub fn true_peak_db(&self) -> f64 {
        gain_to_db(self.true_peak).max(-120.0)
    }
}

/// Integrated loudness (BS.1770-4) and true peak (Annex 2) of interleaved stereo pushed in any
/// chunks. Only complete 400 ms blocks count, as in the standard.
pub struct Meter {
    weighting: [[Biquad; 2]; CHANNELS],
    step_power: f64,
    step_frames: usize,
    recent: [f64; 4],
    steps: usize,
    blocks: Vec<f64>,
    peak: TruePeak,
    true_peak: f32,
}

impl Default for Meter {
    fn default() -> Self {
        Self::new()
    }
}

impl Meter {
    pub fn new() -> Self {
        Self {
            weighting: [[SHELF, HIGH_PASS]; CHANNELS],
            step_power: 0.0,
            step_frames: 0,
            recent: [0.0; 4],
            steps: 0,
            blocks: Vec::new(),
            peak: TruePeak::new(),
            true_peak: 0.0,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(CHANNELS) {
            for ([shelf, high_pass], &x) in self.weighting.iter_mut().zip(frame) {
                let y = high_pass.run(shelf.run(x as f64));
                self.step_power += y * y;
            }
            self.true_peak = self.true_peak.max(self.peak.push(frame));
            self.step_frames += 1;
            if self.step_frames == STEP {
                self.recent[self.steps % 4] = self.step_power;
                self.steps += 1;
                (self.step_power, self.step_frames) = (0.0, 0);
                if self.steps >= 4 {
                    self.blocks.push(self.recent.iter().sum::<f64>() / BLOCK as f64);
                }
            }
        }
    }

    pub fn finish(mut self) -> Loudness {
        // The last samples' oversampled peaks lie in the detector's delay.
        for _ in 0..DELAY {
            self.true_peak = self.true_peak.max(self.peak.push(&[0.0; CHANNELS]));
        }
        Loudness { integrated: gated(&self.blocks), true_peak: self.true_peak }
    }
}

/// Taps per oversampling phase; the interpolation reads 8 samples either side.
const TAPS: usize = 16;
const HALF: usize = TAPS / 2;
/// Frames between a sample entering `TruePeak` and the peak reported for it.
pub const DELAY: usize = TAPS - HALF;
const KAISER_BETA: f64 = 7.0;

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, q) = (1.0, 1.0, x * x / 4.0);
    for k in 1..64 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-15 {
            break;
        }
    }
    sum
}

/// The three in-between phases of 4x oversampling: a Kaiser-windowed sinc cut off at the 24 kHz
/// Nyquist frequency, each phase scaled to unity gain at DC. Phase 0 is the sample itself.
fn interpolation_taps() -> [[f32; TAPS]; 3] {
    let mut taps = [[0.0; TAPS]; 3];
    for (p, phase) in taps.iter_mut().enumerate() {
        let fraction = (p + 1) as f64 / 4.0;
        let mut row = [0.0f64; TAPS];
        for (k, tap) in row.iter_mut().enumerate() {
            let d = k as f64 - (HALF - 1) as f64 - fraction;
            let window =
                bessel_i0(KAISER_BETA * (1.0 - (d / HALF as f64).powi(2)).max(0.0).sqrt()) / bessel_i0(KAISER_BETA);
            *tap = (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d) * window;
        }
        let sum: f64 = row.iter().sum();
        for (tap, value) in phase.iter_mut().zip(row) {
            *tap = (value / sum) as f32;
        }
    }
    taps
}

/// 4x oversampled peak detector, BS.1770-4 Annex 2. Every `push` takes one frame and returns the
/// largest magnitude of all channels from the frame `DELAY` frames earlier up to the next one.
#[derive(Clone)]
pub struct TruePeak {
    taps: [[f32; TAPS]; 3],
    /// Each channel's last `TAPS` samples, written twice so they always read as one slice.
    history: [[f32; 2 * TAPS]; CHANNELS],
    next: usize,
}

impl Default for TruePeak {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeak {
    pub fn new() -> Self {
        Self { taps: interpolation_taps(), history: [[0.0; 2 * TAPS]; CHANNELS], next: 0 }
    }

    pub fn push(&mut self, frame: &[f32]) -> f32 {
        for (history, &x) in self.history.iter_mut().zip(frame) {
            history[self.next] = x;
            history[self.next + TAPS] = x;
        }
        self.next = (self.next + 1) % TAPS;
        let mut peak = 0f32;
        for history in &self.history {
            let window = &history[self.next..self.next + TAPS];
            peak = peak.max(window[HALF - 1].abs());
            for phase in &self.taps {
                let y: f32 = window.iter().zip(phase).map(|(x, h)| x * h).sum();
                peak = peak.max(y.abs());
            }
        }
        peak
    }
}

/// Look-ahead of the limiter: gain reduction ramps in over 5 ms before a peak.
const LOOKAHEAD: usize = SAMPLE_RATE as usize / 200;
/// Time constant for the gain to come back up after a peak.
const RELEASE_S: f64 = 0.08;

/// A deterministic look-ahead limiter that keeps the true peak at or under a ceiling. It never
/// raises the level (no make-up gain) and delays the sound by `latency()` frames, which the caller
/// reads ahead so nothing moves against the picture.
///
/// The gain is the moving average over `LOOKAHEAD + 1` frames of a curve that is never above the
/// gain any frame within the next `LOOKAHEAD` frames needs, so at every peak the average is at most
/// what that peak needs: the ramp is finished when the peak arrives. `LOOKAHEAD` frames of silence
/// run through first, so a peak in the very first frame is ramped into as well.
pub struct Limiter {
    ceiling: f32,
    release: f32,
    peaks: TruePeak,
    pushed: u64,
    /// Frames waiting for their gain, oldest first.
    waiting: VecDeque<[f32; CHANNELS]>,
    /// Increasing gains needed by the frames in the look-ahead window, with their index.
    needed: VecDeque<(u64, f32)>,
    held: f32,
    averaged: VecDeque<f32>,
    sum: f64,
    /// Frames of the leading silence still to drop from the output.
    lead_in: usize,
}

impl Limiter {
    /// `ceiling` is the highest true peak let through, linear.
    pub fn new(ceiling: f32) -> Self {
        let mut limiter = Self {
            ceiling,
            release: (1.0 - (-1.0 / (RELEASE_S * SAMPLE_RATE as f64)).exp()) as f32,
            peaks: TruePeak::new(),
            pushed: 0,
            waiting: VecDeque::with_capacity(Self::latency() + 1),
            needed: VecDeque::new(),
            held: 1.0,
            averaged: std::iter::repeat_n(1.0, LOOKAHEAD).collect(),
            sum: LOOKAHEAD as f64,
            lead_in: 0,
        };
        for _ in 0..LOOKAHEAD {
            limiter.push([0.0; CHANNELS]);
        }
        limiter.lead_in = LOOKAHEAD;
        limiter
    }

    /// Frames between a frame going in and the same frame coming out.
    pub const fn latency() -> usize {
        DELAY + LOOKAHEAD
    }

    /// Takes one frame and returns the frame `latency()` frames earlier, once there is one.
    pub fn push(&mut self, frame: [f32; CHANNELS]) -> Option<[f32; CHANNELS]> {
        self.waiting.push_back(frame);
        let peak = self.peaks.push(&frame);
        self.pushed += 1;
        // The detector reports the frame `DELAY` frames back; before that it saw nothing real.
        let detected = self.pushed.checked_sub(1 + DELAY as u64)?;
        let need = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };
        while self.needed.back().is_some_and(|&(_, g)| g >= need) {
            self.needed.pop_back();
        }
        self.needed.push_back((detected, need));
        let index = detected.checked_sub(LOOKAHEAD as u64)?;
        while self.needed.front().is_some_and(|&(i, _)| i < index) {
            self.needed.pop_front();
        }
        let target = self.needed.front().map_or(1.0, |&(_, g)| g);
        self.held = target.min(self.held + (1.0 - self.held) * self.release);
        self.averaged.push_back(self.held);
        self.sum += self.held as f64;
        let gain = (self.sum / (LOOKAHEAD + 1) as f64) as f32;
        self.sum -= self.averaged.pop_front().unwrap_or(1.0) as f64;
        let frame = self.waiting.pop_front().expect("a frame waits for every gain");
        if self.lead_in > 0 {
            self.lead_in -= 1;
            return None;
        }
        Some(frame.map(|x| x * gain))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = SAMPLE_RATE as f64;

    /// Stereo sine with the given peak in dBFS on both channels (`left_only`: right silent). It fades
    /// in and out over 5 ms, as a tone that starts with a jump has peaks of its own between samples.
    fn sine(freq: f64, db: f64, seconds: f64, phase_deg: f64, left_only: bool) -> Vec<f32> {
        let amplitude = 10f64.powf(db / 20.0);
        let phase = phase_deg.to_radians();
        let frames = (seconds * RATE) as usize;
        let ramp = 240.min(frames / 2) as f64;
        (0..frames)
            .flat_map(|n| {
                let edge = ((n.min(frames - 1 - n) as f64 + 0.5) / ramp).min(1.0);
                let fade = (edge * std::f64::consts::FRAC_PI_2).sin().powi(2);
                let x = (fade * amplitude * (std::f64::consts::TAU * freq * n as f64 / RATE + phase).sin()) as f32;
                [x, if left_only { 0.0 } else { x }]
            })
            .collect()
    }

    fn measure(samples: &[f32]) -> Loudness {
        let mut meter = Meter::new();
        // Odd chunks: a block must not depend on how the audio arrives.
        for chunk in samples.chunks(1234 * CHANNELS) {
            meter.push(chunk);
        }
        meter.finish()
    }

    fn integrated(samples: &[f32]) -> f64 {
        measure(samples).integrated.unwrap()
    }

    /// EBU Tech 3341 test 1: a 1 kHz sine at -23 dBFS on both channels reads -23.0 ± 0.1 LUFS.
    /// On one channel it is 3 dB quieter.
    #[test]
    fn a_minus_23_dbfs_sine_reads_minus_23_lufs() {
        let stereo = integrated(&sine(1000.0, -23.0, 20.0, 0.0, false));
        assert!((stereo + 23.0).abs() < 0.1, "{stereo}");
        let mono = integrated(&sine(1000.0, -23.0, 20.0, 0.0, true));
        assert!((mono + 26.01).abs() < 0.1, "{mono}");
    }

    /// EBU Tech 3341 tests 3 to 5: quiet parts below the gates do not pull the result down.
    #[test]
    fn gates_leave_out_quiet_passages() {
        let parts = |parts: &[(f64, f64)]| -> Vec<f32> {
            parts.iter().flat_map(|&(db, seconds)| sine(1000.0, db, seconds, 0.0, false)).collect()
        };
        let relative = integrated(&parts(&[(-36.0, 10.0), (-23.0, 60.0), (-36.0, 10.0)]));
        let absolute = integrated(&parts(&[(-72.0, 10.0), (-36.0, 10.0), (-23.0, 60.0), (-36.0, 10.0), (-72.0, 10.0)]));
        let mixed = integrated(&parts(&[(-26.0, 20.0), (-20.0, 20.1), (-26.0, 20.0)]));
        for value in [relative, absolute, mixed] {
            assert!((value + 23.0).abs() < 0.1, "{relative} {absolute} {mixed}");
        }
        // Nothing above the absolute gate has no integrated loudness at all.
        assert_eq!(measure(&vec![0.0; 10 * SAMPLE_RATE as usize * CHANNELS]).integrated, None);
        assert_eq!(measure(&sine(1000.0, -80.0, 5.0, 0.0, false)).integrated, None);
        assert_eq!(measure(&sine(1000.0, -10.0, 0.39, 0.0, false)).integrated, None, "no complete block");
        assert_eq!(measure(&[]).true_peak_db(), -120.0);
    }

    /// EBU Tech 3341 tests 15 to 18: sines whose peaks fall between samples read -6.0 +0.2/-0.4 dBTP,
    /// though their samples are up to 3 dB lower.
    #[test]
    fn true_peak_finds_peaks_between_samples() {
        for (divisor, phase) in [(4.0, 0.0), (4.0, 45.0), (6.0, 60.0), (8.0, 67.5)] {
            let tone = sine(RATE / divisor, -6.0, 1.0, phase, false);
            let sample_peak = tone.iter().fold(0f32, |m, x| m.max(x.abs()));
            let peak = measure(&tone).true_peak_db();
            assert!((-6.4..=-5.8).contains(&peak), "fs/{divisor} at {phase}°: {peak} dBTP");
            if phase == 45.0 {
                assert!(gain_to_db(sample_peak) < -8.9, "the samples miss the peak: {sample_peak}");
            }
        }
    }

    fn limit(samples: &[f32], ceiling_db: f64) -> Vec<f32> {
        let mut limiter = Limiter::new(db_to_gain(ceiling_db));
        let mut out = Vec::with_capacity(samples.len());
        let tail = std::iter::repeat_n([0.0; CHANNELS], Limiter::latency());
        for frame in samples.chunks_exact(CHANNELS).map(|f| [f[0], f[1]]).chain(tail) {
            if let Some(y) = limiter.push(frame) {
                out.extend(y);
            }
        }
        out
    }

    #[test]
    fn limiter_holds_the_true_peak_without_moving_the_sound() {
        // Quiet tone with loud, short bursts, like speech: the bursts are pulled down, the rest stays.
        let mut sound = sine(997.0, -30.0, 2.0, 0.0, false);
        for (start, db) in [(0.5, 0.0), (1.2, 6.0), (1.21, 3.0)] {
            let at = (start * RATE) as usize * CHANNELS;
            let burst = sine(3000.0, db, 0.01, 45.0, false);
            for (s, b) in sound[at..at + burst.len()].iter_mut().zip(burst) {
                *s += b;
            }
        }
        let out = limit(&sound, -2.0);
        assert_eq!(out.len(), sound.len(), "same number of samples");
        let peak = measure(&out).true_peak_db();
        assert!(peak <= -1.95, "true peak {peak} dBTP over a -2 dBTP ceiling");
        assert!(measure(&sound).true_peak_db() > 5.0);
        // Before the first burst the sound passes untouched; away from the bursts the gain is back
        // within 0.02 dB and nothing moved: a frame of delay would differ by 13 % of the tone.
        let i = (0.25 * RATE) as usize * CHANNELS;
        assert_eq!(out[i..i + 200], sound[i..i + 200]);
        for at in [1.0, 1.9] {
            let i = (at * RATE) as usize * CHANNELS;
            let worst = out[i..i + 200].iter().zip(&sound[i..i + 200]).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
            assert!(worst < 0.0316 * 0.0025, "changed or moved at {at} s by {worst}");
        }
    }

    #[test]
    fn limiter_keeps_silence_and_is_deterministic() {
        assert!(limit(&vec![0.0; 48_000 * CHANNELS], -1.0).iter().all(|&x| x == 0.0));
        let noise: Vec<f32> = (0..96_000u32)
            .map(|i| ((i.wrapping_mul(2_654_435_761) >> 8) as f32 / (1 << 24) as f32 - 0.5) * 3.0)
            .collect();
        assert_eq!(limit(&noise, -1.0), limit(&noise, -1.0));
        let peak = measure(&limit(&noise, -1.0)).true_peak_db();
        assert!(peak <= -0.95, "{peak}");
    }
}
