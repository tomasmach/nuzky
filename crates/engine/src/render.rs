//! Turns a project and a time into a composited frame. Preview and export both use this.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::effects::{max_animation_scale, source_time, transform_at, transition_at, transition_window};
use crate::gpu::{Draw, Gpu, Image, Layer};
use crate::media::decode_size;
use crate::model::{
    Adjust, Asset, AssetKind, Clip, ClipContent, Project, Track, TrackKind, Transform, TransitionKind, parse_color,
};
use crate::text::TextRenderer;
use crate::worker::VideoWorker;

const BACKGROUND_SIZE: f32 = 96.0;
/// Samples per block side when shrinking a frame for the background blur.
const BLOCK_SAMPLES: usize = 4;
const BACKGROUND_BLUR_PASSES: usize = 3;
const BACKGROUND_MAX_RADIUS: f32 = 9.0;
const BACKGROUND_BRIGHTNESS: f32 = 0.85;
const MAX_TEXT_SCALE: f32 = 8.0;
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
    blurred: HashMap<(usize, usize), (Image, Image)>,
    solids: [Image; 3],
    pub late_layers: u64,
}

struct Placement {
    transform: Transform,
    size: (f32, f32),
    text: Option<Image>,
}

#[derive(Clone, Copy)]
struct TransitionRole {
    kind: TransitionKind,
    progress: f32,
    incoming: bool,
}

#[derive(Clone, Copy)]
struct VisibleClip<'a> {
    clip: &'a Clip,
    transition: Option<TransitionRole>,
}

fn visible_clips(track: &Track, t_us: i64) -> impl Iterator<Item = VisibleClip<'_>> {
    let clips = if track.hidden || track.kind == TrackKind::Audio {
        [None, None]
    } else if let Some((a, b, transition, progress)) = transition_at(track, t_us) {
        [(a, false), (b, true)].map(|(clip, incoming)| {
            Some(VisibleClip { clip, transition: Some(TransitionRole { kind: transition.kind, progress, incoming }) })
        })
    } else {
        [track.clips.iter().find(|c| c.contains(t_us)).map(|clip| VisibleClip { clip, transition: None }), None]
    };
    clips.into_iter().flatten()
}

fn placement(
    project: &Project,
    visible: VisibleClip,
    t_us: i64,
    k: f32,
    text_renderer: &mut TextRenderer,
) -> Option<Placement> {
    let clip = visible.clip;
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
            let zoom = match visible.transition {
                Some(TransitionRole { kind: TransitionKind::ZoomIn, progress, incoming: false }) => 1.0 + progress,
                _ => 1.0,
            };
            let scale = 2.0_f32.powf((k * transform.scale * zoom).clamp(1.0, MAX_TEXT_SCALE).log2().ceil());
            let wrap = style.max_width.unwrap_or(canvas.width as f32 * 0.9);
            let image = text_renderer.render(&text[..end], style, scale, wrap * scale);
            ((image.width as f32 / scale, image.height as f32 / scale), Some(image))
        }
    };
    Some(Placement { transform, size, text })
}

pub fn layer_bounds(project: &Project, t_us: i64, text: &mut TextRenderer) -> Vec<(String, [[f32; 2]; 4])> {
    let mut bounds = Vec::new();
    for visible in project.tracks.iter().flat_map(|track| visible_clips(track, t_us)) {
        let Some(place) = placement(project, visible, t_us, 1.0, text) else { continue };
        let mut corners = placement_quad(project, &place, 1.0);
        let mut opacity = place.transform.opacity;
        let mut clip_rect = None;
        if let Some(role) = visible.transition {
            (corners, opacity, clip_rect) = transition_geometry(
                corners,
                opacity,
                role.kind,
                role.progress,
                role.incoming,
                project.canvas.width,
                project.canvas.height,
            );
        }
        let rect = clip_rect.unwrap_or([0.0, 0.0, project.canvas.width as f32, project.canvas.height as f32]);
        if opacity > 0.0 && intersects_rect(&corners, rect) {
            bounds.push((visible.clip.id.clone(), corners));
        }
    }
    bounds
}

