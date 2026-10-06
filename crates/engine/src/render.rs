//! Turns a project and a time into a composited frame. Preview and export both use this.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

use crate::gpu::{Gpu, Image, Layer};
use crate::media::decode_size;
use crate::model::{AssetKind, Clip, ClipContent, Project, TrackKind, Transform, parse_color};
use crate::text::TextRenderer;
use crate::worker::VideoWorker;

const PREFETCH_US: i64 = 1_000_000;
const IDLE_WORKER: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wait {
    /// Use whatever frames are ready (playback).
    Ready,
    /// Wait for the exact frame of every layer (paused preview, export).
    Exact,
}

pub struct Renderer {
    gpu: Gpu,
    text: TextRenderer,
    workers: HashMap<String, VideoWorker>,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        Ok(Self { gpu: Gpu::new()?, text: TextRenderer::new(), workers: HashMap::new() })
    }

    pub fn adapter_name(&self) -> &str {
        &self.gpu.adapter_name
    }

    /// Renders the timeline at `t_us` into `out_w`×`out_h` straight RGBA.
    pub fn render(&mut self, project: &Project, t_us: i64, out_w: u32, out_h: u32, wait: Wait, playing: bool) -> Result<Vec<u8>> {
        let canvas = &project.canvas;
        let k = out_w as f32 / canvas.width.max(1) as f32;
        let mut layers = Vec::new();

        for track in &project.tracks {
            if track.hidden || track.kind == TrackKind::Audio {
                continue;
            }
            let Some(clip) = track.clips.iter().find(|c| c.contains(t_us)) else { continue };
            if let Some(layer) = self.layer_for(project, clip, t_us, k, wait, playing) {
                layers.push(layer);
            }
        }

        if playing {
            self.prefetch(project, t_us, k);
        }
        self.workers.retain(|_, w| w.last_used.elapsed() < IDLE_WORKER);

        self.gpu.render(out_w, out_h, parse_color(&canvas.background), &layers)
    }

    /// Opens and seeks decoders for clips that start soon, so cuts do not stall.
    fn prefetch(&mut self, project: &Project, t_us: i64, k: f32) {
        for track in &project.tracks {
            if track.hidden || track.kind != TrackKind::Video {
                continue;
            }
            for clip in &track.clips {
                if clip.start_us > t_us && clip.start_us <= t_us + PREFETCH_US {
                    self.layer_for(project, clip, clip.start_us, k, Wait::Ready, false);
                }
            }
        }
    }

    fn layer_for(&mut self, project: &Project, clip: &Clip, t_us: i64, k: f32, wait: Wait, playing: bool) -> Option<Layer> {
        let canvas = &project.canvas;
        let (cw, ch) = (canvas.width as f32, canvas.height as f32);
        match &clip.content {
            ClipContent::Media { asset_id, source_in_us, transform, .. } => {
                let asset = project.asset(asset_id)?;
                if asset.kind == AssetKind::Audio || asset.width == 0 {
                    return None;
                }
                let (aw, ah) = (asset.width as f32, asset.height as f32);
                let fit = (cw / aw).min(ch / ah);
                let disp = (aw * fit * transform.scale * k, ah * fit * transform.scale * k);
                if disp.0 < 1.0 || disp.1 < 1.0 {
                    return None;
                }
                let src = if asset.rotation % 180 == 90 { (asset.height, asset.width) } else { (asset.width, asset.height) };
                let size = decode_size(src, asset.rotation, disp);
                let source_t = if asset.kind == AssetKind::Image { 0 } else { source_in_us + (t_us - clip.start_us) };
                let worker = self
                    .workers
                    .entry(clip.id.clone())
                    .or_insert_with(|| VideoWorker::spawn(PathBuf::from(&asset.path)));
                let frame = worker.get(source_t, size, playing, wait == Wait::Exact)?;
                let image = Image { width: frame.width, height: frame.height, data: frame.data };
                Some(Layer {
                    image,
                    corners: quad(transform, disp, cw, ch, k),
                    uv_rotation: asset.rotation,
                    opacity: transform.opacity,
                })
            }
            ClipContent::Text { text, style, transform } => {
                let scale = k * transform.scale;
                let image = self.text.render(text, style, scale, cw * k * 0.9);
                let disp = (image.width as f32, image.height as f32);
                Some(Layer { corners: quad(transform, disp, cw, ch, k), image, uv_rotation: 0, opacity: transform.opacity })
            }
        }
    }
}

/// Corner positions (tl, tr, br, bl) in output pixels for a layer of `size` output pixels.
fn quad(t: &Transform, size: (f32, f32), cw: f32, ch: f32, k: f32) -> [[f32; 2]; 4] {
    let cx = (cw / 2.0 + t.x * cw) * k;
    let cy = (ch / 2.0 + t.y * ch) * k;
    let (hw, hh) = (size.0 / 2.0, size.1 / 2.0);
    let (s, c) = t.rotation.to_radians().sin_cos();
    [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|(x, y)| [cx + x * c - y * s, cy + x * s + y * c])
}
