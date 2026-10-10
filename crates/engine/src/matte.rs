//! Person mattes: the outline of the person in every frame of a file as 8-bit alpha, made once in the
//! background and kept per file like preview proxies, so every clip of the file, its splits and other projects
//! share it. A clip with a background (`model::Background`) draws the person over its fill with it, in the
//! preview and in export alike.
//!
//! A matte covers the whole frame in the file's stored orientation on the person model's own 256 × 256 grid,
//! so the GPU samples it with the frame's texture coordinates. Files are cut into chunks of 2 s of source time
//! and only the chunks clips show are made: a long take cut down to a minute costs a minute. A frame is looked
//! up by the time the decoder gave it, so a preview proxy, which keeps the original's frame times, finds the
//! same mattes as export. Making them needs a person model, which this crate does not run: the caller passes
//! one in (`nuzky_vision::matte`).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use crate::audio::lock_cache;
use crate::effects::{source_time, transition_window};
use crate::gpu::texture_uv;
use crate::media::{VideoDecoder, file_key};
use crate::model::{Asset, AssetKind, ClipContent, Project, TrackKind};
use ffmpeg_next as ff;

/// Raised whenever mattes are made differently, so ones made before are made again.
pub const MATTE_VERSION: &str = "m1";
/// Side of the grid, the person model's input and output.
pub const SIDE: u32 = 256;
const CHUNK_US: i64 = 2_000_000;
/// Frames on each side of a frame its matte is smoothed over.
const RADIUS: usize = 3;
/// Decoded around a run of chunks, so the frames at its edges have neighbours to smooth with.
const CONTEXT_US: i64 = 300_000;
/// Most frames a chunk can hold: 2 s at 240 fps, with room for odd timestamps.
const MAX_FRAMES: usize = 1024;
const MAGIC: &[u8; 4] = b"NZM1";
/// How far after its own chunk a held frame is looked for: an hour of video without a new frame.
const MAX_HELD_CHUNKS: i64 = 1800;

fn chunk_of(t_us: i64) -> i64 {
    t_us.max(0) / CHUNK_US
}

/// Names are `<size>-<time>-<path hash>.<chunk>.<version>.bin`, after the file as it is now.
fn chunk_path(dir: &Path, key: &str, chunk: i64) -> PathBuf {
    dir.join("matte").join(format!("{key}.{chunk}.{MATTE_VERSION}.bin"))
}

/// Each file a drawn clip with a background shows, with the chunks of it that clips show. Hidden tracks
/// are left out, as the preview and export leave them out.
pub fn needed(project: &Project) -> Vec<(Asset, BTreeSet<i64>)> {
    let mut files: Vec<(Asset, BTreeSet<i64>)> = Vec::new();
    for track in project.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.hidden) {
        for (i, clip) in track.clips.iter().enumerate() {
            let ClipContent::Media { asset_id, background, .. } = &clip.content else { continue };
            let Some(asset) = project.asset(asset_id).filter(|a| a.kind != AssetKind::Audio) else { continue };
            if background.is_none() {
                continue;
            }
            let chunks = if asset.kind == AssetKind::Image {
                0..=0
            } else {
                // The outgoing clip of a transition plays on past its end, the incoming one holds its first frame.
                let next = track.clips.get(i + 1).filter(|_| track.id == crate::edit::MAIN_TRACK);
                let end = next.and_then(transition_window).map_or(clip.end_us(), |w| w.1.max(clip.end_us()));
                let last = (asset.duration_us - 1).max(0);
                chunk_of(source_time(clip, clip.start_us).clamp(0, last))
                    ..=chunk_of(source_time(clip, end).clamp(0, last))
            };
            match files.iter_mut().find(|(a, _)| a.path == asset.path) {
                Some((_, set)) => set.extend(chunks),
                None => files.push((asset.clone(), chunks.collect())),
            }
        }
    }
    files
}

/// What `needed` asks for that is not made yet.
pub fn missing(cache_dir: &Path, project: &Project) -> Vec<(Asset, BTreeSet<i64>)> {
    needed(project)
        .into_iter()
        .filter_map(|(asset, chunks)| {
            let key = file_key(&asset.path);
            let left: BTreeSet<i64> =
                chunks.into_iter().filter(|&c| !chunk_path(cache_dir, &key, c).exists()).collect();
            (!left.is_empty()).then_some((asset, left))
        })
        .collect()
}

