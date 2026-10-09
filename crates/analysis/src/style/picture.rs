//! What a finished cut does to the picture: when its burned-in captions change, and how it
//! zooms and frames each moment against the same moment of the recording.

use std::path::Path;

use anyhow::{Context, Result, ensure};
use nuzky_engine::media::{VideoDecoder, orient};
use nuzky_engine::model::{Asset, AssetKind};
use serde::Serialize;

use super::{Alignment, Piece};

/// Captions are found on frames this wide; their outline is still a pixel or two here.
const MASK_WIDTH: usize = 270;
/// Framing is compared on frames this wide.
const SMALL_WIDTH: usize = 72;
/// Pictures of longer cuts are not analysed: every frame is kept in memory, about 25 kB each.
/// The reason `picture` gives says this length too.
const MAX_CUT_US: i64 = 5 * 60 * 1_000_000;
/// Framing is measured about this often within a piece, and this far from its ends.
const SAMPLE_US: i64 = 500_000;
const EDGE_US: i64 = 200_000;
/// Below this match the framing of a moment is unknown, such as over B-roll.
const MIN_MATCH: f32 = 0.75;
/// Zoom steps smaller than this between samples are noise.
const STEP: f32 = 0.012;
/// A zoom change smaller than this in total is not a zoom.
const MIN_ZOOM: f32 = 0.03;

/// One caption on screen, in cut time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Caption {
    pub start_us: i64,
    pub end_us: i64,
    /// Words on screen, counted from the gaps between them.
    pub words: usize,
}

/// How a moment of the recording is framed in the cut, in Nuzky transform terms: scale 1
/// fits the recording inside the frame, x and y move its centre by fractions of the frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Framing {
    pub time_us: i64,
    pub scale: f32,
    pub x: f32,
    pub y: f32,
}

/// A change of zoom: instant when `start_us == end_us`, otherwise a gradual move.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ZoomChange {
    pub start_us: i64,
    pub end_us: i64,
    pub from: f32,
    pub to: f32,
    /// It happens where the cut joins two pieces of the recording.
    pub at_cut: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Picture {
    /// Why the picture was not analysed, if it was not.
    pub skipped: Option<String>,
    pub captions: Vec<Caption>,
    /// Top and bottom of the caption area, as fractions of the frame height.
    pub caption_band: Option<(f32, f32)>,
    pub framing: Vec<Framing>,
    pub zooms: Vec<ZoomChange>,
}

pub fn picture(recording: &Asset, cut: &Asset, alignment: &Alignment, cancelled: &dyn Fn() -> bool) -> Result<Picture> {
    let skipped = |why: &str| Ok(Picture { skipped: Some(why.into()), ..Picture::default() });
    if recording.kind != AssetKind::Video || cut.kind != AssetKind::Video {
        return skipped("the recording or the cut has no picture");
    }
    if cut.duration_us > MAX_CUT_US {
        return skipped("pictures are analysed in cuts up to 5 minutes long");
    }
    let frames = decode_cut(cut, cancelled)?;
    if frames.times.is_empty() {
        return skipped("no frame of the cut could be decoded");
    }
    let (captions, caption_band) = captions(&frames);
    let keep_rows = |row: usize| {
        caption_band.is_none_or(|(top, bottom)| {
            let y = (row as f32 + 0.5) / frames.small_height as f32;
            y < top - 0.02 || y > bottom + 0.02
        })
    };
    let shots = shots(&frames, alignment, &keep_rows);
    let framing = framing(recording, cut, &frames, &shots, &keep_rows, cancelled)?;
    let zooms = zooms(&framing, &frames, &shots, &keep_rows);
    Ok(Picture { skipped: None, captions, caption_band, framing, zooms })
}

/// The pieces as the picture shows them. Sound puts a cut anywhere in its pause, the picture
/// cuts where it jumps: each join moves to the biggest jump of the picture within that pause.
fn shots(frames: &CutFrames, alignment: &Alignment, keep_rows: &dyn Fn(usize) -> bool) -> Vec<Piece> {
    let mut shots = alignment.pieces.clone();
    for i in 1..shots.len() {
        let at = shots[i].start_us;
        if shots[i - 1].end_us != at {
            continue;
        }
        let near = |p: &&crate::Range| p.start_us - 60_000 <= at && at <= p.end_us + 60_000;
        let (from, to) = alignment
            .cut_pauses
            .iter()
            .find(near)
            .map_or((at - 100_000, at + 100_000), |p| (p.start_us - 60_000, p.end_us + 60_000));
        let cut = jump(frames, from.max(shots[i - 1].start_us), to.min(shots[i].end_us - 1), keep_rows);
        shots[i - 1].end_us = cut;
        shots[i].start_us = cut;
    }
    shots
}

