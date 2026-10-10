//! Clean voice for speech recorded on a phone. A high-pass takes away rumble from handling, traffic
//! and air conditioning, and mains hum at 50 or 60 Hz; RNNoise (the `nnnoiseless` port) lowers
//! steady background noise and is mixed with the original, so speech keeps its character; a
//! de-esser softens harsh s sounds.
//!
//! The result is a second PCM cache per file, made once in the background and played instead of the
//! raw cache by clips with Clean voice on. It has the raw cache's length and every sound on the same
//! sample, so cuts, crossfades, waveforms and word times do not move.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use nnnoiseless::DenoiseState;

use crate::audio::{PCM_VERSION, Pcm, ensure_pcm, pcm_path};
use crate::model::{Asset, CHANNELS, SAMPLE_RATE};

/// Raised whenever the processing changes its output, so voices cleaned before are cleaned again.
pub const VERSION: &str = "voice1";

/// RNNoise works on 10 ms frames and hands each one back a frame later.
const FRAME: usize = DenoiseState::FRAME_SIZE;
const LATENCY: usize = FRAME;
/// RNNoise takes 16-bit sample values in f32.
const I16: f32 = 32_768.0;
/// Share of the denoised sound; the rest is the original. Steady noise ends about 16 dB lower
/// instead of gone, and speech does not take on RNNoise's watery sound.
const WET: f32 = 0.85;
const HIGH_PASS_HZ: f64 = 80.0;
/// Quality factors of a 6th-order Butterworth as three biquads: 36 dB per octave, so 50 Hz hum
/// drops by 24 dB while a low male voice at 100 Hz loses 0.3 dB.
const BUTTERWORTH_Q: [f64; 3] = [0.517_638_090_205_041_5, std::f64::consts::FRAC_1_SQRT_2, 1.931_851_652_578_136_6];
/// Sibilants carry most of their energy above this; voiced sound very little.
const SIBILANT_HZ: f64 = 4_500.0;
/// Above this share of the power in the sibilant band, the sound is turned down.
const SIBILANT_SHARE: f64 = 0.3;
/// dB of reduction per dB the share is over, and the most the sound is turned down.
const DE_ESS_SLOPE: f64 = 2.0;
const DE_ESS_MAX_DB: f64 = 6.0;
/// Quieter than -50 dBFS is never de-essed: that is room tone, not a voice.
const DE_ESS_GATE: f64 = 1e-5;
/// Progress is reported (and a stop request seen) once per second of sound.
const PROGRESS_BLOCKS: usize = SAMPLE_RATE as usize / FRAME;
const _: () = assert!(CHANNELS == 2, "the caches are stereo");
/// Part of the progress bar the raw cache takes when it is extracted first.
const RAW_SHARE: f32 = 0.3;

/// Where the cleaned sound of `asset` is cached: beside the raw cache's revision, with both versions in
/// the name, so a change of the source, the extraction or the processing makes a new one.
pub fn voice_pcm_path(cache_dir: &Path, asset: &Asset) -> PathBuf {
    path_for(cache_dir, &pcm_path(cache_dir, asset))
}

/// The cleaned cache made from the raw cache at `raw`.
pub fn path_for(cache_dir: &Path, raw: &Path) -> PathBuf {
    let stem = raw.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    cache_dir.join("voice").join(format!("{stem}.{VERSION}.f32"))
}

/// Cleans the voice of `asset` unless that is done already, extracting its raw cache first when needed.
/// Concurrent callers for the same file (the app's preparation and an export) wait for one of them. An
/// error from `progress` stops the work; the next call starts it again.
pub fn ensure_voice_pcm(
    cache_dir: &Path,
    asset: &Asset,
    mut progress: impl FnMut(f32) -> Result<()>,
) -> Result<PathBuf> {
    let raw_share = if pcm_path(cache_dir, asset).exists() { 0.0 } else { RAW_SHARE };
    let raw = ensure_pcm(cache_dir, asset, |p| progress(p * raw_share))?;
    let path = path_for(cache_dir, &raw);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock = File::options().read(true).write(true).create(true).truncate(false).open(lock_path)?;
    // Waiting for another caller still asks `progress`, so a cancelled export stops at once.
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                progress(raw_share)?;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    if !path.exists() {
        clean_file(&raw, &path, |p| progress(raw_share + p * (1.0 - raw_share)))?;
        remove_older_versions(&path, asset);
    }
    Ok(path)
}

