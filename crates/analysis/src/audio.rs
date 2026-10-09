use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use nuzky_engine::{
    audio::{Pcm, ensure_pcm, has_audio, samples_to_us, us_to_samples},
    loudness::Meter,
    model::{Asset, CHANNELS},
};
use serde::{Deserialize, Serialize};

use crate::Range;

const FLOOR_DB: f32 = -120.0;
const WINDOW_US: i64 = 10_000;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SilenceParams {
    /// None: 10th percentile of 10 ms RMS windows + 6 dB, capped at -35 dBFS.
    pub threshold_db: Option<f32>,
    pub min_silence_us: i64,
    /// Silence returned for removal is shortened at both ends by this amount.
    pub pad_us: i64,
}

impl Default for SilenceParams {
    fn default() -> Self {
        Self { threshold_db: None, min_silence_us: 400_000, pad_us: 120_000 }
    }
}

pub(crate) fn open_pcm(asset: &Asset, cache: &Path, cancelled: &dyn Fn() -> bool) -> Result<Pcm> {
    ensure!(has_audio(asset), "Asset {} has no audio", asset.name);
    let path = ensure_pcm(cache, asset, |_| {
        if cancelled() {
            bail!("CANCELLED: analysis cancelled");
        }
        Ok(())
    })
    .context("Preparing analysis PCM")?;
    let size = std::fs::metadata(&path).context("Reading PCM metadata")?.len();
    ensure!(size > 0 && size % (CHANNELS * 4) as u64 == 0, "PCM must contain complete stereo f32 frames");
    Pcm::open(&path).with_context(|| format!("Mapping PCM {}", path.display()))
}

/// Non-overlapping RMS windows in dBFS, NOT LUFS. Includes a final partial window.
/// Digital silence is -120 dBFS, keeping JSON finite. Channels contribute equally.
pub fn loudness(asset: &Asset, cache: &Path, window_us: i64) -> Result<Vec<f32>> {
    loudness_cancellable(asset, cache, window_us, || false)
}

pub fn loudness_cancellable(
    asset: &Asset,
    cache: &Path,
    window_us: i64,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<f32>> {
    let pcm = open_pcm(asset, cache, &cancelled)?;
    levels(pcm.samples(), window_us, &cancelled)
}

fn power(samples: &[f32]) -> Result<f64> {
    let mut sum = 0.0;
    for &sample in samples {
        ensure!(sample.is_finite(), "PCM contains a non-finite sample");
        sum += (sample as f64).powi(2);
    }
    Ok(sum / samples.len().max(1) as f64)
}

fn db(power: f64) -> f32 {
    (10.0 * power.max(1e-12).log10()) as f32
}

/// `cancelled` is asked every few seconds of audio, so a long file stops soon after a request.
fn levels(samples: &[f32], window_us: i64, cancelled: &dyn Fn() -> bool) -> Result<Vec<f32>> {
    ensure!(window_us > 0, "Loudness window must be positive");
    let frames = usize::try_from(us_to_samples(window_us).max(1)).context("Window too large")?;
    let size = frames.checked_mul(CHANNELS).context("Window too large")?;
    let every = (CANCEL_CHECK_FRAMES / frames).max(1);
    samples
        .chunks(size)
        .enumerate()
        .map(|(i, s)| {
            if i % every == 0 && cancelled() {
                bail!("CANCELLED: analysis cancelled");
            }
            power(s).map(db)
        })
        .collect()
}

/// About ten seconds of audio between cancel checks.
const CANCEL_CHECK_FRAMES: usize = 480_000;

/// Integrated loudness and true peak of a whole file after ITU-R BS.1770-4 (EBU R128), read by
/// the meter the Reels export levels with.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgramLoudness {
    /// LUFS; `None` when nothing is louder than -70 LUFS, as in silence.
    pub integrated_lufs: Option<f32>,
    /// dBTP of the 4x oversampled signal; digital silence is -120.
    pub true_peak_dbtp: f32,
}

pub fn program_loudness(asset: &Asset, cache: &Path) -> Result<ProgramLoudness> {
    program_loudness_cancellable(asset, cache, || false)
}

