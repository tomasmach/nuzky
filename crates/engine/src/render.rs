//! Turns a project and a time into a composited frame. Preview and export both use this.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::effects::{max_animation_scale, source_time, transform_at, transition_at, transition_window};
use crate::gpu::{Draw, Gpu, Image, Layer, Mask};
use crate::media::{Transfer, decode_size};
use crate::model::{
    Adjust, Asset, AssetKind, Clip, ClipContent, Crop, MAX_BORDER_WIDTH, Project, Track, TrackKind, Transform,
    TransitionKind, parse_color, spoken_word,
};
use crate::text::{TextRenderer, TextStats};
use crate::worker::VideoWorker;

const BACKGROUND_SIZE: f32 = 96.0;
/// Samples per block side when shrinking a frame for the background blur.
const BLOCK_SAMPLES: usize = 4;
const BACKGROUND_BLUR_PASSES: usize = 3;
pub(crate) const BACKGROUND_MAX_RADIUS: f32 = 9.0;
const BACKGROUND_BRIGHTNESS: f32 = 0.85;
pub(crate) const MAX_TEXT_SCALE: f32 = 8.0;
const PREFETCH_US: i64 = 1_000_000;
const IDLE_WORKER: Duration = Duration::from_secs(5);
/// Blur of a layer's shadow, as a share of the canvas's shorter side.
const SHADOW_BLUR: f32 = 0.025;
pub(crate) const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

pub(crate) type Quad = [[f32; 2]; 4];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wait {
    /// Use whatever frames are ready (playback).
    Ready,
    /// Wait for the exact frame of every layer (paused preview, export).
    Exact,
}

pub struct Renderer {
    pub(crate) gpu: Gpu,
    pub(crate) text: TextRenderer,
    /// Keyed by clip and source path, so a clip relinked to other media gets a fresh decoder.
    workers: HashMap<(String, String), VideoWorker>,
    /// Clips shown now or prefetched for the next second, whose decoders other clips must not take.
    needed: HashSet<String>,
    blurred: HashMap<(usize, usize), (Image, Image)>,
    pub(crate) solids: [Image; 3],
    /// The cache to find preview proxies in; `None` decodes the originals, as export must.
    proxies: Option<PathBuf>,
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

/// Video clips that start within the prefetch window after `t_us`.
fn upcoming(track: &Track, t_us: i64) -> impl Iterator<Item = &Clip> {
    let video = !track.hidden && track.kind == TrackKind::Video;
    track.clips.iter().filter(move |clip| {
        let start = if track.id == crate::edit::MAIN_TRACK {
            transition_window(clip).map(|w| w.0).unwrap_or(clip.start_us)
        } else {
            clip.start_us
        };
        video && start > t_us && start <= t_us + PREFETCH_US
    })
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
        ClipContent::Text { text, style, words, .. } => {
            let style = style.bounded(canvas);
            let count = (text.chars().count() as f32 * reveal).ceil() as usize;
            let end = text.char_indices().nth(count).map(|(i, _)| i).unwrap_or(text.len());
            let zoom = match visible.transition {
                Some(TransitionRole { kind: TransitionKind::ZoomIn, progress, incoming: false }) => 1.0 + progress,
                _ => 1.0,
            };
            let scale = 2.0_f32.powf((k * transform.scale * zoom).clamp(1.0, MAX_TEXT_SCALE).log2().ceil());
            let wrap = style.max_width.unwrap_or(canvas.width as f32 * 0.9);
            // Karaoke: the word being said, by its place in the whole text, which a typewriter reveals from the start.
            let spoken = style.highlight.as_ref().and_then(|_| spoken_word(text, words, t_us - clip.start_us));
            let text = text_renderer.render_spoken(&text[..end], &style, scale, wrap * scale, spoken);
            ((text.image.width as f32 / text.scale, text.image.height as f32 / text.scale), Some(text.image))
        }
    };
    Some(Placement { transform, size, text })
}

/// Layers visible at `t_us`, bottom to top: the clip, the corners of its visible part and of the whole
/// layer before the crop, in canvas pixels.
pub fn layer_bounds(project: &Project, t_us: i64, text: &mut TextRenderer) -> Vec<(String, Quad, Quad)> {
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
        let shown = sub_quad(&corners, Crop::visible(place.transform.crop));
        if opacity > 0.0 && intersects_rect(&shown, rect) {
            bounds.push((visible.clip.id.clone(), shown, corners));
        }
    }
    bounds
}

