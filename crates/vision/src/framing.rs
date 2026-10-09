//! Where a face sits in a 9:16 cover (Reels, TikTok) and a 16:9 thumbnail (YouTube), cut from the
//! canvas whatever its own shape.
use nuzky_engine::model::Canvas;
use serde::Serialize;

use crate::frame::{Rect, inside};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Format {
    #[serde(rename = "9:16")]
    Vertical,
    #[serde(rename = "16:9")]
    Wide,
}

impl Format {
    pub fn aspect(self) -> f32 {
        match self {
            Format::Vertical => 9.0 / 16.0,
            Format::Wide => 16.0 / 9.0,
        }
    }

    /// The format a canvas is closest to.
    pub fn of(canvas: &Canvas) -> Format {
        if canvas.height > canvas.width { Format::Vertical } else { Format::Wide }
    }
}

/// The largest `format` crop of the canvas, slid along its free side to hold `focus`: centred
/// across a tall crop, at 40 % of the height in a wide one, so the head keeps room above.
pub fn crop(canvas: &Canvas, format: Format, focus: [f32; 2]) -> Rect {
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    let aspect = format.aspect();
    if ((w / h) / aspect - 1.0).abs() < 0.02 {
        return [0.0, 0.0, w, h];
    }
    if w / h > aspect {
        let cw = h * aspect;
        [(focus[0] - cw / 2.0).clamp(0.0, w - cw), 0.0, cw, h]
    } else {
        let ch = w / aspect;
        [0.0, (focus[1] - ch * 0.4).clamp(0.0, h - ch), w, ch]
    }
}

/// 1 inside `[low, high]`, falling to 0 at half `low` and at twice `high`.
fn band(value: f32, low: f32, high: f32) -> f32 {
    if value < low {
        ((value - low / 2.0) / (low / 2.0)).clamp(0.0, 1.0)
    } else if value > high {
        (1.0 - (value - high) / high).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// How well `face` (canvas pixels) is framed in `crop` for `format`, 0..1.
///
/// 9:16: face height 15-35 % of the cover, inside the area Reels and TikTok leave free of their
/// buttons (`Canvas::safe_area` proportions) and inside the 3:4 centre the profile grid shows,
/// roughly centred. 16:9: face height 25-55 %, whole with some headroom, and clear of the
/// bottom-right corner where YouTube prints the duration.
pub fn score(format: Format, crop: Rect, face: Rect) -> f32 {
    let [x, y, w, h] = crop;
    let size = face[3] / h;
    let (cx, cy) = (face[0] + face[2] / 2.0, face[1] + face[3] / 2.0);
    match format {
        Format::Vertical => {
            let safe = [x + w * 60.0 / 1080.0, y + h * 250.0 / 1920.0, w * 840.0 / 1080.0, h * 1170.0 / 1920.0];
            let grid_h = (w * 4.0 / 3.0).min(h);
            let grid = [x, y + (h - grid_h) / 2.0, w, grid_h];
            let centred = 1.0 - ((cx - (x + w / 2.0)).abs() / (w / 2.0)).clamp(0.0, 1.0);
            0.35 * band(size, 0.15, 0.35) + 0.25 * inside(face, safe) + 0.25 * inside(face, grid) + 0.15 * centred
        }
        Format::Wide => {
            let headroom = ((face[1] - y) / (0.04 * h)).clamp(0.0, 1.0);
            let badge = (cx > x + 0.75 * w && cy > y + 0.75 * h) as u8 as f32;
            0.4 * band(size, 0.25, 0.55) + 0.25 * inside(face, crop) + 0.15 * headroom + 0.2 * (1.0 - badge)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(width: u32, height: u32) -> Canvas {
        let mut canvas = nuzky_engine::Project::new("framing").canvas;
        (canvas.width, canvas.height) = (width, height);
        canvas
    }

    #[test]
    fn crops_keep_the_face_and_stay_on_the_canvas() {
        let tall = canvas(1080, 1920);
        assert_eq!(crop(&tall, Format::Vertical, [540.0, 600.0]), [0.0, 0.0, 1080.0, 1920.0]);
        // 1080 × 607.5 around a face at 600 px, 40 % from its top.
        assert_eq!(crop(&tall, Format::Wide, [540.0, 600.0]), [0.0, 357.0, 1080.0, 607.5]);
        // A face at the very top: the crop stops at the edge.
        assert_eq!(crop(&tall, Format::Wide, [540.0, 50.0])[1], 0.0);
        let wide = canvas(1920, 1080);
        assert_eq!(crop(&wide, Format::Vertical, [1800.0, 400.0]), [1312.5, 0.0, 607.5, 1080.0]);
    }

    #[test]
    fn covers_prefer_a_centred_face_of_a_fair_size_clear_of_the_buttons() {
        let cover = [0.0, 0.0, 1080.0, 1920.0];
        let good = score(Format::Vertical, cover, [390.0, 500.0, 300.0, 420.0]);
        assert!(good > 0.99, "{good}");
        // Tiny, in the bottom caption area, or off to the like rail: all worse.
        assert!(score(Format::Vertical, cover, [500.0, 500.0, 60.0, 80.0]) < 0.8);
        assert!(score(Format::Vertical, cover, [390.0, 1500.0, 300.0, 380.0]) < 0.65);
        assert!(score(Format::Vertical, cover, [900.0, 500.0, 180.0, 420.0]) < good - 0.2);
        // YouTube: under the duration badge or cut by the top edge.
        let thumb = [0.0, 0.0, 1920.0, 1080.0];
        let centre = score(Format::Wide, thumb, [760.0, 200.0, 400.0, 420.0]);
        assert!(centre > 0.99, "{centre}");
        assert!(score(Format::Wide, thumb, [1600.0, 700.0, 300.0, 330.0]) < centre - 0.15);
        assert!(score(Format::Wide, thumb, [760.0, -150.0, 400.0, 420.0]) < centre - 0.2);
    }
}