fn clean_file(raw: &Path, out: &Path, progress: impl FnMut(f32) -> Result<()>) -> Result<()> {
    let pcm = Pcm::open(raw).with_context(|| format!("Reading {}", raw.display()))?;
    let tmp = out.with_file_name(format!(".nuzky-voice-{}.part", crate::edit::new_id()));
    let file = File::options().write(true).create_new(true).open(&tmp)?;
    let result = (|| {
        let mut writer = BufWriter::with_capacity(1 << 20, file);
        clean(pcm.samples(), |frames| Ok(writer.write_all(bytemuck::cast_slice(frames))?), progress)?;
        writer.flush()?;
        drop(writer);
        std::fs::rename(&tmp, out)?;
        Ok(())
    })();
    if result.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    result
}

/// Removes this asset's cleaned caches made by older versions, which nothing reads any more.
fn remove_older_versions(path: &Path, asset: &Asset) {
    let (Some(dir), prefix, current) =
        (path.parent(), format!("{}.", asset.id), format!(".{PCM_VERSION}.{VERSION}.f32"))
    else {
        return;
    };
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let cache = name.strip_suffix(".lock").unwrap_or(&name);
        if name.starts_with(&prefix) && cache.ends_with(".f32") && !cache.ends_with(&current) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
}

/// Cleans interleaved stereo `input`, handing the result to `out` in order. The output has exactly
/// the input's length and every sound on the same sample. `progress` gets the share done once per
/// second of sound; an error from it stops the work.
pub fn clean(
    input: &[f32],
    mut out: impl FnMut(&[f32]) -> Result<()>,
    mut progress: impl FnMut(f32) -> Result<()>,
) -> Result<()> {
    let frames = input.len() / CHANNELS;
    // Phone video is often mono stored as two equal channels; it is cleaned once.
    let mono = input.as_chunks::<CHANNELS>().0.iter().all(|f| f.iter().all(|&s| s == f[0]));
    let mut channels: Vec<Channel> = (0..if mono { 1 } else { CHANNELS }).map(|_| Channel::new()).collect();
    let mut block = vec![[0f32; FRAME]; channels.len()];
    let mut cleaned = vec![[0f32; FRAME]; channels.len()];
    let mut interleaved = Vec::with_capacity(FRAME * CHANNELS);
    let blocks = (frames + LATENCY).div_ceil(FRAME);
    for index in 0..blocks {
        let start = index * FRAME;
        for (c, samples) in block.iter_mut().enumerate() {
            for (i, sample) in samples.iter_mut().enumerate() {
                *sample = input.get((start + i) * CHANNELS + c).copied().filter(|_| start + i < frames).unwrap_or(0.0);
            }
        }
        for ((channel, samples), result) in channels.iter_mut().zip(&block).zip(&mut cleaned) {
            channel.frame(samples, result);
        }
        // This block's output is the sound of the input `LATENCY` frames earlier.
        let from = LATENCY.saturating_sub(start);
        let to = (frames + LATENCY).saturating_sub(start).min(FRAME);
        if from < to {
            // A mono source has one cleaned channel, written to both sides.
            let (left, right) = (&cleaned[0][from..to], &cleaned[cleaned.len() - 1][from..to]);
            interleaved.clear();
            interleaved.extend(left.iter().zip(right).flat_map(|(&l, &r)| [l, r]));
            out(&interleaved)?;
        }
        if index % PROGRESS_BLOCKS == PROGRESS_BLOCKS - 1 {
            progress(index as f32 / blocks as f32)?;
        }
    }
    Ok(())
}

/// One channel through the whole chain.
struct Channel {
    high_pass: [Biquad; 3],
    denoise: Box<DenoiseState<'static>>,
    /// The high-passed frame before this one: the original in step with what RNNoise hands back.
    dry: [f32; FRAME],
    de_ess: DeEsser,
}