/// Every frame of the cut: where caption-like pixels are, and a small grey picture.
struct CutFrames {
    times: Vec<i64>,
    /// White pixels next to black ones, one bit each, `words` u64 per row of MASK_WIDTH.
    masks: Vec<Vec<u64>>,
    mask_height: usize,
    /// Grey SMALL_WIDTH pictures.
    small: Vec<Vec<u8>>,
    small_height: usize,
}

const WORDS: usize = MASK_WIDTH.div_ceil(64);

fn decode_cut(cut: &Asset, cancelled: &dyn Fn() -> bool) -> Result<CutFrames> {
    let mask_height = (MASK_WIDTH as u64 * cut.height as u64 / cut.width.max(1) as u64).max(2) as usize;
    let small_height = (SMALL_WIDTH as u64 * cut.height as u64 / cut.width.max(1) as u64).max(2) as usize;
    let mut decoder = VideoDecoder::open(Path::new(&cut.path)).context("Opening the cut")?;
    let (w, h) = if cut.rotation % 180 == 90 { (mask_height, MASK_WIDTH) } else { (MASK_WIDTH, mask_height) };
    let mut out = CutFrames { times: Vec::new(), masks: Vec::new(), mask_height, small: Vec::new(), small_height };
    while let Some((t, frame)) = decoder.next_frame().context("Decoding the cut")? {
        ensure!(!cancelled(), "CANCELLED: style analysis cancelled");
        // Thirty pictures a second are enough to time captions; faster footage keeps memory bounded.
        if out.times.last().is_some_and(|&last| t < last + 30_000) {
            continue;
        }
        let rgba = decoder.convert(&frame, t, w as u32, h as u32)?;
        let (rgba, _, _) = orient(&rgba.data, w as u32, h as u32, cut);
        out.masks.push(caption_mask(&rgba, mask_height));
        let small = Gray::shrink(&rgba, MASK_WIDTH, mask_height, SMALL_WIDTH, small_height);
        out.small.push(small.px.iter().map(|&v| v.round() as u8).collect());
        out.times.push(t);
    }
    Ok(out)
}

/// Bright pixels within two pixels of dark ones: white text with a black outline, or black text on a white box.
fn caption_mask(rgba: &[u8], height: usize) -> Vec<u64> {
    let bits = |test: &dyn Fn(&[u8]) -> bool| -> Vec<u64> {
        let mut out = vec![0u64; WORDS * height];
        for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
            if test(px) {
                let (y, x) = (i / MASK_WIDTH, i % MASK_WIDTH);
                out[y * WORDS + x / 64] |= 1 << (x % 64);
            }
        }
        out
    };
    let white = bits(&|p| p[0].min(p[1]).min(p[2]) >= 200);
    let dark = bits(&|p| p[0].max(p[1]).max(p[2]) <= 70);
    // Grow the dark pixels by two in every direction: along rows by shifting, then across rows.
    let mut wide = vec![0u64; dark.len()];
    for y in 0..height {
        let row = &dark[y * WORDS..(y + 1) * WORDS];
        for k in 0..WORDS {
            let mut v = row[k];
            for s in 1..=2 {
                v |= row[k] << s | row[k] >> s;
                if k > 0 {
                    v |= row[k - 1] >> (64 - s);
                }
                if k + 1 < WORDS {
                    v |= row[k + 1] << (64 - s);
                }
            }
            wide[y * WORDS + k] = v;
        }
    }
    let mut out = vec![0u64; dark.len()];
    for y in 0..height {
        for k in 0..WORDS {
            let near = (y.saturating_sub(2)..=(y + 2).min(height - 1)).fold(0, |v, r| v | wide[r * WORDS + k]);
            out[y * WORDS + k] = white[y * WORDS + k] & near;
        }
    }
    out
}

