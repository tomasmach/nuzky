//! Turns a project and a time into a composited frame. Preview and export both use this.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::effects::{max_animation_scale, source_time, transform_at, transition_at, transition_window};
use crate::gpu::{Draw, Gpu, Image, Layer};
use crate::media::decode_size;
use crate::model::{Adjust, Asset, AssetKind, Clip, ClipContent, Project, TrackKind, Transform, TransitionKind, parse_color};
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
    blurred: HashMap<usize, (Image, Image)>,
    solids: [Image; 3],
    pub late_layers: u64,
}

struct Placement {
    transform: Transform,
    size: (f32, f32),
    text: Option<Image>,
}

fn placement(project: &Project, clip: &Clip, t_us: i64, text_renderer: &mut TextRenderer) -> Option<Placement> {
    let (transform, reveal) = transform_at(clip, t_us);
    if transform.opacity <= 0.0 || transform.scale <= 0.0 || reveal <= 0.0 {
        return None;
    }
    let canvas = &project.canvas;
    let (size, text) = match &clip.content {
        ClipContent::Media { asset_id, .. } => {
            let asset = project.asset(asset_id)?;
            if asset.kind == AssetKind::Audio || asset.width == 0 || asset.height == 0 {
                return None;
            }
            let fit = (canvas.width as f32 / asset.width as f32).min(canvas.height as f32 / asset.height as f32);
            ((asset.width as f32 * fit, asset.height as f32 * fit), None)
        }
        ClipContent::Text { text, style, .. } => {
            let count = (text.chars().count() as f32 * reveal).ceil() as usize;
            let end = text.char_indices().nth(count).map(|(i, _)| i).unwrap_or(text.len());
            // Rasterise in canvas pixels so preview, export and selection use identical wrapping.
            let image = text_renderer.render(&text[..end], style, 1.0, canvas.width as f32 * 0.9);
            ((image.width as f32, image.height as f32), Some(image))
        }
    };
    Some(Placement { transform, size, text })
}

pub fn layer_bounds(project: &Project, t_us: i64, text: &mut TextRenderer) -> Vec<(String, [[f32; 2]; 4])> {
    let mut bounds = Vec::new();
    for track in &project.tracks {
        if track.hidden || track.kind == TrackKind::Audio { continue; }
        if let Some((a, b, transition, p)) = transition_at(track, t_us) {
            for (clip, incoming) in [(a, false), (b, true)] {
                if let Some(place) = placement(project, clip, t_us, text) {
                    let (corners, opacity, clip_rect) = transition_geometry(placement_quad(project, &place, 1.0), place.transform.opacity,
                        transition.kind, p, incoming, project.canvas.width, project.canvas.height);
                    let rect = clip_rect.unwrap_or([0.0, 0.0, project.canvas.width as f32, project.canvas.height as f32]);
                    if opacity > 0.0 && intersects_rect(&corners, rect) {
                        bounds.push((clip.id.clone(), corners));
                    }
                }
            }
        } else if let Some(clip) = track.clips.iter().find(|c| c.contains(t_us)) {
            if let Some(place) = placement(project, clip, t_us, text) {
                let corners = placement_quad(project, &place, 1.0);
                if intersects_canvas(project, &corners) { bounds.push((clip.id.clone(), corners)); }
            }
        }
    }
    bounds
}

fn intersects_canvas(project: &Project, corners: &[[f32; 2]; 4]) -> bool {
    intersects_rect(corners, [0.0, 0.0, project.canvas.width as f32, project.canvas.height as f32])
}

fn intersects_rect(corners: &[[f32; 2]; 4], rect: [f32; 4]) -> bool {
    let [x0, y0, x1, y1] = rect;
    if x0 >= x1 || y0 >= y1 { return false; }
    let rectangle = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let edge = |i: usize, j: usize| [corners[i][1] - corners[j][1], corners[j][0] - corners[i][0]];
    // Separating axes also reject rotated quads whose bounding box touches the wipe.
    [[1.0, 0.0], [0.0, 1.0], edge(0, 1), edge(1, 2)].into_iter().all(|axis| {
        let range = |points: &[[f32; 2]; 4]| points.iter().map(|p| p[0] * axis[0] + p[1] * axis[1])
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| (lo.min(p), hi.max(p)));
        let (a, b) = (range(corners), range(&rectangle));
        a.0 < b.1 && b.0 < a.1
    })
}

fn placement_quad(project: &Project, place: &Placement, k: f32) -> [[f32; 2]; 4] {
    let scale = place.transform.scale * k;
    quad(&place.transform, (place.size.0 * scale, place.size.1 * scale), project.canvas.width as f32, project.canvas.height as f32, k)
}