/// The person model: an upright `SIDE` × `SIDE` RGBA frame in, the probability of a person in each cell out.
pub type Segment<'a> = dyn FnMut(&[u8]) -> Result<Vec<f32>> + 'a;

/// Makes the `chunks` of `asset`'s matte that are not made yet, from the file itself, whose frames export shows.
/// Other callers for the same file wait for this one. An error from `progress` stops it and leaves no unfinished
/// chunk.
pub fn prepare(
    cache_dir: &Path,
    asset: &Asset,
    chunks: &BTreeSet<i64>,
    segment: &mut Segment,
    progress: &mut dyn FnMut(f32) -> Result<()>,
) -> Result<()> {
    let key = file_key(&asset.path);
    let _lock = lock_cache(&cache_dir.join("matte").join(&key), &mut |p| progress(p))?;
    let todo: Vec<i64> = chunks.iter().copied().filter(|&c| !chunk_path(cache_dir, &key, c).exists()).collect();
    if todo.is_empty() {
        return Ok(());
    }
    let source = Path::new(&asset.path);
    let mut decoder = VideoDecoder::open(source)?;
    // Runs of consecutive chunks decode in one pass.
    let mut runs: Vec<(i64, i64)> = Vec::new();
    for c in todo {
        match runs.last_mut() {
            Some(run) if run.1 + 1 == c => run.1 = c,
            _ => runs.push((c, c)),
        }
    }
    let fps = if asset.fps > 0.0 { asset.fps.min(240.0) } else { 30.0 };
    let total = if decoder.is_image() {
        1.0
    } else {
        runs.iter().map(|(a, b)| (b - a + 1) as f64 * CHUNK_US as f64 / 1e6 * fps).sum::<f64>().max(1.0)
    };
    let n = SIDE as usize;
    let upright = upright_cells(n, asset);
    let mut done = 0.0;
    for (first, last) in runs {
        let (start, end) = (first * CHUNK_US, (last + 1) * CHUNK_US);
        let mut writer = ChunkWriter { dir: cache_dir, key: &key, current: first, frames: Vec::new(), held: None };
        let mut window = Window::default();
        if !decoder.is_image() {
            decoder.seek((start - CONTEXT_US).max(0))?;
        }
        // The frame waiting to be looked at. A decoder shows the last of frames at one time, and the last frame before
        // the run is the one shown at its start, so a later frame at the same time, or before the context, takes its place.
        let mut pending: Option<(i64, ff::frame::Video)> = None;
        loop {
            let next = if decoder.is_image() && pending.is_some() { None } else { decoder.next_frame()? };
            if let (Some((t, _)), Some((waiting, _))) = (&next, &pending) {
                // A frame stamped before the one waiting is never shown.
                if *t < *waiting {
                    continue;
                }
                if *t == *waiting || *t < start - CONTEXT_US {
                    pending = next;
                    continue;
                }
            }
            let Some((t, frame)) = std::mem::replace(&mut pending, next) else {
                if pending.is_none() {
                    break;
                }
                continue;
            };
            let small = decoder.convert(&frame, t, SIDE, SIDE)?;
            let pixels = small.data.as_chunks::<4>().0;
            let person = segment(upright.iter().flat_map(|&cell| pixels[cell]).collect::<Vec<u8>>().as_slice())?;
            anyhow::ensure!(person.len() == n * n, "The person model gave {} cells", person.len());
            let mut alpha = vec![0.0; n * n];
            for (&cell, &p) in upright.iter().zip(&person) {
                alpha[cell] = p;
            }
            let item = Item { t, luma: luma(pixels, n), alpha };
            for (t, alpha) in window.push(item) {
                writer.add(t, &alpha, last)?;
            }
            done += 1.0;
            progress((done / total).min(1.0) as f32)?;
            if pending.as_ref().is_some_and(|(t, _)| *t >= end + CONTEXT_US) {
                break;
            }
        }
        for (t, alpha) in window.finish() {
            writer.add(t, &alpha, last)?;
        }
        // Chunks past the last frame are written empty, so nothing waits for them again.
        while writer.current <= last {
            writer.flush()?;
        }
    }
    Ok(())
}

struct Item {
    t: i64,
    luma: Vec<u8>,
    alpha: Vec<f32>,
}

/// The frames around the next one to finish.
#[derive(Default)]
struct Window {
    items: VecDeque<Item>,
    /// Index in `items` of the next frame to finish.
    next: usize,
}