/// Captions sit in one band of the frame and change all at once; the band is where caption-like
/// pixels are by far the most common.
fn captions(frames: &CutFrames) -> (Vec<Caption>, Option<(f32, f32)>) {
    let height = frames.mask_height;
    let mut rows = vec![0u64; height];
    for mask in &frames.masks {
        for (y, count) in rows.iter_mut().enumerate() {
            *count += mask[y * WORDS..(y + 1) * WORDS].iter().map(|w| w.count_ones() as u64).sum::<u64>();
        }
    }
    let Some((peak, &most)) = rows.iter().enumerate().max_by_key(|(y, c)| (**c, std::cmp::Reverse(*y))) else {
        return (Vec::new(), None);
    };
    // On average at least five caption pixels in the busiest row of every frame.
    if most < 20 * frames.masks.len() as u64 / 4 {
        return (Vec::new(), None);
    }
    let (mut top, mut bottom) = (peak, peak);
    while top > 0 && rows[top - 1] * 20 >= most {
        top -= 1;
    }
    while bottom + 1 < height && rows[bottom + 1] * 20 >= most {
        bottom += 1;
    }
    let (top, bottom) = (top.saturating_sub(2), (bottom + 2).min(height - 1));
    let band = |mask: &Vec<u64>| mask[top * WORDS..(bottom + 1) * WORDS].to_vec();
    let counts: Vec<u32> = frames.masks.iter().map(|m| band(m).iter().map(|w| w.count_ones()).sum::<u32>()).collect();
    let mut sorted = counts.clone();
    sorted.sort_unstable();
    let shown = (sorted[sorted.len() * 9 / 10] as f32 * 0.15).max(20.0) as u32;
    // Shown captions as frame ranges: a caption changes when most of its pixels change.
    let mut shown_at: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<(usize, Vec<u64>)> = None;
    for (i, mask) in frames.masks.iter().enumerate() {
        let current = (counts[i] >= shown).then(|| band(mask));
        let changed = match (&open, &current) {
            (Some((_, before)), Some(now)) => {
                let both: u32 = before.iter().zip(now).map(|(a, b)| (a & b).count_ones()).sum();
                let either: u32 = before.iter().zip(now).map(|(a, b)| (a | b).count_ones()).sum();
                both * 2 < either
            }
            (None, None) => false,
            _ => true,
        };
        if changed {
            if let Some((first, _)) = open.take() {
                shown_at.push((first, i));
            }
            open = current.map(|mask| (i, mask));
        } else if let (Some((_, before)), Some(now)) = (&mut open, current) {
            *before = now;
        }
    }
    if let Some((first, _)) = open {
        shown_at.push((first, frames.times.len()));
    }
    // Letters of a word nearly touch, words are several pixels apart. Where one kind of gap ends
    // is found over all captions at once, and is never under a third of the text height.
    let rows = bottom - top + 1;
    let shapes: Vec<(Vec<usize>, usize)> =
        shown_at.iter().map(|&(first, end)| gaps(&band(&frames.masks[(first + end - 1) / 2]), rows)).collect();
    let mut heights: Vec<usize> = shapes.iter().map(|s| s.1).filter(|&h| h > 0).collect();
    heights.sort_unstable();
    let height_floor = heights.get(heights.len() / 2).map_or(0, |h| h.div_ceil(3));
    let all: Vec<usize> = shapes.iter().flat_map(|s| s.0.iter().copied()).collect();
    let split = otsu(&all).max(height_floor).max(2);
    let captions = shown_at
        .iter()
        .zip(&shapes)
        .map(|(&(first, end), (gaps, _))| Caption {
            start_us: frames.times[first],
            end_us: frames.times.get(end).copied().unwrap_or(frames.times[end - 1] + 33_333),
            words: 1 + gaps.iter().filter(|&&g| g >= split).count(),
        })
        .collect();
    (captions, Some((top as f32 / height as f32, (bottom + 1) as f32 / height as f32)))
}

/// Widths of the empty column runs inside a caption's text, and how many rows the text spans.
fn gaps(band: &[u64], rows: usize) -> (Vec<usize>, usize) {
    let column = |x: usize| (0..rows).any(|r| band[r * WORDS + x / 64] >> (x % 64) & 1 == 1);
    let height = (0..rows).filter(|&r| band[r * WORDS..(r + 1) * WORDS].iter().any(|&w| w != 0)).count();
    let filled: Vec<bool> = (0..MASK_WIDTH).map(column).collect();
    let (Some(first), Some(last)) = (filled.iter().position(|&f| f), filled.iter().rposition(|&f| f)) else {
        return (Vec::new(), 0);
    };
    let mut out = Vec::new();
    let mut run = 0;
    for &f in &filled[first..=last] {
        if f {
            if run > 0 {
                out.push(run);
            }
            run = 0;
        } else {
            run += 1;
        }
    }
    (out, height)
}