fn decode_resolution(project: &Project, clip: &Clip, asset: &Asset, k: f32) -> (u32, u32) {
    let base = match clip.content { ClipContent::Media { transform, .. } | ClipContent::Text { transform, .. } => transform.scale };
    let mut scale = clip.keyframes.iter().map(|key| key.transform.scale).reduce(f32::max).unwrap_or(base);
    scale *= max_animation_scale(clip);
    if project.tracks.iter().filter(|t| t.id == crate::edit::MAIN_TRACK).any(|t| {
        t.clips.windows(2).any(|pair| pair[0].id == clip.id && pair[1].transition_in.is_some_and(|t| t.kind == TransitionKind::ZoomIn))
    }) { scale *= 2.0; }
    let fit = (project.canvas.width as f32 / asset.width as f32).min(project.canvas.height as f32 / asset.height as f32);
    let src = if asset.rotation % 180 == 90 { (asset.height, asset.width) } else { (asset.width, asset.height) };
    decode_size(src, asset.rotation, (asset.width as f32 * fit * scale * k, asset.height as f32 * fit * scale * k))
}

impl Renderer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            gpu: Gpu::new()?, text: TextRenderer::new(), workers: HashMap::new(), blurred: HashMap::new(),
            solids: [[0, 0, 0, 0], [0, 0, 0, 255], [255; 4]].map(|c| Image { width: 1, height: 1, data: Arc::new(c.to_vec()) }),
            late_layers: 0,
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.gpu.adapter_name
    }

    pub fn max_texture_dimension(&self) -> u32 {
        self.gpu.max_texture_dimension()
    }

    /// Renders the timeline at `t_us` into `out_w`×`out_h` straight RGBA.
    pub fn render(&mut self, project: &Project, t_us: i64, out_w: u32, out_h: u32, wait: Wait, playing: bool) -> Result<Vec<u8>> {
        let canvas = &project.canvas;
        let k = out_w as f32 / canvas.width.max(1) as f32;
        let mut draws = Vec::new();
        let mut blur_used = Vec::new();
        for track in &project.tracks {
            if track.hidden || track.kind == TrackKind::Audio { continue; }
            let transition = transition_at(track, t_us);
            if let Some((a, b, transition, p)) = transition {
                let mut pair = [None, None];
                for (index, (clip, incoming)) in [(a, false), (b, true)].into_iter().enumerate() {
                    let mut layer = self.layer_for(project, clip, t_us, k, wait, playing);
                    if let Some(layer) = &mut layer {
                        apply_transition(layer, transition.kind, p, incoming, out_w, out_h);
                    }
                    pair[index] = layer;
                }
                if canvas.background_blur > 0.0 {
                    let backgrounds = pair.each_ref().map(|layer| match layer {
                        Some(layer) => self.background(layer, canvas.background_blur, out_w, out_h, &mut blur_used),
                        None => solid(&self.solids[0], out_w, out_h),
                    });
                    draws.push(Draw::Transition(backgrounds));
                }
                if matches!(transition.kind, TransitionKind::FadeBlack | TransitionKind::FadeWhite) {
                    let index = if transition.kind == TransitionKind::FadeWhite { 2 } else { 1 };
                    let mut colour = solid(&self.solids[index], out_w, out_h);
                    colour.opacity = 1.0 - (2.0 * p - 1.0).abs();
                    pair[if p < 0.5 { 1 } else { 0 }] = Some(colour);
                }
                draws.push(Draw::Transition(pair.map(|layer| layer.unwrap_or_else(|| solid(&self.solids[0], out_w, out_h)))));
            } else if let Some(clip) = track.clips.iter().find(|c| c.contains(t_us)) {
                if let Some(layer) = self.layer_for(project, clip, t_us, k, wait, playing) {
                    if track.id == crate::edit::MAIN_TRACK && canvas.background_blur > 0.0 {
                        draws.push(Draw::Layer(self.background(&layer, canvas.background_blur, out_w, out_h, &mut blur_used)));
                    }
                    draws.push(Draw::Layer(layer));
                }
            }
        }
        if playing { self.prefetch(project, t_us, k); }
        self.workers.retain(|_, w| w.last_used.elapsed() < IDLE_WORKER);
        self.blurred.retain(|key, _| blur_used.contains(key));
        self.gpu.render(out_w, out_h, parse_color(&canvas.background), &draws)
    }

    fn background(&mut self, layer: &Layer, strength: f32, w: u32, h: u32, used: &mut Vec<usize>) -> Layer {
        let key = Arc::as_ptr(&layer.image.data) as usize;
        used.push(key);
        let image = self.blurred.entry(key).or_insert_with(|| (layer.image.clone(), small_image(&layer.image))).1.clone();
        let (iw, ih) = if layer.uv_rotation % 180 == 90 { (image.height, image.width) } else { (image.width, image.height) };
        let cover = (w as f32 / iw as f32).max(h as f32 / ih as f32);
        Layer {
            image, corners: quad(&Transform::default(), (iw as f32 * cover, ih as f32 * cover), w as f32, h as f32, 1.0),
            uv_rotation: layer.uv_rotation, opacity: layer.opacity, adjust: layer.adjust,
            blur: 0.5 + strength.clamp(0.0, 1.0) * 4.0, clip: layer.clip,
        }
    }

    fn prefetch(&mut self, project: &Project, t_us: i64, k: f32) {
        for track in &project.tracks {
            if track.hidden || track.kind != TrackKind::Video { continue; }
            for clip in &track.clips {
                let start = if track.id == crate::edit::MAIN_TRACK { transition_window(clip).map(|w| w.0).unwrap_or(clip.start_us) } else { clip.start_us };
                if start > t_us && start <= t_us + PREFETCH_US {
                    let ClipContent::Media { asset_id, .. } = &clip.content else { continue };
                    let Some(asset) = project.asset(asset_id).filter(|a| a.kind != AssetKind::Audio && a.width > 0 && a.height > 0) else { continue };
                    let size = decode_resolution(project, clip, asset, k);
                    let worker = self.workers.entry(clip.id.clone()).or_insert_with(|| VideoWorker::spawn(PathBuf::from(&asset.path)));
                    worker.get(source_time(clip, clip.start_us), size, false, false);
                }
            }
        }
    }

    fn layer_for(&mut self, project: &Project, clip: &Clip, t_us: i64, k: f32, wait: Wait, playing: bool) -> Option<Layer> {
        let place = placement(project, clip, t_us, &mut self.text)?;
        let corners = placement_quad(project, &place, k);
        let (image, rotation, adjust) = match &clip.content {
            ClipContent::Media { asset_id, adjust, .. } => {
                let asset = project.asset(asset_id)?;
                // A stable conversion size avoids flushing the decoder queue on every animation frame.
                let size = decode_resolution(project, clip, asset, k);
                let source_t = if asset.kind == AssetKind::Image { 0 } else { source_time(clip, t_us).min((asset.duration_us - 1).max(0)) };
                let worker = self.workers.entry(clip.id.clone()).or_insert_with(|| VideoWorker::spawn(PathBuf::from(&asset.path)));
                let frame = worker.get(source_t, size, playing, wait == Wait::Exact);
                if playing && !worker.exact { self.late_layers += 1; }
                let frame = frame?;
                (Image { width: frame.width, height: frame.height, data: frame.data }, asset.rotation, *adjust)
            }
            ClipContent::Text { .. } => (place.text?, 0, Adjust::default()),
        };
        Some(Layer { image, corners, uv_rotation: rotation, opacity: place.transform.opacity, adjust, blur: 0.0, clip: None })
    }
}

