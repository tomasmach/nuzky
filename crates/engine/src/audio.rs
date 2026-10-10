//! Sample-exact timeline mixing from per-asset PCM caches (48 kHz stereo f32).

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use memmap2::Mmap;

use crate::effects::transition_window;
use crate::loudness::db_to_gain;
use crate::media::extract_pcm_with_peaks;
use crate::model::{Asset, AssetKind, CHANNELS, Clip, ClipContent, Project, SAMPLE_RATE, Track, TrackKind};
use crate::speech::is_heard;
use crate::stretch::Stretch;

/// Short fades at clip edges with nothing to crossfade with, so they do not click.
pub(crate) const EDGE_FADE: i64 = (SAMPLE_RATE / 200) as i64; // 5 ms
/// How far a cut's crossfade reaches on each side of it.
const CUT_FADE_US: i64 = 10_000;
/// Sound past a cut joins the crossfade only where it is quiet: below this level (-45 dBFS), or
/// this far below (-20 dB) the loudest moment of the sound kept next to the cut. Otherwise it may
/// be the start of a deleted word, which must not come back.
pub(crate) const QUIET_RMS: f32 = 0.0056;
const QUIET_BELOW_KEPT: f32 = 0.1;
const KEPT_CONTEXT_US: i64 = 200_000;
/// Ducking listens to speech in 10 ms blocks of the files: a block louder than -40 dBFS is speech.
const SPEECH_BLOCK: i64 = (SAMPLE_RATE / 100) as i64;
const SPEECH_RMS: f32 = 0.01;
/// A ducked clip starts going down this long before speech, so the first word is clear, stays down
/// through gaps between words up to the hold and comes back over the release.
const DUCK_ATTACK: i64 = SAMPLE_RATE as i64 * 150 / 1000;
const DUCK_HOLD: i64 = SAMPLE_RATE as i64 * 250 / 1000;
const DUCK_RELEASE: i64 = SAMPLE_RATE as i64 * 400 / 1000;

/// Where a cut is in the files on both sides: their caches, where each side stops or starts in its
/// file and its speed.
type CutKey = (PathBuf, i64, u32, PathBuf, i64, u32);

pub fn us_to_samples(us: i64) -> i64 {
    (us as i128 * SAMPLE_RATE as i128 / 1_000_000) as i64
}

pub fn samples_to_us(s: i64) -> i64 {
    (s as i128 * 1_000_000 / SAMPLE_RATE as i128) as i64
}

pub struct Pcm {
    map: Mmap,
}

impl Pcm {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        Ok(Self { map: unsafe { Mmap::map(&file)? } })
    }

    pub fn samples(&self) -> &[f32] {
        let len = self.map.len() / 4 * 4;
        bytemuck::cast_slice(&self.map[..len])
    }

    pub fn frames(&self) -> usize {
        self.samples().len() / CHANNELS
    }
}

pub fn has_audio(asset: &Asset) -> bool {
    asset.has_audio && asset.kind != AssetKind::Image
}

/// All consumers use the same source revision, including desktop waveform extraction. The name
/// holds the asset id, the source's size and time and a hash of its path, since a copied project
/// can reuse an asset id for another file of the same size and time.
pub fn pcm_path(cache_dir: &Path, asset: &Asset) -> PathBuf {
    cache_dir.join("pcm").join(format!("{}.{}.{PCM_VERSION}.f32", asset.id, crate::media::file_key(&asset.path)))
}

/// Raised whenever extraction changes its output, so caches made before are extracted again.
/// v4: gaps are filled by timestamp for long audio-only recordings too.
pub(crate) const PCM_VERSION: &str = "v4";

/// Raised whenever waveform peaks change, so peaks made before are made again. Their name also carries
/// the extraction's version, since they describe its samples.
const PEAKS_VERSION: &str = "p1";

/// Where the waveform peaks of the cache at `pcm` are: written while it is extracted.
fn peaks_for(pcm: &Path) -> PathBuf {
    pcm.with_extension(format!("{PEAKS_VERSION}.peaks"))
}

/// Removes this asset's caches and peaks from older versions, which nothing reads any more.
/// Other revisions of the current version stay: a project may still play from them.
fn remove_older_versions(path: &Path, asset: &Asset) {
    let (Some(dir), prefix) = (path.parent(), format!("{}.", asset.id)) else {
        return;
    };
    let (current, current_peaks) = (format!(".{PCM_VERSION}.f32"), format!(".{PCM_VERSION}.{PEAKS_VERSION}.peaks"));
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let cache = name.strip_suffix(".lock").unwrap_or(&name);
        let old_cache = cache.ends_with(".f32") && !cache.ends_with(&current);
        let old_peaks = name.ends_with(".peaks") && !name.ends_with(&current_peaks);
        if name.starts_with(&prefix) && (old_cache || old_peaks) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
}

