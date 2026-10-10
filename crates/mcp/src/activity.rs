//! Where something happens on the timeline, read from tiny renders and the mix, so an agent looks
//! at frames only where the picture changes.
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use nuzky_analysis::picture_difference;
use nuzky_engine::{
    Project, Renderer, Wait,
    audio::{Mixer, us_to_samples},
    model::CHANNELS,
};
use serde_json::{Value, json};

/// Long side of the compared renders, the size scene analysis decodes at.
const LOOK_SIDE: u32 = 64;
const LOOK_EVERY_US: i64 = 250_000;
/// Looks per activity call, so a long range samples more sparsely instead of taking longer.
const MAX_LOOKS: i64 = 240;
const MOTION_US: i64 = 100_000;
const MIN_STEP_US: i64 = 10_000;
const PEAKS: usize = 6;
/// Candidates per changes range; a longer range spaces them further apart.
const MAX_CANDIDATES: i64 = 120;
/// A candidate is kept when it differs from each of the last kept frames.
const HISTORY: usize = 4;
pub const DEFAULT_MIN_CHANGE: u32 = 16;
const MIX_FRAMES: i64 = 48_000;

/// Renders the timeline at a size small enough to compare cheaply.
struct Looker {
    renderer: Renderer,
    width: u32,
    height: u32,
}

impl Looker {
    fn new(project: &Project) -> Result<Self> {
        let (w, h) = (project.canvas.width.max(1), project.canvas.height.max(1));
        let (width, height) = if w >= h { (LOOK_SIDE, LOOK_SIDE * h / w) } else { (LOOK_SIDE * w / h, LOOK_SIDE) };
        let renderer = Renderer::new().context("Starting frame renderer")?;
        Ok(Self { renderer, width: width.max(9), height: height.max(8) })
    }

