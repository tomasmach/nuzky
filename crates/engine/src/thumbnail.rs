//! Covers and thumbnails: one frame of the video with the person cut out of it by a mask, an outline
//! around them and text, some of it behind them. The compositor of preview and export draws them, so
//! the app, an agent's look and the saved file show the same picture.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use ff::software::scaling;
use ff::util::{color, format::Pixel, frame};
use ffmpeg_next as ff;

use crate::gpu::{Draw, Image, Layer, Mask};
use crate::media::{Transfer, init, set_sws_colorspace};
use crate::model::{Adjust, Canvas, Crop, Project, Thumbnail, TrackKind, parse_color};
use crate::render::{BACKGROUND_MAX_RADIUS, MAX_TEXT_SCALE, Quad, Renderer, WHOLE, Wait, quad, small_image, solid};

/// YouTube takes thumbnails up to 2 MB.
pub const MAX_JPEG_BYTES: usize = 2 * 1024 * 1024;

/// The project as a thumbnail's frame is taken from it: the picture as exported, without text tracks.
pub fn picture(project: &Project) -> Project {
    let mut picture = project.clone();
    picture.tracks.retain(|track| track.kind != TrackKind::Text);
    picture
}

pub struct RenderedThumbnail {
    pub width: u32,
    pub height: u32,
    /// Straight RGBA.
    pub rgba: Vec<u8>,
    /// For each text, the share of it the person covers, 0..1; always 0 for text in front of them.
    pub hidden: Vec<f32>,
    /// For each text, its corners in thumbnail pixels (top-left, top-right, bottom-right, bottom-left), so the
    /// app can select and drag it; none for a text that is not drawn.
    pub bounds: Vec<Option<[[f32; 2]; 4]>>,
    /// Where the frame is drawn in thumbnail pixels: its visible part, and the whole frame before its crop.
    pub frame_bounds: [[[f32; 2]; 4]; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
}

impl ImageKind {
    /// By the file's extension: `.png`, or `.jpg` and `.jpeg`.
    pub fn of(path: &Path) -> Option<ImageKind> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => Some(ImageKind::Png),
            "jpg" | "jpeg" => Some(ImageKind::Jpeg),
            _ => None,
        }
    }
}

impl Renderer {
    /// The thumbnail's frame at canvas resolution, as exported without text: what its mask is made from.
    pub fn thumbnail_frame(&mut self, project: &Project, thumbnail: &Thumbnail) -> Result<Image> {
        ensure!(
            thumbnail.time_us < project.duration_us(),
            "INVALID_RANGE: the thumbnail's frame at {:.2} s is past the end of the video; choose its frame again",
            thumbnail.time_us as f64 / 1e6
        );
        let (width, height) = (project.canvas.width, project.canvas.height);
        let rgba = self.render(&picture(project), thumbnail.time_us, width, height, Wait::Exact, false)?;
        Ok(Image { width, height, data: Arc::new(rgba) })
    }

