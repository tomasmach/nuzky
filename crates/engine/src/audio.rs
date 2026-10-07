//! Sample-exact timeline mixing from per-asset PCM caches (48 kHz stereo f32).

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use memmap2::Mmap;

use crate::effects::transition_window;
use crate::media::extract_pcm;
use crate::model::{Asset, AssetKind, CHANNELS, Clip, ClipContent, Project, SAMPLE_RATE, TrackKind};

/// Short fades at clip edges so cuts do not click.
const EDGE_FADE: i64 = (SAMPLE_RATE / 200) as i64; // 5 ms

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
    let revision = match std::fs::metadata(&asset.path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .map(|time| {
                    time.duration_since(std::time::UNIX_EPOCH).map_or_else(
                        |before| format!("pre{}", before.duration().as_nanos()),
                        |since| since.as_nanos().to_string(),
                    )
                })
                .unwrap_or_else(|| "unknown".into());
            format!("{}-{modified}", metadata.len())
        }
        Err(_) => "missing".into(),
    };
    // FNV-1a: stable across builds, so the cache survives an update.
    let source =
        asset.path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
    cache_dir.join("pcm").join(format!("{}.{revision}-{source:016x}.{PCM_VERSION}.f32", asset.id))
}

/// Raised whenever extraction changes its output, so caches made before are extracted again.
/// v4: gaps are filled by timestamp for long audio-only recordings too.
const PCM_VERSION: &str = "v4";

/// Removes this asset's caches from older extraction versions, which nothing reads any more.
/// Other revisions of the current version stay: a project may still play from them.
fn remove_older_versions(path: &Path, asset: &Asset) {
    let (Some(dir), prefix, current) = (path.parent(), format!("{}.", asset.id), format!(".{PCM_VERSION}.f32")) else {
        return;
    };
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let cache = name.strip_suffix(".lock").unwrap_or(&name);
        if name.starts_with(&prefix) && cache.ends_with(".f32") && !cache.ends_with(&current) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
}

/// Extracts the PCM cache for `asset` unless it already exists. Concurrent callers for the
/// same file (import, export and captions) wait for one extraction instead of racing.
/// An error from `progress` stops the extraction; the next call starts it again.
pub fn ensure_pcm(cache_dir: &Path, asset: &Asset, progress: impl FnMut(f32) -> Result<()>) -> Result<PathBuf> {
    let path = pcm_path(cache_dir, asset);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock = File::options().read(true).write(true).create(true).truncate(false).open(lock_path)?;
    lock.lock()?;
    if !path.exists() {
        extract_pcm(Path::new(&asset.path), &path, progress)?;
        remove_older_versions(&path, asset);
    }
    Ok(path)
}

pub struct Mixer {
    cache_dir: PathBuf,
    /// Keyed by cache file, which names the source revision, so media rebound under the same
    /// asset id plays its own sound.
    sources: HashMap<PathBuf, Option<Arc<Pcm>>>,
}

