//! Timeline edits and undo history. The main track is magnetic like in CapCut:
//! its clips always sit back to back from zero, so deleting or moving closes gaps.

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::model::{Adjust, Animation, Asset, AssetKind, Clip, ClipContent, Keyframe, Project, TextStyle, Track, TrackKind, Transform, Transition};

pub const MAIN_TRACK: &str = "main";
const IMAGE_DURATION_US: i64 = 3_000_000;
const TEXT_DURATION_US: i64 = 3_000_000;
const UNDO_LIMIT: usize = 200;
const COALESCE_WINDOW: Duration = Duration::from_millis(1500);
const MIN_SPEED: f32 = 0.1;
const MAX_SPEED: f32 = 10.0;
const MAX_TRANSITION_US: i64 = 2_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptionSegment {
    pub start_us: i64,
    pub end_us: i64,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EditCmd {
    AddAssets { assets: Vec<Asset> },
    RemoveAsset { asset_id: String },
    AddClip { asset_id: String, start_us: Option<i64>, track_id: Option<String> },
    AddText { start_us: i64, text: String, style: TextStyle },
    /// `track_id: None` moves the clip to a new track of the right kind.
    MoveClip { clip_id: String, track_id: Option<String>, start_us: i64 },
    TrimClip { clip_id: String, start_us: i64, duration_us: i64, source_in_us: Option<i64> },
    SplitClip { clip_id: String, at_us: i64 },
    DeleteClips { clip_ids: Vec<String> },
    UpdateClip {
        clip_id: String,
        transform: Option<Transform>,
        volume: Option<f32>,
        text: Option<String>,
        style: Option<TextStyle>,
        /// Changing speed keeps the source range, so the clip gets shorter or longer.
        speed: Option<f32>,
        adjust: Option<Adjust>,
        fade_in_us: Option<i64>,
        fade_out_us: Option<i64>,
    },
    SetAnimation { clip_id: String, slot: AnimationSlot, animation: Option<Animation> },
    /// Main-track clips only, and not the first one.
    SetTransition { clip_id: String, transition: Option<Transition> },
    SetKeyframes { clip_id: String, keyframes: Vec<Keyframe> },
    /// Places a copy right after the clip.
    DuplicateClip { clip_id: String },
    /// Moves a video clip's sound to an audio track and silences the video clip.
    DetachAudio { clip_id: String },
    UpdateTrack { track_id: String, muted: Option<bool>, hidden: Option<bool> },
    SetCanvas { width: u32, height: u32, background: Option<String>, background_blur: Option<f32> },
    AddCaptions { segments: Vec<CaptionSegment>, style: TextStyle },
    RenameProject { name: String },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AnimationSlot {
    In,
    Out,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOutcome {
    /// Clips the UI should select after the edit, e.g. a newly added clip.
    pub select: Vec<String>,
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

fn track_kind_for(asset: &Asset) -> TrackKind {
    match asset.kind {
        AssetKind::Audio => TrackKind::Audio,
        AssetKind::Video | AssetKind::Image => TrackKind::Video,
    }
}

fn clip_track_kind(project: &Project, clip: &Clip) -> TrackKind {
    match &clip.content {
        ClipContent::Text { .. } => TrackKind::Text,
        ClipContent::Media { asset_id, .. } => project.asset(asset_id).map(track_kind_for).unwrap_or(TrackKind::Video),
    }
}

fn min_duration(project: &Project) -> i64 {
    project.frame_duration_us().ceil() as i64
}

impl Project {
    fn find_clip(&self, id: &str) -> Option<(usize, usize)> {
        self.tracks.iter().enumerate().find_map(|(ti, t)| t.clips.iter().position(|c| c.id == id).map(|ci| (ti, ci)))
    }

    fn track_index(&self, id: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    fn is_free(&self, track: usize, start: i64, end: i64, ignore: Option<&str>) -> bool {
        self.tracks[track]
            .clips
            .iter()
            .filter(|c| Some(c.id.as_str()) != ignore)
            .all(|c| end <= c.start_us || start >= c.end_us())
    }

    fn insert_track(&mut self, at: usize, kind: TrackKind) -> usize {
        let name = match kind {
            TrackKind::Video => "Overlay",
            TrackKind::Audio => "Audio",
            TrackKind::Text => "Text",
        };
        let at = at.clamp(1, self.tracks.len());
        self.tracks.insert(at, Track { id: new_id(), kind, name: name.into(), muted: false, hidden: false, clips: Vec::new() });
        at
    }

    /// A track of `kind` free over [start, end), creating one if needed.
    fn free_track(&mut self, kind: TrackKind, start: i64, end: i64) -> usize {
        let found = self
            .tracks
            .iter()
            .enumerate()
            .skip(1)
            .find(|(i, t)| t.kind == kind && self.is_free(*i, start, end, None))
            .map(|(i, _)| i);
        found.unwrap_or_else(|| {
            let at = match kind {
                TrackKind::Video => self.tracks.iter().rposition(|t| t.kind == TrackKind::Video).unwrap_or(0) + 1,
                TrackKind::Audio | TrackKind::Text => self.tracks.len(),
            };
            self.insert_track(at, kind)
        })
    }

    /// Lays out main-track clips back to back. Existing clips keep their order; a clip
    /// that was moved or added goes before the first clip whose middle is past `start`.
    fn pack_main(&mut self, moved: Option<(&str, i64)>) {
        let Some(main) = self.track_index(MAIN_TRACK) else { return };
        let clips = &mut self.tracks[main].clips;
        let moved_clip = moved.and_then(|(id, _)| clips.iter().position(|c| c.id == id)).map(|i| clips.remove(i));
        clips.sort_by_key(|c| c.start_us);
        if let (Some(clip), Some((_, start))) = (moved_clip, moved) {
            let at = clips.iter().position(|c| c.start_us + c.duration_us / 2 > start).unwrap_or(clips.len());
            clips.insert(at, clip);
        }
        let mut t = 0;
        for c in clips.iter_mut() {
            c.start_us = t;
            t += c.duration_us;
        }
    }

    fn tidy(&mut self) {
        self.tracks.retain(|t| t.id == MAIN_TRACK || !t.clips.is_empty());
        for t in &mut self.tracks {
            t.clips.sort_by_key(|c| c.start_us);
        }
    }

    pub fn apply(&mut self, cmd: EditCmd) -> Result<EditOutcome> {
        let mut out = EditOutcome::default();
        let mut moved: Option<(String, i64)> = None;
        let min = min_duration(self);
        match cmd {
            EditCmd::AddAssets { assets } => {
                for a in assets {
                    if self.asset(&a.id).is_none() {
                        self.assets.push(a);
                    }
                }
            }
            EditCmd::RemoveAsset { asset_id } => {
                self.assets.retain(|a| a.id != asset_id);
                for t in &mut self.tracks {
                    t.clips.retain(|c| !matches!(&c.content, ClipContent::Media { asset_id: a, .. } if *a == asset_id));
                }
            }
            EditCmd::AddClip { asset_id, start_us, track_id } => {
                let asset = self.asset(&asset_id).ok_or_else(|| anyhow!("Unknown media"))?.clone();
                let kind = track_kind_for(&asset);
                let duration = if asset.kind == AssetKind::Image { IMAGE_DURATION_US } else { asset.duration_us.max(min) };
                let clip = Clip::new(new_id(), 0, duration, ClipContent::Media {
                    asset_id,
                    source_in_us: 0,
                    volume: 1.0,
                    transform: Transform::default(),
                    speed: 1.0,
                    adjust: Adjust::default(),
                    fade_in_us: 0,
                    fade_out_us: 0,
                });
                out.select.push(clip.id.clone());
                let requested = track_id.and_then(|id| self.track_index(&id)).filter(|&i| self.tracks[i].kind == kind);
                let target = match (kind, requested) {
                    (_, Some(i)) => i,
                    (TrackKind::Video, None) => 0,
                    (_, None) => self.free_track(kind, start_us.unwrap_or(0), start_us.unwrap_or(0) + duration),
                };
                let start = start_us.unwrap_or(if target == 0 { i64::MAX / 4 } else { 0 }).max(0);
                if target != 0 && !self.is_free(target, start, start + duration, None) {
                    let t = self.free_track(kind, start, start + duration);
                    self.tracks[t].clips.push(Clip { start_us: start, ..clip });
                } else {
                    if target == 0 {
                        moved = Some((clip.id.clone(), start));
                    }
                    self.tracks[target].clips.push(Clip { start_us: start, ..clip });
                }
            }
            EditCmd::AddText { start_us, text, style } => {
                let start = start_us.max(0);
                let t = self.free_track(TrackKind::Text, start, start + TEXT_DURATION_US);
                let clip = Clip::new(new_id(), start, TEXT_DURATION_US, ClipContent::Text {
                    text,
                    style,
                    transform: Transform { y: 0.3, ..Transform::default() },
                });
                out.select.push(clip.id.clone());
                self.tracks[t].clips.push(clip);
            }
            EditCmd::MoveClip { clip_id, track_id, start_us } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let kind = clip_track_kind(self, &self.tracks[ti].clips[ci]);
                let mut clip = self.tracks[ti].clips.remove(ci);
                let start = start_us.max(0);
                clip.start_us = start;
                let end = start + clip.duration_us;
                let target = track_id.and_then(|id| self.track_index(&id)).filter(|&i| self.tracks[i].kind == kind);
                let target = match target {
                    Some(0) => {
                        moved = Some((clip.id.clone(), start));
                        0
                    }
                    Some(i) if self.is_free(i, start, end, None) => i,
                    // Dropped onto an occupied spot or no track: a new track right above.
                    Some(i) => self.insert_track(i + 1, kind),
                    None => {
                        let at = match kind {
                            TrackKind::Video => ti.max(0) + 1,
                            _ => self.tracks.len(),
                        };
                        self.insert_track(at, kind)
                    }
                };
                out.select.push(clip.id.clone());
                self.tracks[target].clips.push(clip);
            }
            EditCmd::TrimClip { clip_id, start_us, duration_us, source_in_us } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let (limit, speed) = match &self.tracks[ti].clips[ci].content {
                    ClipContent::Media { asset_id, speed, .. } => {
                        (self.asset(asset_id).filter(|a| a.kind != AssetKind::Image).map(|a| a.duration_us), *speed as f64)
                    }
                    ClipContent::Text { .. } => (None, 1.0),
                };
                let clip = &mut self.tracks[ti].clips[ci];
                let (old_start, old_end) = (clip.start_us, clip.end_us());
                let mut start = start_us.max(0);
                let mut duration = duration_us.max(min);
                if let ClipContent::Media { source_in_us: src, .. } = &mut clip.content {
                    let mut new_src = source_in_us.unwrap_or(*src).max(0);
                    if source_in_us.is_some() && start_us != old_start {
                        // Trimming the left edge: keep the right edge in place.
                        let shift = ((new_src - *src) as f64 / speed).round() as i64;
                        start = (old_start + shift).max(0);
                        new_src = *src + ((start - old_start) as f64 * speed).round() as i64;
                        duration = (old_end - start).max(min);
                    }
                    if let Some(limit) = limit {
                        new_src = new_src.min((limit - min).max(0));
                        duration = duration.min(((limit - new_src) as f64 / speed) as i64).max(min);
                    }
                    *src = new_src;
                }
                // Keyframes stay attached to the content when the left edge moves.
                let shift = start - old_start;
                for k in &mut clip.keyframes {
                    k.t_us -= shift;
                }
                clip.start_us = start;
                clip.duration_us = duration;
                if ti != 0 {
                    let end = start + duration;
                    if !self.is_free(ti, start, end, Some(&clip_id)) {
                        bail!("Clips on the same track cannot overlap");
                    }
                }
            }
            EditCmd::SplitClip { clip_id, at_us } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let clip = &mut self.tracks[ti].clips[ci];
                if at_us < clip.start_us + min || at_us > clip.end_us() - min {
                    bail!("Move the playhead inside the clip to split it");
                }
                let mut second = clip.clone();
                let offset = at_us - clip.start_us;
                second.id = new_id();
                second.start_us = at_us;
                second.duration_us = clip.end_us() - at_us;
                if let ClipContent::Media { source_in_us, speed, .. } = &mut second.content {
                    *source_in_us += (offset as f64 * *speed as f64).round() as i64;
                }
                // The first half keeps the entry animation, the second the exit.
                clip.anim_out = None;
                second.anim_in = None;
                second.transition_in = None;
                for k in &mut second.keyframes {
                    k.t_us -= offset;
                }
                clip.duration_us = offset;
                out.select.push(second.id.clone());
                self.tracks[ti].clips.insert(ci + 1, second);
            }
            EditCmd::DeleteClips { clip_ids } => {
                for t in &mut self.tracks {
                    t.clips.retain(|c| !clip_ids.contains(&c.id));
                }
            }
            EditCmd::UpdateClip { clip_id, transform, volume, text, style, speed, adjust, fade_in_us, fade_out_us } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let clip = &mut self.tracks[ti].clips[ci];
                let half = clip.duration_us / 2;
                match &mut clip.content {
                    ClipContent::Media { transform: tr, volume: v, speed: sp, adjust: adj, fade_in_us: fi, fade_out_us: fo, .. } => {
                        if let Some(x) = transform {
                            *tr = x;
                        }
                        if let Some(x) = volume {
                            *v = x.clamp(0.0, 4.0);
                        }
                        if let Some(x) = adjust {
                            *adj = x;
                        }
                        if let Some(x) = fade_in_us {
                            *fi = x.clamp(0, half);
                        }
                        if let Some(x) = fade_out_us {
                            *fo = x.clamp(0, half);
                        }
                        if let Some(x) = speed {
                            let x = x.clamp(MIN_SPEED, MAX_SPEED);
                            let ratio = *sp as f64 / x as f64;
                            *sp = x;
                            clip.duration_us = ((clip.duration_us as f64 * ratio).round() as i64).max(min);
                            for k in &mut clip.keyframes {
                                k.t_us = (k.t_us as f64 * ratio).round() as i64;
                            }
                        }
                    }
                    ClipContent::Text { transform: tr, text: tx, style: st } => {
                        if let Some(x) = transform {
                            *tr = x;
                        }
                        if let Some(x) = text {
                            *tx = x;
                        }
                        if let Some(x) = style {
                            *st = x;
                        }
                    }
                }
            }
            EditCmd::SetAnimation { clip_id, slot, animation } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let clip = &mut self.tracks[ti].clips[ci];
                let animation = animation.map(|a| Animation { duration_us: a.duration_us.clamp(min, clip.duration_us), ..a });
                match slot {
                    AnimationSlot::In => clip.anim_in = animation,
                    AnimationSlot::Out => clip.anim_out = animation,
                }
            }
            EditCmd::SetTransition { clip_id, transition } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                if self.tracks[ti].id != MAIN_TRACK || ci == 0 {
                    bail!("Transitions go between two clips on the main track");
                }
                let shortest = self.tracks[ti].clips[ci - 1].duration_us.min(self.tracks[ti].clips[ci].duration_us);
                self.tracks[ti].clips[ci].transition_in = transition
                    .map(|t| Transition { duration_us: t.duration_us.clamp(min, MAX_TRANSITION_US.min(shortest).max(min)), ..t });
            }
            EditCmd::SetKeyframes { clip_id, mut keyframes } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                keyframes.sort_by_key(|k| k.t_us);
                keyframes.dedup_by_key(|k| k.t_us);
                self.tracks[ti].clips[ci].keyframes = keyframes;
            }
            EditCmd::DuplicateClip { clip_id } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let mut copy = self.tracks[ti].clips[ci].clone();
                copy.id = new_id();
                copy.start_us = self.tracks[ti].clips[ci].end_us();
                copy.transition_in = None;
                out.select.push(copy.id.clone());
                let target = if ti == 0 || self.is_free(ti, copy.start_us, copy.end_us(), None) {
                    ti
                } else {
                    let kind = self.tracks[ti].kind;
                    self.free_track(kind, copy.start_us, copy.end_us())
                };
                if target == 0 {
                    moved = Some((copy.id.clone(), copy.start_us));
                }
                self.tracks[target].clips.push(copy);
            }
            EditCmd::DetachAudio { clip_id } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let clip = self.tracks[ti].clips[ci].clone();
                let ClipContent::Media { asset_id, volume, .. } = &clip.content else { bail!("Only video clips have sound to detach") };
                let asset = self.asset(asset_id).ok_or_else(|| anyhow!("Unknown media"))?;
                if asset.kind != AssetKind::Video || !asset.has_audio || *volume <= 0.0 {
                    bail!("This clip has no sound to detach");
                }
                let mut sound = Clip::new(new_id(), clip.start_us, clip.duration_us, clip.content.clone());
                if let ClipContent::Media { transform, adjust, .. } = &mut sound.content {
                    *transform = Transform::default();
                    *adjust = Adjust::default();
                }
                if let ClipContent::Media { volume, .. } = &mut self.tracks[ti].clips[ci].content {
                    *volume = 0.0;
                }
                let target = self.free_track(TrackKind::Audio, clip.start_us, clip.end_us());
                out.select.push(sound.id.clone());
                self.tracks[target].clips.push(sound);
            }
            EditCmd::UpdateTrack { track_id, muted, hidden } => {
                let i = self.track_index(&track_id).ok_or_else(|| anyhow!("Unknown track"))?;
                let t = &mut self.tracks[i];
                if let Some(m) = muted {
                    t.muted = m;
                }
                if let Some(h) = hidden {
                    t.hidden = h;
                }
            }
            EditCmd::SetCanvas { width, height, background, background_blur } => {
                if let Some(b) = background_blur {
                    self.canvas.background_blur = b.clamp(0.0, 1.0);
                }
                if width < 16 || height < 16 || width > 7680 || height > 7680 {
                    bail!("Unsupported canvas size");
                }
                self.canvas.width = width & !1;
                self.canvas.height = height & !1;
                if let Some(b) = background {
                    self.canvas.background = b;
                }
            }
            EditCmd::AddCaptions { segments, style } => {
                self.tracks.retain(|t| !(t.kind == TrackKind::Text && t.name == "Captions"));
                let clips: Vec<Clip> = segments
                    .into_iter()
                    .filter(|s| s.end_us > s.start_us && !s.text.trim().is_empty())
                    .map(|s| {
                        Clip::new(new_id(), s.start_us.max(0), (s.end_us - s.start_us).max(min), ClipContent::Text {
                            text: s.text.trim().to_string(),
                            style: style.clone(),
                            transform: Transform { y: 0.28, ..Transform::default() },
                        })
                    })
                    .collect();
                // Whisper segments can overlap by a few ms; trim each to the next start.
                let mut clips = clips;
                clips.sort_by_key(|c| c.start_us);
                for i in 1..clips.len() {
                    let next = clips[i].start_us;
                    let prev = &mut clips[i - 1];
                    if prev.end_us() > next {
                        prev.duration_us = (next - prev.start_us).max(1);
                    }
                }
                out.select = clips.iter().map(|c| c.id.clone()).take(1).collect();
                self.tracks.push(Track { id: new_id(), kind: TrackKind::Text, name: "Captions".into(), muted: false, hidden: false, clips });
            }
            EditCmd::RenameProject { name } => {
                let name = name.trim();
                if !name.is_empty() {
                    self.name = name.to_string();
                }
            }
        }
        self.pack_main(moved.as_ref().map(|(id, s)| (id.as_str(), *s)));
        self.tidy();
        Ok(out)
    }
}