impl Channel {
    fn new() -> Self {
        Self {
            high_pass: BUTTERWORTH_Q.map(|q| Biquad::high_pass(HIGH_PASS_HZ, q)),
            denoise: DenoiseState::new(),
            dry: [0.0; FRAME],
            de_ess: DeEsser::new(),
        }
    }

    /// Takes one frame and gives back the frame before it, cleaned.
    fn frame(&mut self, input: &[f32; FRAME], out: &mut [f32; FRAME]) {
        let mut filtered = [0f32; FRAME];
        for (f, &x) in filtered.iter_mut().zip(input) {
            let x = if x.is_finite() { x as f64 } else { 0.0 };
            *f = self.high_pass.iter_mut().fold(x, |s, stage| stage.run(s)) as f32;
        }
        let scaled = filtered.map(|s| s * I16);
        let mut denoised = [0f32; FRAME];
        self.denoise.process_frame(&mut denoised, &scaled);
        for ((out, &wet), &dry) in out.iter_mut().zip(&denoised).zip(&self.dry) {
            *out = self.de_ess.run((WET * wet / I16 + (1.0 - WET) * dry) as f64) as f32;
        }
        self.dry = filtered;
    }
}

/// Transposed direct form II in f64, which an 80 Hz corner at 48 kHz needs for precision.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    /// RBJ cookbook high-pass.
    fn high_pass(hz: f64, q: f64) -> Self {
        let w = std::f64::consts::TAU * hz / SAMPLE_RATE as f64;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b = [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0].map(|v| v / a0);
        Self { b, a: [-2.0 * cos / a0, (1.0 - alpha) / a0], z: [0.0; 2] }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Turns the sound down while the band above 4.5 kHz holds most of it, as it does in an s or sh,
/// so sibilants soften by up to 6 dB. A gain and no filter on the sound itself: between sibilants
/// it passes bit for bit, with no colour and no shift in time.
struct DeEsser {
    /// Two biquads, a 4th-order Butterworth, so vowels with bright formants stay out of the band.
    band: [Biquad; 2],
    band_power: f64,
    power: f64,
    gain: f64,
}

/// One-pole smoothing coefficients: power over 5 ms, the gain falling in 1 ms and recovering in 40 ms.
fn smoothing(seconds: f64) -> f64 {
    1.0 - (-1.0 / (seconds * SAMPLE_RATE as f64)).exp()
}

impl DeEsser {
    fn new() -> Self {
        Self {
            band: [0.541_196_100_146_197, 1.306_562_964_876_376_6].map(|q| Biquad::high_pass(SIBILANT_HZ, q)),
            band_power: 0.0,
            power: 0.0,
            gain: 1.0,
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let band = self.band.iter_mut().fold(x, |s, stage| stage.run(s));
        let detect = smoothing(0.005);
        self.band_power += (band * band - self.band_power) * detect;
        self.power += (x * x - self.power) * detect;
        let target = if self.power > DE_ESS_GATE && self.band_power > SIBILANT_SHARE * self.power {
            let over_db = 10.0 * (self.band_power / (SIBILANT_SHARE * self.power)).log10();
            10f64.powf(-(over_db * DE_ESS_SLOPE).min(DE_ESS_MAX_DB) / 20.0)
        } else {
            1.0
        };
        let rate = if target < self.gain { smoothing(0.001) } else { smoothing(0.040) };
        self.gain += (target - self.gain) * rate;
        if self.gain > 1.0 - 1e-9 {
            self.gain = 1.0;
        }
        x * self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    const RATE: f64 = SAMPLE_RATE as f64;

    /// Deterministic white noise in -1..1.
    struct Noise(u64);
    impl Noise {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        }
    }

    /// A sung vowel: a glottal pulse train around 130 Hz with a slow vibrato through three formant
    /// resonators of an "a" (700, 1220, 2600 Hz), which RNNoise hears as a voice.
    fn vowel(n: usize) -> Vec<f64> {
        let formants = [(700.0, 80.0, 1.0), (1220.0, 90.0, 0.5), (2600.0, 120.0, 0.25)];
        let mut resonators: Vec<(f64, f64, f64, [f64; 2])> = formants
            .iter()
            .map(|&(hz, bandwidth, gain): &(f64, f64, f64)| {
                let r = (-std::f64::consts::PI * bandwidth / RATE).exp();
                (2.0 * r * (TAU * hz / RATE).cos(), -r * r, gain, [0.0; 2])
            })
            .collect();
        let mut phase = 0.0;
        (0..n)
            .map(|i| {
                let f0 = 130.0 + 6.0 * (TAU * 5.0 * i as f64 / RATE).sin();
                phase += f0 / RATE;
                let pulse = if phase >= 1.0 {
                    phase -= 1.0;
                    1.0
                } else {
                    0.0
                };
                resonators
                    .iter_mut()
                    .map(|(a1, a2, gain, y)| {
                        let out = pulse + *a1 * y[0] + *a2 * y[1];
                        *y = [out, y[0]];
                        out * *gain
                    })
                    .sum::<f64>()
            })
            .collect()
    }

    fn cleaned(input: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        let collect = |frames: &[f32]| {
            out.extend_from_slice(frames);
            Ok(())
        };
        clean(input, collect, |_| Ok(())).unwrap();
        out
    }

    /// Mono test signal as interleaved stereo, cleaned.
    fn run(signal: &[f64]) -> Vec<f64> {
        let input: Vec<f32> = signal.iter().flat_map(|&s| [s as f32; CHANNELS]).collect();
        let out = cleaned(&input);
        assert_eq!(out.len(), input.len());
        out.as_chunks::<CHANNELS>().0.iter().map(|f| f[0] as f64).collect()
    }

    fn rms_db(x: &[f64]) -> f64 {
        10.0 * (x.iter().map(|s| s * s).sum::<f64>() / x.len().max(1) as f64).log10()
    }

    /// Level of one frequency in dB.
    fn tone_db(x: &[f64], hz: f64) -> f64 {
        let (re, im) = x.iter().enumerate().fold((0.0, 0.0), |(re, im), (n, &s)| {
            let a = TAU * hz * n as f64 / RATE;
            (re + s * a.cos(), im + s * a.sin())
        });
        20.0 * ((re * re + im * im).sqrt() * 2.0 / x.len() as f64).log10()
    }

    fn sine(hz: f64, amplitude: f64, n: usize) -> impl Iterator<Item = f64> {
        (0..n).map(move |i| amplitude * (TAU * hz * i as f64 / RATE).sin())
    }

    #[test]
    fn high_pass_takes_hum_and_keeps_the_voice_band() {
        let n = SAMPLE_RATE as usize * 2;
        let input: Vec<f64> = sine(50.0, 0.3, n)
            .zip(sine(1_000.0, 0.1, n))
            .zip(sine(300.0, 0.1, n))
            .zip(sine(120.0, 0.1, n))
            .map(|(((a, b), c), d)| a + b + c + d)
            .collect();
        let mut stages = BUTTERWORTH_Q.map(|q| Biquad::high_pass(HIGH_PASS_HZ, q));
        let output: Vec<f64> = input.iter().map(|&x| stages.iter_mut().fold(x, |s, b| b.run(s))).collect();
        // Past the filter's settling.
        let (input, output) = (&input[n / 2..], &output[n / 2..]);
        let change = |hz| tone_db(output, hz) - tone_db(input, hz);
        eprintln!(
            "high-pass: 50 Hz {:+.1} dB, 120 Hz {:+.2} dB, 300 Hz {:+.3} dB, 1 kHz {:+.4} dB",
            change(50.0),
            change(120.0),
            change(300.0),
            change(1_000.0)
        );
        assert!(change(50.0) <= -20.0, "50 Hz hum only {:.1} dB lower", change(50.0));
        assert!(change(1_000.0).abs() < 1.0 && change(300.0).abs() < 1.0, "the voice band moved");
        assert!(change(120.0).abs() < 1.0, "a low voice lost {:.2} dB", change(120.0));
    }

    #[test]
    fn de_esser_softens_a_sibilant_and_leaves_voiced_sound_alone() {
        let (voiced, burst) = (SAMPLE_RATE as usize / 2, SAMPLE_RATE as usize / 5);
        let mut noise = Noise(7);
        // An "s": noise between 5 and 9 kHz, summed from sines with random phases.
        let hiss: Vec<f64> = {
            let partials: Vec<(f64, f64)> = (0..80).map(|k| (5_000.0 + 50.0 * k as f64, TAU * noise.next())).collect();
            (0..burst)
                .map(|i| {
                    partials.iter().map(|(hz, phase)| (TAU * hz * i as f64 / RATE + phase).sin()).sum::<f64>() * 0.02
                })
                .collect()
        };
        let mut input: Vec<f64> = sine(300.0, 0.2, voiced).collect();
        input.extend(&hiss);
        input.extend(sine(300.0, 0.2, voiced));
        let mut de_ess = DeEsser::new();
        let output: Vec<f64> = input.iter().map(|&x| de_ess.run(x)).collect();
        let part = |x: &[f64], from: usize, len: usize| rms_db(&x[from..from + len]);
        let sibilant = part(&output, voiced, burst) - part(&input, voiced, burst);
        let before = part(&output, voiced / 2, voiced / 2) - part(&input, voiced / 2, voiced / 2);
        let after = part(&output, voiced + burst + voiced / 2, voiced / 2)
            - part(&input, voiced + burst + voiced / 2, voiced / 2);
        eprintln!("de-esser: sibilant {sibilant:+.1} dB, voiced before {before:+.3} dB, after {after:+.3} dB");
        assert!(sibilant < -3.0, "the sibilant is only {sibilant:.1} dB softer");
        assert!(before.abs() < 0.1 && after.abs() < 0.1, "voiced sound changed: {before:.3} and {after:.3} dB");
    }

    /// Speech bursts in steady noise with mains hum under them: the noise between the bursts and the
    /// hum drop, the bursts keep their level, and nothing moves in time.
    #[test]
    fn chain_lowers_noise_and_hum_between_words_and_keeps_the_voice() {
        let second = SAMPLE_RATE as usize;
        let (word, pause) = (second * 3 / 10, second * 4 / 10);
        let voice = vowel(word);
        let peak = voice.iter().fold(0.0f64, |m, s| m.max(s.abs()));
        let mut noise = Noise(42);
        let mut speech = Vec::new();
        let mut words = Vec::new();
        for _ in 0..6 {
            speech.extend(std::iter::repeat_n(0.0, pause));
            words.push(speech.len());
            speech.extend(voice.iter().map(|s| s / peak * 0.3));
        }
        speech.extend(std::iter::repeat_n(0.0, pause));
        let n = speech.len();
        // White noise at -45 dBFS in one take, hum at -26 dBFS in another.
        let input: Vec<f64> = speech.iter().map(|s| s + noise.next() * 0.0097).collect();
        let output = run(&input);
        let humming: Vec<f64> = speech.iter().zip(sine(50.0, 0.07, n)).map(|(s, h)| s + h).collect();
        let hum_drop = tone_db(&run(&humming)[second..], 50.0) - tone_db(&humming[second..], 50.0);
        // Pauses and words after the first second, when RNNoise has settled; their middles only.
        let pauses = words[2..].iter().map(|&w| (w - pause * 3 / 4, pause / 2));
        let level = |x: &[f64], parts: &mut dyn Iterator<Item = (usize, usize)>| {
            rms_db(&parts.flat_map(|(from, len)| x[from..from + len].iter().copied()).collect::<Vec<_>>())
        };
        let noise_drop = level(&output, &mut pauses.clone()) - level(&input, &mut pauses.clone());
        let spoken = || words[2..].iter().map(|&w| (w + word / 6, word * 2 / 3));
        let voice_change = level(&output, &mut spoken()) - level(&input, &mut spoken());
        eprintln!(
            "chain: noise between words {noise_drop:+.1} dB, voice {voice_change:+.2} dB, 50 Hz hum {hum_drop:+.1} dB"
        );
        assert!(noise_drop < -10.0, "noise between words only {noise_drop:.1} dB lower");
        assert!(voice_change.abs() < 3.0, "the voice changed by {voice_change:.2} dB");
        assert!(hum_drop < -20.0, "hum only {hum_drop:.1} dB lower");
        // In step: the voice lines up at lag 0, not a frame early or late. The reference is the
        // input through the same high-pass, whose phase moves a 700 Hz formant by 5 samples.
        let mut stages = BUTTERWORTH_Q.map(|q| Biquad::high_pass(HIGH_PASS_HZ, q));
        let reference: Vec<f64> = input.iter().map(|&x| stages.iter_mut().fold(x, |s, b| b.run(s))).collect();
        let a = &reference[words[3]..words[3] + word];
        let lag = (-600i64..=600)
            .max_by(|&x, &y| {
                let corr = |lag: i64| {
                    a.iter()
                        .enumerate()
                        .map(|(i, s)| s * output[(words[3] as i64 + i as i64 + lag) as usize])
                        .sum::<f64>()
                };
                corr(x).total_cmp(&corr(y))
            })
            .unwrap();
        assert_eq!(lag, 0, "the cleaned voice is {lag} samples off");
    }

    #[test]
    fn output_keeps_length_click_position_and_is_deterministic() {
        for frames in [0, 1, 479, 480, 481, 12_345] {
            let input = vec![0.25f32; frames * CHANNELS];
            assert_eq!(cleaned(&input).len(), input.len(), "{frames} frames");
        }
        let mut click = vec![0.0; SAMPLE_RATE as usize];
        click[20_000] = 0.5;
        let output = run(&click);
        let loudest = (0..output.len()).max_by(|&a, &b| output[a].abs().total_cmp(&output[b].abs())).unwrap();
        assert_eq!(loudest, 20_000, "the click moved");
        // Real stereo: two channels cleaned apart, twice with the same result.
        let mut noise = Noise(3);
        let stereo: Vec<f32> = (0..SAMPLE_RATE as usize * 2).map(|_| (noise.next() * 0.1) as f32).collect();
        let first = cleaned(&stereo);
        assert!(
            first.iter().zip(cleaned(&stereo)).all(|(a, b)| a.to_bits() == b.to_bits()),
            "same input, other output"
        );
        assert!(first.iter().all(|s| s.is_finite()));
    }

    /// An export waiting for the app's own cleaning of the same file still stops when cancelled.
    #[test]
    fn waiting_for_another_cleaning_stays_cancellable() {
        use crate::model::AssetKind;
        let cache = std::env::temp_dir().join(format!("nuzky-voice-wait-{}", crate::edit::new_id()));
        std::fs::create_dir_all(cache.join("pcm")).unwrap();
        let asset = Asset {
            id: "take".into(),
            name: "take".into(),
            path: String::new(),
            kind: AssetKind::Video,
            duration_us: 1_000_000,
            width: 2,
            height: 2,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
            credit: None,
        };
        let raw = pcm_path(&cache, &asset);
        std::fs::write(&raw, bytemuck::cast_slice(&vec![0.1f32; SAMPLE_RATE as usize * CHANNELS])).unwrap();
        let path = path_for(&cache, &raw);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let other = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(format!("{}.lock", path.display()))
            .unwrap();
        other.lock().unwrap();
        let started = std::time::Instant::now();
        let error = ensure_voice_pcm(&cache, &asset, |_| anyhow::bail!("CANCELLED: stop")).unwrap_err();
        let waited = started.elapsed();
        drop(other);
        std::fs::remove_dir_all(cache).unwrap();
        assert!(error.to_string().starts_with("CANCELLED"), "{error}");
        assert!(waited < std::time::Duration::from_secs(1), "waited {waited:?}");
    }

    #[test]
    fn a_stop_request_ends_cleaning_within_a_second_of_sound() {
        let input = vec![0.1f32; SAMPLE_RATE as usize * 10 * CHANNELS];
        let mut written = 0;
        let count = |frames: &[f32]| {
            written += frames.len() / CHANNELS;
            Ok(())
        };
        let error = clean(&input, count, |_| anyhow::bail!("CANCELLED: stop")).unwrap_err();
        assert!(error.to_string().starts_with("CANCELLED"));
        assert!(written <= SAMPLE_RATE as usize, "{written} frames cleaned after the stop");
    }
}