impl Window {
    /// Adds a frame; gives back the frames that now have every neighbour they are smoothed with.
    fn push(&mut self, item: Item) -> Vec<(i64, Vec<f32>)> {
        self.items.push_back(item);
        let mut out = Vec::new();
        while self.next < self.items.len() && self.items.len() - 1 - self.next >= RADIUS {
            out.push(self.finish_next());
        }
        out
    }

    fn finish(&mut self) -> Vec<(i64, Vec<f32>)> {
        let mut out = Vec::new();
        while self.next < self.items.len() {
            out.push(self.finish_next());
        }
        out
    }

    fn finish_next(&mut self) -> (i64, Vec<f32>) {
        let items: Vec<&Item> = self.items.iter().collect();
        let out = (items[self.next].t, smooth(&items, self.next));
        self.next += 1;
        if self.next > RADIUS {
            self.items.pop_front();
            self.next -= 1;
        }
        out
    }
}

/// How far apart in time, in frames, and in brightness, in 0..1, neighbours still count.
const SIGMA_FRAMES: f32 = 2.0;
const SIGMA_LUMA: f32 = 8.0 / 255.0;

/// Each cell of `frames[centre]` averaged with its neighbours in time, weighted by how near they are and how
/// alike the picture is there: where the picture stands still the model's frame-to-frame jitter of the edge
/// goes, where it moved each frame keeps its own outline, so nothing trails behind a moving person.
fn smooth(frames: &[&Item], centre: usize) -> Vec<f32> {
    let me = frames[centre];
    let near: Vec<f32> = (0..frames.len())
        .map(|k| {
            let dt = k as f32 - centre as f32;
            (-(dt * dt) / (2.0 * SIGMA_FRAMES * SIGMA_FRAMES)).exp()
        })
        .collect();
    // How alike two brightnesses are, by their difference in 1/255 steps: a table, as it is asked 300 000 times a frame.
    let alike: [f32; 256] = std::array::from_fn(|d| {
        let dl = d as f32 / 255.0;
        (-(dl * dl) / (2.0 * SIGMA_LUMA * SIGMA_LUMA)).exp()
    });
    let mut out = Vec::with_capacity(me.alpha.len());
    for i in 0..me.alpha.len() {
        let (mut sum, mut weights) = (0.0, 0.0);
        for (other, near) in frames.iter().zip(&near) {
            let w = near * alike[other.luma[i].abs_diff(me.luma[i]) as usize];
            sum += w * other.alpha[i];
            weights += w;
        }
        out.push(sum / weights);
    }
    out
}

/// Rec. 709 luma of each cell, averaged over its 3 × 3 neighbours (fewer at the edges), so noise in the picture
/// does not read as movement.
fn luma(pixels: &[[u8; 4]], n: usize) -> Vec<u8> {
    let plain: Vec<f32> =
        pixels.iter().map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32).collect();
    // Rows, then columns: each sum and how many cells it took.
    let mut rows = vec![(0.0f32, 0.0f32); n * n];
    for y in 0..n {
        for x in 0..n {
            let span = x.saturating_sub(1)..=(x + 1).min(n - 1);
            rows[y * n + x] = (span.clone().map(|xx| plain[y * n + xx]).sum(), span.count() as f32);
        }
    }
    let mut out = Vec::with_capacity(n * n);
    for y in 0..n {
        for x in 0..n {
            let (sum, count) = (y.saturating_sub(1)..=(y + 1).min(n - 1))
                .map(|yy| rows[yy * n + x])
                .fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
            out.push((sum / count).round() as u8);
        }
    }
    out
}

/// For each cell of the upright `n` × `n` grid, the cell of the stored one it shows: the way the asset is shown,
/// its rotation and then its mirroring, as `texture_uv` samples it.
fn upright_cells(n: usize, asset: &Asset) -> Vec<usize> {
    (0..n * n)
        .map(|i| {
            let (x, y) = (i % n, i / n);
            let [u, v] =
                texture_uv((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32, asset.rotation, asset.mirror);
            ((v * n as f32) as usize).min(n - 1) * n + ((u * n as f32) as usize).min(n - 1)
        })
        .collect()
}

struct ChunkWriter<'a> {
    dir: &'a Path,
    key: &'a str,
    /// The chunk the frames collected so far belong to.
    current: i64,
    frames: Vec<(i64, Vec<u8>)>,
    /// The last frame before the run, shown at its start until the first frame of it: kept in its first chunk too,
    /// where the renderer looks when the chunk of the frame's own time was never made.
    held: Option<(i64, Vec<u8>)>,
}