    fn look(&mut self, project: &Project, t_us: i64, cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>> {
        if cancelled() {
            bail!("CANCELLED: looking at the timeline cancelled");
        }
        self.renderer
            .render(project, t_us, self.width, self.height, Wait::Exact, false)
            .context("Rendering the timeline")
    }

    fn hash(&mut self, project: &Project, t_us: i64, cancelled: &dyn Fn() -> bool) -> Result<u64> {
        Ok(dhash(&self.look(project, t_us, cancelled)?, self.width, self.height))
    }
}

/// The half-open range to read, the whole timeline when none is given.
pub fn timeline_range(project: &Project, range: Option<[i64; 2]>) -> Result<(i64, i64)> {
    let duration = project.duration_us();
    ensure!(duration > 0, "INVALID_RANGE: the timeline is empty");
    let [start, end] = range.unwrap_or([0, duration]);
    let end = end.min(duration);
    ensure!(start >= 0 && start < end, "INVALID_RANGE: range_us must be a half-open range inside 0..{duration}");
    Ok((start, end))
}

/// Curves of picture change, motion and loudness in `points` steps of the range, with their peaks.
pub fn activity(
    project: &Project,
    cache: &Path,
    (start, end): (i64, i64),
    points: usize,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(f32),
) -> Result<Value> {
    let n = (points as i64).min((end - start) / MIN_STEP_US).max(1);
    let step = (end - start) / n;
    let bucket = |i: i64| (start + i * step, if i + 1 == n { end } else { start + (i + 1) * step });
    let per_step = (step / LOOK_EVERY_US).clamp(1, (MAX_LOOKS / n).max(1));
    let every = step / per_step;
    let motion_us = MOTION_US.min(every / 2).max(1);
    let last = project.duration_us() - 1;
    // Where the picture changes does not depend on what is behind a person, which would need their outline made.
    let project = &nuzky_engine::matte::without_backgrounds(project);
    let mut looker = Looker::new(project)?;
    let mut previous = match start - every {
        before if before >= 0 => Some(looker.look(project, before, cancelled)?),
        _ => None,
    };
    // The biggest jump between consecutive looks in each step and when it showed.
    let mut change: Vec<(f32, i64)> = (0..n).map(|i| (0.0, bucket(i).0)).collect();
    let mut motion = vec![0.0f32; n as usize];
    for i in 0..n {
        progress(i as f32 / n as f32);
        for j in 0..per_step {
            let t = bucket(i).0 + j * every;
            let picture = looker.look(project, t, cancelled)?;
            if let Some(previous) = &previous {
                let jump = picture_difference(previous, &picture);
                if jump > change[i as usize].0 {
                    change[i as usize] = (jump, t);
                }
            }
            let moved = looker.look(project, (t + motion_us).min(last), cancelled)?;
            motion[i as usize] += picture_difference(&picture, &moved) / per_step as f32;
            previous = Some(picture);
        }
    }
    let mut mixer = Mixer::new(cache.to_path_buf());
    let mut buffer = vec![0f32; MIX_FRAMES as usize * CHANNELS];
    let mut loudness = Vec::with_capacity(n as usize);
    for i in 0..n {
        let (from, to) = bucket(i);
        let (mut at, to) = (us_to_samples(from), us_to_samples(to));
        let (frames, mut power) = (to - at, 0.0f64);
        while at < to {
            if cancelled() {
                bail!("CANCELLED: activity cancelled");
            }
            let chunk = &mut buffer[..(to - at).min(MIX_FRAMES) as usize * CHANNELS];
            mixer.mix(project, at, chunk);
            power += chunk.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            at += (chunk.len() / CHANNELS) as i64;
        }
        let rms = power / (frames.max(1) as f64 * CHANNELS as f64);
        loudness.push((10.0 * rms.max(1e-12).log10()).max(-120.0).round());
    }
    let percent = |v: f32| (f64::from(v) * 1000.0).round() / 10.0;
    let change_values: Vec<f64> = change.iter().map(|c| percent(c.0)).collect();
    let motion_values: Vec<f64> = motion.iter().map(|m| percent(*m)).collect();
    let starts: Vec<i64> = (0..n).map(|i| bucket(i).0).collect();
    let jumps: Vec<i64> = change.iter().map(|c| c.1).collect();
    Ok(json!({
        "range_us": [start, end],
        "points": n,
        "step_us": step,
        "look_every_us": every,
        "change": numbers(&change_values),
        "motion": numbers(&motion_values),
        "loudness_db": numbers(&loudness),
        "peaks": {
            "change": peaks(&change_values, &jumps, |median| (3.0 * median).max(2.0)),
            "motion": peaks(&motion_values, &starts, |median| (2.0 * median).max(1.0)),
            "loudness_db": peaks(&loudness, &starts, |median| median + 6.0),
        },
    }))
}

/// Whole values without a decimal point, which saves a few characters per point.
fn number(value: f64) -> Value {
    if value.fract() == 0.0 { json!(value as i64) } else { json!(value) }
}

fn numbers(values: &[f64]) -> Vec<Value> {
    values.iter().copied().map(number).collect()
}

/// The strongest local maxima at or above `floor(median)`, in time order.
fn peaks(values: &[f64], times: &[i64], floor: impl Fn(f64) -> f64) -> Vec<Value> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let floor = floor(sorted[sorted.len() / 2]);
    let mut found: Vec<usize> = (0..values.len())
        .filter(|&i| {
            let v = values[i];
            v >= floor && (i == 0 || values[i - 1] <= v) && values.get(i + 1).is_none_or(|next| *next < v)
        })
        .collect();
    found.sort_by(|a, b| values[*b].total_cmp(&values[*a]));
    found.truncate(PEAKS);
    found.sort();
    found.into_iter().map(|i| json!({"t_us": times[i], "value": number(values[i])})).collect()
}

/// Where a changes scan continues: its grid of candidates, how far it got and the hashes of the
/// last kept frames, so the next page repeats none of them.
#[derive(Debug, PartialEq)]
pub struct Cursor {
    session_epoch: String,
    revision: u64,
    start: i64,
    end: i64,
    every: i64,
    index: i64,
    min_change: u32,
    history: Vec<u64>,
}

