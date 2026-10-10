//! The timeline as a still is taken from it: rendered as exported, with colours, zooms and
//! overlays, but without text, and away from cuts, transitions and clip animations.
use anyhow::{Context, Result, ensure};
use nuzky_engine::{
    Project, Renderer, Wait,
    effects::transition_window,
    model::{AssetKind, ClipContent, TrackKind},
};

use crate::frame::Frame;

/// How far around a cut the picture is not yet settled: motion blur, a half-done jump of framing
/// or the first frames of a new shot.
pub const EDGE_US: i64 = 400_000;

/// The project without its text tracks: captions and titles are added on the thumbnail later.
pub use nuzky_engine::thumbnail::picture;

/// Time ranges, sorted, where a visible clip starts or ends, a transition runs or a clip animates.
pub fn unsettled(project: &Project) -> Vec<(i64, i64)> {
    let mut ranges = Vec::new();
    for track in project.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.hidden) {
        for clip in &track.clips {
            let visible = match &clip.content {
                ClipContent::Media { asset_id, .. } => {
                    project.asset(asset_id).is_some_and(|asset| asset.kind != AssetKind::Audio)
                }
                ClipContent::Text { .. } => true,
            };
            if !visible {
                continue;
            }
            let (start, end) = (clip.start_us, clip.start_us + clip.duration_us);
            ranges.push((start - EDGE_US, start + EDGE_US));
            ranges.push((end - EDGE_US, end + EDGE_US));
            if let Some(animation) = clip.anim_in.filter(|a| a.duration_us > 0) {
                ranges.push((start, start + animation.duration_us + EDGE_US));
            }
            if let Some(animation) = clip.anim_out.filter(|a| a.duration_us > 0) {
                ranges.push((end - animation.duration_us - EDGE_US, end));
            }
            if let Some((from, to)) = transition_window(clip) {
                ranges.push((from - EDGE_US, to + EDGE_US));
            }
        }
    }
    ranges.sort_unstable();
    ranges
}

pub fn settled(unsettled: &[(i64, i64)], t: i64) -> bool {
    !unsettled.iter().any(|&(from, to)| from <= t && t < to)
}

/// Times every `every_us` across the timeline, further apart when there would be more than `max`.
pub fn times(project: &Project, every_us: i64, max: usize) -> Vec<i64> {
    let duration = project.duration_us();
    let every = every_us.max(duration / max.max(1) as i64).max(1);
    (0..).map(|i| every / 2 + i * every).take_while(|&t| t < duration).collect()
}

/// Output size whose long side is `side`, in the canvas's shape.
pub fn size(project: &Project, side: u32) -> (u32, u32) {
    let (w, h) = (project.canvas.width.max(1) as f32, project.canvas.height.max(1) as f32);
    let k = side as f32 / w.max(h);
    (((w * k).round() as u32).max(16), ((h * k).round() as u32).max(16))
}

pub fn render(renderer: &mut Renderer, project: &Project, t: i64, (width, height): (u32, u32)) -> Result<Frame> {
    ensure!(t >= 0 && t < project.duration_us(), "INVALID_RANGE: time is outside the timeline");
    let rgba = renderer.render(project, t, width, height, Wait::Exact, false).context("Rendering the timeline")?;
    Ok(Frame { width, height, rgba })
}
