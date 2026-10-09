//! Picking frames for a cover or thumbnail in two rounds. A cheap look at the whole timeline
//! (sharpness, exposure, a face) shortlists the best moments; a closer look at those (eyes, smile,
//! mouth, gaze, face sharpness, framing) scores them, and the best few far enough apart win.
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use nuzky_engine::{Project, Renderer};
use serde::Serialize;

use crate::frame::{Frame, Rect, exposure, sharpness};
use crate::framing::{self, Format};
use crate::models;
use crate::nets::{self, Blendshapes, Detection, FaceMesh, Selfie, Yunet, check_cancel};
use crate::timeline;

/// Long side of the cheap look; YuNet finds faces down to about 10 px there.
const LOOK_SIDE: u32 = 384;
const LOOK_EVERY_US: i64 = 250_000;
/// Looks per timeline; a longer timeline is looked at more sparsely instead of for longer.
const MAX_LOOKS: usize = 1_200;
const SHORTLIST: usize = 40;
const SHORTLIST_GAP_US: i64 = 500_000;
/// Long side of the close look, where the face crop and its sharpness come from.
const DETAIL_SIDE: u32 = 1280;
/// The detector's view of the close look.
const DETECT_SIDE: u32 = 640;
const CANDIDATES: usize = 8;
const CANDIDATE_GAP_US: i64 = 2_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The cheap look over the whole timeline.
    Looking,
    /// The close look at the shortlist.
    Scoring,
}

#[derive(Debug, Serialize)]
pub struct FaceBox {
    #[serde(rename = "box")]
    pub rect: [i64; 4],
    pub confidence: f32,
}

/// One frame worth a cover, in canvas pixels and timeline microseconds.
#[derive(Debug, Serialize)]
pub struct Candidate {
    pub time_us: i64,
    pub score: f32,
    /// Each 0..1, higher is better; face parts are missing when no face was found.
    pub parts: BTreeMap<&'static str, f32>,
    pub faces: Vec<FaceBox>,
    /// The person's outline box, when there is a person.
    pub subject_box: Option<[i64; 4]>,
    /// Where a 9:16 cover and a 16:9 thumbnail would be cut from the canvas.
    pub crops: BTreeMap<&'static str, [i64; 4]>,
}

struct Look {
    t: i64,
    sharp: f32,
    exposure: f32,
    /// Largest face, in canvas pixels.
    face: Option<Detection>,
}

struct Close {
    t: i64,
    exposure: f32,
    sharp: f32,
    /// Largest first, in canvas pixels.
    faces: Vec<Detection>,
    expression: Option<nets::Expression>,
    person: Option<Rect>,
}

fn largest_first(faces: &mut [Detection]) {
    faces.sort_by(|a, b| (b.rect[2] * b.rect[3]).total_cmp(&(a.rect[2] * a.rect[3])));
}

fn int(rect: Rect) -> [i64; 4] {
    rect.map(|v| v.round() as i64)
}

fn centre(rect: Rect) -> [f32; 2] {
    [rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0]
}

fn whole_sharpness(frame: &Frame) -> f32 {
    sharpness(&frame.luma(), frame.width, frame.height, [0.0, 0.0, frame.width as f32, frame.height as f32])
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Greedy best-first picks at least `gap` apart; ties go to the earlier time.
fn spaced<T>(mut items: Vec<(f32, i64, T)>, count: usize, gap: i64) -> Vec<(f32, i64, T)> {
    items.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut kept: Vec<(f32, i64, T)> = Vec::new();
    for item in items {
        if kept.len() == count {
            break;
        }
        if kept.iter().all(|k| (k.1 - item.1).abs() >= gap) {
            kept.push(item);
        }
    }
    kept
}

/// Worker threads, each with its own renderer and models. A decoder already uses four threads.
fn workers() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get() / 4).clamp(1, 4)
}

/// Runs `work` over `items` in contiguous runs, one per worker with the state `start` makes, and
/// keeps their order. Reports the share done; the first failure stops every worker.
fn in_parallel<I: Sync, O: Send, S>(
    items: &[I],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
    start: impl Fn() -> Result<S> + Sync,
    work: impl Fn(&mut S, &I, &AtomicBool) -> Result<O> + Sync,
) -> Result<Vec<O>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let done = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let run = items.len().div_ceil(workers());
    let (start, work, done, failed) = (&start, &work, &done, &failed);
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(run)
            .map(|part| {
                scope.spawn(move || {
                    let result = (|| {
                        let mut state = start()?;
                        let mut out = Vec::with_capacity(part.len());
                        for item in part {
                            check_cancel(cancel)?;
                            anyhow::ensure!(!failed.load(Ordering::Relaxed), "CANCELLED: another worker failed");
                            out.push(work(&mut state, item, cancel)?);
                            done.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(out)
                    })();
                    if result.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    result
                })
            })
            .collect();
        while handles.iter().any(|h| !h.is_finished()) {
            progress(done.load(Ordering::Relaxed) as f32 / items.len() as f32);
            std::thread::sleep(Duration::from_millis(50));
        }
        let results: Vec<Result<Vec<O>>> = handles
            .into_iter()
            .map(|h| h.join().map_err(|_| anyhow::anyhow!("JOB_FAILED: frame worker panicked")).and_then(|r| r))
            .collect();
        // The worker that failed first explains it; the others only stopped because of it.
        let cancelled = |e: &anyhow::Error| format!("{e:#}").starts_with("CANCELLED");
        let mut errors: Vec<anyhow::Error> = Vec::new();
        let mut all = Vec::with_capacity(items.len());
        for result in results {
            match result {
                Ok(part) => all.extend(part),
                Err(error) => errors.push(error),
            }
        }
        match errors.iter().position(|e| !cancelled(e)).or((!errors.is_empty()).then_some(0)) {
            Some(i) => Err(errors.swap_remove(i)),
            None => Ok(all),
        }
    })
}