impl Mixer {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir, sources: HashMap::new() }
    }

    fn source(&mut self, asset: &Asset) -> Option<Arc<Pcm>> {
        let path = pcm_path(&self.cache_dir, asset);
        if let Some(Some(pcm)) = self.sources.get(&path) {
            return Some(pcm.clone());
        }
        // Missing caches are retried, since extraction may still be running. Another revision of
        // this asset is no longer played.
        let pcm = Pcm::open(&path).ok().map(Arc::new);
        let prefix = format!("{}.", asset.id);
        self.sources.retain(|other, _| !other.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix)));
        self.sources.insert(path, pcm.clone());
        pcm
    }

    /// Mixes `out.len() / 2` stereo frames starting at timeline sample `start`.
    pub fn mix(&mut self, project: &Project, start: i64, out: &mut [f32]) {
        out.fill(0.0);
        let frames = (out.len() / CHANNELS) as i64;
        let end = start + frames;
        for track in &project.tracks {
            if track.muted || track.kind == TrackKind::Text {
                continue;
            }
            for (index, clip) in track.clips.iter().enumerate() {
                let ClipContent::Media { asset_id, source_in_us, volume, speed, fade_in_us, fade_out_us, .. } =
                    &clip.content
                else {
                    continue;
                };
                let c0 = us_to_samples(clip.start_us);
                let c1 = us_to_samples(clip.end_us());
                let incoming =
                    if track.id == crate::edit::MAIN_TRACK && index > 0 { transition_window(clip) } else { None };
                let outgoing = if track.id == crate::edit::MAIN_TRACK {
                    track.clips.get(index + 1).and_then(transition_window)
                } else {
                    None
                };
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
                let Some(pcm) = self.source(asset) else { continue };
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
                for i in from.max(begin)..to.min(finish) {
                    let src = src0 + (i as f64 - origin) * *speed as f64;
                    let gain = volume
                        * gain_at(i, begin, finish, edges, incoming, outgoing)
                        * fade_gain(i, c0, c1, us_to_samples(*fade_in_us), us_to_samples(*fade_out_us));
                    let o = ((i - start) as usize) * CHANNELS;
                    for ch in 0..CHANNELS {
                        out[o + ch] += sample_at(samples, src, ch) * gain;
                    }
                }
            }
        }
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }
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

/// Whether `next` starts where `clip` ends and plays on from the same source at the same
/// level, as the two halves of a split do.
fn continues(clip: &Clip, next: &Clip) -> bool {
    let (
        ClipContent::Media { asset_id: a, source_in_us: source_a, volume: volume_a, speed: speed_a, .. },
        ClipContent::Media { asset_id: b, source_in_us: source_b, volume: volume_b, speed: speed_b, .. },
    ) = (&clip.content, &next.content)
    else {
        return false;
    };
    let source_end = source_a + (clip.duration_us as f64 * *speed_a as f64).round() as i64;
    a == b
        && clip.end_us() == next.start_us
        && speed_a == speed_b
        && volume_a == volume_b
        && (source_b - source_end).abs() <= samples_to_us(1)
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
    samples
        .chunks(bucket.max(CHANNELS))
        .map(|c| (c.iter().fold(0f32, |m, s| m.max(s.abs())).min(1.0) * 255.0) as u8)
        .collect()
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
        let Some(dir) = std::env::var_os("CAPOPEN_PCM_LOCK_TEST") else { return };
        let dir = PathBuf::from(dir);
        let asset: Asset = serde_json::from_slice(&std::fs::read(dir.join("asset.json")).unwrap()).unwrap();
        let id = std::process::id();
        std::fs::write(dir.join(format!("ready-{id}")), b"").unwrap();
        let path = ensure_pcm(&dir, &asset, |_| panic!("published cache must be reused")).unwrap();
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
                    .env("CAPOPEN_PCM_LOCK_TEST", &dir)
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
        };
        let (first, second) = (asset(&one), asset(&two));
        assert_ne!(pcm_path(&cache, &first), pcm_path(&cache, &second));
        ensure_pcm(&cache, &first, |_| Ok(())).unwrap();
        ensure_pcm(&cache, &second, |_| Ok(())).unwrap();
        let mut mixer = Mixer::new(cache.clone());
        let level = |mixer: &mut Mixer, asset: &Asset| mixer.source(asset).unwrap().samples()[1000];
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
        assert_eq!(ensure_pcm(&cache, &asset, |_| panic!("unchanged source extracted again")).unwrap(), second);
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
        let cache = std::env::temp_dir().join(format!("capopen-audio-edges-{}", std::process::id()));
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
                    adjust: Default::default(),
                    fade_in_us: 0,
                    fade_out_us: 0,
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
            .chunks_exact(CHANNELS)
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
                    adjust: Default::default(),
                    fade_in_us: 0,
                    fade_out_us: 0,
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
        let cache = std::env::temp_dir().join(format!("capopen-audio-cuts-{}", crate::edit::new_id()));
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
            out.chunks_exact(CHANNELS).map(|f| f[0]).collect::<Vec<_>>()
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