fn transition_geometry(mut corners: [[f32; 2]; 4], opacity: f32, kind: TransitionKind, p: f32, incoming: bool, w: u32, h: u32) -> ([[f32; 2]; 4], f32, Option<[f32; 4]>) {
    use TransitionKind::*;
    let weight = if incoming { p } else { 1.0 - p };
    let mut t = Transform::default();
    match kind {
        Dissolve | Blur => t.opacity = weight,
        FadeBlack | FadeWhite => t.opacity = (weight * 2.0 - 1.0).max(0.0),
        SlideLeft => t.x = if incoming { 1.0 - p } else { -p },
        SlideUp => t.y = if incoming { 1.0 - p } else { -p },
        ZoomIn => { t.opacity = weight; if !incoming { t.scale = 1.0 + p; } }
        WipeLeft => {}
    }
    let cx = (corners[0][0] + corners[2][0]) / 2.0;
    let cy = (corners[0][1] + corners[2][1]) / 2.0;
    for point in &mut corners {
        point[0] = (point[0] - cx) * t.scale + cx + t.x * w as f32;
        point[1] = (point[1] - cy) * t.scale + cy + t.y * h as f32;
    }
    let clip = match kind {
        TransitionKind::WipeLeft | TransitionKind::SlideLeft => {
            let edge = w as f32 * (1.0 - p);
            Some(if incoming { [edge, 0.0, w as f32, h as f32] } else { [0.0, 0.0, edge, h as f32] })
        }
        TransitionKind::SlideUp => {
            let edge = h as f32 * (1.0 - p);
            Some(if incoming { [0.0, edge, w as f32, h as f32] } else { [0.0, 0.0, w as f32, edge] })
        }
        _ => None,
    };
    (corners, opacity * t.opacity, clip)
}

fn apply_transition(layer: &mut Layer, kind: TransitionKind, p: f32, incoming: bool, w: u32, h: u32) {
    (layer.corners, layer.opacity, layer.clip) = transition_geometry(layer.corners, layer.opacity, kind, p, incoming, w, h);
    if kind == TransitionKind::Blur { layer.blur = if incoming { 1.0 - p } else { p } * 12.0; }
}