/// Up to [`CANDIDATES`] frames for a cover, best first, at least [`CANDIDATE_GAP_US`] apart.
/// `format` decides which framing counts in the score; by default the canvas's own.
pub fn thumbnail_frames(
    project: &Project,
    models_dir: &Path,
    format: Option<Format>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Phase, f32),
) -> Result<Vec<Candidate>> {
    models::require(models::FRAMES, models_dir)?;
    let picture = timeline::picture(project);
    anyhow::ensure!(picture.duration_us() > 0, "INVALID_RANGE: the timeline is empty");
    let canvas = picture.canvas.clone();
    let format = format.unwrap_or(Format::of(&canvas));
    let unsettled = timeline::unsettled(&picture);
    let times: Vec<i64> = timeline::times(&picture, LOOK_EVERY_US, MAX_LOOKS)
        .into_iter()
        .filter(|&t| timeline::settled(&unsettled, t))
        .collect();

    // Cheap round over the whole timeline.
    let look_size = timeline::size(&picture, LOOK_SIDE);
    let look_to_canvas = canvas.width as f32 / look_size.0 as f32;
    let looks = in_parallel(
        &times,
        cancel,
        &mut |done| progress(Phase::Looking, done),
        || Ok((Renderer::new().context("Starting frame renderer")?, Yunet::load(models_dir)?)),
        |(renderer, yunet), &t, cancel| {
            let frame = timeline::render(renderer, &picture, t, look_size)?;
            let luma = frame.luma();
            let whole = [0.0, 0.0, frame.width as f32, frame.height as f32];
            let mut faces = yunet.detect(&frame, cancel)?;
            largest_first(&mut faces);
            Ok(Look {
                t,
                sharp: sharpness(&luma, frame.width, frame.height, whole),
                exposure: exposure(&luma),
                face: faces.first().map(|f| f.scaled(look_to_canvas)),
            })
        },
    )?;
    let mut sharps: Vec<f32> = looks.iter().map(|l| l.sharp).collect();
    sharps.sort_by(f32::total_cmp);
    let p90 = sharps.get(sharps.len() * 9 / 10).copied().unwrap_or(1.0).max(1e-3);
    let cheap: Vec<_> = looks
        .into_iter()
        .map(|look| {
            let face = look.face.as_ref().map_or(0.0, |f| {
                f.score * framing::score(format, framing::crop(&canvas, format, centre(f.rect)), f.rect)
            });
            (0.45 * (look.sharp / p90).min(1.0) + 0.2 * look.exposure + 0.35 * face, look.t, look)
        })
        .collect();
    let mut shortlist: Vec<Look> = spaced(cheap, SHORTLIST, SHORTLIST_GAP_US).into_iter().map(|(_, _, l)| l).collect();
    // In time order each worker's decoders read forward instead of seeking back and forth.
    shortlist.sort_by_key(|look| look.t);

    // Close round over the shortlist.
    let detail_size = timeline::size(&picture, DETAIL_SIDE.min(canvas.width.max(canvas.height)));
    let detail_to_canvas = canvas.width as f32 / detail_size.0 as f32;
    let close = in_parallel(
        &shortlist,
        cancel,
        &mut |done| progress(Phase::Scoring, done),
        || {
            Ok((
                Renderer::new().context("Starting frame renderer")?,
                Yunet::load(models_dir)?,
                FaceMesh::load(models_dir)?,
                Blendshapes::load(models_dir)?,
                Selfie::load(models_dir)?,
            ))
        },
        |(renderer, yunet, mesh, blend, selfie), look, cancel| {
            let frame = timeline::render(renderer, &picture, look.t, detail_size)?;
            let small = frame.fit(DETECT_SIDE);
            let k = frame.width as f32 / small.width as f32;
            let mut faces: Vec<Detection> = yunet.detect(&small, cancel)?.iter().map(|f| f.scaled(k)).collect();
            largest_first(&mut faces);
            let mut expression = None;
            let mut sharp = None;
            if let Some(face) = faces.first()
                && let Some(landmarks) = mesh.landmarks(&frame, face, cancel)?
            {
                let side = landmarks.crop.width;
                sharp = Some(sharpness(&landmarks.crop.luma(), side, side, [0.0, 0.0, side as f32, side as f32]));
                expression = Some(blend.expression(&landmarks, cancel)?);
            }
            let person = selfie.person(&frame, cancel)?;
            Ok(Close {
                t: look.t,
                exposure: look.exposure,
                sharp: sharp.unwrap_or_else(|| whole_sharpness(&frame)),
                faces: faces.iter().map(|f| f.scaled(detail_to_canvas)).collect(),
                expression,
                person: person.map(|r| r.map(|v| v * detail_to_canvas)),
            })
        },
    )?;
    progress(Phase::Scoring, 1.0);
    let scored = score(close, &canvas, format).into_iter().map(|c| (c.score, c.time_us, c)).collect();
    Ok(spaced(scored, CANDIDATES, CANDIDATE_GAP_US).into_iter().map(|(_, _, c)| c).collect())
}