/// Locks `<path>.lock`, so only one process makes the cache at `path`; released when the file is dropped.
/// Waiting for another caller still asks `progress`, so a cancelled job stops at once.
pub(crate) fn lock_cache(path: &Path, progress: &mut impl FnMut(f32) -> Result<()>) -> Result<File> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock = File::options().read(true).write(true).create(true).truncate(false).open(lock_path)?;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => {
                progress(0.0)?;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

/// Extracts the PCM cache for `asset` unless it already exists. Concurrent callers for the
/// same file (import, export and captions) wait for one extraction instead of racing.
/// An error from `progress` stops the extraction, or the wait for another caller's; the next call
/// starts it again.
pub fn ensure_pcm(cache_dir: &Path, asset: &Asset, mut progress: impl FnMut(f32) -> Result<()>) -> Result<PathBuf> {
    let path = pcm_path(cache_dir, asset);
    // A cache appears whole by a rename, so one that exists needs no lock. Its lock may still be held
    // after its owner let go: a process that another thread is starting keeps a copy until it runs.
    if path.exists() {
        return Ok(path);
    }
    let _lock = lock_cache(&path, &mut progress)?;
    if !path.exists() {
        extract_pcm_with_peaks(Path::new(&asset.path), &path, &peaks_for(&path), progress)?;
        remove_older_versions(&path, asset);
    }
    Ok(path)
}

pub struct Mixer {
    cache_dir: PathBuf,
    /// Keyed by cache file, which names the source revision, so media rebound under the same
    /// asset id plays its own sound.
    sources: HashMap<PathBuf, Option<Arc<Pcm>>>,
    /// How far each cut's crossfade reads past its clips, measured once from the files.
    cuts: HashMap<CutKey, (i64, i64)>,
    /// Which 10 ms blocks of each raw cache are speech, measured once when ducking first asks:
    /// 0 not yet, 1 quiet, 2 speech.
    speech: HashMap<PathBuf, Vec<u8>>,
    /// Pieces of sound that keeps its pitch, by cache, the file frame at the anchor and speed.
    stretches: HashMap<(PathBuf, i64, u32), Stretch>,
}

impl Mixer {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            sources: HashMap::new(),
            cuts: HashMap::new(),
            speech: HashMap::new(),
            stretches: HashMap::new(),
        }
    }

    /// The sound `asset` plays with: its cleaned cache for a clip with Clean voice once that is
    /// ready, otherwise the raw cache. Only cache files change hands here; no processing runs.
    fn source(&mut self, asset: &Asset, clean_voice: bool) -> Option<(PathBuf, Arc<Pcm>)> {
        let raw = pcm_path(&self.cache_dir, asset);
        let cleaned = crate::voice::path_for(&self.cache_dir, &raw);
        if clean_voice && let Some(pcm) = self.open(cleaned.clone(), asset) {
            return Some((cleaned, pcm));
        }
        self.open(raw.clone(), asset).map(|pcm| (raw, pcm))
    }

    fn open(&mut self, path: PathBuf, asset: &Asset) -> Option<Arc<Pcm>> {
        if let Some(Some(pcm)) = self.sources.get(&path) {
            return Some(pcm.clone());
        }
        // Missing caches are retried, since extraction or cleaning may still be running. Another
        // revision of this asset's cache of the same kind (raw or cleaned) is no longer played.
        let pcm = Pcm::open(&path).ok().map(Arc::new);
        let prefix = format!("{}.", asset.id);
        self.sources.retain(|other, _| {
            other.parent() != path.parent()
                || !other.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
        });
        self.sources.insert(path, pcm.clone());
        pcm
    }

    /// Mixes `out.len() / 2` stereo frames starting at timeline sample `start`.
    pub fn mix(&mut self, project: &Project, start: i64, out: &mut [f32]) {
        out.fill(0.0);
        let frames = (out.len() / CHANNELS) as i64;
        let end = start + frames;
        let mut ducking: Option<Vec<f32>> = None;
        for track in &project.tracks {
            if track.muted || track.kind == TrackKind::Text {
                continue;
            }
            for (index, clip) in track.clips.iter().enumerate() {
                let ClipContent::Media {
                    asset_id, source_in_us, volume, speed, fade_in_us, fade_out_us, duck_db, ..
                } = &clip.content
                else {
                    continue;
                };
                let c0 = us_to_samples(clip.start_us);
                let c1 = us_to_samples(clip.end_us());
                let previous = index.checked_sub(1).map(|i| &track.clips[i]);
                let next = track.clips.get(index + 1);
                let transition_in = previous.and_then(|_| transition_into(track, clip));
                let transition_out = next.and_then(|next| transition_into(track, next));
                // Cuts read the files, so only clips that can sound in this buffer look at theirs.
                let reach = us_to_samples(CUT_FADE_US);
                let earliest = transition_in.map_or(c0 - reach, |(a, _)| us_to_samples(a));
                let latest = transition_out.map_or(c1 + reach, |(_, b)| us_to_samples(b));
                if start.max(earliest) >= end.min(latest) || *volume <= 0.0 {
                    continue;
                }
                let incoming = self.incoming(project, track, index);
                let outgoing = self.outgoing(project, track, index);
                let incoming = incoming.map(|(a, b)| (us_to_samples(a), us_to_samples(b)));
                let outgoing = outgoing.map(|(a, b)| (us_to_samples(a), us_to_samples(b)));
                let begin = incoming.map(|w| w.0).unwrap_or(c0);
                let finish = outgoing.map(|w| w.1).unwrap_or(c1);
                let (from, to) = (start.max(begin), end.min(finish));
                if from >= to || *volume <= 0.0 {
                    continue;
                }
                let Some(asset) = project.asset(asset_id) else { continue };
                if !has_audio(asset) {
                    continue;
                }
                let Some((path, pcm)) = self.source(asset, cleans_voice(clip)) else { continue };
                let samples = pcm.samples();
                let src0 = *source_in_us as f64 * SAMPLE_RATE as f64 / 1_000_000.0;
                let origin = clip.start_us as f64 * SAMPLE_RATE as f64 / 1_000_000.0;
                // PCM cannot hold an edge sample like video holds a frame.
                let available_start = (origin - src0 / *speed as f64).ceil() as i64;
                let available_end = (origin + (pcm.frames() as f64 - src0) / *speed as f64).ceil() as i64;
                let begin = begin.max(available_start);
                let finish = finish.min(available_end);
                // A split leaves pieces that play on seamlessly; ramping there would dip the sound.
                let joined_before = begin == c0 && index > 0 && continues(&track.clips[index - 1], clip);
                let joined_after = finish == c1 && track.clips.get(index + 1).is_some_and(|next| continues(clip, next));
                let edges = (if joined_before { 0 } else { EDGE_FADE }, if joined_after { 0 } else { EDGE_FADE });
                let duck = (*duck_db > 0.0).then(|| &*ducking.get_or_insert_with(|| self.ducking(project, start, end)));
                let (from, to) = (from.max(begin), to.min(finish));
                if from >= to {
                    continue;
                }
                let stretch = keeps_pitch(clip).then(|| {
                    // Pieces split from one clip share its stretch, so their sound plays on seamlessly. Its
                    // pieces read only the part of the file the pieces play, so sound cut away stays away.
                    let run = |i: &usize| continues(&track.clips[*i], &track.clips[*i + 1]);
                    let first = (0..index).rev().take_while(run).last().unwrap_or(index);
                    let last = (index..track.clips.len() - 1).take_while(run).last().map_or(index, |i| i + 1);
                    let source_at = |clip: &Clip, us: i64| {
                        let ClipContent::Media { source_in_us, .. } = &clip.content else { unreachable!() };
                        let source_us = *source_in_us as f64 + (us - clip.start_us) as f64 * *speed as f64;
                        source_us * SAMPLE_RATE as f64 / 1_000_000.0
                    };
                    let heard_from = self.incoming(project, track, first).map(|w| w.0);
                    let heard_until = self.outgoing(project, track, last).map(|w| w.1);
                    let (first, last) = (&track.clips[first], &track.clips[last]);
                    let (heard_from, heard_until) =
                        (heard_from.unwrap_or(first.start_us), heard_until.unwrap_or(last.end_us()));
                    let kept =
                        (source_at(first, heard_from).floor() as i64, source_at(last, heard_until).ceil() as i64);
                    let ClipContent::Media { source_in_us, .. } = &first.content else { unreachable!() };
                    let origin = first.start_us as f64 * SAMPLE_RATE as f64 / 1_000_000.0;
                    let place = origin.round();
                    let src0 = *source_in_us as f64 * SAMPLE_RATE as f64 / 1_000_000.0;
                    let anchor = (src0 + (place - origin) * *speed as f64).round() as i64;
                    let key = (path, anchor, speed.to_bits());
                    if self.stretches.len() >= 256 && !self.stretches.contains_key(&key) {
                        self.stretches.clear();
                    }
                    let stretch = self.stretches.entry(key).or_insert_with(|| Stretch::new(anchor, *speed as f64));
                    let place = place as i64;
                    stretch.reach(samples, from - place, to - place);
                    (place, &*stretch, kept)
                });
                for i in from..to {
                    let src = src0 + (i as f64 - origin) * *speed as f64;
                    let mut gain = volume
                        * gain_at(i, begin, finish, edges, incoming, outgoing)
                        * fade_gain(i, c0, c1, us_to_samples(*fade_in_us), us_to_samples(*fade_out_us));
                    if let Some(duck) = duck {
                        gain *= db_to_gain(-(*duck_db * duck[(i - start) as usize]) as f64);
                    }
                    let o = ((i - start) as usize) * CHANNELS;
                    for ch in 0..CHANNELS {
                        let sample = match stretch {
                            Some((place, stretch, kept)) => stretch.sample(samples, i - place, ch, kept),
                            None => sample_at(samples, src, ch),
                        };
                        out[o + ch] += sample * gain;
                    }
                }
            }
        }
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }
}

