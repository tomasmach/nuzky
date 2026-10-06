use std::path::Path;

use anyhow::{Context, Result, ensure};
use capopen_engine::{Project, Renderer, Wait, model::ClipContent};

const DEFAULT_WIDTH: u32 = 320;
const MAX_FRAMES: usize = 16;
const LABEL_HEIGHT: u32 = 24;
const MAX_SHEET_PIXELS: u64 = 24_000_000;

pub fn check_media(project: &Project) -> Result<()> {
    for clip in project.tracks.iter().flat_map(|t| &t.clips) {
        if let ClipContent::Media { asset_id, .. } = &clip.content {
            let asset = project
                .asset(asset_id)
                .context("UNKNOWN_ASSET: clip references absent asset")?;
            ensure!(
                Path::new(&asset.path).is_file(),
                "MEDIA_MISSING: {}",
                asset.path
            );
        }
    }
    Ok(())
}

pub fn contact_sheet(project: &Project, times: &[i64], width: Option<u32>, safe_area: bool) -> Result<Vec<u8>> {
    ensure!(
        !times.is_empty() && times.len() <= MAX_FRAMES,
        "Provide 1..={MAX_FRAMES} frame times"
    );
    ensure!(
        times.iter().all(|t| *t >= 0 && *t < project.duration_us()),
        "Frame times must be inside the timeline in integer microseconds"
    );
    check_media(project)?;
    let width = width.unwrap_or(DEFAULT_WIDTH);
    ensure!(
        (96..=1280).contains(&width),
        "Frame width must be 96..=1280 pixels"
    );
    let height = (width as u64 * project.canvas.height as u64 / project.canvas.width as u64) as u32;
    let columns = (times.len() as u32).min(4);
    let rows = (times.len() as u32).div_ceil(columns);
    let (sheet_w, sheet_h) = (columns * width, rows * (height + LABEL_HEIGHT));
    ensure!(
        sheet_w as u64 * sheet_h as u64 <= MAX_SHEET_PIXELS,
        "Contact sheet too large; use a smaller width or fewer times"
    );
    let mut pixels = vec![0u8; sheet_w as usize * sheet_h as usize * 4];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    let mut renderer = Renderer::new().context("Starting frame renderer")?;
    for (index, &time) in times.iter().enumerate() {
        let mut rgba = renderer
            .render(project, time, width, height, Wait::Exact, false)
            .context("Rendering contact sheet frame")?;
        if safe_area { shade_unsafe(&mut rgba, width, height, &project.canvas); }
        let (x, y) = (
            index as u32 % columns * width,
            index as u32 / columns * (height + LABEL_HEIGHT),
        );
        for row in 0..height as usize {
            let dest = ((y as usize + row) * sheet_w as usize + x as usize) * 4;
            let src = row * width as usize * 4;
            pixels[dest..dest + width as usize * 4]
                .copy_from_slice(&rgba[src..src + width as usize * 4]);
        }
        label(
            &mut pixels,
            sheet_w,
            x + 6,
            y + height + 5,
            &format!("{}.{:06}", time / 1_000_000, time % 1_000_000),
            width - 12,
        );
    }
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, sheet_w, sheet_h);
    encoder.set_color(png::ColorType::Rgba);
    encoder
        .write_header()
        .context("Writing PNG header")?
        .write_image_data(&pixels)
        .context("Encoding contact sheet")?;
    Ok(png)
}

// A tiny fixed numeric font keeps timestamp labels independent of system fonts.
fn label(pixels: &mut [u8], stride: u32, x: u32, y: u32, text: &str, available: u32) {
    const DIGITS: [[u8; 5]; 11] = [
        [7, 5, 5, 5, 7],
        [2, 6, 2, 2, 7],
        [7, 1, 7, 4, 7],
        [7, 1, 7, 1, 7],
        [5, 5, 7, 1, 1],
        [7, 4, 7, 1, 7],
        [7, 4, 7, 5, 7],
        [7, 1, 1, 1, 1],
        [7, 5, 7, 5, 7],
        [7, 5, 7, 1, 7],
        [0, 0, 0, 0, 2],
    ];
    for (index, byte) in text.bytes().take((available / 8) as usize).enumerate() {
        let glyph = if byte == b'.' {
            10
        } else {
            (byte - b'0') as usize
        };
        for (row, bits) in DIGITS[glyph].iter().enumerate() {
            for col in 0..3 {
                if bits & (1 << (2 - col)) == 0 {
                    continue;
                }
                for dy in 0..2 {
                    for dx in 0..2 {
                        let offset = (((y + row as u32 * 2 + dy) * stride
                            + x
                            + index as u32 * 8
                            + col * 2
                            + dx)
                            * 4) as usize;
                        pixels[offset..offset + 4].fill(255);
                    }
                }
            }
        }
    }
}

fn shade_unsafe(pixels: &mut [u8], width: u32, height: u32, canvas: &capopen_engine::model::Canvas) {
    let Some(area) = canvas.safe_area() else { return };
    for y in 0..height {
        for x in 0..width {
            let cx = (x as f32 + 0.5) * canvas.width as f32 / width as f32;
            let cy = (y as f32 + 0.5) * canvas.height as f32 / height as f32;
            if cx < area.left || cx >= area.right || cy < area.top || cy >= area.bottom {
                let offset = ((y * width + x) * 4) as usize;
                for (channel, overlay) in [240u16, 80, 60].into_iter().enumerate() {
                    pixels[offset + channel] = ((u16::from(pixels[offset + channel]) * 3 + overlay) / 4) as u8;
                }
            }
        }
    }
}