fn intersects_rect(corners: &[[f32; 2]; 4], rect: [f32; 4]) -> bool {
    let [x0, y0, x1, y1] = rect;
    if x0 >= x1 || y0 >= y1 {
        return false;
    }
    let rectangle = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let edge = |i: usize, j: usize| [corners[i][1] - corners[j][1], corners[j][0] - corners[i][0]];
    // Separating axes also reject rotated quads whose bounding box touches the wipe.
    [[1.0, 0.0], [0.0, 1.0], edge(0, 1), edge(1, 2)].into_iter().all(|axis| {
        let range = |points: &[[f32; 2]; 4]| {
            points
                .iter()
                .map(|p| p[0] * axis[0] + p[1] * axis[1])
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| (lo.min(p), hi.max(p)))
        };
        let (a, b) = (range(corners), range(&rectangle));
        a.0 < b.1 && b.0 < a.1
    })
}

fn placement_quad(project: &Project, place: &Placement, k: f32) -> [[f32; 2]; 4] {
    let scale = place.transform.scale * k;
    quad(
        &place.transform,
        (place.size.0 * scale, place.size.1 * scale),
        project.canvas.width as f32,
        project.canvas.height as f32,
        k,
    )
}

fn decode_resolution(project: &Project, clip: &Clip, asset: &Asset, k: f32) -> (u32, u32) {
    let base = match clip.content {
        ClipContent::Media { transform, .. } | ClipContent::Text { transform, .. } => transform.scale,
    };
    let mut scale = clip.keyframes.iter().map(|key| key.transform.scale).reduce(f32::max).unwrap_or(base);
    scale *= max_animation_scale(clip);
    if project.tracks.iter().filter(|t| t.id == crate::edit::MAIN_TRACK).any(|t| {
        t.clips.windows(2).any(|pair| {
            pair[0].id == clip.id && pair[1].transition_in.is_some_and(|t| t.kind == TransitionKind::ZoomIn)
        })
    }) {
        scale *= 2.0;
    }
    let fit =
        (project.canvas.width as f32 / asset.width as f32).min(project.canvas.height as f32 / asset.height as f32);
    let src = if asset.rotation % 180 == 90 { (asset.height, asset.width) } else { (asset.width, asset.height) };
    decode_size(src, asset.rotation, (asset.width as f32 * fit * scale * k, asset.height as f32 * fit * scale * k))
}

