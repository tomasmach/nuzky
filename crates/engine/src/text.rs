//! CPU text rasterisation with outline and background box, cached by text content, style and output size.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use cosmic_text::{Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};

use crate::gpu::Image;
use crate::model::{TextStyle, max_stroke_width, parse_color};

// Physical raster limits also bound legacy files, zoomed text, many lines and cache retention.
const MAX_GLYPH_PX: f32 = 2048.0;
const MAX_RASTER_SIDE: usize = 8192;
const MAX_RASTER_PIXELS: usize = 16 * 1024 * 1024;
const MAX_TEXT_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_GLYPH_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_RASTER_SCALE: f32 = 8.0;
const MIN_RASTER_SCALE: f32 = 0.01;
const MAX_LAYOUT_WIDTH: f32 = 4.0 * 7680.0;
/// Bounds layout work for pasted walls of text at any font size.
const MAX_TEXT_LINES: f32 = 1000.0;

fn finite_clamp(value: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() { value.clamp(min, max) } else { min }
}

/// Rasterised text and the output pixels per canvas pixel it was drawn at. Large text is drawn at
/// a lower scale than requested, so callers size the layer with this one.
#[derive(Clone)]
pub struct TextImage {
    pub image: Image,
    pub scale: f32,
}

#[derive(serde::Deserialize)]
pub struct FontFace {
    pub family: String,
    pub file: String,
    pub weight: String,
}

pub static FONT_MANIFEST: std::sync::LazyLock<Vec<FontFace>> = std::sync::LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../assets/fonts/manifest.json")).expect("valid bundled font manifest")
});

pub static BUNDLED_FONT_FAMILIES: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
    FONT_MANIFEST.iter().map(|face| face.family.as_str()).collect::<BTreeSet<_>>().into_iter().collect()
});

macro_rules! embed_fonts {
    ($($file:literal),* $(,)?) => {
        &[$(($file, include_bytes!(concat!("../../../assets/fonts/", $file)) as &[u8])),*]
    };
}
const FONT_DATA: &[(&str, &[u8])] = embed_fonts![
    "anton/Anton-Regular.ttf",
    "bebasneue/BebasNeue-Regular.ttf",
    "inter/Inter[opsz,wght].ttf",
    "lexend/Lexend[wght].ttf",
    "montserrat/Montserrat[wght].ttf",
    "oswald/Oswald[wght].ttf",
    "poppins/Poppins-Regular.ttf",
    "poppins/Poppins-Bold.ttf",
    "roboto/Roboto[wdth,wght].ttf",
];

static BUNDLED_FONTS: std::sync::LazyLock<Vec<&'static [u8]>> = std::sync::LazyLock::new(|| {
    FONT_MANIFEST
        .iter()
        .map(|face| FONT_DATA.iter().find(|(file, _)| *file == face.file).expect("manifest font is embedded").1)
        .collect()
});

/// Text without a chosen font uses this bundled family, so a project looks the same on every machine.
const DEFAULT_FAMILY: &str = "Inter";