/// Owns the current project and its undo history.
pub struct Editor {
    pub project: Project,
    undo: Vec<Project>,
    redo: Vec<Project>,
    coalesce: Option<(String, Instant)>,
    pub revision: u64,
}

impl Editor {
    pub fn new(project: Project) -> Self {
        Self { project, undo: Vec::new(), redo: Vec::new(), coalesce: None, revision: 0 }
    }

    /// Applies `cmd`. Edits sharing a `coalesce` key in quick succession (slider drags,
    /// typing) form a single undo step.
    pub fn apply(&mut self, cmd: EditCmd, coalesce: Option<String>) -> Result<EditOutcome> {
        let before = self.project.clone();
        let outcome = match self.project.apply(cmd) {
            Ok(o) => o,
            Err(e) => {
                // A rejected edit may have changed the project halfway.
                self.project = before;
                return Err(e);
            }
        };
        if self.project == before {
            return Ok(outcome);
        }
        let merge = match (&coalesce, &self.coalesce) {
            (Some(k), Some((last, at))) => k == last && at.elapsed() < COALESCE_WINDOW,
            _ => false,
        };
        if !merge {
            self.undo.push(before);
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.coalesce = coalesce.map(|k| (k, Instant::now()));
        self.redo.clear();
        self.revision += 1;
        Ok(outcome)
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.project, prev));
        self.coalesce = None;
        self.revision += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.project, next));
        self.coalesce = None;
        self.revision += 1;
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: &str, kind: AssetKind, secs: i64) -> Asset {
        Asset {
            id: id.into(),
            name: id.into(),
            path: format!("/tmp/{id}"),
            kind,
            duration_us: secs * 1_000_000,
            width: 1080,
            height: 1920,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        }
    }

    fn project() -> Project {
        let mut p = Project::new("t");
        p.apply(EditCmd::AddAssets {
            assets: vec![asset("a", AssetKind::Video, 5), asset("b", AssetKind::Video, 3), asset("m", AssetKind::Audio, 20)],
        })
        .unwrap();
        p
    }

    fn main_layout(p: &Project) -> Vec<(i64, i64)> {
        p.tracks[0].clips.iter().map(|c| (c.start_us / 1000, c.duration_us / 1000)).collect()
    }

    #[test]
    fn clips_append_to_main_track_back_to_back() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        assert_eq!(main_layout(&p), vec![(0, 5000), (5000, 3000)]);
    }

    #[test]
    fn audio_goes_to_its_own_track() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(0), track_id: None }).unwrap();
        assert_eq!(p.tracks.len(), 2);
        assert_eq!(p.tracks[1].kind, TrackKind::Audio);
    }

    #[test]
    fn split_keeps_source_continuity() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        let id = p.tracks[0].clips[0].id.clone();
        p.apply(EditCmd::SplitClip { clip_id: id, at_us: 2_000_000 }).unwrap();
        assert_eq!(main_layout(&p), vec![(0, 2000), (2000, 3000)]);
        let ClipContent::Media { source_in_us, .. } = &p.tracks[0].clips[1].content else { panic!() };
        assert_eq!(*source_in_us, 2_000_000);
    }

    #[test]
    fn deleting_on_main_track_closes_the_gap() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        let first = p.tracks[0].clips[0].id.clone();
        p.apply(EditCmd::DeleteClips { clip_ids: vec![first] }).unwrap();
        assert_eq!(main_layout(&p), vec![(0, 3000)]);
    }

    #[test]
    fn moving_on_main_track_reorders() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        let b = p.tracks[0].clips[1].id.clone();
        p.apply(EditCmd::MoveClip { clip_id: b.clone(), track_id: Some(MAIN_TRACK.into()), start_us: 0 }).unwrap();
        assert_eq!(p.tracks[0].clips[0].id, b);
        assert_eq!(main_layout(&p), vec![(0, 3000), (3000, 5000)]);
    }

    #[test]
    fn moving_off_main_creates_overlay_and_removes_empty_tracks() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        let b = p.tracks[0].clips[1].id.clone();
        p.apply(EditCmd::MoveClip { clip_id: b.clone(), track_id: None, start_us: 1_000_000 }).unwrap();
        assert_eq!(p.tracks.len(), 2);
        assert_eq!(p.tracks[1].clips[0].start_us, 1_000_000);
        p.apply(EditCmd::MoveClip { clip_id: b, track_id: Some(MAIN_TRACK.into()), start_us: 9_000_000 }).unwrap();
        assert_eq!(p.tracks.len(), 1);
        assert_eq!(main_layout(&p), vec![(0, 5000), (5000, 3000)]);
    }

    #[test]
    fn trim_is_clamped_to_source_length() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        let id = p.tracks[0].clips[0].id.clone();
        p.apply(EditCmd::TrimClip { clip_id: id.clone(), start_us: 0, duration_us: 60_000_000, source_in_us: None }).unwrap();
        assert_eq!(p.tracks[0].clips[0].duration_us, 3_000_000);
        // Left edge trim by one second keeps the right edge, then the main track packs.
        p.apply(EditCmd::TrimClip { clip_id: id, start_us: 1_000_000, duration_us: 2_000_000, source_in_us: Some(1_000_000) })
            .unwrap();
        let c = &p.tracks[0].clips[0];
        assert_eq!((c.start_us, c.duration_us), (0, 2_000_000));
        let ClipContent::Media { source_in_us, .. } = &c.content else { panic!() };
        assert_eq!(*source_in_us, 1_000_000);
    }

    #[test]
    fn extending_a_main_clip_keeps_the_order() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        let a = p.tracks[0].clips[0].id.clone();
        p.apply(EditCmd::TrimClip { clip_id: a.clone(), start_us: 0, duration_us: 2_000_000, source_in_us: None }).unwrap();
        p.apply(EditCmd::TrimClip { clip_id: a.clone(), start_us: 0, duration_us: 5_000_000, source_in_us: None }).unwrap();
        assert_eq!(p.tracks[0].clips[0].id, a);
        assert_eq!(main_layout(&p), vec![(0, 5000), (5000, 3000)]);
    }

    #[test]
    fn rejected_edit_leaves_the_project_untouched() {
        let mut e = Editor::new(project());
        e.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(0), track_id: None }, None).unwrap();
        let track = e.project.tracks[1].id.clone();
        let first = e.project.tracks[1].clips[0].id.clone();
        e.apply(EditCmd::TrimClip { clip_id: first.clone(), start_us: 0, duration_us: 5_000_000, source_in_us: None }, None).unwrap();
        e.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(10_000_000), track_id: Some(track) }, None).unwrap();
        assert_eq!(e.project.tracks[1].clips.len(), 2);
        let before = e.project.clone();
        let overlap = EditCmd::TrimClip { clip_id: first, start_us: 0, duration_us: 15_000_000, source_in_us: None };
        assert!(e.apply(overlap, None).is_err());
        assert_eq!(e.project, before);
    }

    fn update(id: &str) -> EditCmd {
        EditCmd::UpdateClip {
            clip_id: id.into(),
            transform: None,
            volume: None,
            text: None,
            style: None,
            speed: None,
            adjust: None,
            fade_in_us: None,
            fade_out_us: None,
        }
    }

    #[test]
    fn speed_changes_length_and_split_keeps_source() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        let id = p.tracks[0].clips[0].id.clone();
        let EditCmd::UpdateClip { clip_id, transform, volume, text, style, adjust, fade_in_us, fade_out_us, .. } = update(&id) else { unreachable!() };
        p.apply(EditCmd::UpdateClip { clip_id, transform, volume, text, style, speed: Some(2.0), adjust, fade_in_us, fade_out_us }).unwrap();
        assert_eq!(main_layout(&p), vec![(0, 2500)]);
        p.apply(EditCmd::SplitClip { clip_id: id, at_us: 1_000_000 }).unwrap();
        let ClipContent::Media { source_in_us, .. } = &p.tracks[0].clips[1].content else { panic!() };
        assert_eq!(*source_in_us, 2_000_000);
    }

    #[test]
    fn duplicate_lands_right_after_the_original() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        let a = p.tracks[0].clips[0].id.clone();
        let out = p.apply(EditCmd::DuplicateClip { clip_id: a }).unwrap();
        assert_eq!(p.tracks[0].clips[1].id, out.select[0]);
        assert_eq!(main_layout(&p), vec![(0, 5000), (5000, 5000), (10000, 3000)]);
    }

    #[test]
    fn detach_audio_silences_the_video_clip() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        let a = p.tracks[0].clips[0].id.clone();
        p.apply(EditCmd::DetachAudio { clip_id: a.clone() }).unwrap();
        let ClipContent::Media { volume, .. } = &p.tracks[0].clips[0].content else { panic!() };
        assert_eq!(*volume, 0.0);
        assert_eq!(p.tracks[1].kind, TrackKind::Audio);
        assert!(p.apply(EditCmd::DetachAudio { clip_id: a }).is_err());
    }

    #[test]
    fn transitions_only_between_main_clips() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        let (a, b) = (p.tracks[0].clips[0].id.clone(), p.tracks[0].clips[1].id.clone());
        let t = Transition { kind: crate::model::TransitionKind::Dissolve, duration_us: 9_000_000 };
        assert!(p.apply(EditCmd::SetTransition { clip_id: a, transition: Some(t) }).is_err());
        p.apply(EditCmd::SetTransition { clip_id: b, transition: Some(t) }).unwrap();
        assert_eq!(p.tracks[0].clips[1].transition_in.unwrap().duration_us, 2_000_000);
    }

    #[test]
    fn undo_redo_and_coalescing() {
        let mut e = Editor::new(project());
        e.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }, None).unwrap();
        let id = e.project.tracks[0].clips[0].id.clone();
        for v in [0.5, 0.6, 0.7] {
            let cmd = EditCmd::UpdateClip {
                clip_id: id.clone(),
                transform: None,
                volume: Some(v),
                text: None,
                style: None,
                speed: None,
                adjust: None,
                fade_in_us: None,
                fade_out_us: None,
            };
            e.apply(cmd, Some("vol".into())).unwrap();
        }
        assert!(e.undo());
        let ClipContent::Media { volume, .. } = &e.project.tracks[0].clips[0].content else { panic!() };
        assert_eq!(*volume, 1.0);
        assert!(e.undo());
        assert!(e.project.tracks[0].clips.is_empty());
        assert!(e.redo());
        assert_eq!(e.project.tracks[0].clips.len(), 1);
    }

    #[test]
    fn captions_replace_previous_caption_track() {
        let mut p = project();
        let style = TextStyle {
            font_size: 70.0,
            color: "#fff".into(),
            bold: true,
            stroke_width: 5.0,
            stroke_color: "#000".into(),
            background: None,
        };
        let seg = |s, e, t: &str| CaptionSegment { start_us: s, end_us: e, text: t.into() };
        p.apply(EditCmd::AddCaptions { segments: vec![seg(0, 1_200_000, "Ahoj"), seg(1_000_000, 2_000_000, "světe")], style: style.clone() })
            .unwrap();
        p.apply(EditCmd::AddCaptions { segments: vec![seg(0, 1_000_000, "Znovu")], style }).unwrap();
        let caption_tracks: Vec<_> = p.tracks.iter().filter(|t| t.name == "Captions").collect();
        assert_eq!(caption_tracks.len(), 1);
        assert_eq!(caption_tracks[0].clips.len(), 1);
    }
}
