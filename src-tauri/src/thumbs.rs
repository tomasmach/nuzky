//! Small PNG thumbnails for the media panel and timeline clips.

use std::path::Path;

use anyhow::Result;
use base64::Engine as _;
use capopen_engine::media::{VideoDecoder, decode_size};
use capopen_engine::model::{Asset, AssetKind};

const THUMB_HEIGHT: f32 = 120.0;

pub fn thumbnail(asset: &Asset) -> Result<Option<String>> {
    if asset.kind == AssetKind::Audio || asset.width == 0 {
        return Ok(None);
    }
    let mut decoder = VideoDecoder::open(Path::new(&asset.path))?;
    let target = if asset.kind == AssetKind::Image { 0 } else { (asset.duration_us / 10).min(1_000_000) };
    decoder.seek(target)?;
    let mut picked = None;
    while let Some((t, f)) = decoder.next_frame()? {
        let done = t >= target;
        picked = Some((t, f));
        if done {
            break;
        }
    }
    let Some((t, f)) = picked else { return Ok(None) };
    let display = (asset.width as f32 * THUMB_HEIGHT / asset.height as f32, THUMB_HEIGHT);
    let (w, h) = decode_size(decoder.source_size(), asset.rotation, display);
    let frame = decoder.convert(&f, t, w, h)?;
    let (rgba, w, h) = rotate(&frame.data, frame.width, frame.height, asset.rotation);

    let mut png_bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_bytes, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.write_header()?.write_image_data(&rgba)?;
    }
    Ok(Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png_bytes))))
}

/// Rotates tightly packed RGBA clockwise by 0/90/180/270 degrees.
fn rotate(src: &[u8], w: u32, h: u32, rotation: u32) -> (Vec<u8>, u32, u32) {
    let (w, h) = (w as usize, h as usize);
    let px = |x: usize, y: usize| &src[(y * w + x) * 4..(y * w + x) * 4 + 4];
    match rotation {
        90 | 180 | 270 => {
            let (ow, oh) = if rotation == 180 { (w, h) } else { (h, w) };
            let mut out = Vec::with_capacity(src.len());
            for oy in 0..oh {
                for ox in 0..ow {
                    let (sx, sy) = match rotation {
                        90 => (oy, h - 1 - ox),
                        180 => (w - 1 - ox, h - 1 - oy),
                        _ => (w - 1 - oy, ox),
                    };
                    out.extend_from_slice(px(sx, sy));
                }
            }
            (out, ow as u32, oh as u32)
        }
        _ => (src.to_vec(), w as u32, h as u32),
    }
}