    /// Draws the thumbnail `width` pixels wide from its frame and, when it needs one, the person's mask: 8-bit
    /// alpha over the frame's pixels.
    pub fn render_thumbnail(
        &mut self,
        thumbnail: &Thumbnail,
        frame: &Image,
        mask: Option<&[u8]>,
        width: u32,
    ) -> Result<RenderedThumbnail> {
        let (tw, th) = thumbnail.format.size();
        ensure!((16..=tw).contains(&width), "A thumbnail is 16 to {tw} pixels wide");
        let mask = match mask {
            Some(mask) => {
                ensure!(mask.len() == (frame.width * frame.height) as usize, "The mask does not fit the frame");
                Some(mask)
            }
            None if thumbnail.needs_mask() => bail!("This thumbnail needs the person's mask"),
            None => None,
        };
        let height = ((width as u64 * th as u64 + tw as u64 / 2) / tw as u64) as u32;
        let k = width as f32 / tw as f32;
        let (w, h) = (width as f32, height as f32);
        let canvas = Canvas {
            width: tw,
            height: th,
            fps: 30,
            background: thumbnail.background.color.clone(),
            background_blur: 0.0,
        };

        // The frame fits the thumbnail at scale 1, like media on the canvas.
        let t = thumbnail.frame;
        let fit = (tw as f32 / frame.width as f32).min(th as f32 / frame.height as f32);
        let frame_size = (frame.width as f32 * fit * t.scale * k, frame.height as f32 * fit * t.scale * k);
        let corners = quad(&t, frame_size, tw as f32, th as f32, k);
        let rect = Crop::visible(t.crop);
        // Shrunk on the CPU first, so a frame drawn much smaller than it is does not alias.
        let step = (frame.width as f32 / frame_size.0).floor().max(1.0) as usize;
        let picture = shrink(frame, step);

        let mut texts = Vec::with_capacity(thumbnail.texts.len());
        let mut bounds = Vec::with_capacity(thumbnail.texts.len());
        for text in &thumbnail.texts {
            let t = text.transform;
            if t.opacity <= 0.0 || t.scale <= 0.0 {
                texts.push(None);
                bounds.push(None);
                continue;
            }
            let style = text.style.bounded(&canvas);
            let raster = (k * t.scale).clamp(0.01, MAX_TEXT_SCALE);
            let wrap = style.max_width.unwrap_or(tw as f32 * 0.9);
            let image = self.text.render(&text.text, &style, raster, wrap * raster);
            let size = (image.image.width as f32 / image.scale, image.image.height as f32 / image.scale);
            let corners = quad(&t, (size.0 * t.scale * k, size.1 * t.scale * k), tw as f32, th as f32, k);
            bounds.push(Some(corners.map(|[x, y]| [x / k, y / k])));
            texts.push(Some(layer(image.image, corners, t.opacity, Crop::visible(t.crop))));
        }

        let mut draws = Vec::new();
        let background = &thumbnail.background;
        if background.picture {
            // The frame as placed, enlarged about the middle of its visible part until it covers the thumbnail.
            let shown = crate::render::sub_quad(&corners, rect);
            let centre = [(shown[0][0] + shown[2][0]) / 2.0, (shown[0][1] + shown[2][1]) / 2.0];
            let shown_size = (frame_size.0 * (rect[2] - rect[0]), frame_size.1 * (rect[3] - rect[1]));
            let enlarge = cover_scale(centre, shown_size, t.rotation, (w, h));
            let enlarged =
                corners.map(|[x, y]| [centre[0] + (x - centre[0]) * enlarge, centre[1] + (y - centre[1]) * enlarge]);
            let image = if background.blur > 0.0 {
                let radius = (1.0 + background.blur.clamp(0.0, 1.0) * (BACKGROUND_MAX_RADIUS - 1.0)).round() as usize;
                small_image(&picture, radius)
            } else {
                picture.clone()
            };
            draws.push(Draw::Layer(layer(image, enlarged, 1.0, rect)));
        }
        if background.dim > 0.0 {
            let mut dim = solid(&self.solids[1], width, height);
            dim.opacity = background.dim.clamp(0.0, 1.0);
            draws.push(Draw::Layer(dim));
        }
        let behind = |behind: bool| thumbnail.texts.iter().zip(&texts).filter(move |(text, _)| text.behind == behind);
        draws.extend(behind(true).filter_map(|(_, layer)| layer.clone().map(Draw::Layer)));
        let mut hidden = vec![0.0; thumbnail.texts.len()];
        match mask {
            Some(mask) => {
                let person = cut_out(&picture, &shrink_alpha(mask, frame.width, frame.height, step));
                draws.push(Draw::Layer(layer(person, corners, t.opacity, rect)));
                let coverage = (thumbnail.outline.is_some() || thumbnail.texts.iter().any(|t| t.behind))
                    .then(|| coverage(mask, (frame.width, frame.height), &corners, rect, (width, height)));
                if let (Some(outline), Some(coverage)) = (&thumbnail.outline, &coverage) {
                    let ring = ring(coverage, (width, height), outline.width * k, parse_color(&outline.color));
                    draws.push(Draw::Layer(solid(&ring, width, height)));
                }
                if let Some(coverage) = &coverage {
                    for (i, (text, layer)) in thumbnail.texts.iter().zip(&texts).enumerate() {
                        if let (true, Some(layer)) = (text.behind, layer) {
                            hidden[i] = hidden_share(layer, coverage, (width, height)) * t.opacity;
                        }
                    }
                }
            }
            None => draws.push(Draw::Layer(layer(picture, corners, t.opacity, rect))),
        }
        draws.extend(behind(false).filter_map(|(_, layer)| layer.clone().map(Draw::Layer)));
        let rgba = self.gpu.render(width, height, parse_color(&background.color), &draws)?;
        let unscaled = |q: Quad| q.map(|[x, y]| [x / k, y / k]);
        let frame_bounds = [unscaled(crate::render::sub_quad(&corners, rect)), unscaled(corners)];
        Ok(RenderedThumbnail { width, height, rgba, hidden, bounds, frame_bounds })
    }
}