impl Cursor {
    pub fn new(session_epoch: &str, revision: u64, (start, end): (i64, i64), min_change: u32) -> Self {
        Self {
            session_epoch: session_epoch.to_owned(),
            revision,
            start,
            end,
            every: candidate_every(start, end),
            index: 0,
            min_change,
            history: Vec::new(),
        }
    }

    fn encode(&self) -> String {
        let fields = [self.revision as i64, self.start, self.end, self.every, self.index, self.min_change as i64];
        let history = self.history.iter().map(|h| format!("{h:016x}"));
        std::iter::once(self.session_epoch.clone())
            .chain(fields.iter().map(i64::to_string))
            .chain(history)
            .collect::<Vec<_>>()
            .join(".")
    }

    /// Only a cursor this session handed out for the project as it is now; anything else could
    /// skip new pictures or scan far more candidates than a page allows.
    pub fn decode(text: &str, session_epoch: &str, revision: u64) -> Result<Self> {
        let invalid = || anyhow::anyhow!("INVALID_ARGUMENTS: cursor is not a next value from inspect_frames");
        let parts: Vec<&str> = text.split('.').collect();
        ensure!((7..=7 + HISTORY).contains(&parts.len()), invalid());
        let number = |i: usize| parts[i].parse::<i64>().map_err(|_| invalid());
        let cursor = Self {
            session_epoch: parts[0].to_owned(),
            revision: number(1)?.try_into().map_err(|_| invalid())?,
            start: number(2)?,
            end: number(3)?,
            every: number(4)?,
            index: number(5)?,
            min_change: number(6)?.try_into().map_err(|_| invalid())?,
            history: parts[7..]
                .iter()
                .map(|h| u64::from_str_radix(h, 16).map_err(|_| invalid()))
                .collect::<Result<_>>()?,
        };
        ensure!(
            cursor.start >= 0
                && cursor.start < cursor.end
                && cursor.every == candidate_every(cursor.start, cursor.end)
                && (0..=MAX_CANDIDATES).contains(&cursor.index)
                && cursor.min_change < 64,
            invalid()
        );
        ensure!(
            cursor.session_epoch == session_epoch && cursor.revision == revision,
            "STALE_REVISION: the project changed since this cursor; scan the range again without cursor"
        );
        Ok(cursor)
    }

    pub fn range(&self) -> (i64, i64) {
        (self.start, self.end)
    }

    pub fn every(&self) -> i64 {
        self.every
    }
}

/// Candidates 0.25 s apart, further apart when a range would hold more than `MAX_CANDIDATES`.
fn candidate_every(start: i64, end: i64) -> i64 {
    LOOK_EVERY_US.max((end - start - 1) / MAX_CANDIDATES + 1)
}

pub struct Changes {
    pub times: Vec<i64>,
    pub skipped: usize,
    pub next: Option<String>,
}

/// Up to `max` frames of the cursor's range that look unlike the frames kept before them.
pub fn changes(project: &Project, mut cursor: Cursor, max: usize, cancelled: &dyn Fn() -> bool) -> Result<Changes> {
    ensure!(cursor.end <= project.duration_us(), "INVALID_RANGE: the cursor reaches past the timeline");
    let project = &nuzky_engine::matte::without_backgrounds(project);
    let mut looker = Looker::new(project)?;
    let (mut times, mut skipped) = (Vec::new(), 0);
    loop {
        let t = cursor.start + cursor.index * cursor.every;
        if t >= cursor.end {
            return Ok(Changes { times, skipped, next: None });
        }
        let hash = looker.hash(project, t, cancelled)?;
        if cursor.history.iter().all(|kept| (kept ^ hash).count_ones() > cursor.min_change) {
            if times.len() == max {
                return Ok(Changes { times, skipped, next: Some(cursor.encode()) });
            }
            times.push(t);
            cursor.history.push(hash);
            if cursor.history.len() > HISTORY {
                cursor.history.remove(0);
            }
        } else {
            skipped += 1;
        }
        cursor.index += 1;
    }
}