pub fn program_loudness_cancellable(
    asset: &Asset,
    cache: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<ProgramLoudness> {
    let pcm = open_pcm(asset, cache, &cancelled)?;
    measure_program(pcm.samples(), &cancelled)
}

fn measure_program(samples: &[f32], cancelled: &dyn Fn() -> bool) -> Result<ProgramLoudness> {
    let mut meter = Meter::new();
    for chunk in samples.chunks(CANCEL_CHECK_FRAMES * CHANNELS) {
        if cancelled() {
            bail!("CANCELLED: analysis cancelled");
        }
        ensure!(chunk.iter().all(|s| s.is_finite()), "PCM contains a non-finite sample");
        meter.push(chunk);
    }
    let loudness = meter.finish();
    Ok(ProgramLoudness {
        integrated_lufs: loudness.integrated.map(|lufs| lufs as f32),
        true_peak_dbtp: loudness.true_peak_db().max(FLOOR_DB as f64) as f32,
    })
}

/// Quiet ranges safe to propose for removal, after breath padding. This is an
/// energy detector, not a voice detector: a loud/varying music bed can hide pauses.
pub fn silences(asset: &Asset, cache: &Path, params: SilenceParams) -> Result<Vec<Range>> {
    silences_cancellable(asset, cache, params, || false)
}

pub fn silences_cancellable(
    asset: &Asset,
    cache: &Path,
    params: SilenceParams,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<Range>> {
    let pcm = open_pcm(asset, cache, &cancelled)?;
    quiet_ranges(pcm.samples(), params, &cancelled)
}

fn quiet_ranges(samples: &[f32], params: SilenceParams, cancelled: &dyn Fn() -> bool) -> Result<Vec<Range>> {
    ensure!(params.min_silence_us > 0 && params.pad_us >= 0, "Invalid silence duration or padding");
    if let Some(threshold) = params.threshold_db {
        ensure!(threshold.is_finite(), "Silence threshold must be finite");
    }
    let rms = levels(samples, WINDOW_US, cancelled)?;
    if rms.is_empty() {
        return Ok(Vec::new());
    }
    let threshold = params.threshold_db.unwrap_or_else(|| {
        let mut sorted = rms.clone();
        sorted.sort_unstable_by(f32::total_cmp);
        (sorted[(sorted.len() - 1) / 10] + 6.0).min(-35.0)
    });
    let duration = samples_to_us((samples.len() / CHANNELS) as i64);
    let mut ranges = Vec::new();
    let mut start = None;
    for i in 0..=rms.len() {
        let quiet = rms.get(i).is_some_and(|db| *db <= threshold);
        if quiet {
            start.get_or_insert(i as i64 * WINDOW_US);
        } else if let Some(from) = start.take() {
            let end = (i as i64 * WINDOW_US).min(duration);
            let padded_start = from.saturating_add(params.pad_us);
            let padded_end = end.saturating_sub(params.pad_us);
            if end - from >= params.min_silence_us && padded_start < padded_end {
                ranges.push(Range { start_us: padded_start, end_us: padded_end });
            }
        }
    }
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_rms_partial_window_and_invalid_input() -> Result<()> {
        let tone: Vec<f32> = (0..4800)
            .flat_map(|i| {
                let x = (i as f32 * std::f32::consts::TAU / 48.0).sin() * 0.5;
                [x, -x]
            })
            .collect();
        let values = levels(&tone, 60_000, &|| false)?;
        assert_eq!(values.len(), 2);
        assert!(values.iter().all(|x| (*x + 9.0309).abs() < 0.001));
        assert_eq!(levels(&[0.0; 96], 1_000, &|| false)?, vec![-120.0]);
        assert!(levels(&tone, 0, &|| false).is_err());
        assert!(levels(&[f32::NAN], 10_000, &|| false).is_err());
        // A stop request ends a long analysis instead of waiting for the end of the file.
        let asked = std::cell::Cell::new(0);
        let stop = || {
            asked.set(asked.get() + 1);
            asked.get() > 1
        };
        let error = levels(&vec![0.1; 60 * 48_000 * 2], 10_000, &stop).unwrap_err();
        assert!(error.to_string().starts_with("CANCELLED"), "{error}");
        assert_eq!(asked.get(), 2, "asked again after about ten seconds of audio");
        Ok(())
    }

    #[test]
    fn program_loudness_is_bs1770_and_stays_finite() -> Result<()> {
        // A 1 kHz sine at -23 dBFS on both channels is -23 LUFS; its peak is -23 dBTP.
        let amplitude = 10f32.powf(-23.0 / 20.0);
        let tone: Vec<f32> = (0..20 * 48_000)
            .flat_map(|i| [(i as f32 * std::f32::consts::TAU * 1000.0 / 48_000.0).sin() * amplitude; CHANNELS])
            .collect();
        let program = measure_program(&tone, &|| false)?;
        assert!((program.integrated_lufs.unwrap() + 23.0).abs() < 0.1, "{program:?}");
        assert!((program.true_peak_dbtp + 23.0).abs() < 0.1, "{program:?}");
        let silence = measure_program(&[0.0; 96_000], &|| false)?;
        assert_eq!(silence, ProgramLoudness { integrated_lufs: None, true_peak_dbtp: -120.0 });
        assert!(measure_program(&[f32::NAN; 8], &|| false).is_err());
        let error = measure_program(&tone, &|| true).unwrap_err();
        assert!(error.to_string().starts_with("CANCELLED"), "{error}");
        Ok(())
    }

    #[test]
    fn noisy_pause_padding() -> Result<()> {
        let mut samples = vec![0.2; 3 * 48_000 * CHANNELS];
        let mut seed = 17u32;
        for sample in &mut samples[48_000 * CHANNELS..96_000 * CHANNELS] {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *sample = (seed as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32 * 0.001;
        }
        let gaps = quiet_ranges(&samples, SilenceParams::default(), &|| false)?;
        assert_eq!(gaps, vec![Range { start_us: 1_120_000, end_us: 1_880_000 }]);
        assert!(quiet_ranges(&[0.2; 48_000], SilenceParams::default(), &|| false)?.is_empty());
        assert!(quiet_ranges(&[0.0; 960], SilenceParams::default(), &|| false)?.is_empty());
        assert_eq!(
            quiet_ranges(&vec![0.0; 96_000], SilenceParams::default(), &|| false)?,
            vec![Range { start_us: 120_000, end_us: 880_000 }]
        );
        Ok(())
    }
}
