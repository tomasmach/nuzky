//! Small PNG thumbnails for the media panel and timeline clips.

use std::path::Path;

use anyhow::Result;
use base64::Engine as _;
use capopen_engine::media::{VideoDecoder, decode_size};
use capopen_engine::model::{Asset, AssetKind};

const THUMB_HEIGHT: f32 = 120.0;
const MAX_SPRITE_PIXELS: u64 = 4_000_000;

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
    let (w, h) = decode_size(decoder.source_size(), asset.rotation, display, u32::MAX);
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

pub fn filmstrip(asset: &Asset) -> Result<Option<crate::Filmstrip>> {
    if asset.kind == AssetKind::Audio || asset.width == 0 || asset.height == 0 {
        return Ok(None);
    }
    let (frame_width, frame_height, count) = filmstrip_size(asset)?;
    let interval_us = (asset.duration_us / count as i64).max(500_000);
    let (dw, dh) = if asset.rotation % 180 == 90 { (frame_height, frame_width) } else { (frame_width, frame_height) };
    let width = frame_width * count;
    let mut sprite = vec![0; (width * frame_height * 4) as usize];
    let mut decoder = VideoDecoder::open(Path::new(&asset.path))?;
    let mut frames = [None, None];
    for i in 0..count {
        let target = i as i64 * interval_us;
        if i == 0 || interval_us >= 2_000_000 {
            decoder.seek(target)?;
            frames = [None, None];
        }
        frames = decoder.frame_covering(target, frames)?;
        let Some((t, frame)) = frames[0].as_ref().or(frames[1].as_ref()) else {
            anyhow::bail!("No filmstrip frame at {target} us in {}", asset.path)
        };
        let frame = decoder.convert(frame, *t, dw, dh)?;
        let (rgba, _, _) = rotate(&frame.data, dw, dh, asset.rotation);
        let row = frame_width as usize * 4;
        for y in 0..frame_height as usize {
            let offset = (y * width as usize + i as usize * frame_width as usize) * 4;
            sprite[offset..offset + row].copy_from_slice(&rgba[y * row..(y + 1) * row]);
        }
    }
    let mut bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut bytes, width, frame_height);
        enc.set_color(png::ColorType::Rgba);
        enc.write_header()?.write_image_data(&sprite)?;
    }
    let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
    Ok(Some(crate::Filmstrip { url, frame_width, frame_height, interval_us, count }))
}

fn filmstrip_size(asset: &Asset) -> Result<(u32, u32, u32)> {
    let height = 90;
    let width = (asset.width as f64 * height as f64 / asset.height as f64).round().max(2.0) as u64;
    let pixels = width * height as u64;
    if pixels > MAX_SPRITE_PIXELS {
        anyhow::bail!("This aspect ratio is too wide for a filmstrip");
    }
    let count = if asset.kind == AssetKind::Image { 1 } else { (asset.duration_us / 500_000).clamp(1, 40) as u32 };
    Ok((width as u32, height, count.min((MAX_SPRITE_PIXELS / pixels) as u32)))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filmstrip_limits_total_pixels() {
        let mut asset = Asset {
            id: "wide".into(),
            name: String::new(),
            path: String::new(),
            kind: AssetKind::Video,
            duration_us: 600_000_000,
            width: 10000,
            height: 100,
            fps: 25.0,
            has_audio: false,
            rotation: 0,
        };
        let (w, h, count) = filmstrip_size(&asset).unwrap();
        assert_eq!(count, 4);
        assert!(w as u64 * h as u64 * count as u64 <= MAX_SPRITE_PIXELS);
        asset.width = u32::MAX;
        asset.height = 1;
        assert!(filmstrip_size(&asset).is_err());
    }

    #[test]
    #[ignore = "requires tmp-test/filmstrip-long.mp4"]
    fn filmstrip_long_gop_timing() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tmp-test/filmstrip-long.mp4");
        let mut asset = capopen_engine::media::probe(&path, "long".into()).unwrap();
        for duration in [600_000_000, 60_000_000] {
            asset.duration_us = duration;
            let start = std::time::Instant::now();
            let strip = filmstrip(&asset).unwrap().unwrap();
            eprintln!("filmstrip {duration} us: {:?}, {} frames", start.elapsed(), strip.count);
            assert_eq!(strip.count, 40);
        }
    }

    #[test]
    fn rotation_preserves_corner_order() {
        let pixels: Vec<u8> = (0..6).flat_map(|i| [i, 0, 0, 255]).collect();
        for (angle, expected) in
            [(90, vec![3, 0, 4, 1, 5, 2]), (180, vec![5, 4, 3, 2, 1, 0]), (270, vec![2, 5, 1, 4, 0, 3])]
        {
            let (out, w, h) = rotate(&pixels, 3, 2, angle);
            assert_eq!(out.chunks_exact(4).map(|p| p[0]).collect::<Vec<_>>(), expected);
            assert_eq!((w, h), if angle == 180 { (3, 2) } else { (2, 3) });
        }
    }

    #[test]
    #[ignore = "requires the local tmp-test media fixtures"]
    fn filmstrip_media_fixtures() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tmp-test");
        let out = root.join("engine-evidence");
        std::fs::create_dir_all(&out).unwrap();
        for name in ["portrait.mp4", "wide.mp4", "phone_hevc_vfr.mov", "music.mp3", "engine-evidence/identity.png"] {
            let asset = capopen_engine::media::probe(&root.join(name), name.into()).unwrap();
            let strip = filmstrip(&asset).unwrap();
            if asset.kind == AssetKind::Audio {
                assert!(strip.is_none());
                continue;
            }
            let strip = strip.unwrap();
            assert_eq!(strip.frame_height, 90);
            assert!(strip.count <= 40 && strip.interval_us >= 500_000);
            if asset.kind == AssetKind::Image {
                assert_eq!(strip.count, 1);
            }
            let bytes = base64::engine::general_purpose::STANDARD.decode(strip.url.split_once(',').unwrap().1).unwrap();
            let reader = png::Decoder::new(std::io::Cursor::new(&bytes)).read_info().unwrap();
            assert_eq!(reader.info().width, strip.frame_width * strip.count);
            assert_eq!(reader.info().height, 90);
            std::fs::write(out.join(format!("filmstrip-{}.png", asset.name)), bytes).unwrap();
        }
    }
}