fn solid(image: &Image, w: u32, h: u32) -> Layer {
    Layer {
        image: image.clone(),
        corners: [[0.0, 0.0], [w as f32, 0.0], [w as f32, h as f32], [0.0, h as f32]],
        uv_rotation: 0, opacity: 1.0, adjust: Adjust::default(), blur: 0.0, clip: None,
    }
}

fn small_image(image: &Image) -> Image {
    let scale = (64.0 / image.width.max(image.height) as f32).min(1.0);
    let w = (image.width as f32 * scale).round().max(1.0) as u32;
    let h = (image.height as f32 * scale).round().max(1.0) as u32;
    let mut data = vec![0; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let sx = x * image.width / w;
            let sy = y * image.height / h;
            let src = ((sy * image.width + sx) * 4) as usize;
            let dst = ((y * w + x) * 4) as usize;
            data[dst..dst + 4].copy_from_slice(&image.data[src..src + 4]);
        }
    }
    Image { width: w, height: h, data: Arc::new(data) }
}

/// Corner positions (tl, tr, br, bl) in output pixels for a layer of `size` output pixels.
fn quad(t: &Transform, size: (f32, f32), cw: f32, ch: f32, k: f32) -> [[f32; 2]; 4] {
    let cx = (cw / 2.0 + t.x * cw) * k;
    let cy = (ch / 2.0 + t.y * ch) * k;
    let (hw, hh) = (size.0 / 2.0, size.1 / 2.0);
    let (s, c) = t.rotation.to_radians().sin_cos();
    [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|(x, y)| [cx + x * c - y * s, cy + x * s + y * c])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TextStyle, Track};

    #[test]
    fn wipe_bounds_exclude_layers_outside_visible_rect() {
        use crate::model::Transition;
        let mut project = Project::new("wipe");
        project.assets.push(Asset { id: "a".into(), name: String::new(), path: String::new(), kind: AssetKind::Video,
            duration_us: 2_000_000, width: 1080, height: 1920, fps: 30.0, has_audio: false, rotation: 0 });
        for (id, start, x) in [("a", 0, 0.3), ("b", 1_000_000, -0.3)] {
            project.tracks[0].clips.push(Clip::new(id.into(), start, 1_000_000, ClipContent::Media {
                asset_id: "a".into(), source_in_us: 0, speed: 1.0, volume: 1.0,
                transform: Transform { x, scale: 0.2, rotation: 25.0, ..Transform::default() },
                adjust: Default::default(), fade_in_us: 0, fade_out_us: 0,
            }));
        }
        project.tracks[0].clips[1].transition_in = Some(Transition { kind: TransitionKind::WipeLeft, duration_us: 400_000 });
        let mut text = TextRenderer::new();
        assert!(layer_bounds(&project, 1_000_000, &mut text).is_empty());
        assert_eq!(layer_bounds(&project, 800_000, &mut text).len(), 1);
        assert_eq!(layer_bounds(&project, 1_199_999, &mut text).len(), 1);
    }

    #[test]
    fn text_bounds_match_canvas_raster_and_hidden_tracks_are_excluded() {
        let mut project = Project::new("bounds");
        let mut text = TextRenderer::new();
        let style = TextStyle { font_size: 60.0, color: "#ffffff".into(), bold: true, stroke_width: 3.0, stroke_color: "#000000".into(), background: None };
        let image = text.render("Ahoj světe", &style, 1.0, 972.0);
        let clip = Clip::new("text".into(), 0, 1_000_000, ClipContent::Text { text: "Ahoj světe".into(), style, transform: Transform { scale: 1.5, x: 0.1, ..Transform::default() } });
        project.tracks.push(Track { id: "text".into(), kind: TrackKind::Text, name: String::new(), muted: false, hidden: false, clips: vec![clip] });
        let bounds = layer_bounds(&project, 500_000, &mut text);
        let quad = bounds[0].1;
        assert_eq!(quad[1][0] - quad[0][0], image.width as f32 * 1.5);
        assert_eq!(quad[3][1] - quad[0][1], image.height as f32 * 1.5);
        assert!(((quad[0][0] + quad[2][0]) / 2.0 - 648.0).abs() < 1e-4);
        let mut renderer = Renderer::new().unwrap();
        let layer = renderer.layer_for(&project, &project.tracks[1].clips[0], 500_000, 0.5, Wait::Exact, false).unwrap();
        assert_eq!(layer.corners, quad.map(|p| [p[0] * 0.5, p[1] * 0.5]));
        project.tracks[1].hidden = true;
        assert!(layer_bounds(&project, 500_000, &mut text).is_empty());
    }
}