/// How much the visible part of the frame, `size` output pixels around `centre` and turned by `rotation` degrees
/// clockwise, must grow about its centre to cover a `w` × `h` picture: at least 1.
fn cover_scale(centre: [f32; 2], size: (f32, f32), rotation: f32, (w, h): (f32, f32)) -> f32 {
    let (sin, cos) = rotation.to_radians().sin_cos();
    let need = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
        .into_iter()
        .map(|[x, y]| {
            // Each corner of the picture in the frame's own, unturned directions.
            let (dx, dy) = (x - centre[0], y - centre[1]);
            (2.0 * (dx * cos + dy * sin).abs() / size.0).max(2.0 * (dy * cos - dx * sin).abs() / size.1)
        })
        .fold(1.0, f32::max);
    if need.is_finite() { need.min(1e4) } else { 1.0 }
}

/// An image drawn on `corners`, showing the part `rect` (left, top, right and bottom in 0..1) of it.
fn layer(image: Image, corners: Quad, opacity: f32, rect: [f32; 4]) -> Layer {
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
        corners,
        uv_rotation: 0,
        mirror: false,
        opacity,
        adjust: Adjust::default(),
        blur: 0.0,
        clip: None,
        transfer: Transfer::Sdr,
        mask,
        matte: None,
    }
}

/// Averages `step` × `step` blocks; the GPU's bilinear filter then only has to shrink by less than two.
fn shrink(image: &Image, step: usize) -> Image {
    if step <= 1 {
        return image.clone();
    }
    let (sw, sh) = (image.width as usize, image.height as usize);
    let (w, h) = (sw.div_ceil(step), sh.div_ceil(step));
    let mut data = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            // A whole 7680 px frame shrunk to one pixel adds up to more than a u32 holds.
            let mut sum = [0u64; 4];
            let mut count = 0;
            for sy in y * step..((y + 1) * step).min(sh) {
                for sx in x * step..((x + 1) * step).min(sw) {
                    let i = (sy * sw + sx) * 4;
                    for (sum, &value) in sum.iter_mut().zip(&image.data[i..i + 4]) {
                        *sum += value as u64;
                    }
                    count += 1;
                }
            }
            data.extend(sum.map(|s| ((s + count / 2) / count) as u8));
        }
    }
    Image { width: w as u32, height: h as u32, data: Arc::new(data) }
}

/// The mask shrunk like the frame by `shrink`.
fn shrink_alpha(mask: &[u8], width: u32, height: u32, step: usize) -> Vec<u8> {
    let rgba: Vec<u8> = mask.iter().flat_map(|&a| [a, a, a, a]).collect();
    let image = shrink(&Image { width, height, data: Arc::new(rgba) }, step);
    image.data.as_chunks::<4>().0.iter().map(|p| p[3]).collect()
}

/// The frame with the mask as its alpha.
fn cut_out(frame: &Image, alpha: &[u8]) -> Image {
    let mut data = frame.data.to_vec();
    for (pixel, &a) in data.as_chunks_mut::<4>().0.iter_mut().zip(alpha) {
        pixel[3] = a;
    }
    Image { data: Arc::new(data), ..*frame }
}