/// The part of a quad between the edges `rect` (left, top, right and bottom in 0..1).
pub(crate) fn sub_quad(q: &Quad, rect: [f32; 4]) -> Quad {
    if rect == WHOLE {
        return *q;
    }
    let at = |u: f32, v: f32| std::array::from_fn(|i| q[0][i] + u * (q[1][i] - q[0][i]) + v * (q[3][i] - q[0][i]));
    let [left, top, right, bottom] = rect;
    [at(left, top), at(right, top), at(right, bottom), at(left, bottom)]
}

/// The crop and edge of a layer, or `None` when it shows whole with plain edges.
fn layer_mask(project: &Project, clip: &Clip, transform: &Transform, k: f32) -> Option<Mask> {
    let shape = match &clip.content {
        ClipContent::Media { shape, .. } => shape.clone().unwrap_or_default(),
        ClipContent::Text { .. } => Default::default(),
    };
    let rect = Crop::visible(transform.crop);
    if rect == WHOLE && !(shape.radius > 0.0 || shape.border_width > 0.0 || shape.shadow > 0.0) {
        return None;
    }
    let canvas = &project.canvas;
    // Older project files can bypass session validation when rendered by the CLI.
    let bounded = |v: f32, max: f32| if v.is_finite() { v.clamp(0.0, max) } else { 0.0 };
    Some(Mask {
        rect,
        radius: bounded(shape.radius, 1.0),
        border: bounded(shape.border_width, MAX_BORDER_WIDTH) * k,
        border_color: parse_color(&shape.border_color),
        shadow: bounded(shape.shadow, 1.0),
        shadow_blur: SHADOW_BLUR * canvas.width.min(canvas.height) as f32 * k,
    })
}

fn intersects_rect(corners: &Quad, rect: [f32; 4]) -> bool {
    let [x0, y0, x1, y1] = rect;
    if x0 >= x1 || y0 >= y1 {
        return false;
    }
    let rectangle = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let edge = |i: usize, j: usize| [corners[i][1] - corners[j][1], corners[j][0] - corners[i][0]];
    // Separating axes also reject rotated quads whose bounding box touches the wipe.
    [[1.0, 0.0], [0.0, 1.0], edge(0, 1), edge(1, 2)].into_iter().all(|axis| {
        let range = |points: &Quad| {
            points
                .iter()
                .map(|p| p[0] * axis[0] + p[1] * axis[1])
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| (lo.min(p), hi.max(p)))
        };
        let (a, b) = (range(corners), range(&rectangle));
        a.0 < b.1 && b.0 < a.1
    })
}

fn placement_quad(project: &Project, place: &Placement, k: f32) -> Quad {
    let scale = place.transform.scale * k;
    quad(
        &place.transform,
        (place.size.0 * scale, place.size.1 * scale),
        project.canvas.width as f32,
        project.canvas.height as f32,
        k,
    )
}

fn decode_resolution(project: &Project, clip: &Clip, asset: &Asset, k: f32, max_side: u32) -> (u32, u32) {
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
    let display = (asset.width as f32 * fit * scale * k, asset.height as f32 * fit * scale * k);
    decode_size(src, asset.rotation, display, max_side)
}