/// The value that best splits `values` into a small and a large group (Otsu's method).
fn otsu(values: &[usize]) -> usize {
    let Some(&max) = values.iter().max() else { return 0 };
    let total = values.len() as f64;
    let sum: f64 = values.iter().map(|&v| v as f64).sum();
    let (mut best, mut split) = (0.0, 0);
    for t in 1..=max {
        let low: Vec<f64> = values.iter().filter(|&&v| v < t).map(|&v| v as f64).collect();
        let (n, s) = (low.len() as f64, low.iter().sum::<f64>());
        if n == 0.0 || n == total {
            continue;
        }
        let between = n * (total - n) * (s / n - (sum - s) / (total - n)).powi(2);
        if between > best {
            (best, split) = (between, t);
        }
    }
    split
}

/// Framing at moments spread over every piece, against the recording at the same moment.
fn framing(
    recording: &Asset,
    cut: &Asset,
    frames: &CutFrames,
    pieces: &[Piece],
    keep_rows: &dyn Fn(usize) -> bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<Framing>> {
    let mut wanted: Vec<(i64, usize)> = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        // The sound of a cut lands somewhere in its pause, so frames near either end may still
        // show the neighbouring piece.
        let (start, end) = (piece.start_us + EDGE_US, piece.end_us - EDGE_US);
        if end <= start {
            wanted.push(((piece.start_us + piece.end_us) / 2, index));
            continue;
        }
        let count = ((end - start) / SAMPLE_US).max(1);
        for i in 0..=count {
            wanted.push((start + (end - start) * i / count, index));
        }
    }
    // The recording is read in its own order, seeking only for jumps.
    let mut order: Vec<usize> = (0..wanted.len()).collect();
    order.sort_by_key(|&i| (wanted[i].0 + pieces[wanted[i].1].offset_us, i));
    let mut source = SourceFrames::open(recording)?;
    let mut sources: Vec<Option<Gray>> = vec![None; wanted.len()];
    for i in order {
        ensure!(!cancelled(), "CANCELLED: style analysis cancelled");
        sources[i] = source.at(wanted[i].0 + pieces[wanted[i].1].offset_us)?;
    }
    let fit = contain(recording, cut);
    let mut out = Vec::new();
    let mut previous: Option<(usize, Fit)> = None;
    for (i, &(time, piece)) in wanted.iter().enumerate() {
        let Some(raw) = &sources[i] else { continue };
        let Some(f) = frames.times.iter().rposition(|&t| t <= time) else { continue };
        let frame =
            Gray { w: SMALL_WIDTH, h: frames.small_height, px: frames.small[f].iter().map(|&v| v as f32).collect() };
        let pair = Pair { cut: &frame, raw, fit, keep_rows };
        let tracked = previous.filter(|(p, _)| *p == piece).map(|(_, start)| pair.descend(start));
        let best = match tracked {
            Some(found) if found.score >= MIN_MATCH => found,
            _ => pair.search(),
        };
        if best.score >= MIN_MATCH {
            out.push(Framing { time_us: time, scale: best.scale, x: best.x, y: best.y });
            previous = Some((piece, best));
        } else {
            previous = None;
        }
    }
    Ok(out)
}

/// Zoom changes between consecutive framings. Across a cut the zoom changes at the cut; within a
/// piece one step is an instant zoom and several steps the same way are a gradual move.
fn zooms(
    framing: &[Framing],
    frames: &CutFrames,
    pieces: &[Piece],
    keep_rows: &dyn Fn(usize) -> bool,
) -> Vec<ZoomChange> {
    let piece =
        |k: usize| pieces.iter().position(|p| p.start_us <= framing[k].time_us && framing[k].time_us < p.end_us);
    let step = |k: usize| framing[k + 1].scale - framing[k].scale;
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < framing.len() {
        let (a, b) = (&framing[i], &framing[i + 1]);
        if piece(i) != piece(i + 1) {
            if let Some(next) = piece(i + 1).filter(|_| step(i).abs() >= MIN_ZOOM) {
                let at = pieces[next].start_us;
                out.push(ZoomChange { start_us: at, end_us: at, from: a.scale, to: b.scale, at_cut: true });
            }
            i += 1;
            continue;
        }
        if b.time_us - a.time_us > 2 * SAMPLE_US || step(i).abs() < STEP {
            i += 1;
            continue;
        }
        let sign = step(i).signum();
        let mut j = i;
        while j + 1 < framing.len()
            && piece(j + 1) == piece(i)
            && framing[j + 1].time_us - framing[j].time_us <= 2 * SAMPLE_US
            && step(j) * sign >= STEP
        {
            j += 1;
        }
        let b = &framing[j];
        if (b.scale - a.scale).abs() >= MIN_ZOOM {
            let (start_us, end_us) = if j == i + 1 {
                let at = jump(frames, a.time_us, b.time_us, keep_rows);
                (at, at)
            } else {
                (a.time_us, b.time_us)
            };
            out.push(ZoomChange { start_us, end_us, from: a.scale, to: b.scale, at_cut: false });
        }
        i = j;
    }
    out
}