/// How much of each output pixel the person covers, 0..1: the mask through the frame's placement and crop.
/// Pixels outside the visible part of the frame are -1.
fn coverage(mask: &[u8], (mw, mh): (u32, u32), corners: &Quad, rect: [f32; 4], (w, h): (u32, u32)) -> Vec<f32> {
    let [tl, tr, _, bl] = *corners;
    let (ax, ay, bx, by) = (tr[0] - tl[0], tr[1] - tl[1], bl[0] - tl[0], bl[1] - tl[1]);
    let det = ax * by - ay * bx;
    let mut out = vec![-1.0; (w * h) as usize];
    if det.abs() < 1e-6 {
        return out;
    }
    let alpha = |x: i64, y: i64| {
        if x < 0 || y < 0 || x >= mw as i64 || y >= mh as i64 {
            0.0
        } else {
            mask[(y * mw as i64 + x) as usize] as f32 / 255.0
        }
    };
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5 - tl[0], y as f32 + 0.5 - tl[1]);
            let (u, v) = ((px * by - py * bx) / det, (ax * py - ay * px) / det);
            if u < rect[0] || u >= rect[2] || v < rect[1] || v >= rect[3] {
                continue;
            }
            let (fx, fy) = (u * mw as f32 - 0.5, v * mh as f32 - 0.5);
            let (x0, y0) = (fx.floor(), fy.floor());
            let (dx, dy) = (fx - x0, fy - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let top = alpha(x0, y0) * (1.0 - dx) + alpha(x0 + 1, y0) * dx;
            let bottom = alpha(x0, y0 + 1) * (1.0 - dx) + alpha(x0 + 1, y0 + 1) * dx;
            out[(y * w + x) as usize] = top * (1.0 - dy) + bottom * dy;
        }
    }
    out
}

/// The outline: a band `width` pixels wide around the person, with a smooth outer edge, left out where the
/// person is, so it can go over them, and outside the frame, which cuts it like the person.
fn ring(coverage: &[f32], (w, h): (u32, u32), width: f32, color: [f32; 4]) -> Image {
    let distance = distance_to_person(coverage, w as usize, h as usize);
    let rgb = color.map(|c| (c * 255.0).round() as u8);
    let mut data = Vec::with_capacity(coverage.len() * 4);
    for (&d, &c) in distance.iter().zip(coverage) {
        let band = if c < 0.0 { 0.0 } else { (width + 0.5 - d).clamp(0.0, 1.0) * (1.0 - c) };
        data.extend([rgb[0], rgb[1], rgb[2], (band * color[3] * 255.0).round() as u8]);
    }
    Image { width: w, height: h, data: Arc::new(data) }
}

/// Euclidean distance in pixels from each pixel to the nearest one mostly covered by the person
/// (Felzenszwalb and Huttenlocher's transform, a column pass then a row pass).
fn distance_to_person(coverage: &[f32], w: usize, h: usize) -> Vec<f32> {
    const FAR: f32 = 1e12;
    let mut squared: Vec<f32> = coverage.iter().map(|&c| if c >= 0.5 { 0.0 } else { FAR }).collect();
    let mut line = Vec::new();
    for x in 0..w {
        line.clear();
        line.extend((0..h).map(|y| squared[y * w + x]));
        for (y, d) in transform_line(&line).into_iter().enumerate() {
            squared[y * w + x] = d;
        }
    }
    for y in 0..h {
        let row = transform_line(&squared[y * w..(y + 1) * w]);
        squared[y * w..(y + 1) * w].copy_from_slice(&row);
    }
    squared.into_iter().map(f32::sqrt).collect()
}