impl Renderer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            gpu: Gpu::new()?,
            text: TextRenderer::new(),
            workers: HashMap::new(),
            needed: HashSet::new(),
            blurred: HashMap::new(),
            solids: [[0, 0, 0, 0], [0, 0, 0, 255], [255; 4]].map(|c| Image {
                width: 1,
                height: 1,
                data: Arc::new(c.to_vec()),
            }),
            proxies: None,
            late_layers: 0,
        })
    }

    /// Decodes each file's preview proxy (`crate::proxy`) from `cache_dir` once it is made. For the preview
    /// only: export keeps a renderer without, so it reads the originals.
    pub fn use_proxies(&mut self, cache_dir: PathBuf) {
        self.proxies = Some(cache_dir);
    }

    pub fn adapter_name(&self) -> &str {
        &self.gpu.adapter_name
    }

    /// Live video decoder threads, for diagnostics.
    pub fn decoders(&self) -> usize {
        self.workers.len()
    }

    /// Text layouts and paints so far, for diagnostics.
    pub fn text_stats(&self) -> TextStats {
        self.text.stats()
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
        self.drop_stale_workers(project);
        self.needed.clear();
        for track in &project.tracks {
            let visible = visible_clips(track, t_us).map(|v| v.clip);
            self.needed.extend(visible.chain(upcoming(track, t_us)).map(|clip| clip.id.clone()));
        }
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
        self.blurred.retain(|key, _| blur_used.contains(key));
        self.gpu.render(out_w, out_h, parse_color(&canvas.background), &draws)
    }

    /// Decoders of removed or relinked clips go at once, others after idling. Runs before drawing,
    /// so a frame that fails on unreadable media still releases them.
    fn drop_stale_workers(&mut self, project: &Project) {
        if self.workers.is_empty() {
            return;
        }
        let current: HashSet<(&str, &str)> = project
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter_map(|c| match &c.content {
                ClipContent::Media { asset_id, .. } => Some((c.id.as_str(), project.asset(asset_id)?.path.as_str())),
                ClipContent::Text { .. } => None,
            })
            .collect();
        self.workers.retain(|(clip_id, path), w| {
            w.last_used.elapsed() < IDLE_WORKER && current.contains(&(clip_id.as_str(), path.as_str()))
        });
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
        // A cropped layer's visible part covers the canvas, so nothing cut off shows behind it.
        let rect = layer.mask.map_or(WHOLE, |mask| mask.rect);
        let (sw, sh) = (iw as f32 * (rect[2] - rect[0]), ih as f32 * (rect[3] - rect[1]));
        let cover = (w as f32 / sw).max(h as f32 / sh);
        let size = (iw as f32 * cover, ih as f32 * cover);
        // The whole picture moves so the middle of the visible part sits in the middle of the canvas.
        let offset = Transform {
            x: (0.5 - (rect[0] + rect[2]) / 2.0) * size.0 / w as f32,
            y: (0.5 - (rect[1] + rect[3]) / 2.0) * size.1 / h as f32,
            ..Transform::default()
        };
        let mask = (rect != WHOLE).then_some(Mask {
            rect,
            radius: 0.0,
            border: 0.0,
            border_color: [0.0; 4],
            shadow: 0.0,
            shadow_blur: 0.0,
        });
        Layer {
            image,
            corners: quad(&offset, size, w as f32, h as f32, 1.0),
            uv_rotation: layer.uv_rotation,
            mirror: layer.mirror,
            opacity: layer.opacity,
            adjust: layer.adjust,
            blur: 0.0,
            clip: layer.clip,
            transfer: layer.transfer,
            mask,
        }
    }

    fn prefetch(&mut self, project: &Project, t_us: i64, k: f32) {
        for clip in project.tracks.iter().flat_map(|track| upcoming(track, t_us)) {
            let ClipContent::Media { asset_id, .. } = &clip.content else { continue };
            let Some(asset) =
                project.asset(asset_id).filter(|a| a.kind != AssetKind::Audio && a.width > 0 && a.height > 0)
            else {
                continue;
            };
            let size = decode_resolution(project, clip, asset, k, self.gpu.max_texture_dimension());
            self.worker(&clip.id, &asset.path, false).get(source_time(clip, clip.start_us), size, false, false);
        }
    }

    /// The clip's decoder. A clip without one takes over the most recently used decoder of the same
    /// file that no needed clip uses, so a file cut into many pieces does not open one per piece,
    /// and the next piece usually continues where the previous one stopped.
    /// A decoder reads the file's proxy once there is one. One made before switches only when `switch`
    /// (a paused frame): a new decoder has no frame ready at once, so playback would miss the clip.
    fn worker(&mut self, clip_id: &str, path: &str, switch: bool) -> &mut VideoWorker {
        let key = (clip_id.to_owned(), path.to_owned());
        let proxy = self.proxies.as_deref().and_then(|dir| crate::proxy::ready(dir, Path::new(path)));
        let spawn = |proxy: Option<PathBuf>| match proxy {
            Some(proxy) => VideoWorker::spawn_proxy(proxy),
            None => VideoWorker::spawn(PathBuf::from(path)),
        };
        if !self.workers.contains_key(&key) {
            let idle = self
                .workers
                .iter()
                .filter(|((id, p), _)| p == path && !self.needed.contains(id))
                .max_by_key(|(_, w)| w.last_used)
                .map(|(k, _)| k.clone());
            let worker = match idle {
                Some(idle) => self.workers.remove(&idle).unwrap(),
                None => spawn(proxy.clone()),
            };
            self.workers.insert(key.clone(), worker);
        }
        let worker = self.workers.get_mut(&key).unwrap();
        if switch && worker.proxy != proxy {
            let old = std::mem::replace(worker, spawn(proxy));
            // Its thread may be in the middle of a long seek in the original; the frame does not wait for it.
            std::thread::Builder::new().name("video-decode-end".into()).spawn(move || drop(old)).ok();
        }
        worker
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
        let (image, (rotation, mirror), adjust, transfer) = match &clip.content {
            ClipContent::Media { asset_id, adjust, .. } => {
                let Some(asset) = project.asset(asset_id) else { return Ok(None) };
                // A stable conversion size avoids flushing the decoder queue on every animation frame.
                let size = decode_resolution(project, clip, asset, k, self.gpu.max_texture_dimension());
                let source_t = if asset.kind == AssetKind::Image {
                    0
                } else {
                    source_time(clip, t_us).min((asset.duration_us - 1).max(0))
                };
                let worker = self.worker(&clip.id, &asset.path, wait == Wait::Exact);
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
                let image = Image { width: frame.width, height: frame.height, data: frame.data };
                (image, (asset.rotation, asset.mirror), *adjust, frame.transfer)
            }
            ClipContent::Text { .. } => {
                let Some(text) = place.text else { return Ok(None) };
                (text, (0, false), Adjust::default(), Transfer::Sdr)
            }
        };
        Ok(Some(Layer {
            image,
            corners,
            uv_rotation: rotation,
            mirror,
            opacity: place.transform.opacity,
            adjust,
            blur: 0.0,
            clip: None,
            transfer,
            mask: layer_mask(project, clip, &place.transform, k),
        }))
    }
}