/// Difference hash: whether each of 9×8 cells of mean luma is darker than its right neighbour.
/// Pictures that look alike differ in few of the 64 bits.
fn dhash(rgba: &[u8], width: u32, height: u32) -> u64 {
    let span = |cell: u32, cells: u32, size: u32| {
        (cell * size / cells, ((cell + 1) * size / cells).max(cell * size / cells + 1))
    };
    let mut cells = [[0f32; 9]; 8];
    for (row, line) in cells.iter_mut().enumerate() {
        let (y0, y1) = span(row as u32, 8, height);
        for (col, cell) in line.iter_mut().enumerate() {
            let (x0, x1) = span(col as u32, 9, width);
            let mut sum = 0.0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = &rgba[((y * width + x) * 4) as usize..];
                    sum += 0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]);
                }
            }
            *cell = sum / ((y1 - y0) * (x1 - x0)) as f32;
        }
    }
    cells.iter().flat_map(|line| line.windows(2)).fold(0, |hash, pair| hash << 1 | u64::from(pair[0] < pair[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_and_refuses_another_project_state_or_a_wider_scan() {
        let mut cursor = Cursor::new("e1", 7, (1_000, 61_001_000), 12);
        assert_eq!(cursor.every, 508_334);
        cursor.index = 9;
        cursor.history = vec![0, u64::MAX, 0x0123_4567_89ab_cdef];
        assert_eq!(Cursor::decode(&cursor.encode(), "e1", 7).unwrap(), cursor);
        for (epoch, revision) in [("e1", 8), ("e2", 7)] {
            let stale = Cursor::decode(&cursor.encode(), epoch, revision).unwrap_err().to_string();
            assert!(stale.starts_with("STALE_REVISION"), "{stale}");
        }
        // A grid other than the one a page hands out would scan more than 120 candidates.
        for bad in [
            "",
            "e1.7.0.1.1.0",
            "e1.7.5.5.250000.0.12",
            "e1.7.0.10000000.1.0.12",
            "e1.7.0.10000000.250000.121.12",
            "e1.7.0.10000000.250000.0.64",
            "e1.7.0.10000000.250000.0.12.zz",
            "e1.x.0.10000000.250000.0.12",
        ] {
            assert!(Cursor::decode(bad, "e1", 7).unwrap_err().to_string().starts_with("INVALID_ARGUMENTS"), "{bad}");
        }
        assert!(Cursor::decode("e1.7.0.10000000.250000.40.12", "e1", 7).is_ok());
        // A huge end decodes without overflowing; changes then refuses it as past the timeline.
        assert!(Cursor::decode("e1.7.0.9223372036854775807.76861433640456466.0.12", "e1", 7).is_ok());
    }

    #[test]
    fn dhash_tells_patterns_apart_and_ignores_brightness() {
        let picture = |f: &dyn Fn(u32, u32) -> u8| {
            (0..64 * 36).flat_map(|i| [f(i % 64, i / 64); 3].into_iter().chain([255])).collect::<Vec<u8>>()
        };
        let ramp = picture(&|x, _| (x * 4) as u8);
        let brighter = picture(&|x, _| (x * 3 + 40) as u8);
        let reversed = picture(&|x, _| 255 - (x * 4) as u8);
        assert_eq!(dhash(&ramp, 64, 36), dhash(&brighter, 64, 36));
        assert_eq!((dhash(&ramp, 64, 36) ^ dhash(&reversed, 64, 36)).count_ones(), 64);
    }

    #[test]
    fn peaks_are_the_strongest_local_maxima_in_time_order() {
        let values = [0.0, 30.0, 0.0, 5.0, 5.0, 0.0, 50.0, 1.0, 0.0];
        let times: Vec<i64> = (0..9).collect();
        let found = peaks(&values, &times, |median| (3.0 * median).max(2.0));
        assert_eq!(
            found,
            vec![json!({"t_us":1,"value":30}), json!({"t_us":4,"value":5}), json!({"t_us":6,"value":50})]
        );
        assert!(peaks(&[0.0; 4], &[0, 1, 2, 3], |m| (3.0 * m).max(2.0)).is_empty());
    }
}