impl Mixer {
    /// Where the sound of `track.clips[index]` starts before the clip: a transition or a cut's crossfade.
    fn incoming(&mut self, project: &Project, track: &Track, index: usize) -> Option<(i64, i64)> {
        let (previous, clip) = (track.clips.get(index.checked_sub(1)?)?, &track.clips[index]);
        transition_into(track, clip).or_else(|| self.cut_window(project, previous, clip))
    }

    /// Where the sound of `track.clips[index]` plays on after the clip: a transition or a cut's crossfade.
    fn outgoing(&mut self, project: &Project, track: &Track, index: usize) -> Option<(i64, i64)> {
        let (clip, next) = (&track.clips[index], track.clips.get(index + 1)?);
        transition_into(track, next).or_else(|| self.cut_window(project, clip, next))
    }
}

/// Where a transition into `clip` plays; only the main track has them.
fn transition_into(track: &Track, clip: &Clip) -> Option<(i64, i64)> {
    if track.id == crate::edit::MAIN_TRACK { transition_window(clip) } else { None }
}

fn sample_at(samples: &[f32], position: f64, channel: usize) -> f32 {
    if !position.is_finite() || position < 0.0 || position >= (samples.len() / CHANNELS) as f64 {
        return 0.0;
    }
    let index = position.floor() as usize;
    let p = (position - index as f64) as f32;
    let a = samples[index * CHANNELS + channel];
    let b = samples.get((index + 1) * CHANNELS + channel).copied().unwrap_or(0.0);
    a + (b - a) * p
}

fn fade_gain(i: i64, start: i64, end: i64, fade_in: i64, fade_out: i64) -> f32 {
    let ramp = |n: i64, d: i64| if d > 0 { (n as f32 / d as f32).clamp(0.0, 1.0) } else { 1.0 };
    ramp(i - start, fade_in) * ramp(end - 1 - i, fade_out)
}

fn cleans_voice(clip: &Clip) -> bool {
    matches!(clip.content, ClipContent::Media { clean_voice: true, .. })
}

fn keeps_pitch(clip: &Clip) -> bool {
    matches!(clip.content, ClipContent::Media { keep_pitch: true, speed, .. } if speed != 1.0)
}

/// Whether `next` starts where `clip` ends and plays on from the same source at the same
/// level, as the two halves of a split do. Halves of which only one cleans the voice or keeps the
/// pitch, or that duck differently, sound different, so they crossfade like a cut.
fn continues(clip: &Clip, next: &Clip) -> bool {
    let (
        ClipContent::Media {
            asset_id: a,
            source_in_us: source_a,
            volume: volume_a,
            speed: speed_a,
            duck_db: duck_a,
            ..
        },
        ClipContent::Media {
            asset_id: b,
            source_in_us: source_b,
            volume: volume_b,
            speed: speed_b,
            duck_db: duck_b,
            ..
        },
    ) = (&clip.content, &next.content)
    else {
        return false;
    };
    let source_end = source_a + (clip.duration_us as f64 * *speed_a as f64).round() as i64;
    a == b
        && clip.end_us() == next.start_us
        && speed_a == speed_b
        && volume_a == volume_b
        && duck_a == duck_b
        && cleans_voice(clip) == cleans_voice(next)
        && keeps_pitch(clip) == keeps_pitch(next)
        && (source_b - source_end).abs() <= samples_to_us(1)
}

impl Mixer {
    /// Where two pieces of sound that do not play on from each other meet, they overlap and
    /// crossfade over up to 10 ms on each side of the cut, so a cut neither clicks nor dips. Each
    /// side reaches past its clip only into quiet sound its file has; a cut with nothing quiet to
    /// spare on either side keeps the edge ramps.
    fn cut_window(&mut self, project: &Project, before: &Clip, after: &Clip) -> Option<(i64, i64)> {
        let (
            ClipContent::Media { asset_id: id_before, source_in_us: source_before, speed: speed_before, .. },
            ClipContent::Media { asset_id: id_after, source_in_us: source_after, speed: speed_after, .. },
        ) = (&before.content, &after.content)
        else {
            return None;
        };
        if before.end_us() != after.start_us || continues(before, after) {
            return None;
        }
        let (asset_before, asset_after) = (project.asset(id_before)?, project.asset(id_after)?);
        let (source_after, speed_before, speed_after) = (*source_after, *speed_before as f64, *speed_after as f64);
        let stop = source_before + (before.duration_us as f64 * speed_before).round() as i64;
        let key = (
            pcm_path(&self.cache_dir, asset_before),
            stop,
            (speed_before as f32).to_bits(),
            pcm_path(&self.cache_dir, asset_after),
            source_after,
            (speed_after as f32).to_bits(),
        );
        let (early, late) = match self.cuts.get(&key) {
            Some(&reach) => reach,
            None => {
                // How much of each file lies past the cut, in its own time.
                let ahead = ((CUT_FADE_US as f64 * speed_after) as i64).min(source_after).max(0);
                let behind = ((CUT_FADE_US as f64 * speed_before) as i64).min(asset_before.duration_us - stop).max(0);
                let early = self.quiet(
                    asset_after,
                    (source_after - ahead, source_after),
                    (source_after, source_after + KEPT_CONTEXT_US),
                );
                let late = self.quiet(asset_before, (stop, stop + behind), (stop - KEPT_CONTEXT_US, stop));
                // A cache still being extracted is measured again once it is there.
                let (Some(early), Some(late)) = (early, late) else { return None };
                let reach = (
                    if early && ahead > 0 { (ahead as f64 / speed_after) as i64 } else { 0 },
                    if late && behind > 0 { (behind as f64 / speed_before) as i64 } else { 0 },
                );
                if self.cuts.len() >= 4096 {
                    self.cuts.clear();
                }
                self.cuts.insert(key, reach);
                reach
            }
        };
        (early + late > 0).then_some((after.start_us - early, after.start_us + late))
    }

    /// Whether `range` of the file is quiet next to the sound kept in `kept` (both in the file's
    /// own time); None while its cache is not there yet.
    fn quiet(&mut self, asset: &Asset, range: (i64, i64), kept: (i64, i64)) -> Option<bool> {
        if !has_audio(asset) {
            return Some(false);
        }
        let (_, pcm) = self.source(asset, false)?;
        let samples = pcm.samples();
        let frame = |us: i64| (us_to_samples(us.max(0)) as usize).min(pcm.frames());
        let rms = |a: usize, b: usize| {
            let part = &samples[a * CHANNELS..b.max(a) * CHANNELS];
            (part.iter().map(|s| s * s).sum::<f32>() / part.len().max(1) as f32).sqrt()
        };
        let level = rms(frame(range.0), frame(range.1));
        if level <= QUIET_RMS {
            return Some(true);
        }
        let (from, to) = (frame(kept.0), frame(kept.1));
        let window = (SAMPLE_RATE / 100) as usize;
        let loudest = (from..to).step_by(window).map(|a| rms(a, (a + window).min(to))).fold(0.0, f32::max);
        Some(level <= loudest * QUIET_BELOW_KEPT)
    }

