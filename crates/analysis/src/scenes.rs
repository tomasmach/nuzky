use std::path::Path;

use anyhow::{Context, Result, ensure};
use capopen_engine::{
    media::VideoDecoder,
    model::{Asset, AssetKind},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneParams {
    /// Minimum normalized RGB absolute difference, 0..=1. Default 0.18.
    pub threshold: f32,
    pub min_gap_us: i64,
}

impl Default for SceneParams {
    fn default() -> Self {
        Self { threshold: 0.18, min_gap_us: 300_000 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneCut {
    pub time_us: i64,
    pub score: f32,
}

/// Hard-cut proposals with real decoder PTS. Uses 64x36 RGB differences (colour
/// distinguishes equal-luma cuts), requiring an abrupt rise above recent motion.
/// Rotation is invariant: both compared frames retain the same source orientation.
/// Fades are intentionally excluded; flashes and abrupt camera motion can trigger.
pub fn scene_cuts(asset: &Asset, params: SceneParams) -> Result<Vec<SceneCut>> {
    ensure!(asset.kind == AssetKind::Video, "Scene analysis requires video");
    ensure!(
        params.threshold.is_finite() && params.threshold > 0.0 && params.threshold <= 1.0,
        "Scene threshold must be in (0, 1]"
    );
    ensure!(params.min_gap_us >= 0, "Scene gap must be nonnegative");
    let mut decoder = VideoDecoder::open(Path::new(&asset.path)).context("Opening scene video")?;
    let mut previous = None;
    let mut cuts: Vec<SceneCut> = Vec::new();
    let mut motion = 0.0f32;
    let mut last_time = None;
    while let Some((time_us, frame)) = decoder.next_frame().context("Decoding scene frame")? {
        let rgba = decoder.convert(&frame, time_us, 64, 36).context("Downscaling scene frame")?;
        if let Some(ref old) = previous {
            let score = difference(old, &rgba.data);
            let increasing = last_time.is_none_or(|last| time_us > last);
            let separated = cuts.last().is_none_or(|cut| time_us.saturating_sub(cut.time_us) >= params.min_gap_us);
            let abrupt = score >= params.threshold.max(motion * 3.0 + 0.02);
            if increasing && time_us >= 0 && separated && abrupt {
                cuts.push(SceneCut { time_us, score });
            }
            // A cut should not raise the motion baseline and hide the following cut.
            if !abrupt {
                motion = 0.8 * motion + 0.2 * score;
            }
        }
        last_time = Some(time_us);
        previous = Some(rgba.data);
    }
    Ok(cuts)
}

fn difference(a: &std::sync::Arc<Vec<u8>>, b: &[u8]) -> f32 {
    let sum: u64 = a
        .chunks_exact(4)
        .zip(b.chunks_exact(4))
        .map(|(a, b)| (0..3).map(|ch| a[ch].abs_diff(b[ch]) as u64).sum::<u64>())
        .sum();
    sum as f32 / (64.0 * 36.0 * 3.0 * 255.0)
}
