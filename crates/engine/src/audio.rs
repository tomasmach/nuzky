//! Sample-exact timeline mixing from per-asset PCM caches (48 kHz stereo f32).

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use memmap2::Mmap;

use crate::media::{extract_pcm, pcm_path};
use crate::model::{Asset, AssetKind, CHANNELS, ClipContent, Project, SAMPLE_RATE, TrackKind};

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

/// Extracts the PCM cache for `asset` unless it already exists. Concurrent callers for the
/// same file (import, export and captions) wait for one extraction instead of racing.
pub fn ensure_pcm(cache_dir: &Path, asset: &Asset, progress: impl FnMut(f32)) -> Result<PathBuf> {
    static LOCKS: std::sync::OnceLock<std::sync::Mutex<HashMap<PathBuf, Arc<std::sync::Mutex<()>>>>> = std::sync::OnceLock::new();
    let path = pcm_path(cache_dir, asset);
    let lock = LOCKS.get_or_init(Default::default).lock().unwrap().entry(path.clone()).or_default().clone();
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    if !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap())?;
        extract_pcm(Path::new(&asset.path), &path, progress)?;
    }
    Ok(path)
}

pub struct Mixer {
    cache_dir: PathBuf,
    sources: HashMap<String, Option<Arc<Pcm>>>,
}

impl Mixer {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir, sources: HashMap::new() }
    }

    fn source(&mut self, asset: &Asset) -> Option<Arc<Pcm>> {
        if let Some(Some(pcm)) = self.sources.get(&asset.id) {
            return Some(pcm.clone());
        }
        // Missing caches are retried, since extraction may still be running.
        let path = pcm_path(&self.cache_dir, asset);
        let pcm = Pcm::open(&path).ok().map(Arc::new);
        self.sources.insert(asset.id.clone(), pcm.clone());
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
            for clip in &track.clips {
                let ClipContent::Media { asset_id, source_in_us, volume, .. } = &clip.content else { continue };
                let c0 = us_to_samples(clip.start_us);
                let c1 = us_to_samples(clip.end_us());
                let (from, to) = (start.max(c0), end.min(c1));
                if from >= to || *volume <= 0.0 {
                    continue;
                }
                let Some(asset) = project.asset(asset_id) else { continue };
                if !has_audio(asset) {
                    continue;
                }
                let Some(pcm) = self.source(asset) else { continue };
                let samples = pcm.samples();
                let src0 = us_to_samples(*source_in_us);
                for i in from..to {
                    let src = src0 + (i - c0);
                    if src < 0 || (src as usize + 1) * CHANNELS > samples.len() {
                        continue;
                    }
                    let edge = (i - c0).min(c1 - 1 - i);
                    let gain = volume * if edge < EDGE_FADE { edge as f32 / EDGE_FADE as f32 } else { 1.0 };
                    let o = ((i - start) as usize) * CHANNELS;
                    let s = src as usize * CHANNELS;
                    out[o] += samples[s] * gain;
                    out[o + 1] += samples[s + 1] * gain;
                }
            }
        }
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }
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