/// The frame between two times where the picture changes the most, outside the captions.
fn jump(frames: &CutFrames, from: i64, to: i64, keep_rows: &dyn Fn(usize) -> bool) -> i64 {
    let rows: Vec<usize> = (0..frames.small_height).filter(|&r| keep_rows(r)).collect();
    (1..frames.times.len())
        .filter(|&f| frames.times[f] > from && frames.times[f] <= to)
        .map(|f| {
            let (a, b) = (&frames.small[f - 1], &frames.small[f]);
            let diff: u32 = rows
                .iter()
                .flat_map(|&r| (0..SMALL_WIDTH).map(move |x| r * SMALL_WIDTH + x))
                .map(|p| u32::from(a[p].abs_diff(b[p])))
                .sum();
            (frames.times[f], diff)
        })
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map_or((from + to) / 2, |(t, _)| t)
}

/// The recording's size inside the cut's frame at scale 1, as fractions of the frame.
fn contain(recording: &Asset, cut: &Asset) -> (f32, f32) {
    let raw = recording.width.max(1) as f32 / recording.height.max(1) as f32;
    let frame = cut.width.max(1) as f32 / cut.height.max(1) as f32;
    if raw > frame { (1.0, frame / raw) } else { (raw / frame, 1.0) }
}

/// Grey pixels, 0..=255.
#[derive(Clone)]
struct Gray {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

impl Gray {
    /// Averages RGBA into a smaller grey picture.
    fn shrink(rgba: &[u8], w: usize, h: usize, to_w: usize, to_h: usize) -> Self {
        let mut sum = vec![0f32; to_w * to_h];
        let mut count = vec![0f32; to_w * to_h];
        for (i, p) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (i % w * to_w / w, i / w * to_h / h);
            sum[y * to_w + x] += 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
            count[y * to_w + x] += 1.0;
        }
        Self { w: to_w, h: to_h, px: sum.iter().zip(&count).map(|(s, c)| s / c.max(1.0)).collect() }
    }

    fn half(&self) -> Self {
        let (w, h) = (self.w / 2, self.h / 2);
        let at = |x: usize, y: usize| self.px[y * self.w + x];
        let px = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w * 2, i / w * 2);
                (at(x, y) + at(x + 1, y) + at(x, y + 1) + at(x + 1, y + 1)) / 4.0
            })
            .collect();
        Self { w, h, px }
    }

    /// Bilinear sample at fractions of the picture, `None` outside it.
    fn sample(&self, u: f32, v: f32) -> Option<f32> {
        let (x, y) = (u * self.w as f32 - 0.5, v * self.h as f32 - 0.5);
        if !(0.0..=(self.w - 1) as f32).contains(&x) || !(0.0..=(self.h - 1) as f32).contains(&y) {
            return None;
        }
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let row = |y: usize| self.px[y * self.w + x0] * (1.0 - fx) + self.px[y * self.w + x1] * fx;
        Some(row(y0) * (1.0 - fy) + row(y1) * fy)
    }
}

#[derive(Clone, Copy, Debug)]
struct Fit {
    scale: f32,
    x: f32,
    y: f32,
    /// Normalised cross-correlation of the cut with the transformed recording.
    score: f32,
}

struct Pair<'a> {
    cut: &'a Gray,
    raw: &'a Gray,
    fit: (f32, f32),
    keep_rows: &'a dyn Fn(usize) -> bool,
}