impl Renderer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            gpu: Gpu::new()?,
            text: TextRenderer::new(),
            workers: HashMap::new(),
            blurred: HashMap::new(),
            solids: [[0, 0, 0, 0], [0, 0, 0, 255], [255; 4]].map(|c| Image {
                width: 1,
                height: 1,
                data: Arc::new(c.to_vec()),
            }),
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
    pub fn render(
        &mut self,
        project: &Project,
        t_us: i64,
        out_w: u32,
        out_h: u32,
        wait: Wait,
        playing: bool,
    ) -> Result<Vec<u8>> {
        let canvas = &project.canvas;
        let k = out_w as f32 / canvas.width.max(1) as f32;
        let mut draws = Vec::new();
        let mut blur_used = Vec::new();
        for track in &project.tracks {
            let mut visible = visible_clips(track, t_us);
            let Some(first) = visible.next() else { continue };
            if let Some(transition) = first.transition {
                let p = transition.progress;
                let mut pair = [None, None];
                for (index, visible) in std::iter::once(first).chain(visible).enumerate() {
                    let mut layer = self.layer_for(project, visible, t_us, k, wait, playing)?;
                    if let (Some(layer), Some(role)) = (&mut layer, visible.transition) {
                        apply_transition(layer, role.kind, role.progress, role.incoming, out_w, out_h);
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
                draws.push(Draw::Transition(
                    pair.map(|layer| layer.unwrap_or_else(|| solid(&self.solids[0], out_w, out_h))),
                ));
            } else if let Some(layer) = self.layer_for(project, first, t_us, k, wait, playing)? {
                if track.id == crate::edit::MAIN_TRACK && canvas.background_blur > 0.0 {
                    draws.push(Draw::Layer(self.background(
                        &layer,
                        canvas.background_blur,
                        out_w,
                        out_h,
                        &mut blur_used,
                    )));
                }
                draws.push(Draw::Layer(layer));
            }
        }
        if playing {
            self.prefetch(project, t_us, k);
        }
        self.workers.retain(|_, w| w.last_used.elapsed() < IDLE_WORKER);
        self.blurred.retain(|key, _| blur_used.contains(key));
        self.gpu.render(out_w, out_h, parse_color(&canvas.background), &draws)
    }

    fn background(&mut self, layer: &Layer, strength: f32, w: u32, h: u32, used: &mut Vec<(usize, usize)>) -> Layer {
        let radius = (1.0 + strength.clamp(0.0, 1.0) * (BACKGROUND_MAX_RADIUS - 1.0)).round() as usize;
        let key = (Arc::as_ptr(&layer.image.data) as usize, radius);
        used.push(key);
        let image = self
            .blurred
            .entry(key)
            .or_insert_with(|| (layer.image.clone(), small_image(&layer.image, radius)))
            .1
            .clone();
        let (iw, ih) =
            if layer.uv_rotation % 180 == 90 { (image.height, image.width) } else { (image.width, image.height) };
        let cover = (w as f32 / iw as f32).max(h as f32 / ih as f32);
        Layer {
            image,
            corners: quad(&Transform::default(), (iw as f32 * cover, ih as f32 * cover), w as f32, h as f32, 1.0),
            uv_rotation: layer.uv_rotation,
            opacity: layer.opacity,
            adjust: layer.adjust,
            blur: 0.0,
            clip: layer.clip,
        }
    }

    fn prefetch(&mut self, project: &Project, t_us: i64, k: f32) {
        for track in &project.tracks {
            if track.hidden || track.kind != TrackKind::Video {
                continue;
            }
            for clip in &track.clips {
                let start = if track.id == crate::edit::MAIN_TRACK {
                    transition_window(clip).map(|w| w.0).unwrap_or(clip.start_us)
                } else {
                    clip.start_us
                };
                if start > t_us && start <= t_us + PREFETCH_US {
                    let ClipContent::Media { asset_id, .. } = &clip.content else { continue };
                    let Some(asset) =
                        project.asset(asset_id).filter(|a| a.kind != AssetKind::Audio && a.width > 0 && a.height > 0)
                    else {
                        continue;
                    };
                    let size = decode_resolution(project, clip, asset, k);
                    let worker = self
                        .workers
                        .entry(clip.id.clone())
                        .or_insert_with(|| VideoWorker::spawn(PathBuf::from(&asset.path)));
                    worker.get(source_time(clip, clip.start_us), size, false, false);
                }
            }
        }
    }

    fn layer_for(
        &mut self,
        project: &Project,
        visible: VisibleClip,
        t_us: i64,
        k: f32,
        wait: Wait,
        playing: bool,
    ) -> Result<Option<Layer>> {
        let clip = visible.clip;
        let Some(place) = placement(project, visible, t_us, k, &mut self.text) else { return Ok(None) };
        let corners = placement_quad(project, &place, k);
        let (image, rotation, adjust) = match &clip.content {
            ClipContent::Media { asset_id, adjust, .. } => {
                let Some(asset) = project.asset(asset_id) else { return Ok(None) };
                // A stable conversion size avoids flushing the decoder queue on every animation frame.
                let size = decode_resolution(project, clip, asset, k);
                let source_t = if asset.kind == AssetKind::Image {
                    0
                } else {
                    source_time(clip, t_us).min((asset.duration_us - 1).max(0))
                };
                let worker = self
                    .workers
                    .entry(clip.id.clone())
                    .or_insert_with(|| VideoWorker::spawn(PathBuf::from(&asset.path)));
                let frame = worker.get(source_t, size, playing, wait == Wait::Exact);
                if wait == Wait::Exact {
                    if let Some(error) = worker.error() {
                        bail!("Cannot decode {}: {error}", asset.path);
                    }
                    if !worker.exact || frame.is_none() {
                        bail!("Timed out waiting for frame at {source_t} us in {}", asset.path);
                    }
                }
                if playing && !worker.exact {
                    self.late_layers += 1;
                }
                let Some(frame) = frame else { return Ok(None) };
                (Image { width: frame.width, height: frame.height, data: frame.data }, asset.rotation, *adjust)
            }
            ClipContent::Text { .. } => {
                let Some(text) = place.text else { return Ok(None) };
                (text, 0, Adjust::default())
            }
        };
        Ok(Some(Layer {
            image,
            corners,
            uv_rotation: rotation,
            opacity: place.transform.opacity,
            adjust,
            blur: 0.0,
            clip: None,
        }))
    }
}

fn transition_geometry(
    mut corners: [[f32; 2]; 4],
    opacity: f32,
    kind: TransitionKind,
    p: f32,
    incoming: bool,
    w: u32,
    h: u32,
) -> ([[f32; 2]; 4], f32, Option<[f32; 4]>) {
    use TransitionKind::*;
    let weight = if incoming { p } else { 1.0 - p };
    let mut t = Transform::default();
    match kind {
        Dissolve | Blur => t.opacity = weight,
        FadeBlack | FadeWhite => t.opacity = (weight * 2.0 - 1.0).max(0.0),
        SlideLeft => t.x = if incoming { 1.0 - p } else { -p },
        SlideUp => t.y = if incoming { 1.0 - p } else { -p },
        ZoomIn => {
            t.opacity = weight;
            if !incoming {
                t.scale = 1.0 + p;
            }
        }
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
    (layer.corners, layer.opacity, layer.clip) =
        transition_geometry(layer.corners, layer.opacity, kind, p, incoming, w, h);
    if kind == TransitionKind::Blur {
        layer.blur = if incoming { 1.0 - p } else { p } * 12.0;
    }
}

fn solid(image: &Image, w: u32, h: u32) -> Layer {
    Layer {
        image: image.clone(),
        corners: [[0.0, 0.0], [w as f32, 0.0], [w as f32, h as f32], [0.0, h as f32]],
        uv_rotation: 0,
        opacity: 1.0,
        adjust: Adjust::default(),
        blur: 0.0,
        clip: None,
    }
}

fn small_image(image: &Image, radius: usize) -> Image {
    let scale = (BACKGROUND_SIZE / image.width.max(image.height) as f32).min(1.0);
    let w = (image.width as f32 * scale).round().max(1.0) as usize;
    let h = (image.height as f32 * scale).round().max(1.0) as usize;
    let (sw, sh) = (image.width as usize, image.height as usize);
    let mut pixels = vec![[0.0; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            let (x0, x1) = (x * sw / w, (x + 1) * sw / w);
            let (y0, y1) = (y * sh / h, (y + 1) * sh / h);
            // A grid of samples per block is enough: the result is blurred heavily anyway, and
            // reading every source pixel would cost milliseconds on each new playback frame.
            let (step_x, step_y) = (((x1 - x0) / BLOCK_SAMPLES).max(1), ((y1 - y0) / BLOCK_SAMPLES).max(1));
            let mut sum = [0u64; 4];
            let mut count = 0u64;
            for sy in (y0..y1.max(y0 + 1)).step_by(step_y) {
                for sx in (x0..x1.max(x0 + 1)).step_by(step_x) {
                    let src = &image.data[(sy * sw + sx) * 4..(sy * sw + sx) * 4 + 4];
                    let alpha = src[3] as u64;
                    sum[0] += src[0] as u64 * alpha;
                    sum[1] += src[1] as u64 * alpha;
                    sum[2] += src[2] as u64 * alpha;
                    sum[3] += alpha;
                    count += 1;
                }
            }
            let count = count as f32;
            // Filter premultiplied colours so transparent pixels cannot bleed into the blur.
            pixels[y * w + x] = std::array::from_fn(|c| sum[c] as f32 / count / if c == 3 { 1.0 } else { 255.0 });
        }
    }
    let mut scratch = vec![[0.0; 4]; w * h];
    for _ in 0..BACKGROUND_BLUR_PASSES {
        box_blur(&pixels, &mut scratch, w, h, radius, true);
        box_blur(&scratch, &mut pixels, w, h, radius, false);
    }
    let mut data = Vec::with_capacity(w * h * 4);
    for pixel in pixels {
        let alpha = pixel[3].clamp(0.0, 255.0);
        for channel in &pixel[..3] {
            let value = if alpha > 0.0 { channel * 255.0 / alpha * BACKGROUND_BRIGHTNESS } else { 0.0 };
            data.push(value.clamp(0.0, 255.0).round() as u8);
        }
        data.push(alpha.round() as u8);
    }
    Image { width: w as u32, height: h as u32, data: Arc::new(data) }
}

fn box_blur(src: &[[f32; 4]], dst: &mut [[f32; 4]], w: usize, h: usize, radius: usize, horizontal: bool) {
    let (lines, length, stride) = if horizontal { (h, w, 1) } else { (w, h, w) };
    let divisor = (2 * radius + 1) as f32;
    for line in 0..lines {
        let base = if horizontal { line * w } else { line };
        let mut sum = src[base].map(|v| v * (radius + 1) as f32);
        for offset in 1..=radius {
            for c in 0..4 {
                sum[c] += src[base + offset.min(length - 1) * stride][c];
            }
        }
        for pos in 0..length {
            dst[base + pos * stride] = sum.map(|v| v / divisor);
            let remove = base + pos.saturating_sub(radius) * stride;
            let add = base + (pos + radius + 1).min(length - 1) * stride;
            for c in 0..4 {
                sum[c] += src[add][c] - src[remove][c];
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{CaptionSegment, EditCmd};
    use crate::model::{TextStyle, Track};

    #[test]
    fn background_averages_pixels_and_preserves_alpha() {
        let image = Image {
            width: 192,
            height: 108,
            data: Arc::new(
                (0..108)
                    .flat_map(|y| {
                        (0..192).flat_map(move |x| if (x + y) % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] })
                    })
                    .collect(),
            ),
        };
        let blurred = small_image(&image, 3);
        assert_eq!((blurred.width, blurred.height), (96, 54));
        assert!(blurred.data.chunks_exact(4).all(|px| px == [108, 108, 108, 255]));
        let transparent = Image { width: 2, height: 1, data: Arc::new(vec![255, 0, 0, 0, 0, 255, 0, 255]) };
        let blurred = small_image(&transparent, 3);
        assert!(blurred.data.chunks_exact(4).all(|px| px[0] == 0 && px[1] == 217 && px[2] == 0 && px[3] > 0));
    }

    #[test]
    fn text_scale_keeps_wrapping_bounds_and_animation_cache() {
        use crate::model::{Animation, AnimationKind, Transition};
        let mut project = Project::new("scaled text");
        let style = TextStyle {
            font_family: None,
            font_size: 60.0,
            stroke_width: 2.5,
            background: Some("#222222".into()),
            color: "#ffffff".into(),
            bold: true,
            stroke_color: "#000000".into(),
            max_width: None,
        };
        for (id, start) in [("out", 0), ("in", 1_000_000)] {
            let mut clip = Clip::new(
                id.into(),
                start,
                1_000_000,
                ClipContent::Text {
                    text: "A long wrapped title with accents: Příliš žluťoučký kůň".into(),
                    style: style.clone(),
                    transform: Transform { scale: 1.5, rotation: 12.0, ..Transform::default() },
                },
            );
            clip.anim_in = Some(Animation { kind: AnimationKind::Pop, duration_us: 400_000 });
            project.tracks[0].clips.push(clip);
        }
        project.tracks[0].clips[1].transition_in =
            Some(Transition { kind: TransitionKind::ZoomIn, duration_us: 400_000 });
        let mut text = TextRenderer::new();
        for t in [200_000, 201_000, 800_000, 900_000, 1_100_000] {
            let bounds = layer_bounds(&project, t, &mut text);
            for visible in visible_clips(&project.tracks[0], t) {
                let Some(base) = placement(&project, visible, t, 1.0, &mut text) else { continue };
                for k in [0.5, 1.0, 2.0, 4.0] {
                    let place = placement(&project, visible, t, k, &mut text).unwrap();
                    assert_eq!(place.size, base.size);
                    let mut corners = placement_quad(&project, &place, k);
                    if let Some(role) = visible.transition {
                        corners = transition_geometry(
                            corners,
                            place.transform.opacity,
                            role.kind,
                            role.progress,
                            role.incoming,
                            (project.canvas.width as f32 * k) as u32,
                            (project.canvas.height as f32 * k) as u32,
                        )
                        .0;
                    }
                    if let Some((_, expected)) = bounds.iter().find(|(id, _)| *id == visible.clip.id) {
                        assert_eq!(corners.map(|p| [p[0] / k, p[1] / k]), *expected);
                    }
                }
            }
        }
        let visible = visible_clips(&project.tracks[0], 200_000).next().unwrap();
        let a = placement(&project, visible, 200_000, 1.0, &mut text).unwrap().text.unwrap();
        let b = placement(&project, visible, 201_000, 1.0, &mut text).unwrap().text.unwrap();
        assert!(Arc::ptr_eq(&a.data, &b.data));
    }

    #[test]
    fn wipe_bounds_exclude_layers_outside_visible_rect() {
        use crate::model::Transition;
        let mut project = Project::new("wipe");
        project.assets.push(Asset {
            id: "a".into(),
            name: String::new(),
            path: String::new(),
            kind: AssetKind::Video,
            duration_us: 2_000_000,
            width: 1080,
            height: 1920,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        for (id, start, x) in [("a", 0, 0.3), ("b", 1_000_000, -0.3)] {
            project.tracks[0].clips.push(Clip::new(
                id.into(),
                start,
                1_000_000,
                ClipContent::Media {
                    asset_id: "a".into(),
                    source_in_us: 0,
                    speed: 1.0,
                    volume: 1.0,
                    transform: Transform { x, scale: 0.2, rotation: 25.0, ..Transform::default() },
                    adjust: Default::default(),
                    fade_in_us: 0,
                    fade_out_us: 0,
                },
            ));
        }
        project.tracks[0].clips[1].transition_in =
            Some(Transition { kind: TransitionKind::WipeLeft, duration_us: 400_000 });
        let mut text = TextRenderer::new();
        assert!(layer_bounds(&project, 1_000_000, &mut text).is_empty());
        assert_eq!(layer_bounds(&project, 800_000, &mut text).len(), 1);
        assert_eq!(layer_bounds(&project, 1_199_999, &mut text).len(), 1);
    }

    #[test]
    fn captions_on_vertical_videos_stay_inside_the_reels_and_tiktok_safe_area() {
        let mut project = Project::new("safe");
        let area = project.canvas.safe_area().unwrap();
        assert_eq!((area.left, area.top, area.right, area.bottom), (60.0, 250.0, 900.0, 1420.0));
        let style = TextStyle {
            font_family: None,
            font_size: 95.0,
            color: "#ffffff".into(),
            bold: false,
            stroke_width: 7.5,
            stroke_color: "#000000".into(),
            background: None,
            max_width: None,
        };
        // The longest reel caption, 15 characters, is wider than the safe area at 95 px and wraps.
        let segment = CaptionSegment { start_us: 0, end_us: 1_000_000, text: "Největší rozdíl".into() };
        project.apply(EditCmd::AddCaptions { segments: vec![segment], style }).unwrap();
        let ClipContent::Text { style, .. } = &project.tracks[1].clips[0].content else { panic!() };
        assert_eq!(style.max_width, Some(720.0));
        let quad = layer_bounds(&project, 500_000, &mut TextRenderer::new())[0].1;
        let (xs, ys): (Vec<f32>, Vec<f32>) = quad.iter().map(|p| (p[0], p[1])).unzip();
        // The outline and padding may reach a few pixels past the wrap width.
        let slack = 20.0;
        assert!(xs.iter().all(|&x| x >= area.left - slack && x <= area.right + slack), "{xs:?}");
        assert!(ys.iter().all(|&y| y >= area.top && y <= area.bottom), "{ys:?}");
        let mut wide = project.canvas.clone();
        (wide.width, wide.height) = (1920, 1080);
        assert!(wide.safe_area().is_none());
    }

    #[test]
    fn text_bounds_match_canvas_raster_and_hidden_tracks_are_excluded() {
        let mut project = Project::new("bounds");
        let mut text = TextRenderer::new();
        let style = TextStyle {
            font_family: None,
            font_size: 60.0,
            color: "#ffffff".into(),
            bold: true,
            stroke_width: 3.0,
            stroke_color: "#000000".into(),
            background: None,
            max_width: None,
        };
        let image = text.render("Ahoj světe", &style, 1.0, 972.0);
        let clip = Clip::new(
            "text".into(),
            0,
            1_000_000,
            ClipContent::Text {
                text: "Ahoj světe".into(),
                style,
                transform: Transform { scale: 1.5, x: 0.1, ..Transform::default() },
            },
        );
        project.tracks.push(Track {
            id: "text".into(),
            kind: TrackKind::Text,
            name: String::new(),
            muted: false,
            hidden: false,
            keep_in_place: false,
            clips: vec![clip],
        });
        let bounds = layer_bounds(&project, 500_000, &mut text);
        let quad = bounds[0].1;
        assert_eq!(quad[1][0] - quad[0][0], image.width as f32 * 1.5);
        assert_eq!(quad[3][1] - quad[0][1], image.height as f32 * 1.5);
        assert!(((quad[0][0] + quad[2][0]) / 2.0 - 648.0).abs() < 1e-4);
        let mut renderer = Renderer::new().unwrap();
        let layer = renderer
            .layer_for(
                &project,
                VisibleClip { clip: &project.tracks[1].clips[0], transition: None },
                500_000,
                0.5,
                Wait::Exact,
                false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(layer.corners, quad.map(|p| [p[0] * 0.5, p[1] * 0.5]));
        project.tracks[1].hidden = true;
        assert!(layer_bounds(&project, 500_000, &mut text).is_empty());
    }
}
