//! CPU text rasterisation with outline and background box, cached per clip and size.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use cosmic_text::{Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};

use crate::gpu::Image;
use crate::model::{TextStyle, parse_color};

pub struct TextRenderer {
    fonts: FontSystem,
    swash: SwashCache,
    cache: HashMap<u64, Image>,
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRenderer {
    pub fn new() -> Self {
        Self { fonts: FontSystem::new(), swash: SwashCache::new(), cache: HashMap::new() }
    }

    /// Renders `text` with `style` scaled by `scale` (output pixels per canvas pixel).
    /// `max_width` is the wrap width in output pixels.
    pub fn render(&mut self, text: &str, style: &TextStyle, scale: f32, max_width: f32) -> Image {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        format!("{style:?}").hash(&mut h);
        scale.to_bits().hash(&mut h);
        max_width.to_bits().hash(&mut h);
        let key = h.finish();
        if let Some(img) = self.cache.get(&key) {
            return img.clone();
        }
        if self.cache.len() > 512 {
            self.cache.clear();
        }
        let img = self.rasterize(text, style, scale, max_width);
        self.cache.insert(key, img.clone());
        img
    }

    fn rasterize(&mut self, text: &str, style: &TextStyle, scale: f32, max_width: f32) -> Image {
        // Layout stays in canvas pixels; only glyph rasterisation uses the output scale.
        let size = style.font_size.max(1.0);
        let stroke = style.stroke_width.max(0.0);
        let pad_box = if style.background.is_some() { size * 0.3 } else { 0.0 };
        let pad = (stroke.ceil() + pad_box.ceil() + 2.0) as i32;

        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(size, size * 1.2));
        let wrap = (max_width / scale - 2.0 * pad as f32).max(size);
        buffer.set_size(Some(wrap), None);
        let attrs = Attrs::new().family(Family::SansSerif).weight(if style.bold { Weight::BOLD } else { Weight::NORMAL });
        let text = if text.trim().is_empty() { " " } else { text };
        buffer.set_text(text, &attrs, Shaping::Advanced, Some(Align::Center));
        buffer.shape_until_scroll(&mut self.fonts, false);

        let mut line_w: f32 = 0.0;
        let mut text_h: f32 = 0.0;
        for run in buffer.layout_runs() {
            line_w = line_w.max(run.line_w);
            text_h = text_h.max(run.line_top + run.line_height);
        }
        // Glyphs are centred inside `wrap`, so crop to the widest line.
        let x0 = ((wrap - line_w) / 2.0).floor() as i32;
        let w = ((line_w.ceil() + 2.0 * pad as f32).max(1.0) * scale).ceil() as usize;
        let h = ((text_h.ceil() + 2.0 * pad as f32).max(1.0) * scale).ceil() as usize;

        let mut fill = vec![0u8; w * h];
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let offset = ((pad - x0) as f32 * scale, (run.line_y + pad as f32) * scale);
                let physical = glyph.physical(offset, scale);
                self.swash.with_pixels(&mut self.fonts, physical.cache_key, Color::rgb(255, 255, 255), |x, y, color| {
                    let (px, py) = (x + physical.x, y + physical.y);
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        let i = py as usize * w + px as usize;
                        fill[i] = fill[i].max(color.a());
                    }
                });
            }
        }

        let outline = if stroke > 0.0 { Some(dilate(&fill, w, h, stroke * scale)) } else { None };
        let fill_c = parse_color(&style.color);
        let stroke_c = parse_color(&style.stroke_color);
        let bg_c = style.background.as_deref().map(parse_color);

        // Premultiplied "over" compositing, converted back to straight alpha at the end.
        let mut out = vec![0u8; w * h * 4];
        let radius = size * scale * 0.25;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let mut acc = [0f32; 4];
                let mut over = |c: [f32; 4], cov: f32| {
                    let a = c[3] * cov;
                    for k in 0..3 {
                        acc[k] = c[k] * a + acc[k] * (1.0 - a);
                    }
                    acc[3] = a + acc[3] * (1.0 - a);
                };
                if let Some(bg) = bg_c {
                    over(bg, rounded_rect_coverage(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, radius));
                }
                if let Some(o) = &outline {
                    over(stroke_c, o[i] as f32 / 255.0);
                }
                over(fill_c, fill[i] as f32 / 255.0);
                if acc[3] > 0.0 {
                    let o = &mut out[i * 4..i * 4 + 4];
                    for k in 0..3 {
                        o[k] = ((acc[k] / acc[3]).clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                    o[3] = (acc[3] * 255.0).round() as u8;
                }
            }
        }
        Image { width: w as u32, height: h as u32, data: Arc::new(out) }
    }
}

/// Approximates a round brush outline by stamping the glyph mask on three rings.
fn dilate(mask: &[u8], w: usize, h: usize, radius: f32) -> Vec<u8> {
    let mut out = mask.to_vec();
    let mut offsets = Vec::new();
    for ring in [1.0, 0.66, 0.33] {
        let r = radius * ring;
        let steps = ((r * 2.0) as usize).clamp(8, 32);
        for k in 0..steps {
            let a = k as f32 / steps as f32 * std::f32::consts::TAU;
            offsets.push(((a.cos() * r).round() as i32, (a.sin() * r).round() as i32));
        }
    }
    offsets.sort_unstable();
    offsets.dedup();
    for (dx, dy) in offsets {
        for y in 0..h as i32 {
            let sy = y - dy;
            if sy < 0 || sy >= h as i32 {
                continue;
            }
            let (row, src) = (y as usize * w, sy as usize * w);
            for x in 0..w as i32 {
                let sx = x - dx;
                if sx >= 0 && sx < w as i32 {
                    let v = mask[src + sx as usize];
                    let o = &mut out[row + x as usize];
                    if v > *o {
                        *o = v;
                    }
                }
            }
        }
    }
    out
}

fn rounded_rect_coverage(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let qx = (x - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (y - h / 2.0).abs() - (h / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
    (0.5 - outside).clamp(0.0, 1.0)
}