impl Pair<'_> {
    fn score(&self, cut: &Gray, raw: &Gray, scale: f32, x: f32, y: f32) -> f32 {
        let (mut n, mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
        let rows_per = self.cut.h / cut.h;
        for row in 0..cut.h {
            if !(self.keep_rows)(row * rows_per) || !(self.keep_rows)(row * rows_per + rows_per - 1) {
                continue;
            }
            let v = (row as f32 + 0.5) / cut.h as f32;
            let vr = (v - 0.5 - y) / (scale * self.fit.1) + 0.5;
            for col in 0..cut.w {
                let u = (col as f32 + 0.5) / cut.w as f32;
                let ur = (u - 0.5 - x) / (scale * self.fit.0) + 0.5;
                let Some(b) = raw.sample(ur, vr) else { continue };
                let a = cut.px[row * cut.w + col];
                n += 1.0;
                (sa, sb, saa, sbb, sab) = (sa + a, sb + b, saa + a * a, sbb + b * b, sab + a * b);
            }
        }
        if n < (cut.w * cut.h) as f32 * 0.4 {
            return -1.0;
        }
        let cov = sab - sa * sb / n;
        let var = ((saa - sa * sa / n) * (sbb - sb * sb / n)).max(1e-6);
        cov / var.sqrt()
    }

    /// Every plausible zoom and position on half-size pictures, then refined.
    fn search(&self) -> Fit {
        let (cut, raw) = (self.cut.half(), self.raw.half());
        let mut best = Fit { scale: 1.0, x: 0.0, y: 0.0, score: -1.0 };
        for s in 0..=22 {
            let scale = 0.9 + s as f32 * 0.05;
            for xi in -5..=5 {
                for yi in -5..=5 {
                    let (x, y) = (xi as f32 * 0.04, yi as f32 * 0.04);
                    let score = self.score(&cut, &raw, scale, x, y);
                    if score > best.score {
                        best = Fit { scale, x, y, score };
                    }
                }
            }
        }
        self.descend(best)
    }

    /// Coordinate descent from `start` on full-size pictures.
    fn descend(&self, start: Fit) -> Fit {
        let mut best = Fit { score: self.score(self.cut, self.raw, start.scale, start.x, start.y), ..start };
        let mut step = (0.04f32, 0.02f32);
        while step.0 >= 0.0025 {
            let moves = [
                (step.0, 0.0, 0.0),
                (-step.0, 0.0, 0.0),
                (0.0, step.1, 0.0),
                (0.0, -step.1, 0.0),
                (0.0, 0.0, step.1),
                (0.0, 0.0, -step.1),
            ];
            let next = moves
                .iter()
                .map(|(ds, dx, dy)| {
                    let (scale, x, y) = (best.scale + ds, best.x + dx, best.y + dy);
                    Fit { scale, x, y, score: self.score(self.cut, self.raw, scale, x, y) }
                })
                .max_by(|a, b| a.score.total_cmp(&b.score));
            match next {
                Some(next) if next.score > best.score + 1e-5 => best = next,
                _ => step = (step.0 / 2.0, step.1 / 2.0),
            }
        }
        best
    }
}

/// Recording frames in display orientation, read forward and seeking only for jumps.
struct SourceFrames {
    decoder: VideoDecoder,
    asset: Asset,
    size: (usize, usize),
    frames: [Option<nuzky_engine::media::DecodedFrame>; 2],
    position: Option<i64>,
}

impl SourceFrames {
    fn open(asset: &Asset) -> Result<Self> {
        let decoder = VideoDecoder::open(Path::new(&asset.path)).context("Opening the recording")?;
        let height = (MASK_WIDTH as u64 * asset.height as u64 / asset.width.max(1) as u64).max(2) as usize;
        Ok(Self { decoder, asset: asset.clone(), size: (MASK_WIDTH, height), frames: [None, None], position: None })
    }

    fn at(&mut self, time: i64) -> Result<Option<Gray>> {
        if self.position.is_none_or(|p| time < p || time > p + 2_000_000) {
            self.decoder.seek(time)?;
            self.frames = [None, None];
        }
        self.position = Some(time);
        self.frames = self.decoder.frame_covering(time, std::mem::take(&mut self.frames))?;
        let Some((t, frame)) = self.frames[0].as_ref().or(self.frames[1].as_ref()) else { return Ok(None) };
        let (w, h) = if self.asset.rotation % 180 == 90 { (self.size.1, self.size.0) } else { self.size };
        let rgba = self.decoder.convert(frame, *t, w as u32, h as u32)?;
        let (rgba, _, _) = orient(&rgba.data, w as u32, h as u32, &self.asset);
        let small_height = (SMALL_WIDTH as u64 * self.size.1 as u64 / self.size.0 as u64).max(2) as usize;
        Ok(Some(Gray::shrink(&rgba, self.size.0, self.size.1, SMALL_WIDTH, small_height)))
    }
}