fn transition_geometry(
    mut corners: Quad,
    opacity: f32,
    kind: TransitionKind,
    p: f32,
    incoming: bool,
    w: u32,
    h: u32,
) -> (Quad, f32, Option<[f32; 4]>) {
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

pub(crate) fn solid(image: &Image, w: u32, h: u32) -> Layer {
    Layer {
        image: image.clone(),
        corners: [[0.0, 0.0], [w as f32, 0.0], [w as f32, h as f32], [0.0, h as f32]],
        uv_rotation: 0,
        mirror: false,
        opacity: 1.0,
        adjust: Adjust::default(),
        blur: 0.0,
        clip: None,
        transfer: Transfer::Sdr,
        mask: None,
    }
}

pub(crate) fn small_image(image: &Image, radius: usize) -> Image {
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
pub(crate) fn quad(t: &Transform, size: (f32, f32), cw: f32, ch: f32, k: f32) -> Quad {
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
        assert!(blurred.data.as_chunks::<4>().0.iter().all(|px| *px == [108, 108, 108, 255]));
        let transparent = Image { width: 2, height: 1, data: Arc::new(vec![255, 0, 0, 0, 0, 255, 0, 255]) };
        let blurred = small_image(&transparent, 3);
        assert!(blurred.data.as_chunks::<4>().0.iter().all(|px| px[0] == 0 && px[1] == 217 && px[2] == 0 && px[3] > 0));
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
            highlight: None,
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
                    words: Vec::new(),
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
                    if let Some((_, expected, _)) = bounds.iter().find(|(id, ..)| *id == visible.clip.id) {
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
    fn large_text_keeps_its_size_at_export_resolutions() {
        let style = TextStyle {
            font_family: None,
            font_size: 300.0,
            stroke_width: 0.0,
            background: None,
            color: "#ffffff".into(),
            bold: false,
            stroke_color: "#000000".into(),
            max_width: None,
            highlight: None,
        };
        let mut text = TextRenderer::new();
        // One line hits the glyph limit at 2160p, two lines the bitmap pixel limit.
        for title in ["A", "BIG\nTITLE"] {
            let mut project = Project::new("large text");
            let transform = Transform { scale: 3.0, ..Transform::default() };
            project.tracks[0].clips.push(Clip::new(
                "big".into(),
                0,
                1_000_000,
                ClipContent::Text { text: title.into(), style: style.clone(), transform, words: Vec::new() },
            ));
            let visible = visible_clips(&project.tracks[0], 0).next().unwrap();
            let base = placement(&project, visible, 0, 1.0, &mut text).unwrap();
            let export = placement(&project, visible, 0, 2.0, &mut text).unwrap();
            assert!(
                (export.size.0 - base.size.0).abs() <= 1.0 && (export.size.1 - base.size.1).abs() <= 1.0,
                "{title}: {:?} vs {:?}",
                export.size,
                base.size
            );
        }
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
            mirror: false,
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
                    keep_pitch: false,
                    volume: 1.0,
                    transform: Transform { x, scale: 0.2, rotation: 25.0, ..Transform::default() },
                    adjust: Default::default(),
                    fade_in_us: 0,
                    fade_out_us: 0,
                    clean_voice: false,
                    shape: None,
                    duck_db: 0.0,
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
            highlight: None,
        };
        // The longest reel caption, 15 characters, is wider than the safe area at 95 px and wraps.
        let segment =
            CaptionSegment { start_us: 0, end_us: 1_000_000, text: "Největší rozdíl".into(), words: Vec::new() };
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
    fn scaled_white_text_over_white_has_no_dark_fringe() {
        let mut project = Project::new("fringe");
        (project.canvas.width, project.canvas.height) = (640, 360);
        project.canvas.background = "#ffffff".into();
        let style = TextStyle {
            font_family: None,
            font_size: 48.0,
            color: "#ffffff".into(),
            bold: false,
            stroke_width: 0.0,
            stroke_color: "#000000".into(),
            background: None,
            max_width: None,
            highlight: None,
        };
        // Rasterised at 2x and drawn at 1.3x, so every glyph edge is filtered down.
        let transform = Transform { scale: 1.3, rotation: 7.0, ..Transform::default() };
        project.tracks[0].clips.push(Clip::new(
            "text".into(),
            0,
            1_000_000,
            ClipContent::Text { text: "Soft edges".into(), style, transform, words: Vec::new() },
        ));
        let frame = Renderer::new().unwrap().render(&project, 0, 640, 360, Wait::Exact, false).unwrap();
        let darkest = frame.as_chunks::<4>().0.iter().map(|p| p[0].min(p[1]).min(p[2])).min().unwrap();
        assert!(darkest >= 253, "darkest pixel {darkest}");
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
            highlight: None,
        };
        let image = text.render("Ahoj světe", &style, 1.0, 972.0).image;
        let clip = Clip::new(
            "text".into(),
            0,
            1_000_000,
            ClipContent::Text {
                text: "Ahoj světe".into(),
                style,
                transform: Transform { scale: 1.5, x: 0.1, ..Transform::default() },
                words: Vec::new(),
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

    const KARAOKE_START: i64 = 1_000_000;
    const KARAOKE_SIZE: (u32, u32) = (540, 960);

    /// A Reel caption of three Czech words: "Kůň" 0.1–0.3 s, "úpěl" 0.4–0.7 s, "ódy" 0.7–1 s of the clip,
    /// which starts at 1 s and holds until 1.5 s.
    fn karaoke_project(highlight: Option<&str>) -> Project {
        use crate::model::CaptionWord;
        let mut project = Project::new("karaoke");
        let style = TextStyle {
            font_family: None,
            font_size: 95.0,
            color: "#ffffff".into(),
            bold: false,
            stroke_width: 7.5,
            stroke_color: "#000000".into(),
            background: None,
            max_width: None,
            highlight: highlight.map(Into::into),
        };
        let word = |text: &str, start: i64, end: i64| CaptionWord {
            text: text.into(),
            start_us: KARAOKE_START + start,
            end_us: KARAOKE_START + end,
        };
        let segment = CaptionSegment {
            start_us: KARAOKE_START,
            end_us: KARAOKE_START + 1_500_000,
            text: "Kůň úpěl ódy".into(),
            words: vec![word("Kůň", 100_000, 300_000), word("úpěl", 400_000, 700_000), word("ódy", 700_000, 1_000_000)],
        };
        project.apply(EditCmd::AddCaptions { segments: vec![segment], style }).unwrap();
        project
    }

    /// Highlight yellow (#ffe14d, also where it fades into the black outline) and white text, by pixel index.
    fn karaoke_pixels(frame: &[u8]) -> (Vec<usize>, Vec<usize>) {
        let (mut yellow, mut white) = (Vec::new(), Vec::new());
        for (i, p) in frame.as_chunks::<4>().0.iter().enumerate() {
            let [r, g, b] = [p[0], p[1], p[2]].map(f32::from);
            if r > 120.0 && g > 0.75 * r && b < 0.5 * r {
                yellow.push(i);
            } else if r > 120.0 && b > 0.9 * r {
                white.push(i);
            }
        }
        (yellow, white)
    }

    /// Smallest box around the pixels: left, top, right, bottom, inclusive.
    fn pixel_box(pixels: &[usize]) -> Option<[usize; 4]> {
        let w = KARAOKE_SIZE.0 as usize;
        pixels.iter().fold(None, |acc, &i| {
            let (x, y) = (i % w, i / w);
            Some(match acc {
                None => [x, y, x, y],
                Some([l, t, r, b]) => [l.min(x), t.min(y), r.max(x), b.max(y)],
            })
        })
    }

    fn inside(i: usize, [l, t, r, b]: [usize; 4]) -> bool {
        let w = KARAOKE_SIZE.0 as usize;
        (l..=r).contains(&(i % w)) && (t..=b).contains(&(i / w))
    }

    #[test]
    fn karaoke_highlights_only_the_word_being_said() {
        let project = karaoke_project(Some("#ffe14d"));
        let mut renderer = Renderer::new().unwrap();
        let (w, h) = KARAOKE_SIZE;
        let mut frame = |t: i64| renderer.render(&project, KARAOKE_START + t, w, h, Wait::Exact, false).unwrap();
        // The middle of each word: only that word is yellow, all of it, accents included.
        let (gap_yellow, gap_white) = karaoke_pixels(&frame(350_000));
        assert!(gap_yellow.is_empty() && gap_white.len() > 500, "{} {}", gap_yellow.len(), gap_white.len());
        let mut boxes = Vec::new();
        for (word, t) in ["Kůň", "úpěl", "ódy"].into_iter().zip([200_000, 550_000, 850_000]) {
            let (yellow, white) = karaoke_pixels(&frame(t));
            let Some(area) = pixel_box(&yellow) else { panic!("{word}: nothing highlighted") };
            // Every white pixel of the word in the gap frame is yellow now, the diacritics too.
            let unlit = white.iter().filter(|&&i| inside(i, area)).count();
            assert_eq!(unlit, 0, "{word}: white left inside its box {area:?}");
            let lit_text = gap_white.iter().filter(|&&i| inside(i, area)).count();
            assert!(lit_text > 50, "{word}: box {area:?} holds {lit_text} text pixels");
            boxes.push((word, area, yellow));
        }
        // Each word's yellow lies in its own box and in no other word's; the words read left to right.
        for (word, area, yellow) in &boxes {
            for (other, other_area, _) in boxes.iter().filter(|(other, ..)| other != word) {
                let stray = yellow.iter().filter(|&&i| inside(i, *other_area)).count();
                assert_eq!(stray, 0, "{word} lit {stray} pixels in the box of {other} {other_area:?} (own {area:?})");
            }
        }
        assert!(
            boxes.windows(2).all(|pair| pair[0].1[2] < pair[1].1[0]),
            "{:?}",
            boxes.iter().map(|b| b.1).collect::<Vec<_>>()
        );
        // Exactly at a word's end nothing is lit; exactly at the next word's start only that word is.
        assert!(karaoke_pixels(&frame(300_000)).0.is_empty(), "end of Kůň");
        let (at_start, _) = karaoke_pixels(&frame(700_000));
        assert!(!at_start.is_empty() && at_start.iter().all(|&i| inside(i, boxes[2].1)), "start of ódy");
        // Before the first word and while the caption holds after the last one, nothing is.
        for t in [0, 99_999, 1_000_000, 1_200_000, 1_499_999] {
            assert!(karaoke_pixels(&frame(t)).0.is_empty(), "{t}");
        }
    }

    #[test]
    fn karaoke_without_a_word_being_said_draws_like_plain_text() {
        let (w, h) = KARAOKE_SIZE;
        let mut renderer = Renderer::new().unwrap();
        let plain = karaoke_project(None);
        let karaoke = karaoke_project(Some("#ffe14d"));
        let mut words_without_highlight = plain.clone();
        // Plain text keeps its words: without a highlight they change nothing, mid-word either.
        for t in [350_000, 200_000] {
            let before = renderer.render(&plain, KARAOKE_START + t, w, h, Wait::Exact, false).unwrap();
            let ClipContent::Text { words, .. } = &mut words_without_highlight.tracks[1].clips[0].content else {
                panic!()
            };
            words.clear();
            let without =
                renderer.render(&words_without_highlight, KARAOKE_START + t, w, h, Wait::Exact, false).unwrap();
            assert!(before == without, "{t}");
        }
        // In a gap the karaoke caption is the plain one, pixel for pixel.
        let gap = |project: &Project, renderer: &mut Renderer| {
            renderer.render(project, KARAOKE_START + 350_000, w, h, Wait::Exact, false).unwrap()
        };
        assert!(gap(&karaoke, &mut renderer) == gap(&plain, &mut Renderer::new().unwrap()));
    }

    #[test]
    fn karaoke_turns_off_when_the_text_gains_or_loses_words() {
        let (w, h) = KARAOKE_SIZE;
        let mut renderer = Renderer::new().unwrap();
        for text in ["Kůň úpěl", "Kůň úpěl ódy navíc", "Kůň úpěl  ódy", "Kůň\núpěl ódy", " Kůň úpěl ódy", ""]
        {
            let mut project = karaoke_project(Some("#ffe14d"));
            let id = project.tracks[1].clips[0].id.clone();
            let edit = serde_json::from_value(serde_json::json!({"type": "updateClip", "clipId": id, "text": text}));
            project.apply(edit.unwrap()).unwrap();
            for t in [200_000, 550_000, 850_000] {
                let frame = renderer.render(&project, KARAOKE_START + t, w, h, Wait::Exact, false).unwrap();
                assert!(karaoke_pixels(&frame).0.is_empty(), "{text:?} at {t}");
            }
        }
    }

    #[test]
    fn karaoke_lights_a_word_corrected_in_place() {
        let (w, h) = KARAOKE_SIZE;
        let mut renderer = Renderer::new().unwrap();
        let mut project = karaoke_project(Some("#ffe14d"));
        let id = project.tracks[1].clips[0].id.clone();
        // A recognition mistake fixed word for word: "úpěl" becomes the longer "úpěla".
        let edit = serde_json::json!({"type": "updateClip", "clipId": id, "text": "Kůň úpěla ódy"});
        project.apply(serde_json::from_value(edit).unwrap()).unwrap();
        let mut lit = |t: i64| {
            let frame = renderer.render(&project, KARAOKE_START + t, w, h, Wait::Exact, false).unwrap();
            pixel_box(&karaoke_pixels(&frame).0)
        };
        let (first, corrected, last) = (lit(200_000), lit(550_000), lit(850_000));
        let [first, corrected, last] = [first, corrected, last].map(|b| b.expect("every word lights up"));
        assert!(first[2] < corrected[0] && corrected[2] < last[0], "{first:?} {corrected:?} {last:?}");
        assert!(lit(350_000).is_none(), "the gap stays unlit");
    }

    #[test]
    fn karaoke_lays_a_caption_out_once_and_paints_each_word_once() {
        let project = karaoke_project(Some("#ffe14d"));
        let mut text = TextRenderer::new();
        // Every frame of the caption at 30 fps and at 60 fps, twice: playback, scrubbing back and export.
        for _ in 0..2 {
            for t in (KARAOKE_START..KARAOKE_START + 1_500_000).step_by(16_667) {
                for visible in visible_clips(&project.tracks[1], t) {
                    placement(&project, visible, t, 1.0, &mut text).unwrap();
                }
            }
        }
        // One layout, and one image without a word plus one for each of the three words.
        assert_eq!(text.stats(), crate::text::TextStats { layouts: 1, paints: 4 });
    }
}