pub struct TextRenderer {
    fonts: FontSystem,
    families: HashSet<String>,
    /// Whether a family draws bold itself, by family name.
    bold: HashMap<String, bool>,
    swash: SwashCache,
    cache: HashMap<u64, TextImage>,
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRenderer {
    pub fn new() -> Self {
        let mut fonts = FontSystem::new();
        for data in BUNDLED_FONTS.iter() {
            fonts.db_mut().load_font_data(data.to_vec());
        }
        fonts.db_mut().set_sans_serif_family(DEFAULT_FAMILY);
        let families = fonts.db().faces().flat_map(|face| face.families.iter().map(|(name, _)| name.clone())).collect();
        Self { fonts, families, bold: HashMap::new(), swash: SwashCache::new(), cache: HashMap::new() }
    }

    /// Bundled families first, then installed families, each group sorted and unique.
    pub fn font_families(&self) -> Vec<String> {
        let system: BTreeSet<&str> =
            self.families.iter().map(String::as_str).filter(|name| !BUNDLED_FONT_FAMILIES.contains(name)).collect();
        BUNDLED_FONT_FAMILIES.iter().copied().chain(system).map(str::to_owned).collect()
    }

    fn family<'a>(&self, name: Option<&'a str>) -> Family<'a> {
        match name {
            Some(name) if self.families.contains(name) => Family::Name(name),
            _ => Family::SansSerif,
        }
    }

    /// Single-weight families such as Anton have no bold face; asking for one would make
    /// cosmic-text draw another family's bold, so they stay regular instead.
    fn draws_bold(&mut self, family: Family) -> bool {
        let name = match family {
            Family::Name(name) => name,
            _ => DEFAULT_FAMILY,
        };
        if let Some(&known) = self.bold.get(name) {
            return known;
        }
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(16.0, 20.0));
        buffer.set_text("A", &Attrs::new().family(family).weight(Weight::BOLD), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts, false);
        let font = buffer.layout_runs().flat_map(|run| run.glyphs.iter()).map(|glyph| glyph.font_id).next();
        let own = font
            .and_then(|id| self.fonts.db().face(id))
            .is_some_and(|face| face.families.iter().any(|(n, _)| n == name));
        self.bold.insert(name.to_owned(), own);
        own
    }

    /// Renders `text` with `style` scaled by `scale` (output pixels per canvas pixel).
    /// `max_width` is the wrap width in output pixels at the requested scale.
    pub fn render(&mut self, text: &str, style: &TextStyle, scale: f32, max_width: f32) -> TextImage {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        format!("{style:?}").hash(&mut h);
        scale.to_bits().hash(&mut h);
        max_width.to_bits().hash(&mut h);
        let key = h.finish();
        if let Some(img) = self.cache.get(&key) {
            return img.clone();
        }
        let img = self.rasterize(text, style, scale, max_width);
        if self.cache.len() >= 512
            || self.cache.values().map(|text| text.image.data.len()).sum::<usize>() + img.image.data.len()
                > MAX_TEXT_CACHE_BYTES
        {
            self.cache.clear();
        }
        self.cache.insert(key, img.clone());
        img
    }

    fn rasterize(&mut self, text: &str, style: &TextStyle, scale: f32, max_width: f32) -> TextImage {
        // Layout stays in canvas pixels; only glyph rasterisation uses the output scale.
        let wanted = finite_clamp(scale, MIN_RASTER_SCALE, MAX_RASTER_SCALE);
        let size = finite_clamp(style.font_size, 1.0, MAX_GLYPH_PX / MIN_RASTER_SCALE);
        let stroke = finite_clamp(style.stroke_width, 0.0, max_stroke_width(size));
        let max_width = finite_clamp(max_width / wanted, 1.0, MAX_LAYOUT_WIDTH);
        let pad_box = if style.background.is_some() { size * 0.3 } else { 0.0 };
        let pad = (stroke.ceil() + pad_box.ceil() + 2.0) as i32;

        let line_height = size * 1.2;
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(size, line_height));
        let wrap = (max_width - 2.0 * pad as f32).max(size);
        buffer.set_size(Some(wrap), Some(MAX_TEXT_LINES * line_height));
        let family = self.family(style.font_family.as_deref());
        let weight = if style.bold && self.draws_bold(family) { Weight::BOLD } else { Weight::NORMAL };
        let attrs = Attrs::new().family(family).weight(weight);
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
        let canvas_w = (line_w.ceil() + 2.0 * pad as f32).max(1.0);
        let canvas_h = (text_h.ceil() + 2.0 * pad as f32).max(1.0);
        // Large text gets a coarser raster rather than a smaller font or a cropped bitmap.
        let mut scale = wanted
            .min(MAX_GLYPH_PX / size)
            .min(MAX_RASTER_SIDE as f32 / canvas_w.max(canvas_h))
            .min((MAX_RASTER_PIXELS as f32 / (canvas_w * canvas_h)).sqrt())
            .max(MIN_RASTER_SCALE);
        let pixels = |scale: f32| ((canvas_w * scale).ceil() as usize, (canvas_h * scale).ceil() as usize);
        // Rounding up can overshoot a limit by a pixel.
        while scale > MIN_RASTER_SCALE
            && let (w, h) = pixels(scale)
            && (w.max(h) > MAX_RASTER_SIDE || w * h > MAX_RASTER_PIXELS)
        {
            scale = (scale * 0.999).max(MIN_RASTER_SCALE);
        }
        // Pathological text below the minimum scale is clipped before any allocation.
        let (w, h) = pixels(scale);
        let w = w.clamp(1, MAX_RASTER_SIDE);
        let h = h.clamp(1, MAX_RASTER_SIDE.min(MAX_RASTER_PIXELS / w));

        let mut fill = vec![0u8; w * h];
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let offset = ((pad - x0) as f32 * scale, (run.line_y + pad as f32) * scale);
                let physical = glyph.physical(offset, scale);
                if self.swash.image_cache.values().flatten().map(|image| image.data.len()).sum::<usize>()
                    > MAX_GLYPH_CACHE_BYTES
                {
                    self.swash.image_cache.clear();
                }
                self.swash.with_pixels(
                    &mut self.fonts,
                    physical.cache_key,
                    Color::rgb(255, 255, 255),
                    |x, y, color| {
                        let (px, py) = (x + physical.x, y + physical.y);
                        if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                            let i = py as usize * w + px as usize;
                            fill[i] = fill[i].max(color.a());
                        }
                    },
                );
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
        TextImage { image: Image { width: w as u32, height: h as u32, data: Arc::new(out) }, scale }
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

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_text::skrifa::{FontRef, MetadataProvider};

    const CZECH: &str = "Příliš žluťoučký kůň ěščřžýáíéúůťďň ĚŠČŘŽÝÁÍÉÚŮŤĎŇ";

    fn style(family: &str) -> TextStyle {
        TextStyle {
            font_family: Some(family.into()),
            font_size: 95.0,
            color: "#ffffff".into(),
            bold: false,
            stroke_width: 7.5,
            stroke_color: "#000000".into(),
            background: None,
            max_width: None,
        }
    }

    #[test]
    fn large_text_gets_a_coarser_raster_instead_of_a_crop() {
        let mut renderer = bundled_renderer();
        let canvas_size =
            |text: &TextImage| (text.image.width as f32 / text.scale, text.image.height as f32 / text.scale);
        // Captions at 2160p: the pixel limit lowers the scale, rounding must not cut the last row.
        let mut title = style("Inter");
        title.font_size = 300.0;
        title.stroke_width = 0.0;
        title.background = Some("#ffffffff".into());
        let exact = canvas_size(&renderer.render("BIG\nTITLE", &title, 1.0, 972.0));
        let export = renderer.render("BIG\nTITLE", &title, 8.0, 972.0 * 8.0);
        assert!(export.scale < 8.0);
        let coarse = canvas_size(&export);
        assert!(coarse.0 >= exact.0 && coarse.1 >= exact.1, "{coarse:?} vs {exact:?}");
        assert!(coarse.0 - exact.0 < 1.0 && coarse.1 - exact.1 < 1.0, "{coarse:?} vs {exact:?}");
        // A valid 8K title keeps all three lines.
        title.font_size = 15360.0;
        title.background = None;
        let lines = renderer.render("A\nA\nA", &title, 1.0, 6912.0);
        assert!(canvas_size(&lines).1 > 3.0 * 15360.0 * 1.2, "{:?}", canvas_size(&lines));
    }

    #[test]
    fn legacy_text_styles_render_with_bounded_allocations() {
        let mut renderer = bundled_renderer();
        let mut canvas = crate::Project::new("old project").canvas;
        canvas.width = 64;
        canvas.height = 64;
        for value in [f32::MAX, f32::INFINITY, f32::NAN, -1.0] {
            let mut old = style("Inter");
            old.font_size = value;
            old.stroke_width = value;
            old.max_width = Some(value);
            let bounded = old.bounded(&canvas);
            let image = renderer.render("Old title", &bounded, 1.0, bounded.max_width.unwrap()).image;
            assert!(image.width > 0 && image.height > 0);
            assert_eq!(image.data.len(), image.width as usize * image.height as usize * 4);
            assert!(image.data.len() <= MAX_RASTER_PIXELS * 4);
        }
        // Direct renderer callers are protected too, including malformed scales and many lines.
        let mut old = style("Inter");
        old.font_size = f32::MAX;
        old.stroke_width = f32::MAX;
        let image = renderer.render("A", &old, 1.0, f32::MAX).image;
        assert!(image.data.len() <= MAX_RASTER_PIXELS * 4);
        let image = renderer.render(&"line\n".repeat(1000), &style("Inter"), f32::NAN, f32::INFINITY).image;
        assert!(image.data.len() <= MAX_RASTER_PIXELS * 4);
    }

    #[test]
    fn manifest_matches_embedded_fonts() {
        let files: BTreeSet<_> = FONT_MANIFEST.iter().map(|face| face.file.as_str()).collect();
        assert_eq!(files.len(), FONT_MANIFEST.len());
        assert_eq!(files, FONT_DATA.iter().map(|(file, _)| *file).collect());
        assert!(FONT_MANIFEST.iter().all(|face| !face.weight.is_empty()));
    }

    fn bundled_renderer() -> TextRenderer {
        let mut db = cosmic_text::fontdb::Database::new();
        for data in BUNDLED_FONTS.iter() {
            db.load_font_data(data.to_vec());
        }
        db.set_sans_serif_family("Inter");
        let fonts = FontSystem::new_with_locale_and_db("cs-CZ".into(), db);
        let families = fonts.db().faces().flat_map(|face| face.families.iter().map(|(name, _)| name.clone())).collect();
        TextRenderer { fonts, families, bold: HashMap::new(), swash: SwashCache::new(), cache: HashMap::new() }
    }

    #[test]
    fn bundled_fonts_cover_and_render_czech_without_fallback() {
        let mut renderer = bundled_renderer();
        for face in renderer.fonts.db().faces() {
            let covered = renderer.fonts.db().with_face_data(face.id, |data, index| {
                let font = FontRef::from_index(data, index).unwrap();
                let cmap = font.charmap();
                CZECH.chars().all(|c| cmap.map(c).is_some_and(|id| id.to_u32() != 0))
            });
            assert_eq!(covered, Some(true), "Missing Czech glyphs in {:?}", face.families);
        }
        for family in BUNDLED_FONT_FAMILIES.iter().copied() {
            let image = renderer.render("Příliš žluťoučký kůň", &style(family), 1.0, 1080.0).image;
            assert!(image.data.chunks_exact(4).any(|pixel| pixel[3] != 0), "Empty {family}");
            let mut buffer = Buffer::new(&mut renderer.fonts, Metrics::new(95.0, 114.0));
            buffer.set_size(Some(4000.0), None);
            buffer.set_text(CZECH, &Attrs::new().family(Family::Name(family)), Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut renderer.fonts, false);
            for run in buffer.layout_runs() {
                for glyph in run.glyphs {
                    assert_ne!(glyph.glyph_id, 0, "Missing glyph in {family}");
                    let face = renderer.fonts.db().face(glyph.font_id).unwrap();
                    assert!(face.families.iter().any(|(name, _)| name == family), "Fallback in {family}");
                }
            }
        }
    }

    #[test]
    fn bundled_variable_and_static_bold_weights_render_differently() {
        let mut renderer = bundled_renderer();
        for family in ["Inter", "Lexend", "Montserrat", "Oswald", "Poppins", "Roboto"] {
            let mut style = style(family);
            style.stroke_width = 0.0;
            let regular = renderer.render("Příliš žluťoučký kůň", &style, 1.0, 2000.0).image;
            style.bold = true;
            let bold = renderer.render("Příliš žluťoučký kůň", &style, 1.0, 2000.0).image;
            let coverage = |image: &Image| image.data.chunks_exact(4).map(|p| u64::from(p[3])).sum::<u64>();
            assert!(coverage(&bold) > coverage(&regular), "Bold did not increase ink coverage in {family}");
        }
    }

    #[test]
    fn single_weight_families_stay_themselves_when_bold() {
        // With installed fonts present, cosmic-text would otherwise pick another family's bold.
        let mut renderer = TextRenderer::new();
        for family in ["Anton", "Bebas Neue"] {
            let mut style = style(family);
            let regular = renderer.render("Příliš žluťoučký kůň", &style, 1.0, 2000.0).image;
            style.bold = true;
            assert_eq!(
                renderer.render("Příliš žluťoučký kůň", &style, 1.0, 2000.0).image.data,
                regular.data,
                "{family}"
            );
        }
    }

    #[test]
    fn family_selection_cache_fallback_and_listing() {
        let mut renderer = TextRenderer::new();
        let families = renderer.font_families();
        assert_eq!(&families[..BUNDLED_FONT_FAMILIES.len()], BUNDLED_FONT_FAMILIES.as_slice());
        assert_eq!(families.iter().collect::<BTreeSet<_>>().len(), families.len());
        assert!(families[BUNDLED_FONT_FAMILIES.len()..].windows(2).all(|pair| pair[0] < pair[1]));
        for family in &families {
            assert_eq!(renderer.family(Some(family)), Family::Name(family));
        }
        let inter = renderer.render("Příliš žluťoučký kůň", &style("Inter"), 1.0, 2000.0).image;
        let anton = renderer.render("Příliš žluťoučký kůň", &style("Anton"), 1.0, 2000.0).image;
        assert_ne!(inter.data, anton.data);
        let missing = renderer.render("Ahoj", &style("CapOpen nonexistent font"), 1.0, 1000.0).image;
        let mut default = style("Inter");
        default.font_family = None;
        let fallback = renderer.render("Ahoj", &default, 1.0, 1000.0).image;
        assert_eq!(missing.data, fallback.data);
    }
}