impl ChunkWriter<'_> {
    /// Keeps a finished frame of the run of chunks up to `last`; frames of the context around it only
    /// smoothed others.
    fn add(&mut self, t: i64, alpha: &[f32], last: i64) -> Result<()> {
        if chunk_of(t) > last {
            return Ok(());
        }
        let bytes: Vec<u8> = alpha.iter().map(|a| (a * 255.0).round().clamp(0.0, 255.0) as u8).collect();
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, SIDE, SIDE);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_compression(png::Compression::Fast);
        encoder.write_header()?.write_image_data(&bytes).context("Encoding a matte")?;
        if chunk_of(t) < self.current {
            self.held = Some((t, png));
            return Ok(());
        }
        while chunk_of(t) > self.current {
            self.flush()?;
        }
        if let Some(held) = self.held.take() {
            self.frames.push(held);
        }
        self.frames.push((t, png));
        Ok(())
    }

    /// Writes the current chunk beside its final name and renames it into place, then moves on to the next.
    fn flush(&mut self) -> Result<()> {
        if let Some(held) = self.held.take() {
            self.frames.insert(0, held);
        }
        let path = chunk_path(self.dir, self.key, self.current);
        let mut bytes = Vec::from(&MAGIC[..]);
        for v in [SIDE, SIDE, self.frames.len() as u32] {
            bytes.extend(v.to_le_bytes());
        }
        for (t, png) in &self.frames {
            bytes.extend(t.to_le_bytes());
            bytes.extend((png.len() as u32).to_le_bytes());
        }
        for (_, png) in self.frames.drain(..) {
            bytes.extend(png);
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let tmp = path.with_file_name(format!(".{name}.{}.part", crate::edit::new_id()));
        let result = std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, &path));
        if result.is_err() {
            std::fs::remove_file(&tmp).ok();
        }
        result.context("CACHE_UNAVAILABLE: writing the person's outline")?;
        remove_others(&path);
        self.current += 1;
        Ok(())
    }
}

/// Removes this chunk as made for an earlier state of the file or by an earlier version: nothing reads it any more.
fn remove_others(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().map(|n| n.to_string_lossy().into_owned())) else {
        return;
    };
    // `<size>-<time>-<path hash>.<chunk>.<version>.bin`
    let parts = |name: &str| -> Option<(String, String)> {
        let mut pieces = name.split('.');
        let (key, chunk) = (pieces.next()?, pieces.next()?);
        Some((key.rsplit('-').next()?.to_owned(), chunk.to_owned()))
    };
    let Some(mine) = parts(&name) else { return };
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let other = entry.file_name().to_string_lossy().into_owned();
        if other != name && other.ends_with(".bin") && parts(&other).as_ref() == Some(&mine) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
}

/// One chunk as read back: each frame's time and where its PNG lies in `bytes`.
struct Chunk {
    bytes: Vec<u8>,
    frames: Vec<(i64, std::ops::Range<usize>)>,
}

impl Chunk {
    /// Every count, size and offset is checked: a damaged file reads as missing and is made again.
    fn read(path: &Path) -> Option<Chunk> {
        let bytes = std::fs::read(path).ok()?;
        let u32_at = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
        if bytes.get(..4)? != MAGIC || (u32_at(4)?, u32_at(8)?) != (SIDE, SIDE) {
            return None;
        }
        let count = u32_at(12)? as usize;
        if count > MAX_FRAMES {
            return None;
        }
        let mut offset = 16 + count * 12;
        let mut frames = Vec::with_capacity(count);
        for i in 0..count {
            let at = 16 + i * 12;
            let t = i64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?);
            let len = u32_at(at + 8)? as usize;
            frames.push((t, offset..offset.checked_add(len)?));
            offset += len;
        }
        (offset == bytes.len()).then_some(Chunk { bytes, frames })
    }

    /// The matte of the frame at `t_us`, or with `exact` false of the last one before it, as white RGBA with the
    /// person in alpha.
    fn alpha(&self, t_us: i64, exact: bool) -> Option<Vec<u8>> {
        let i = self.frames.partition_point(|(t, _)| *t <= t_us).checked_sub(1)?;
        if exact && self.frames[i].0 != t_us {
            return None;
        }
        let mut reader =
            png::Decoder::new(std::io::Cursor::new(&self.bytes[self.frames[i].1.clone()])).read_info().ok()?;
        let mut alpha = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut alpha).ok()?;
        let n = SIDE as usize;
        ((info.width, info.height, info.color_type) == (SIDE, SIDE, png::ColorType::Grayscale))
            .then(|| alpha[..n * n].iter().flat_map(|&a| [255, 255, 255, a]).collect())
    }
}