    /// How far ducked clips are down at each frame of `start..end`: 0 at full level, 1 down by their
    /// whole `duck_db`. Speech is heard clips that do not duck themselves; the result depends only on
    /// the timeline, never on where a buffer starts, so playback and export agree.
    fn ducking(&mut self, project: &Project, start: i64, end: i64) -> Vec<f32> {
        // Speech this far around the buffer still moves it; nothing further can.
        let first = (start - DUCK_HOLD - DUCK_RELEASE).div_euclid(SPEECH_BLOCK);
        let last = (end + DUCK_ATTACK).div_euclid(SPEECH_BLOCK) + 1;
        let mut speech = vec![false; (last - first) as usize];
        for track in &project.tracks {
            for (index, clip) in track.clips.iter().enumerate() {
                // A clip that lowers itself is never speech.
                let ClipContent::Media { asset_id, source_in_us, speed, duck_db: 0.0, .. } = &clip.content else {
                    continue;
                };
                if !is_heard(project, track, clip) {
                    continue;
                }
                // Across a transition its sound starts early or plays on, as the mix has it.
                let begin = (index > 0).then(|| transition_into(track, clip)).flatten().map_or(clip.start_us, |w| w.0);
                let finish = track.clips.get(index + 1).and_then(|next| transition_into(track, next));
                let (c0, c1) = (us_to_samples(begin), us_to_samples(finish.map_or(clip.end_us(), |w| w.1)));
                let blocks = c0.div_euclid(SPEECH_BLOCK).max(first)..(c1.div_euclid(SPEECH_BLOCK) + 1).min(last);
                if blocks.is_empty() {
                    continue;
                }
                let Some(asset) = project.asset(asset_id) else { continue };
                let path = pcm_path(&self.cache_dir, asset);
                let Some(pcm) = self.open(path.clone(), asset) else { continue };
                let samples = pcm.samples();
                let file =
                    self.speech.entry(path).or_insert_with(|| vec![0; pcm.frames().div_ceil(SPEECH_BLOCK as usize)]);
                let src0 = *source_in_us as f64 * SAMPLE_RATE as f64 / 1_000_000.0;
                let origin = clip.start_us as f64 * SAMPLE_RATE as f64 / 1_000_000.0;
                let src = |t: i64| src0 + (t as f64 - origin) * *speed as f64;
                for block in blocks {
                    // The part of the timeline block the clip sounds in, and every block of the file it plays.
                    let (a, b) = ((block * SPEECH_BLOCK).max(c0), ((block + 1) * SPEECH_BLOCK).min(c1));
                    if a >= b || src(b - 1) < 0.0 {
                        continue;
                    }
                    let (from, to) = (src(a).max(0.0) as i64 / SPEECH_BLOCK, src(b - 1) as i64 / SPEECH_BLOCK);
                    for i in from as usize..=to as usize {
                        let Some(level) = file.get_mut(i) else { break };
                        if *level == 0 {
                            let size = SPEECH_BLOCK as usize * CHANNELS;
                            let part = &samples[i * size..((i + 1) * size).min(samples.len())];
                            let rms = (part.iter().map(|s| s * s).sum::<f32>() / part.len().max(1) as f32).sqrt();
                            *level = if rms > SPEECH_RMS { 2 } else { 1 };
                        }
                        speech[(block - first) as usize] |= *level == 2;
                    }
                }
            }
        }
        if self.speech.len() > 256 {
            self.speech.clear();
        }
        // Where the latest speech before each block ended and the next one after it starts.
        let at = |index: usize| (first + index as i64) * SPEECH_BLOCK;
        let mut ended = vec![None; speech.len()];
        let mut starts = vec![None; speech.len()];
        for index in 1..speech.len() {
            ended[index] = if speech[index - 1] { Some(at(index)) } else { ended[index - 1] };
        }
        for index in (0..speech.len().saturating_sub(1)).rev() {
            starts[index] = if speech[index + 1] { Some(at(index + 1)) } else { starts[index + 1] };
        }
        (start..end)
            .map(|i| {
                let index = (i.div_euclid(SPEECH_BLOCK) - first) as usize;
                if speech[index] {
                    return 1.0;
                }
                let release =
                    ended[index].map_or(0.0, |e| 1.0 - (i - e - DUCK_HOLD).max(0) as f32 / DUCK_RELEASE as f32);
                let attack = starts[index].map_or(0.0, |s| 1.0 - (s - i) as f32 / DUCK_ATTACK as f32);
                release.max(attack).clamp(0.0, 1.0)
            })
            .collect()
    }
}

/// `edges` are the de-click ramp lengths at the start and the end; 0 leaves that edge open.
fn gain_at(
    i: i64,
    start: i64,
    end: i64,
    edges: (i64, i64),
    incoming: Option<(i64, i64)>,
    outgoing: Option<(i64, i64)>,
) -> f32 {
    if i < start || i >= end {
        return 0.0;
    }
    let ramp = |n: i64, d: i64| if d > 0 { (n as f32 / d as f32).clamp(0.0, 1.0) } else { 1.0 };
    let mut gain = ramp(i - start, edges.0).min(ramp(end - 1 - i, edges.1));
    for (window, entering) in [(incoming, true), (outgoing, false)] {
        if let Some((a, b)) = window {
            let p = ((i - a) as f64 / (b - a).max(1) as f64).clamp(0.0, 1.0);
            let angle = p * std::f64::consts::FRAC_PI_2;
            gain *= if entering { angle.sin() as f32 } else { angle.cos() as f32 };
        }
    }
    gain
}

/// Peak envelope for waveform drawing, `per_second` buckets of 0..=255.
pub fn peaks(pcm: &Pcm, per_second: u32) -> Vec<u8> {
    let samples = pcm.samples();
    let bucket = (SAMPLE_RATE / per_second.max(1)) as usize * CHANNELS;
    samples.chunks(bucket.max(CHANNELS)).map(|c| peak(loudest(0.0, c))).collect()
}

fn loudest(from: f32, samples: &[f32]) -> f32 {
    samples.iter().fold(from, |m, s| m.max(s.abs()))
}

fn peak(loudest: f32) -> u8 {
    (loudest.min(1.0) * 255.0) as u8
}

/// Waveform peaks per second of sound: each is the loudest sample of either channel over 20 ms.
pub const PEAKS_PER_SECOND: u32 = 50;
const PEAK_FRAMES: usize = (SAMPLE_RATE / PEAKS_PER_SECOND) as usize;

/// Writes the waveform peaks of samples as they are extracted. They reach the file every second of
/// sound, so a reader sees the decoded part grow.
pub(crate) struct PeakWriter {
    file: File,
    /// The loudest sample of the peak being filled, and how many frames it has.
    loudest: f32,
    frames: usize,
    pending: Vec<u8>,
}

impl PeakWriter {
    pub(crate) fn create(path: &Path) -> Result<Self> {
        Ok(Self { file: File::create(path)?, loudest: 0.0, frames: 0, pending: Vec::new() })
    }