/// Sharpness counts relative to the sharpest of the shortlist: the same measure on a face crop
/// and on a whole frame are not comparable, so each is compared with its own kind.
fn score(close: Vec<Close>, canvas: &nuzky_engine::model::Canvas, format: Format) -> Vec<Candidate> {
    let with_faces = close.iter().any(|c| c.expression.is_some());
    // This face's usual smile, so a mouth that looks smiling at rest does not count as a smile.
    let mut smiles: Vec<f32> = close.iter().filter_map(|c| c.expression.as_ref().map(|e| e.smile)).collect();
    smiles.sort_by(f32::total_cmp);
    let usual_smile = smiles.get(smiles.len() / 2).copied().unwrap_or(0.0);
    let best =
        |face: bool| close.iter().filter(|c| c.expression.is_some() == face).map(|c| c.sharp).fold(1e-3f32, f32::max);
    let (face_sharp, frame_sharp) = (best(true), best(false));
    close
        .into_iter()
        .map(|c| {
            let mut parts = BTreeMap::from([("exposure", c.exposure)]);
            let focus = c.faces.first().map(|f| centre(f.rect)).or(c.person.map(centre));
            let focus = focus.unwrap_or([canvas.width as f32 / 2.0, canvas.height as f32 / 2.0]);
            let crops = BTreeMap::from(
                [("9:16", Format::Vertical), ("16:9", Format::Wide)]
                    .map(|(name, f)| (name, int(framing::crop(canvas, f, focus)))),
            );
            let score = match (&c.expression, c.faces.first()) {
                (Some(e), Some(face)) => {
                    let sharp = (c.sharp / face_sharp).min(1.0);
                    let eyes_open = 1.0 - smoothstep(0.35, 0.65, e.blink);
                    let pleased = 1.0 - smoothstep(0.2, 0.5, e.frown);
                    let smile = smoothstep(0.0, 0.3, e.smile - usual_smile) * pleased;
                    // Lips apart or pursed are a word being said; a broad smile may show teeth.
                    let laugh = 0.7 * smoothstep(0.1, 0.3, e.smile - usual_smile) * pleased;
                    let closed = 1.0 - smoothstep(0.03, 0.12, e.lips_apart);
                    let mouth = closed.max(laugh) * (1.0 - smoothstep(0.15, 0.4, e.purse));
                    let facing =
                        (1.0 - smoothstep(0.35, 0.75, e.look_away)) * (1.0 - smoothstep(0.1, 0.35, e.yaw.abs()));
                    let mut framed = 0.0;
                    for (key, f) in [("framing_9x16", Format::Vertical), ("framing_16x9", Format::Wide)] {
                        let value = framing::score(f, framing::crop(canvas, f, focus), face.rect);
                        parts.insert(key, value);
                        if f == format {
                            framed = value;
                        }
                    }
                    parts.extend([
                        ("sharpness", sharp),
                        ("eyes_open", eyes_open),
                        ("mouth", mouth),
                        ("facing", facing),
                        ("smile", smile),
                    ]);
                    let quality = 0.30 * sharp + 0.10 * c.exposure + 0.25 * framed + 0.15 * smile + 0.20 * facing;
                    // Closed eyes or a mouth caught mid-word spoil a cover whatever else is right.
                    quality * (0.25 + 0.75 * eyes_open) * (0.6 + 0.4 * mouth)
                }
                _ => {
                    let sharp = (c.sharp / frame_sharp).min(1.0);
                    parts.insert("sharpness", sharp);
                    // In a video with faces, a frame without one makes a weak cover.
                    (0.7 * sharp + 0.3 * c.exposure) * if with_faces { 0.5 } else { 1.0 }
                }
            };
            Candidate {
                time_us: c.t,
                score,
                parts,
                faces: c.faces.iter().map(|f| FaceBox { rect: int(f.rect), confidence: f.score }).collect(),
                subject_box: c.person.map(int),
                crops,
            }
        })
        .collect()
}