/// Mattes as the renderer reads them, keeping the chunks it read last.
pub(crate) struct Mattes {
    dir: PathBuf,
    /// Chunks read, by path.
    chunks: HashMap<PathBuf, Arc<Chunk>>,
    /// The chunks made of each file, by its key, and when the directory was read for them.
    made: HashMap<String, (Instant, BTreeSet<i64>)>,
    /// The last matte given per file, so a paused frame drawn again uploads nothing new.
    last: HashMap<String, (PathBuf, i64, Arc<Vec<u8>>)>,
}

/// How often the directory is read again for a chunk not found made, as a job adds chunks while it runs.
const LOOK_AGAIN: Duration = Duration::from_millis(100);

impl Mattes {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir, chunks: HashMap::new(), made: HashMap::new(), last: HashMap::new() }
    }

    /// The chunks of the file with `key` that are made, read from the directory the first time or when `again`.
    fn made(&mut self, key: &str, again: bool) -> &BTreeSet<i64> {
        if again || !self.made.contains_key(key) {
            let (prefix, suffix) = (format!("{key}."), format!(".{MATTE_VERSION}.bin"));
            let made = std::fs::read_dir(self.dir.join("matte"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    name.strip_prefix(&prefix)?.strip_suffix(&suffix)?.parse().ok()
                })
                .collect();
            self.made.insert(key.to_owned(), (Instant::now(), made));
        }
        &self.made[key].1
    }

    /// The matte of the frame of `source` the decoder gave at `t_us`, `SIDE` × `SIDE` white RGBA with the person
    /// in alpha. A frame held across chunks that were never made, as over a long pause in variable frame rate
    /// video, is in the first chunk made after its own.
    /// A chunk not found made is looked for in the directory again, at most every `LOOK_AGAIN`, or always with `again`.
    pub(crate) fn alpha(&mut self, source: &str, t_us: i64, again: bool) -> Option<Arc<Vec<u8>>> {
        let key = file_key(source);
        let own = chunk_of(t_us);
        let find = |made: &BTreeSet<i64>| {
            if made.contains(&own) { Some(own) } else { made.range(own + 1..=own + MAX_HELD_CHUNKS).next().copied() }
        };
        let mut chunk = find(self.made(&key, false));
        if chunk.is_none() && (again || self.made.get(&key).is_some_and(|(at, _)| at.elapsed() >= LOOK_AGAIN)) {
            chunk = find(self.made(&key, true));
        }
        let chunk = chunk?;
        self.read(source, chunk_path(&self.dir, &key, chunk), t_us, chunk != own)
    }

    /// The matte at `t_us` in one chunk, of exactly that frame when `exact`, else of the last one before it.
    fn read(&mut self, source: &str, path: PathBuf, t_us: i64, exact: bool) -> Option<Arc<Vec<u8>>> {
        if let Some((p, t, alpha)) = self.last.get(source)
            && (p, *t) == (&path, t_us)
        {
            return Some(alpha.clone());
        }
        if self.chunks.len() > 32 {
            self.chunks.clear();
        }
        let chunk = match self.chunks.get(&path) {
            Some(chunk) => chunk.clone(),
            None => {
                let chunk = Arc::new(Chunk::read(&path)?);
                self.chunks.insert(path.clone(), chunk.clone());
                chunk
            }
        };
        let alpha = Arc::new(chunk.alpha(t_us, exact)?);
        self.last.insert(source.to_owned(), (path, t_us, alpha.clone()));
        Some(alpha)
    }
}

/// The project with every background off, for what looks at where the picture changes rather than how it is
/// finished, so it needs no matte.
pub fn without_backgrounds(project: &Project) -> Project {
    let mut plain = project.clone();
    for clip in plain.tracks.iter_mut().flat_map(|t| &mut t.clips) {
        if let ClipContent::Media { background, .. } = &mut clip.content {
            *background = crate::model::Background::None;
        }
    }
    plain
}

/// Fails when a clip with a background shows a frame whose matte is not made yet, naming the file.
pub fn check(cache_dir: &Path, project: &Project) -> Result<()> {
    if let Some((asset, _)) = missing(cache_dir, project).first() {
        bail!("MATTE_MISSING: the person's outline in {} is not prepared yet", asset.name);
    }
    Ok(())
}