    /// Interleaved samples of `channels` channels.
    pub(crate) fn push(&mut self, samples: &[f32], channels: usize) -> Result<()> {
        let mut rest = samples;
        while !rest.is_empty() {
            let (part, tail) = rest.split_at(((PEAK_FRAMES - self.frames) * channels).min(rest.len()));
            self.loudest = loudest(self.loudest, part);
            self.add(part.len() / channels);
            rest = tail;
        }
        self.write(false)
    }

    pub(crate) fn silence(&mut self, mut frames: u64) -> Result<()> {
        while frames > 0 {
            let part = ((PEAK_FRAMES - self.frames) as u64).min(frames);
            self.add(part as usize);
            frames -= part;
        }
        self.write(false)
    }

    fn add(&mut self, frames: usize) {
        self.frames += frames;
        if self.frames == PEAK_FRAMES {
            self.pending.push(peak(self.loudest));
            (self.loudest, self.frames) = (0.0, 0);
        }
    }

    fn write(&mut self, all: bool) -> Result<()> {
        if self.pending.len() >= PEAKS_PER_SECOND as usize || all {
            self.file.write_all(&self.pending)?;
            self.pending.clear();
        }
        Ok(())
    }

    /// Writes the last peak, which the end of the sound may leave short.
    pub(crate) fn finish(mut self) -> Result<()> {
        if self.frames > 0 {
            self.pending.push(peak(self.loudest));
        }
        self.write(true)
    }
}

/// Waveform peaks `from..to` (at `PEAKS_PER_SECOND`) of `asset`'s sound, and whether they are final.
/// While its cache is extracted, only the part decoded so far is there. A cache extracted before peaks
/// were written gets them from its samples, reading only that part.
pub fn waveform_peaks(cache_dir: &Path, asset: &Asset, from: usize, to: usize) -> Result<(Vec<u8>, bool)> {
    // Nothing will be decoded: no sound, or the file is gone.
    if !has_audio(asset) || !Path::new(&asset.path).is_file() {
        return Ok((Vec::new(), true));
    }
    let pcm = pcm_path(cache_dir, asset);
    let peaks = peaks_for(&pcm);
    let total = match std::fs::metadata(&pcm) {
        Ok(cache) => (cache.len() as usize / (CHANNELS * 4)).div_ceil(PEAK_FRAMES),
        Err(_) => return Ok((read_bytes(&peaks, from, to).unwrap_or_default(), false)),
    };
    let to = to.min(total);
    if from >= to {
        return Ok((Vec::new(), true));
    }
    if std::fs::metadata(&peaks).is_ok_and(|p| p.len() as usize == total) {
        return Ok((read_bytes(&peaks, from, to)?, true));
    }
    let mut file = File::open(&pcm)?;
    file.seek(SeekFrom::Start((from * PEAK_FRAMES * CHANNELS * 4) as u64))?;
    let mut buffer = vec![0f32; 64 * PEAK_FRAMES * CHANNELS];
    let mut out = Vec::with_capacity(to - from);
    while out.len() < to - from {
        let want = ((to - from - out.len()) * PEAK_FRAMES * CHANNELS).min(buffer.len());
        let read = read_full(&mut file, bytemuck::cast_slice_mut(&mut buffer[..want]))? / 4 / CHANNELS * CHANNELS;
        out.extend(buffer[..read].chunks(PEAK_FRAMES * CHANNELS).map(|c| peak(loudest(0.0, c))));
        if read < want {
            break;
        }
    }
    Ok((out, true))
}

/// Bytes `from..to` of the file, fewer where it ends.
fn read_bytes(path: &Path, from: usize, to: usize) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(from as u64))?;
    let mut out = Vec::new();
    file.take(to.saturating_sub(from) as u64).read_to_end(&mut out)?;
    Ok(out)
}