/// The squared distance transform of one line: for each point the least `f(q) + (p - q)²`.
fn transform_line(f: &[f32]) -> Vec<f32> {
    let n = f.len();
    let mut out = vec![0.0; n];
    // Parabolas of the lower envelope, by their vertex, and where each starts.
    let (mut vertex, mut start) = (vec![0usize; n], vec![0.0f32; n + 1]);
    let mut k = 0;
    start[0] = f32::NEG_INFINITY;
    start[1] = f32::INFINITY;
    let cross =
        |q: usize, p: usize| ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
    for q in 1..n {
        let mut s = cross(q, vertex[k]);
        while s <= start[k] {
            k -= 1;
            s = cross(q, vertex[k]);
        }
        k += 1;
        vertex[k] = q;
        start[k] = s;
        start[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, out) in out.iter_mut().enumerate() {
        while start[k + 1] < q as f32 {
            k += 1;
        }
        let d = q as f32 - vertex[k] as f32;
        *out = d * d + f[vertex[k]];
    }
    out
}

/// The share of a text layer's ink, by alpha, over pixels the person covers.
fn hidden_share(layer: &Layer, coverage: &[f32], (w, h): (u32, u32)) -> f32 {
    let image = &layer.image;
    let [tl, tr, _, bl] = layer.corners;
    let rect = layer.mask.map_or(WHOLE, |mask| mask.rect);
    let (mut ink, mut covered) = (0.0f64, 0.0f64);
    for y in 0..image.height {
        let v = (y as f32 + 0.5) / image.height as f32;
        for x in 0..image.width {
            let a = image.data[((y * image.width + x) * 4 + 3) as usize];
            let u = (x as f32 + 0.5) / image.width as f32;
            if a == 0 || u < rect[0] || u >= rect[2] || v < rect[1] || v >= rect[3] {
                continue;
            }
            let a = a as f64 / 255.0;
            ink += a;
            let px = tl[0] + u * (tr[0] - tl[0]) + v * (bl[0] - tl[0]);
            let py = tl[1] + u * (tr[1] - tl[1]) + v * (bl[1] - tl[1]);
            if px >= 0.0 && py >= 0.0 && (px as u32) < w && (py as u32) < h {
                covered += a * coverage[(py as u32 * w + px as u32) as usize].max(0.0) as f64;
            }
        }
    }
    if ink > 0.0 { (covered / ink) as f32 } else { 0.0 }
}

/// PNG, or JPEG at the best quality that stays under `MAX_JPEG_BYTES`.
pub fn encode(rendered: &RenderedThumbnail, kind: ImageKind) -> Result<Vec<u8>> {
    let (w, h) = (rendered.width, rendered.height);
    match kind {
        ImageKind::Png => {
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(&mut bytes, w, h);
            encoder.set_color(png::ColorType::Rgba);
            encoder.write_header()?.write_image_data(&rendered.rgba).context("Encoding the PNG")?;
            Ok(bytes)
        }
        ImageKind::Jpeg => {
            for quality in [2, 3, 4, 6, 9, 13, 20, 31] {
                let bytes = jpeg(&rendered.rgba, w, h, quality)?;
                if bytes.len() <= MAX_JPEG_BYTES {
                    return Ok(bytes);
                }
            }
            bail!("The JPEG stays over 2 MB even at the lowest quality")
        }
    }
}

/// FFmpeg's own JPEG encoder at `quality` (2 best, 31 smallest), full-range BT.601 as JPEG files are read.
fn jpeg(rgba: &[u8], w: u32, h: u32, quality: i32) -> Result<Vec<u8>> {
    init();
    let codec =
        ff::encoder::find(ff::codec::Id::MJPEG).ok_or_else(|| anyhow!("This FFmpeg build has no JPEG encoder"))?;
    let mut encoder = ff::codec::context::Context::new_with_codec(codec).encoder().video()?;
    encoder.set_width(w);
    encoder.set_height(h);
    encoder.set_format(Pixel::YUV420P);
    encoder.set_color_range(color::Range::JPEG);
    encoder.set_colorspace(color::Space::BT470BG);
    encoder.set_time_base((1, 1));
    encoder.set_flags(ff::codec::Flags::QSCALE);
    encoder.set_global_quality(quality * ff::ffi::FF_QP2LAMBDA);
    let mut encoder = encoder.open().context("Opening the JPEG encoder")?;
    let mut source = frame::Video::new(Pixel::RGBA, w, h);
    let stride = source.stride(0);
    let row = w as usize * 4;
    for y in 0..h as usize {
        source.data_mut(0)[y * stride..y * stride + row].copy_from_slice(&rgba[y * row..(y + 1) * row]);
    }
    let mut scaler = scaling::Context::get(Pixel::RGBA, w, h, Pixel::YUV420P, w, h, scaling::Flags::BICUBIC)?;
    set_sws_colorspace(&mut scaler, ff::ffi::SWS_CS_DEFAULT, true, ff::ffi::SWS_CS_ITU601, true);
    let mut yuv = frame::Video::new(Pixel::YUV420P, w, h);
    scaler.run(&source, &mut yuv)?;
    yuv.set_color_range(color::Range::JPEG);
    yuv.set_pts(Some(0));
    // With a fixed quality the encoder takes each frame's own.
    unsafe { (*yuv.as_mut_ptr()).quality = quality * ff::ffi::FF_QP2LAMBDA };
    encoder.send_frame(&yuv)?;
    encoder.send_eof()?;
    let mut packet = ff::Packet::empty();
    encoder.receive_packet(&mut packet).context("Encoding the JPEG")?;
    Ok(packet.data().context("The JPEG encoder wrote nothing")?.to_vec())
}

/// Writes `bytes` beside `out` as a `.nuzky-part-*` file and moves it into place, refusing an existing file
/// unless `replace_existing`.
pub fn save(out: &Path, bytes: &[u8], replace_existing: bool, cancel: &AtomicBool) -> Result<()> {
    let tmp = out.with_file_name(format!(
        ".nuzky-part-{}.{}",
        uuid::Uuid::new_v4(),
        match ImageKind::of(out) {
            Some(ImageKind::Jpeg) => "jpg",
            _ => "png",
        }
    ));
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut file| std::io::Write::write_all(&mut file, bytes).and_then(|()| file.sync_all()))
        .with_context(|| format!("Cannot write beside {}", out.display()))
        .and_then(|()| crate::export::publish(&tmp, out, replace_existing, cancel));
    if result.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_background_grows_until_it_covers_the_thumbnail_also_when_turned() {
        let centre = [540.0, 960.0];
        assert_eq!(cover_scale(centre, (1080.0, 1920.0), 0.0, (1080.0, 1920.0)), 1.0);
        // Turned a quarter, the 9:16 frame lies across the 9:16 cover and must grow by 16:9 to fill it.
        assert!((cover_scale(centre, (1080.0, 1920.0), 90.0, (1080.0, 1920.0)) - 1920.0 / 1080.0).abs() < 1e-4);
        // A vertical frame on the left third of a YouTube thumbnail reaches its right edge.
        let third = cover_scale([427.0, 360.0], (405.0, 720.0), 0.0, (1280.0, 720.0));
        assert!((third - 2.0 * (1280.0 - 427.0) / 405.0).abs() < 1e-4);
        assert_eq!(cover_scale(centre, (0.0, 0.0), 0.0, (1080.0, 1920.0)), 1.0);
    }

    #[test]
    fn shrinking_a_whole_large_frame_to_one_pixel_keeps_its_average() {
        let (w, h) = (4200, 4200);
        let image = Image { width: w, height: h, data: Arc::new([200u8, 100, 50, 255].repeat((w * h) as usize)) };
        assert_eq!(*shrink(&image, usize::MAX).data, [200, 100, 50, 255]);
    }

    #[test]
    fn distance_to_the_person_is_euclidean() {
        let (w, h) = (23, 17);
        let person = [(3, 4), (15, 2), (20, 16), (9, 9)];
        let mut coverage = vec![0.0; w * h];
        for &(x, y) in &person {
            coverage[y * w + x] = 1.0;
        }
        let distance = distance_to_person(&coverage, w, h);
        for y in 0..h {
            for x in 0..w {
                let nearest = person
                    .iter()
                    .map(|&(px, py)| ((x as f32 - px as f32).powi(2) + (y as f32 - py as f32).powi(2)).sqrt())
                    .fold(f32::INFINITY, f32::min);
                assert!((distance[y * w + x] - nearest).abs() < 1e-3, "{x},{y}: {} {nearest}", distance[y * w + x]);
            }
        }
    }
}
