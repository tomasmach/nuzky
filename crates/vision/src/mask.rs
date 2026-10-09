//! The subject of one timeline frame cut out as an 8-bit alpha mask at canvas resolution, for
//! putting text behind a person. BiRefNet takes the main subject: the person when there is one,
//! otherwise the most prominent object.
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, TryLockError};

use anyhow::{Context, Result};
use nuzky_engine::{Project, Renderer};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::models;
use crate::nets::{BIREFNET_SIDE, BiRefNet, Yunet, check_cancel};
use crate::timeline;

/// Bumped when the mask a frame gives changes, so older cached masks are not read as new ones.
const MASK_VERSION: u32 = 1;
/// Alpha from which a pixel counts as subject in the box and share.
const SUBJECT_ALPHA: u8 = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Rendering,
    /// Another mask is being made; one at a time, since each takes a lot of memory.
    Waiting,
    Segmenting,
}

/// Held while BiRefNet runs, so masks asked for together take turns instead of adding up in memory.
static SEGMENTING: Mutex<()> = Mutex::new(());

#[derive(Debug, Serialize)]
pub struct Mask {
    /// Grayscale PNG, the alpha of the subject, in the cache.
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// `[x, y, width, height]` of the subject in canvas pixels; none when nothing stands out.
    pub subject_box: Option<[i64; 4]>,
    /// Share of the canvas the subject covers, 0..1.
    pub subject_share: f32,
    /// A face is in the frame, so the subject is a person.
    pub person: bool,
    /// The mask came from the cache.
    pub cached: bool,
}

/// Masks the subject at `t_us` of the timeline as exported without text. The cache key is the
/// rendered frame itself with the model, so a change of file, time or edit gives a new mask.
pub fn segment_subject(
    project: &Project,
    t_us: i64,
    models_dir: &Path,
    cache_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Phase),
) -> Result<Mask> {
    models::require(models::MASK, models_dir)?;
    let picture = timeline::picture(project);
    progress(Phase::Rendering);
    let size = (picture.canvas.width, picture.canvas.height);
    let mut renderer = Renderer::new().context("Starting frame renderer")?;
    let frame = timeline::render(&mut renderer, &picture, t_us, size)?;
    drop(renderer);
    check_cancel(cancel)?;
    let mut key = Sha256::new();
    key.update(format!("{MASK_VERSION}:{}:{}x{}:", models::BIREFNET.sha256, size.0, size.1));
    key.update(&frame.rgba);
    let key: String = key.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect();
    let path = cache_dir.join("masks").join(format!("mask-v{MASK_VERSION}-{key}.png"));
    let person = !Yunet::load(models_dir)?.detect(&frame.fit(640), cancel)?.is_empty();
    let (alpha, cached) = match read_png(&path, size) {
        Some(alpha) => (alpha, true),
        None => {
            let _turn = loop {
                match SEGMENTING.try_lock() {
                    Ok(turn) => break turn,
                    Err(TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
                    Err(TryLockError::WouldBlock) => {
                        progress(Phase::Waiting);
                        check_cancel(cancel)?;
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            };
            progress(Phase::Segmenting);
            let grid = BiRefNet::load(models_dir)?.segment(&frame, cancel)?;
            let alpha = upsample(&grid, size);
            write_png(&path, size, &alpha)?;
            (alpha, false)
        }
    };
    let (subject_box, subject_share) = measure(&alpha, size);
    Ok(Mask { path, width: size.0, height: size.1, subject_box, subject_share, person, cached })
}

/// Bilinear from the model's square grid to the canvas, as 8-bit alpha.
fn upsample(grid: &[f32], (width, height): (u32, u32)) -> Vec<u8> {
    let n = BIREFNET_SIDE;
    let at = |x: usize, y: usize| grid[y.min(n - 1) * n + x.min(n - 1)];
    let (sx, sy) = (n as f32 / width as f32, n as f32 / height as f32);
    let mut alpha = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height {
        let gy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
        let (y0, fy) = (gy as usize, gy.fract());
        for x in 0..width {
            let gx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
            let (x0, fx) = (gx as usize, gx.fract());
            let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
            let bottom = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
            alpha.push(((top * (1.0 - fy) + bottom * fy) * 255.0).round().clamp(0.0, 255.0) as u8);
        }
    }
    alpha
}

fn measure(alpha: &[u8], (width, height): (u32, u32)) -> (Option<[i64; 4]>, f32) {
    let (mut x0, mut y0, mut x1, mut y1, mut count) = (u32::MAX, u32::MAX, 0, 0, 0u64);
    for (i, &a) in alpha.iter().enumerate() {
        if a >= SUBJECT_ALPHA {
            let (x, y) = (i as u32 % width, i as u32 / width);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            count += 1;
        }
    }
    let share = count as f32 / (width as f32 * height as f32);
    let rect = (count > 0).then(|| [x0, y0, x1 - x0 + 1, y1 - y0 + 1].map(i64::from));
    (rect, share)
}

fn read_png(path: &Path, (width, height): (u32, u32)) -> Option<Vec<u8>> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    let mut reader = decoder.read_info().ok()?;
    let mut alpha = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut alpha).ok()?;
    let expected = (info.width, info.height, info.color_type, info.bit_depth);
    (expected == (width, height, png::ColorType::Grayscale, png::BitDepth::Eight)).then(|| {
        alpha.truncate(width as usize * height as usize);
        alpha
    })
}

/// Written beside its final name and renamed into place, so a reader never sees half a mask.
fn write_png(path: &Path, (width, height): (u32, u32), alpha: &[u8]) -> Result<()> {
    let dir = path.parent().context("Mask has no directory")?;
    std::fs::create_dir_all(dir).context("CACHE_UNAVAILABLE: creating the mask cache")?;
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.write_header()?.write_image_data(alpha).context("Encoding the mask")?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.part", nuzky_engine::edit::new_id()));
    let result = std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.context("Writing the mask")
}