/// Fills `buffer` unless the file ends first; returns how much it read.
fn read_full(file: &mut File, buffer: &mut [u8]) -> Result<usize> {
    let mut read = 0;
    while read < buffer.len() {
        match file.read(&mut buffer[read..])? {
            0 => break,
            n => read += n,
        }
    }
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_wav(path: &Path, sample: i16, frames: u32) {
        let data_bytes = frames * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        for field in [1u16, 1] {
            bytes.extend_from_slice(&field.to_le_bytes());
        }
        for field in [48_000u32, 96_000] {
            bytes.extend_from_slice(&field.to_le_bytes());
        }
        for field in [2u16, 16] {
            bytes.extend_from_slice(&field.to_le_bytes());
        }
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        for _ in 0..frames {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn pcm_cache_child() {
        let Some(dir) = std::env::var_os("NUZKY_PCM_LOCK_TEST") else { return };
        let dir = PathBuf::from(dir);
        let asset: Asset = serde_json::from_slice(&std::fs::read(dir.join("asset.json")).unwrap()).unwrap();
        let id = std::process::id();
        std::fs::write(dir.join(format!("ready-{id}")), b"").unwrap();
        // Waiting asks `progress`; the content shows the published cache was reused.
        let path = ensure_pcm(&dir, &asset, |_| Ok(())).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), vec![0; 8]);
        std::fs::write(dir.join(format!("done-{id}")), b"").unwrap();
    }

    #[test]
    fn pcm_cache_serializes_processes_and_rechecks_after_lock() {
        use std::time::{Duration, Instant};
        let dir = std::env::temp_dir().join(format!("pcm-processes-{}", crate::edit::new_id()));
        std::fs::create_dir_all(dir.join("pcm")).unwrap();
        let source = dir.join("source.wav");
        write_test_wav(&source, 8192, 4800);
        let asset = crate::media::probe(&source, "same".into()).unwrap();
        std::fs::write(dir.join("asset.json"), serde_json::to_vec(&asset).unwrap()).unwrap();
        let path = pcm_path(&dir, &asset);
        let lock_path = format!("{}.lock", path.display());
        let lock = File::options().read(true).write(true).create(true).truncate(false).open(lock_path).unwrap();
        lock.lock().unwrap();
        let mut children: Vec<_> = (0..2)
            .map(|_| {
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "audio::tests::pcm_cache_child", "--nocapture"])
                    .env("NUZKY_PCM_LOCK_TEST", &dir)
                    .spawn()
                    .unwrap()
            })
            .collect();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !children.iter().all(|c| dir.join(format!("ready-{}", c.id())).exists()) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(children.iter_mut().all(|c| c.try_wait().unwrap().is_none()));
        assert!(!path.exists());
        std::fs::write(&path, [0; 8]).unwrap();
        drop(lock);
        for mut child in children {
            while child.try_wait().unwrap().is_none() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(child.wait().unwrap().success());
            assert!(dir.join(format!("done-{}", child.id())).exists());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A job waiting for another one's extraction of the same file stops as soon as it is
    /// cancelled, instead of when that extraction ends.
    #[test]
    fn a_cancelled_wait_for_the_pcm_cache_stops_at_once() {
        use std::time::{Duration, Instant};
        let dir = std::env::temp_dir().join(format!("pcm-wait-{}", crate::edit::new_id()));
        std::fs::create_dir_all(dir.join("pcm")).unwrap();
        let source = dir.join("source.wav");
        write_test_wav(&source, 8192, 4800);
        let asset = crate::media::probe(&source, "waiting".into()).unwrap();
        let lock_path = format!("{}.lock", pcm_path(&dir, &asset).display());
        let lock = File::options().read(true).write(true).create(true).truncate(false).open(lock_path).unwrap();
        lock.lock().unwrap();
        let started = Instant::now();
        let error = ensure_pcm(&dir, &asset, |_| anyhow::bail!("CANCELLED: test")).unwrap_err();
        assert!(error.to_string().starts_with("CANCELLED") && started.elapsed() < Duration::from_secs(1), "{error:#}");
        drop(lock);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A copied project can reuse an asset id for another file of the same size and time; it
    /// gets its own cache, and a mixer that played the first file plays the second.
    #[test]
    fn an_asset_id_rebound_to_another_file_gets_its_own_cache_and_sound() {
        let cache = std::env::temp_dir().join(format!("pcm-rebound-{}", crate::edit::new_id()));
        std::fs::create_dir_all(cache.join("pcm")).unwrap();
        let (one, two) = (cache.join("one.wav"), cache.join("two.wav"));
        write_test_wav(&one, 4_096, 4_800);
        write_test_wav(&two, 12_288, 4_800);
        let time = std::fs::metadata(&one).unwrap().modified().unwrap();
        File::options().write(true).open(&two).unwrap().set_modified(time).unwrap();
        let asset = |path: &Path| Asset {
            id: "same-id".into(),
            name: "source".into(),
            path: path.to_string_lossy().into(),
            kind: AssetKind::Audio,
            duration_us: 100_000,
            width: 0,
            height: 0,
            fps: 0.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
        };
        let (first, second) = (asset(&one), asset(&two));
        assert_ne!(pcm_path(&cache, &first), pcm_path(&cache, &second));
        ensure_pcm(&cache, &first, |_| Ok(())).unwrap();
        ensure_pcm(&cache, &second, |_| Ok(())).unwrap();
        let mut mixer = Mixer::new(cache.clone());
        let level = |mixer: &mut Mixer, asset: &Asset| mixer.source(asset, false).unwrap().1.samples()[1000];
        let quiet = level(&mut mixer, &first);
        let loud = level(&mut mixer, &second);
        assert!((loud - quiet * 3.0).abs() < 1e-3, "{quiet} then {loud}");
        assert_eq!(mixer.sources.len(), 1, "the first file is no longer held");
        std::fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn replaced_source_reextracts_pcm_under_the_same_asset_id() {
        let cache = std::env::temp_dir().join(format!("pcm-replaced-{}", crate::edit::new_id()));
        std::fs::create_dir_all(cache.join("pcm")).unwrap();
        let source = cache.join("source.wav");
        write_test_wav(&source, 8_192, 4_800);
        let asset = Asset {
            id: "same-id".into(),
            name: "source".into(),
            path: source.to_string_lossy().into(),
            kind: AssetKind::Audio,
            duration_us: 100_000,
            width: 0,
            height: 0,
            fps: 0.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
        };
        let legacy = cache.join("pcm/same-id.v2.f32");
        std::fs::write(&legacy, b"old cache").unwrap();
        let older = cache.join("pcm/same-id.123-456.v3.f32");
        std::fs::write(&older, b"old cache").unwrap();
        std::fs::write(cache.join("pcm/same-id.123-456.v3.f32.lock"), b"").unwrap();
        let other = cache.join("pcm/other-id.123-456.v3.f32");
        std::fs::write(&other, b"another asset").unwrap();
        let first = ensure_pcm(&cache, &asset, |_| Ok(())).unwrap();
        assert_ne!(first, legacy);
        // Caches of older extraction versions go; another asset's cache is left alone.
        assert!(!legacy.exists() && !older.exists() && !cache.join("pcm/same-id.123-456.v3.f32.lock").exists());
        assert!(other.exists());
        let old_audio = std::fs::read(&first).unwrap();
        let modified = std::fs::metadata(&source).unwrap().modified().unwrap() + std::time::Duration::from_secs(2);
        write_test_wav(&source, 16_384, 4_800);
        File::options().write(true).open(&source).unwrap().set_modified(modified).unwrap();
        let second = ensure_pcm(&cache, &asset, |_| Ok(())).unwrap();
        assert_ne!(first, second);
        assert_ne!(std::fs::read(&second).unwrap(), old_audio);
        assert_eq!(std::fs::read(&first).unwrap(), old_audio);
        // A process that another thread is starting holds a copy of the lock until it runs its program.
        let held = lock_cache(&second, &mut |_| Ok(())).unwrap();
        assert_eq!(
            ensure_pcm(&cache, &asset, |_| panic!("unchanged source waited or extracted again")).unwrap(),
            second
        );
        drop(held);
        write_test_wav(&source, 16_384, 9_600);
        File::options().write(true).open(&source).unwrap().set_modified(modified).unwrap();
        let third = ensure_pcm(&cache, &asset, |_| Ok(())).unwrap();
        assert_ne!(second, third);
        assert_eq!(std::fs::metadata(&third).unwrap().len(), std::fs::metadata(&second).unwrap().len() * 2);
        std::fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn transition_without_source_handles_has_no_sample_step() {
        use crate::model::{Clip, Transform, Transition, TransitionKind};
        let cache = std::env::temp_dir().join(format!("nuzky-audio-edges-{}", std::process::id()));
        std::fs::create_dir_all(cache.join("pcm")).unwrap();
        let mut project = Project::new("source edges");
        for (id, value, speed, start) in [("a", 0.2f32, 2.0, 0), ("b", 0.4, 0.5, 1_000_000)] {
            let asset = Asset {
                id: id.into(),
                name: id.into(),
                path: String::new(),
                kind: AssetKind::Video,
                duration_us: 2_000_000,
                width: 2,
                height: 2,
                fps: 30.0,
                has_audio: true,
                rotation: 0,
                mirror: false,
            };
            std::fs::write(pcm_path(&cache, &asset), bytemuck::cast_slice(&vec![value; 96000 * CHANNELS])).unwrap();
            project.assets.push(asset);
            let mut clip = Clip::new(
                id.into(),
                start,
                1_000_000,
                ClipContent::Media {
                    asset_id: id.into(),
                    source_in_us: 0,
                    volume: 1.0,
                    transform: Transform::default(),
                    speed,
                    keep_pitch: false,
                    adjust: Default::default(),
                    fade_in_us: 0,
                    fade_out_us: 0,
                    clean_voice: false,
                    shape: None,
                    duck_db: 0.0,
                },
            );
            if id == "b" {
                clip.transition_in = Some(Transition { kind: TransitionKind::Dissolve, duration_us: 400_000 });
            }
            project.tracks[0].clips.push(clip);
        }
        let mut mixer = Mixer::new(cache.clone());
        let mut out = vec![0.0; 1000 * CHANNELS];
        mixer.mix(&project, 47500, &mut out);
        let max_step = out
            .as_chunks::<CHANNELS>()
            .0
            .iter()
            .map(|f| f[0])
            .collect::<Vec<_>>()
            .windows(2)
            .map(|p| (p[1] - p[0]).abs())
            .fold(0.0f32, f32::max);
        drop(mixer);
        std::fs::remove_dir_all(cache).unwrap();
        assert!(max_step < 0.002, "sample step: {max_step}");
    }

    #[test]
    fn mixer_speed_and_crossfade_are_independent_of_buffer_boundaries() {
        use crate::model::{Clip, Transform, Transition, TransitionKind};
        let cache = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tmp-test")
            .join(format!("mixer-{}", std::process::id()));
        std::fs::create_dir_all(cache.join("pcm")).unwrap();
        let mut project = Project::new("audio test");
        for (id, value) in [("a", 0.2f32), ("b", 0.4)] {
            let asset = Asset {
                id: id.into(),
                name: id.into(),
                path: String::new(),
                kind: AssetKind::Video,
                duration_us: 3_000_000,
                width: 2,
                height: 2,
                fps: 30.0,
                has_audio: true,
                rotation: 0,
                mirror: false,
            };
            let samples = vec![value; 144000 * CHANNELS];
            std::fs::write(pcm_path(&cache, &asset), bytemuck::cast_slice(&samples)).unwrap();
            project.assets.push(asset);
            let mut clip = Clip::new(
                id.into(),
                if id == "a" { 0 } else { 1_000_000 },
                1_000_000,
                ClipContent::Media {
                    asset_id: id.into(),
                    source_in_us: 500_000,
                    volume: 1.0,
                    transform: Transform::default(),
                    speed: 2.0,
                    keep_pitch: false,
                    adjust: Default::default(),
                    fade_in_us: 0,
                    fade_out_us: 0,
                    clean_voice: false,
                    shape: None,
                    duck_db: 0.0,
                },
            );
            if id == "b" {
                clip.transition_in = Some(Transition { kind: TransitionKind::Dissolve, duration_us: 400_000 });
            }
            project.tracks[0].clips.push(clip);
        }
        let mut mixer = Mixer::new(cache.clone());
        let mut whole = vec![0.0; 96000 * CHANNELS];
        mixer.mix(&project, 0, &mut whole);
        assert!((whole[48000 * CHANNELS] - 0.6 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        let mut chunks = vec![0.0; whole.len()];
        for (index, chunk) in chunks.chunks_mut(137 * CHANNELS).enumerate() {
            mixer.mix(&project, (index * 137) as i64, chunk);
        }
        assert_eq!(whole, chunks);
        drop(mixer);
        let samples: Vec<f32> = (0..144000).flat_map(|i| [i as f32 / 200000.0; CHANNELS]).collect();
        std::fs::write(pcm_path(&cache, &project.assets[0]), bytemuck::cast_slice(&samples)).unwrap();
        let mut mixer = Mixer::new(cache.clone());
        let mut frame = [0.0; CHANNELS];
        mixer.mix(&project, 1000, &mut frame);
        assert!((frame[0] - 26000.0 / 200000.0).abs() < 1e-6);
        drop(mixer);
        std::fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn cuts_ramp_without_clicks_and_split_halves_play_on_seamlessly() {
        use crate::edit::{EditCmd, TimeRange};
        let cache = std::env::temp_dir().join(format!("nuzky-audio-cuts-{}", crate::edit::new_id()));
        std::fs::create_dir_all(cache.join("pcm")).unwrap();
        let asset = Asset {
            id: "m".into(),
            name: "m".into(),
            path: String::new(),
            kind: AssetKind::Audio,
            duration_us: 4_000_000,
            width: 0,
            height: 0,
            fps: 0.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
        };
        // A rising level, so a cut jumps between different values.
        let samples: Vec<f32> = (0..192_000).flat_map(|i| [i as f32 / 192_000.0; CHANNELS]).collect();
        std::fs::write(pcm_path(&cache, &asset), bytemuck::cast_slice(&samples)).unwrap();
        let mut project = Project::new("cuts");
        project.apply(EditCmd::AddAssets { assets: vec![asset] }).unwrap();
        let id = project.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(0), track_id: None }).unwrap();
        let id = id.select[0].clone();
        let mix = |project: &Project| {
            let mut out = vec![0.0; 192_000 * CHANNELS];
            Mixer::new(cache.clone()).mix(project, 0, &mut out);
            out.as_chunks::<CHANNELS>().0.iter().map(|f| f[0]).collect::<Vec<_>>()
        };
        let whole = mix(&project);
        let mut split = project.clone();
        split.apply(EditCmd::SplitClip { clip_id: id, at_us: 1_000_000 }).unwrap();
        let difference = whole.iter().zip(mix(&split)).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        let mut cut = project.clone();
        let range = TimeRange { start_us: 1_000_000, end_us: 2_000_000 };
        cut.apply(EditCmd::RippleDeleteRanges { ranges: vec![range], keep_track_ids: Some(vec![]) }).unwrap();
        let cut = mix(&cut);
        let max_step = cut.windows(2).map(|p| (p[1] - p[0]).abs()).fold(0.0f32, f32::max);
        std::fs::remove_dir_all(cache).unwrap();
        assert!(difference < 1e-6, "split changed the sound by {difference}");
        // Without the edge ramps the cut would jump from 0.25 to 0.5 in one sample.
        assert!(max_step < 0.01, "sample step at the cut: {max_step}");
        assert!((cut[47_000] - 47_000.0 / 192_000.0).abs() < 1e-6 && (cut[49_000] - 97_000.0 / 192_000.0).abs() < 1e-6);
    }

    /// A take as one asset whose samples `level(t)` gives, and a project playing it whole.
    fn take(cache: &Path, mut level: impl FnMut(f64) -> f32) -> Project {
        use crate::edit::EditCmd;
        let asset = Asset {
            id: "take".into(),
            name: "take".into(),
            path: String::new(),
            kind: AssetKind::Video,
            duration_us: 2_000_000,
            width: 2,
            height: 2,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
        };
        let samples: Vec<f32> = (0..96_000).flat_map(|n| [level(n as f64 / 48_000.0); CHANNELS]).collect();
        std::fs::write(pcm_path(cache, &asset), bytemuck::cast_slice(&samples)).unwrap();
        let mut project = Project::new("take");
        project.apply(EditCmd::AddAssets { assets: vec![asset] }).unwrap();
        project.apply(EditCmd::AddClip { asset_id: "take".into(), start_us: Some(0), track_id: None }).unwrap();
        project
    }

    fn cut(project: &mut Project, start_us: i64, end_us: i64) {
        let range = crate::edit::TimeRange { start_us, end_us };
        project.apply(crate::edit::EditCmd::RippleDeleteRanges { ranges: vec![range], keep_track_ids: None }).unwrap();
    }

    /// Room tone cut out of a pause: the two sides crossfade over the cut, reading past their
    /// clips, so the level neither drops to nothing nor jumps. Without sound to spare at the start
    /// of the file, the edge ramps stay.
    #[test]
    fn a_cut_in_a_pause_crossfades_without_a_dip_and_edges_without_sound_to_spare_still_ramp() {
        let cache = std::env::temp_dir().join(format!("nuzky-audio-crossfade-{}", crate::edit::new_id()));
        std::fs::create_dir_all(cache.join("pcm")).unwrap();
        let mut state = 0x9e37_79b9_u32;
        let mut noise = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as f32 / u32::MAX as f32 - 0.5
        };
        // Speech, a pause of quiet room tone around -53 dBFS, speech again.
        let mut project = take(&cache, |t| if (0.5..1.5).contains(&t) { noise() * 0.008 } else { noise() * 0.4 });
        cut(&mut project, 600_000, 1_400_000);
        let mut mixer = Mixer::new(cache.clone());
        let (a, b) = (project.tracks[0].clips[0].clone(), project.tracks[0].clips[1].clone());
        assert_eq!(mixer.cut_window(&project, &a, &b), Some((590_000, 610_000)));
        let mut out = vec![0.0; 57_600 * CHANNELS];
        mixer.mix(&project, 0, &mut out);
        let left: Vec<f32> = out.as_chunks::<CHANNELS>().0.iter().map(|f| f[0]).collect();
        let rms = |from: usize| (left[from..from + 96].iter().map(|s| s * s).sum::<f32>() / 96.0).sqrt();
        let steady = (26_400..27_360).step_by(96).map(rms).sum::<f32>() / 10.0;
        // 2 ms windows across the 20 ms crossfade around the cut at 0.6 s (sample 28 800).
        let lowest = (28_320..29_280).step_by(96).map(rms).fold(f32::MAX, f32::min);
        // The start of the file has no sound before it: the first clip still fades in.
        let first = left[..48].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        std::fs::remove_dir_all(cache).unwrap();
        assert!(lowest > steady * 0.5, "level at the cut {lowest} against {steady}");
        assert!(first < 0.1, "the first 1 ms rises from silence: {first}");
    }

    /// A word deleted from connected speech: the sound past the cut on both sides is that word,
    /// so nothing of it is read into the crossfade; the cut keeps the edge ramps instead.
    #[test]
    fn a_crossfade_never_brings_back_a_deleted_word() {
        let cache = std::env::temp_dir().join(format!("nuzky-audio-deleted-{}", crate::edit::new_id()));
        std::fs::create_dir_all(cache.join("pcm")).unwrap();
        // Kept speech is a 200 Hz tone; the deleted word, 1.0–1.3 s, a loud steady level.
        let tone = |t: f64| 0.2 * (t * 2.0 * std::f64::consts::PI * 200.0).sin() as f32;
        let mut project = take(&cache, |t| if (1.0..1.3).contains(&t) { 0.8 } else { tone(t) });
        cut(&mut project, 1_000_000, 1_300_000);
        let mut mixer = Mixer::new(cache.clone());
        let (a, b) = (project.tracks[0].clips[0].clone(), project.tracks[0].clips[1].clone());
        assert_eq!(mixer.cut_window(&project, &a, &b), None);
        let mut out = vec![0.0; 960 * CHANNELS];
        mixer.mix(&project, 47_520, &mut out);
        // 20 ms around the cut: whole cycles of the tone, so any of the deleted level shows as an offset.
        let mean = out.as_chunks::<CHANNELS>().0.iter().map(|f| f[0]).sum::<f32>() / 960.0;
        std::fs::remove_dir_all(cache).unwrap();
        assert!(mean.abs() < 0.02, "the deleted word sounds at the cut: offset {mean}");
    }

    /// Ducking hears speech wherever the mix plays it: in every block of a sped-up clip, and before a clip's
    /// start or past its end where a transition plays its sound.
    #[test]
    fn ducking_hears_speech_sped_up_and_across_a_transition() {
        use crate::edit::EditCmd;
        let cache = std::env::temp_dir().join(format!("nuzky-audio-ducking-{}", crate::edit::new_id()));
        std::fs::create_dir_all(cache.join("pcm")).unwrap();
        let heard = |project: &Project, from: f64, to: f64| {
            let (from, to) = ((from * 48_000.0) as i64, (to * 48_000.0) as i64);
            Mixer::new(cache.clone()).ducking(project, from, to).into_iter().fold(0.0f32, f32::max)
        };
        let edit = |project: &mut Project, cmd: serde_json::Value| {
            project.apply(serde_json::from_value::<EditCmd>(cmd).unwrap()).unwrap();
        };
        // A 10 ms sound at 0.92 s of the file played at 2x, at 0.46 s: in a block a reading at the middle of each
        // timeline block would skip.
        let mut fast = take(&cache, |t| if (0.92..0.93).contains(&t) { 0.5 } else { 0.0 });
        let id = fast.tracks[0].clips[0].id.clone();
        edit(&mut fast, serde_json::json!({"type": "updateClip", "clipId": id, "speed": 2.0}));
        let fast_word = heard(&fast, 0.45, 0.47);
        // Speech at 0.8–0.95 s of the file, cut out at 0.6–1.0 s: a 0.6 s dissolve plays it anyway, the first
        // clip's sound going on to 0.9 s and the second's starting at 0.3 s, both silent where the clips are.
        let mut cut = take(&cache, |t| if (0.8..0.95).contains(&t) { 0.5 } else { 0.0 });
        self::cut(&mut cut, 600_000, 1_000_000);
        let id = cut.tracks[0].clips[1].id.clone();
        edit(
            &mut cut,
            serde_json::json!({"type": "setTransition", "clipId": id, "transition": {"kind": "dissolve", "durationUs": 600_000}}),
        );
        assert_eq!(transition_window(&cut.tracks[0].clips[1]), Some((300_000, 900_000)));
        let (early, late) = (heard(&cut, 0.42, 0.53), heard(&cut, 0.81, 0.89));
        std::fs::remove_dir_all(cache).unwrap();
        assert_eq!(fast_word, 1.0, "speech played at 2x goes unheard");
        assert_eq!((early, late), (1.0, 1.0), "speech in a transition goes unheard");
    }

    #[test]
    fn speed_reads_fractional_source_samples_and_silence_past_eof() {
        let samples = [0.0, 0.0, 0.2, -0.2, 0.4, -0.4, 0.6, -0.6];
        assert!((sample_at(&samples, 1.0 * 1.5, 0) - 0.3).abs() < 1e-6);
        assert!((sample_at(&samples, 1.0 * 2.0, 1) + 0.4).abs() < 1e-6);
        assert_eq!(sample_at(&samples, -0.1, 0), 0.0);
        assert_eq!(sample_at(&samples, 4.0, 0), 0.0);
    }

    #[test]
    fn fades_multiply_and_transition_is_equal_power() {
        assert_eq!(fade_gain(0, 0, 48000, 4800, 4800), 0.0);
        assert_eq!(fade_gain(2400, 0, 48000, 4800, 4800), 0.5);
        assert_eq!(fade_gain(45599, 0, 48000, 4800, 4800), 0.5);
        assert_eq!(gain_at(47999, 0, 48000, (EDGE_FADE, EDGE_FADE), None, None), 0.0);
        assert!((fade_gain(120, 0, 48000, 4800, 0) - 0.025).abs() < 1e-6);
        for i in [12000, 18000, 24000] {
            let a = gain_at(i, 0, 36000, (EDGE_FADE, EDGE_FADE), None, Some((12000, 24000)));
            let b = gain_at(i, 0, 36000, (EDGE_FADE, EDGE_FADE), Some((12000, 24000)), None);
            assert!((a * a + b * b - 1.0).abs() < 1e-6);
            if i == 18000 {
                assert!((a * 0.2 + b * 0.4 - 0.6 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
            }
        }
    }
}
