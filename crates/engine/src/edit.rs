//! Timeline edits and undo history. The main track is magnetic like in CapCut:
//! its clips always sit back to back from zero, so deleting or moving closes gaps.

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::model::{
    Adjust, Animation, Asset, AssetKind, CAPTIONS_TRACK, Canvas, Clip, ClipContent, Keyframe, Project, TextStyle,
    Track, TrackKind, Transform, Transition,
};

pub const MAIN_TRACK: &str = "main";
const IMAGE_DURATION_US: i64 = 3_000_000;
const TEXT_DURATION_US: i64 = 3_000_000;
const UNDO_LIMIT: usize = 200;
pub const MIN_SPEED: f32 = 0.1;
pub const MAX_SPEED: f32 = 10.0;
pub const MAX_TRANSITION_US: i64 = 2_000_000;
pub const CAPTION_Y: f32 = 0.15;

#[derive(Deserialize)]
pub struct CaptionPreset {
    pub name: String,
    pub style: TextStyle,
}

pub fn caption_presets() -> &'static [CaptionPreset] {
    static PRESETS: std::sync::LazyLock<Vec<CaptionPreset>> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../../assets/presets/captions.json")).expect("valid caption presets")
    });
    &PRESETS
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub min_speed: f32,
    pub max_speed: f32,
    pub max_transition_us: i64,
    pub caption_y: f32,
}

pub const LIMITS: Limits =
    Limits { min_speed: MIN_SPEED, max_speed: MAX_SPEED, max_transition_us: MAX_TRANSITION_US, caption_y: CAPTION_Y };

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptionSegment {
    pub start_us: i64,
    pub end_us: i64,
    pub text: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EditCmd {
    AddAssets {
        assets: Vec<Asset>,
    },
    RemoveAsset {
        asset_id: String,
    },
    AddClip {
        asset_id: String,
        start_us: Option<i64>,
        track_id: Option<String>,
    },
    AddText {
        start_us: i64,
        text: String,
        style: TextStyle,
    },
    /// `track_id: None` moves the clip to a new track of the right kind.
    MoveClip {
        clip_id: String,
        track_id: Option<String>,
        start_us: i64,
    },
    TrimClip {
        clip_id: String,
        start_us: i64,
        duration_us: i64,
        source_in_us: Option<i64>,
    },
    SplitClip {
        clip_id: String,
        at_us: i64,
    },
    DeleteClips {
        clip_ids: Vec<String>,
    },
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
    SetAnimation {
        clip_id: String,
        slot: AnimationSlot,
        animation: Option<Animation>,
    },
    /// Main-track clips only, and not the first one.
    SetTransition {
        clip_id: String,
        transition: Option<Transition>,
    },
    SetKeyframes {
        clip_id: String,
        keyframes: Vec<Keyframe>,
    },
    /// Places a copy right after the clip.
    DuplicateClip {
        clip_id: String,
    },
    /// Moves a video clip's sound to an audio track and silences the video clip.
    DetachAudio {
        clip_id: String,
    },
    UpdateTrack {
        track_id: String,
        muted: Option<bool>,
        hidden: Option<bool>,
        keep_in_place: Option<bool>,
    },
    SetCanvas {
        width: u32,
        height: u32,
        background: Option<String>,
        background_blur: Option<f32>,
    },
    /// Adds a new captions track; existing tracks are left alone.
    AddCaptions {
        segments: Vec<CaptionSegment>,
        style: TextStyle,
    },
    /// Replaces the clips of an existing captions track.
    ReplaceCaptions {
        track_id: String,
        segments: Vec<CaptionSegment>,
        style: TextStyle,
    },
    /// Cuts the timeline ranges out of every track except `keep_track_ids` (by default the
    /// tracks kept in place) and closes the gaps, so video, overlays, audio and captions stay in sync.
    RippleDeleteRanges {
        ranges: Vec<TimeRange>,
        #[serde(default)]
        keep_track_ids: Option<Vec<String>>,
    },
    RenameProject {
        name: String,
    },
}

/// Timeline range `[start_us, end_us)`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TimeRange {
    pub start_us: i64,
    pub end_us: i64,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AnimationSlot {
    In,
    Out,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOutcome {
    /// Clips the UI should select after the edit, e.g. a newly added clip.
    pub select: Vec<String>,
    /// Clip ids that exist after the edit but not before (including split halves).
    pub created: Vec<String>,
    /// Clip ids that existed before the edit but not after.
    pub removed: Vec<String>,
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

    fn insert_track(&mut self, at: usize, kind: TrackKind, keep_in_place: bool) -> usize {
        let name = match kind {
            TrackKind::Video => "Overlay",
            TrackKind::Audio => "Audio",
            TrackKind::Text => "Text",
        };
        let at = at.clamp(1, self.tracks.len());
        self.tracks.insert(
            at,
            Track {
                id: new_id(),
                kind,
                name: name.into(),
                muted: false,
                hidden: false,
                keep_in_place,
                clips: Vec::new(),
            },
        );
        at
    }

    /// A track of `kind` free over [start, end) that is kept in place or not, creating one if needed.
    fn free_track(&mut self, kind: TrackKind, start: i64, end: i64, keep_in_place: bool) -> usize {
        let found = self
            .tracks
            .iter()
            .enumerate()
            .skip(1)
            .find(|(i, t)| t.kind == kind && t.keep_in_place == keep_in_place && self.is_free(*i, start, end, None))
            .map(|(i, _)| i);
        found.unwrap_or_else(|| {
            let at = match kind {
                TrackKind::Video => self.tracks.iter().rposition(|t| t.kind == TrackKind::Video).unwrap_or(0) + 1,
                TrackKind::Audio | TrackKind::Text => self.tracks.len(),
            };
            self.insert_track(at, kind, keep_in_place)
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

    /// Splits a clip at `at_us` (strictly inside it) and returns the index of the second half.
    fn split_clip(&mut self, ti: usize, ci: usize, at_us: i64) -> usize {
        let clip = &mut self.tracks[ti].clips[ci];
        let offset = at_us - clip.start_us;
        let mut second = clip.clone();
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
        self.tracks[ti].clips.insert(ci + 1, second);
        ci + 1
    }

    /// Removes `range` from one track: clips are cut at its edges, the inside goes, later
    /// clips move left. Pieces shorter than `min` are dropped rather than kept as slivers.
    fn ripple_delete_track(&mut self, ti: usize, range: TimeRange, min: i64) {
        // Splitting a text clip would show its whole text twice; keep only the longer side.
        for c in self.tracks[ti].clips.iter_mut().filter(|c| matches!(c.content, ClipContent::Text { .. })) {
            if c.start_us < range.start_us && range.end_us < c.end_us() {
                let (before, after) = (range.start_us - c.start_us, c.end_us() - range.end_us);
                if before >= after {
                    c.duration_us = before;
                } else {
                    c.start_us = range.end_us;
                    c.duration_us = after;
                }
            }
        }
        for at in [range.start_us, range.end_us] {
            if let Some(ci) = self.tracks[ti].clips.iter().position(|c| c.start_us < at && at < c.end_us()) {
                self.split_clip(ti, ci, at);
            }
        }
        let length = range.end_us - range.start_us;
        let clips = &mut self.tracks[ti].clips;
        clips.retain(|c| !(c.start_us >= range.start_us && c.end_us() <= range.end_us) && c.duration_us >= min);
        for c in clips.iter_mut().filter(|c| c.start_us >= range.end_us) {
            c.start_us -= length;
        }
    }

    /// Source length a media clip may read, or `None` for images and text, which have no end.
    fn source_limit(&self, clip: &Clip) -> Option<i64> {
        match &clip.content {
            ClipContent::Media { asset_id, .. } => {
                self.asset(asset_id).filter(|a| a.kind != AssetKind::Image).map(|a| a.duration_us)
            }
            ClipContent::Text { .. } => None,
        }
    }

    fn caption_track(&mut self, track_id: &str) -> Result<usize> {
        self.track_index(track_id)
            .filter(|&i| self.tracks[i].is_captions())
            .ok_or_else(|| anyhow!("Unknown captions track"))
    }

    /// Revalidate fades and both sides of transitions after duration or adjacency changes.
    fn clamp_timing_windows(&mut self) {
        for track in &mut self.tracks {
            let mut previous_duration = None;
            for clip in &mut track.clips {
                if let ClipContent::Media { fade_in_us, fade_out_us, .. } = &mut clip.content {
                    let half = clip.duration_us.max(0) / 2;
                    *fade_in_us = (*fade_in_us).clamp(0, half);
                    *fade_out_us = (*fade_out_us).clamp(0, half);
                }
                if track.id == MAIN_TRACK
                    && let (Some(previous), Some(transition)) = (previous_duration, &mut clip.transition_in)
                {
                    transition.duration_us =
                        transition.duration_us.min(MAX_TRANSITION_US).min(previous).min(clip.duration_us);
                }
                previous_duration = Some(clip.duration_us);
            }
        }
    }

    fn tidy(&mut self) {
        self.tracks.retain(|t| t.id == MAIN_TRACK || !t.clips.is_empty());
        for t in &mut self.tracks {
            t.clips.sort_by_key(|c| c.start_us);
        }
    }

    /// Applies one edit. On error the project may be partly changed; `Editor::apply`
    /// restores it, so callers that need atomic edits go through the editor.
    pub fn apply(&mut self, cmd: EditCmd) -> Result<EditOutcome> {
        let mut out = EditOutcome::default();
        let mut moved: Option<(String, i64)> = None;
        match cmd {
            EditCmd::AddAssets { .. }
            | EditCmd::RemoveAsset { .. }
            | EditCmd::AddClip { .. }
            | EditCmd::AddText { .. }
            | EditCmd::DetachAudio { .. } => self.apply_media(cmd, &mut out, &mut moved)?,
            EditCmd::MoveClip { .. }
            | EditCmd::TrimClip { .. }
            | EditCmd::SplitClip { .. }
            | EditCmd::DeleteClips { .. }
            | EditCmd::DuplicateClip { .. } => self.apply_placement(cmd, &mut out, &mut moved)?,
            EditCmd::UpdateClip { .. }
            | EditCmd::SetAnimation { .. }
            | EditCmd::SetTransition { .. }
            | EditCmd::SetKeyframes { .. } => self.apply_clip(cmd)?,
            EditCmd::UpdateTrack { .. } | EditCmd::SetCanvas { .. } | EditCmd::RenameProject { .. } => {
                self.apply_tracks(cmd)?
            }
            EditCmd::AddCaptions { .. } | EditCmd::ReplaceCaptions { .. } | EditCmd::RippleDeleteRanges { .. } => {
                self.apply_captions(cmd, &mut out)?
            }
        }
        self.pack_main(moved.as_ref().map(|(id, s)| (id.as_str(), *s)));
        self.tidy();
        self.clamp_timing_windows();
        Ok(out)
    }

    fn apply_media(&mut self, cmd: EditCmd, out: &mut EditOutcome, moved: &mut Option<(String, i64)>) -> Result<()> {
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
                let keep = asset.kind == AssetKind::Audio;
                let duration =
                    if asset.kind == AssetKind::Image { IMAGE_DURATION_US } else { asset.duration_us.max(min) };
                let clip = Clip::new(
                    new_id(),
                    0,
                    duration,
                    ClipContent::Media {
                        asset_id,
                        source_in_us: 0,
                        volume: 1.0,
                        transform: Transform::default(),
                        speed: 1.0,
                        adjust: Adjust::default(),
                        fade_in_us: 0,
                        fade_out_us: 0,
                    },
                );
                out.select.push(clip.id.clone());
                let requested = track_id.and_then(|id| self.track_index(&id)).filter(|&i| self.tracks[i].kind == kind);
                let target = match (kind, requested) {
                    (_, Some(i)) => i,
                    (TrackKind::Video, None) => 0,
                    (_, None) => self.free_track(kind, start_us.unwrap_or(0), start_us.unwrap_or(0) + duration, keep),
                };
                let start = start_us.unwrap_or(if target == 0 { i64::MAX / 4 } else { 0 }).max(0);
                if target != 0 && !self.is_free(target, start, start + duration, None) {
                    let t = self.free_track(kind, start, start + duration, keep);
                    self.tracks[t].clips.push(Clip { start_us: start, ..clip });
                } else {
                    if target == 0 {
                        *moved = Some((clip.id.clone(), start));
                    }
                    self.tracks[target].clips.push(Clip { start_us: start, ..clip });
                }
            }
            EditCmd::AddText { start_us, text, style } => {
                let start = start_us.max(0);
                let t = self.free_track(TrackKind::Text, start, start + TEXT_DURATION_US, false);
                let clip = Clip::new(
                    new_id(),
                    start,
                    TEXT_DURATION_US,
                    ClipContent::Text {
                        text,
                        style,
                        // Centred like CapCut; captions sit lower, so a hook title and captions do not collide.
                        transform: Transform::default(),
                    },
                );
                out.select.push(clip.id.clone());
                self.tracks[t].clips.push(clip);
            }
            EditCmd::DetachAudio { clip_id } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let clip = self.tracks[ti].clips[ci].clone();
                let ClipContent::Media { asset_id, volume, .. } = &clip.content else {
                    bail!("Only video clips have sound to detach")
                };
                if self.tracks[ti].kind != TrackKind::Video {
                    bail!("This clip is already sound on its own track");
                }
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
                // Detached sound is speech: it is cut together with the picture.
                let target = self.free_track(TrackKind::Audio, clip.start_us, clip.end_us(), false);
                out.select.push(sound.id.clone());
                self.tracks[target].clips.push(sound);
            }
            _ => unreachable!("command dispatched to the wrong edit group"),
        }
        Ok(())
    }

    fn apply_placement(
        &mut self,
        cmd: EditCmd,
        out: &mut EditOutcome,
        moved: &mut Option<(String, i64)>,
    ) -> Result<()> {
        let min = min_duration(self);
        match cmd {
            EditCmd::MoveClip { clip_id, track_id, start_us } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                // Clips always sit on a track of their kind; detached sound is a video asset on an audio track.
                let kind = self.tracks[ti].kind;
                let keep = self.tracks[ti].keep_in_place;
                let mut clip = self.tracks[ti].clips.remove(ci);
                let start = start_us.max(0);
                clip.start_us = start;
                let end = start + clip.duration_us;
                let target = track_id.and_then(|id| self.track_index(&id)).filter(|&i| self.tracks[i].kind == kind);
                let target = match target {
                    Some(0) => {
                        *moved = Some((clip.id.clone(), start));
                        0
                    }
                    Some(i) if self.is_free(i, start, end, None) => i,
                    // Dropped onto an occupied spot or no track: a new track right above.
                    Some(i) => self.insert_track(i + 1, kind, keep),
                    None => {
                        let at = match kind {
                            TrackKind::Video => ti + 1,
                            _ => self.tracks.len(),
                        };
                        self.insert_track(at, kind, keep)
                    }
                };
                out.select.push(clip.id.clone());
                self.tracks[target].clips.push(clip);
            }
            EditCmd::TrimClip { clip_id, start_us, duration_us, source_in_us } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let limit = self.source_limit(&self.tracks[ti].clips[ci]);
                let speed = match &self.tracks[ti].clips[ci].content {
                    ClipContent::Media { speed, .. } => *speed as f64,
                    ClipContent::Text { .. } => 1.0,
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
                        // The shortest clip still covers `min * speed` of source.
                        let reserve = (min as f64 * speed).ceil() as i64;
                        new_src = new_src.min((limit - reserve).max(0));
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
                let clip = &self.tracks[ti].clips[ci];
                if at_us < clip.start_us + min || at_us > clip.end_us() - min {
                    bail!("Move the playhead inside the clip to split it");
                }
                let second = self.split_clip(ti, ci, at_us);
                out.select.push(self.tracks[ti].clips[second].id.clone());
            }
            EditCmd::DeleteClips { clip_ids } => {
                for t in &mut self.tracks {
                    t.clips.retain(|c| !clip_ids.contains(&c.id));
                }
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
                    let (kind, keep) = (self.tracks[ti].kind, self.tracks[ti].keep_in_place);
                    self.free_track(kind, copy.start_us, copy.end_us(), keep)
                };
                if target == 0 {
                    *moved = Some((copy.id.clone(), copy.start_us));
                }
                self.tracks[target].clips.push(copy);
            }
            _ => unreachable!("command dispatched to the wrong edit group"),
        }
        Ok(())
    }

    fn apply_clip(&mut self, cmd: EditCmd) -> Result<()> {
        let min = min_duration(self);
        match cmd {
            EditCmd::UpdateClip { clip_id, transform, volume, text, style, speed, adjust, fade_in_us, fade_out_us } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let changes_length = speed.is_some();
                let limit = self.source_limit(&self.tracks[ti].clips[ci]);
                let clip = &mut self.tracks[ti].clips[ci];
                let half = clip.duration_us / 2;
                match &mut clip.content {
                    ClipContent::Media {
                        source_in_us: src,
                        transform: tr,
                        volume: v,
                        speed: sp,
                        adjust: adj,
                        fade_in_us: fi,
                        fade_out_us: fo,
                        ..
                    } => {
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
                            // A one-frame clip covers more source when sped up; move its start back
                            // rather than read past the end of the file.
                            if let Some(limit) = limit {
                                let span = (clip.duration_us as f64 * x as f64).ceil() as i64;
                                if span > limit {
                                    bail!("This clip is too short for that speed");
                                }
                                *src = (*src).min(limit - span);
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
                let clip = &self.tracks[ti].clips[ci];
                if changes_length && ti != 0 && !self.is_free(ti, clip.start_us, clip.end_us(), Some(&clip_id)) {
                    bail!("There is no room on this track for the clip at the new speed");
                }
            }
            EditCmd::SetAnimation { clip_id, slot, animation } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let clip = &mut self.tracks[ti].clips[ci];
                let animation =
                    animation.map(|a| Animation { duration_us: a.duration_us.clamp(min, clip.duration_us), ..a });
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
                self.tracks[ti].clips[ci].transition_in = transition.map(|t| Transition {
                    duration_us: t.duration_us.clamp(min, MAX_TRANSITION_US.min(shortest).max(min)),
                    ..t
                });
            }
            EditCmd::SetKeyframes { clip_id, mut keyframes } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                keyframes.sort_by_key(|k| k.t_us);
                keyframes.dedup_by_key(|k| k.t_us);
                self.tracks[ti].clips[ci].keyframes = keyframes;
            }
            _ => unreachable!("command dispatched to the wrong edit group"),
        }
        Ok(())
    }

    fn apply_tracks(&mut self, cmd: EditCmd) -> Result<()> {
        match cmd {
            EditCmd::UpdateTrack { track_id, muted, hidden, keep_in_place } => {
                let i = self.track_index(&track_id).ok_or_else(|| anyhow!("Unknown track"))?;
                let t = &mut self.tracks[i];
                if let Some(m) = muted {
                    t.muted = m;
                }
                if let Some(h) = hidden {
                    t.hidden = h;
                }
                if let Some(k) = keep_in_place {
                    t.keep_in_place = k;
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
            EditCmd::RenameProject { name } => {
                let name = name.trim();
                if !name.is_empty() {
                    self.name = name.to_string();
                }
            }
            _ => unreachable!("command dispatched to the wrong edit group"),
        }
        Ok(())
    }

    fn apply_captions(&mut self, cmd: EditCmd, out: &mut EditOutcome) -> Result<()> {
        let min = min_duration(self);
        match cmd {
            EditCmd::AddCaptions { segments, style } => {
                let clips = caption_clips(segments, &style, &self.canvas, min);
                out.select = clips.iter().map(|c| c.id.clone()).take(1).collect();
                self.tracks.push(Track {
                    id: new_id(),
                    kind: TrackKind::Text,
                    name: CAPTIONS_TRACK.into(),
                    muted: false,
                    hidden: false,
                    keep_in_place: false,
                    clips,
                });
            }
            EditCmd::ReplaceCaptions { track_id, segments, style } => {
                let ti = self.caption_track(&track_id)?;
                let clips = caption_clips(segments, &style, &self.canvas, min);
                out.select = clips.iter().map(|c| c.id.clone()).take(1).collect();
                self.tracks[ti].clips = clips;
            }
            EditCmd::RippleDeleteRanges { ranges, keep_track_ids } => {
                let kept: Vec<bool> = match &keep_track_ids {
                    Some(ids) => self.tracks.iter().map(|t| ids.contains(&t.id)).collect(),
                    None => self.tracks.iter().map(|t| t.keep_in_place).collect(),
                };
                let mut ranges = merge_ranges(ranges);
                if let Some(main) = self.track_index(MAIN_TRACK).filter(|&ti| !kept[ti]) {
                    let mut slivers = Vec::new();
                    for clip in &self.tracks[main].clips {
                        let mut start = clip.start_us;
                        for range in &ranges {
                            let end = range.start_us.min(clip.end_us());
                            if start < end && end - start < min {
                                slivers.push(TimeRange { start_us: start, end_us: end });
                            }
                            start = start.max(range.end_us);
                            if start >= clip.end_us() {
                                break;
                            }
                        }
                        if start < clip.end_us() && clip.end_us() - start < min {
                            slivers.push(TimeRange { start_us: start, end_us: clip.end_us() });
                        }
                    }
                    // Packing also removes these fragments; every moving track must lose that time.
                    ranges.extend(slivers);
                    ranges = merge_ranges(ranges);
                }
                // Later ranges first, so earlier coordinates stay valid while cutting.
                for range in ranges.into_iter().rev() {
                    for ti in (0..self.tracks.len()).filter(|&ti| !kept[ti]) {
                        self.ripple_delete_track(ti, range, min);
                    }
                }
            }
            _ => unreachable!("command dispatched to the wrong edit group"),
        }
        Ok(())
    }
}

/// Clip ids added and removed between two versions, in timeline order.
fn clip_changes(before: &Project, after: &Project) -> (Vec<String>, Vec<String>) {
    let ids = |p: &Project| p.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.id.clone())).collect::<Vec<_>>();
    let (old, new) = (ids(before), ids(after));
    let created = new.iter().filter(|id| !old.contains(id)).cloned().collect();
    let removed = old.iter().filter(|id| !new.contains(id)).cloned().collect();
    (created, removed)
}

/// Sorted, non-empty, non-overlapping ranges.
pub fn merge_ranges(mut ranges: Vec<TimeRange>) -> Vec<TimeRange> {
    ranges.retain(|r| r.end_us > r.start_us.max(0));
    ranges.sort_by_key(|r| r.start_us);
    let mut out: Vec<TimeRange> = Vec::with_capacity(ranges.len());
    for r in ranges {
        let r = TimeRange { start_us: r.start_us.max(0), ..r };
        match out.last_mut() {
            Some(last) if r.start_us <= last.end_us => last.end_us = last.end_us.max(r.end_us),
            _ => out.push(r),
        }
    }
    out
}

/// One text clip per segment, sorted and never overlapping: a segment starting inside the
/// previous one ends it there, and one starting within a frame of it is merged into it.
/// On vertical videos captions wrap inside the Reels and TikTok safe area unless the style says otherwise.
fn caption_clips(mut segments: Vec<CaptionSegment>, style: &TextStyle, canvas: &Canvas, min: i64) -> Vec<Clip> {
    let mut style = style.clone();
    if style.max_width.is_none() {
        style.max_width = canvas.safe_area().map(|area| area.centered_width(canvas.width as f32));
    }
    segments.retain(|s| s.end_us > s.start_us && !s.text.trim().is_empty());
    segments.sort_by_key(|s| s.start_us);
    let mut merged: Vec<CaptionSegment> = Vec::with_capacity(segments.len());
    for s in segments {
        let text = s.text.trim().to_string();
        let start_us = s.start_us.max(0);
        match merged.last_mut() {
            Some(prev) if start_us < prev.start_us + min => {
                prev.text = format!("{} {text}", prev.text);
                prev.end_us = prev.end_us.max(s.end_us);
            }
            Some(prev) => {
                prev.end_us = prev.end_us.min(start_us);
                merged.push(CaptionSegment { start_us, end_us: s.end_us, text });
            }
            None => merged.push(CaptionSegment { start_us, end_us: s.end_us, text }),
        }
    }
    merged
        .into_iter()
        .map(|s| {
            // Where CapCut puts auto captions on a reel: a bit below the middle.
            let transform = Transform { y: CAPTION_Y, ..Transform::default() };
            let content = ClipContent::Text { text: s.text, style: style.clone(), transform };
            Clip::new(new_id(), s.start_us, (s.end_us - s.start_us).max(min), content)
        })
        .collect()
}

/// One undo or redo entry: the project to return to and the coalesce key that made it.
struct Step {
    project: Project,
    key: Option<String>,
}

/// Owns the current project and its undo history.
pub struct Editor {
    pub project: Project,
    undo: Vec<Step>,
    redo: Vec<Step>,
    coalesce: Option<String>,
    pub revision: u64,
}

impl Editor {
    pub fn new(project: Project) -> Self {
        Self { project, undo: Vec::new(), redo: Vec::new(), coalesce: None, revision: 0 }
    }

    /// Applies `cmd`. Consecutive edits with the same `coalesce` key form one undo step, so
    /// callers scope keys to a gesture (one slider drag, one typing burst) or to an AI run.
    pub fn apply(&mut self, cmd: EditCmd, coalesce: Option<String>) -> Result<EditOutcome> {
        self.apply_batch(vec![cmd], coalesce)
    }

    /// Applies every command or none of them, as one undo step. The outcome selects
    /// everything the commands selected, e.g. every second half of a multi-clip split.
    pub fn apply_batch(&mut self, cmds: Vec<EditCmd>, coalesce: Option<String>) -> Result<EditOutcome> {
        self.apply_batch_checked(cmds, coalesce, |_| Ok(()))
    }

    /// Validates the result before committing history. Rejection or unwinding restores the
    /// project while preserving the current coalesce key and both history stacks.
    pub fn apply_batch_checked(
        &mut self,
        cmds: Vec<EditCmd>,
        coalesce: Option<String>,
        check: impl FnOnce(&Project) -> Result<()>,
    ) -> Result<EditOutcome> {
        let before = self.project.clone();
        let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut outcome = EditOutcome::default();
            for cmd in cmds {
                outcome.select.extend(self.project.apply(cmd)?.select);
            }
            check(&self.project)?;
            Ok(outcome)
        }));
        let mut outcome = match applied {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => {
                self.project = before;
                return Err(error);
            }
            Err(panic) => {
                self.project = before;
                std::panic::resume_unwind(panic);
            }
        };
        (outcome.created, outcome.removed) = clip_changes(&before, &self.project);
        self.commit_project(before, coalesce);
        Ok(outcome)
    }

    /// Replaces a validated project as one undo step, using the same grouping as batches.
    pub fn replace_project_checked(
        &mut self,
        project: Project,
        coalesce: Option<String>,
        check: impl FnOnce(&Project) -> Result<()>,
    ) -> Result<()> {
        check(&project)?;
        let before = std::mem::replace(&mut self.project, project);
        self.commit_project(before, coalesce);
        Ok(())
    }

    fn commit_project(&mut self, before: Project, coalesce: Option<String>) {
        if self.project == before {
            return;
        }
        let merge = coalesce.is_some() && coalesce == self.coalesce;
        if !merge {
            self.undo.push(Step { project: before, key: coalesce.clone() });
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.coalesce = coalesce;
        self.redo.clear();
        self.revision += 1;
    }

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else { return false };
        let current = std::mem::replace(&mut self.project, step.project);
        self.redo.push(Step { project: current, key: step.key });
        self.coalesce = None;
        self.revision += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        let current = std::mem::replace(&mut self.project, step.project);
        self.undo.push(Step { project: current, key: step.key });
        self.coalesce = None;
        self.revision += 1;
        true
    }

    /// Ends the current undo step, so the next edit starts a new one even with the same key.
    pub fn seal(&mut self) {
        self.coalesce = None;
    }

    /// The coalesce key of the newest undo step.
    pub fn last_key(&self) -> Option<&str> {
        self.undo.last().and_then(|step| step.key.as_deref())
    }

    /// Reverts the newest undo step if `key` made it, leaving nothing to redo: discarding an
    /// AI run must not offer it back, and redo steps were made on top of it. False when the
    /// newest step belongs to something else.
    pub fn drop_last(&mut self, key: &str) -> bool {
        if self.last_key() != Some(key) {
            return false;
        }
        if let Some(step) = self.undo.pop() {
            self.project = step.project;
            self.redo.clear();
            self.coalesce = None;
            self.revision += 1;
        }
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
            assets: vec![
                asset("a", AssetKind::Video, 5),
                asset("b", AssetKind::Video, 3),
                asset("m", AssetKind::Audio, 20),
            ],
        })
        .unwrap();
        p
    }

    fn main_layout(p: &Project) -> Vec<(i64, i64)> {
        p.tracks[0].clips.iter().map(|c| (c.start_us / 1000, c.duration_us / 1000)).collect()
    }

    #[test]
    fn checked_batch_rejection_preserves_project_history_and_coalescing() {
        let mut editor = Editor::new(Project::new("original"));
        let rename = |name: &str| EditCmd::RenameProject { name: name.into() };
        editor.apply(rename("first"), Some("gesture".into())).unwrap();
        let before = editor.project.clone();
        assert!(editor.apply_batch_checked(vec![rename("rejected")], None, |_| bail!("invalid")).is_err());
        assert_eq!(editor.project, before);
        assert_eq!(editor.revision, 1);
        editor.apply(rename("second"), Some("gesture".into())).unwrap();
        editor.undo();
        assert_eq!(editor.project.name, "original");
        assert!(!editor.can_undo());
        assert!(editor.apply_batch_checked(vec![rename("rejected")], None, |_| bail!("invalid")).is_err());
        assert!(editor.can_redo());
        editor.redo();
        assert_eq!(editor.project.name, "second");
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            editor.apply_batch_checked(vec![rename("panic")], None, |_| panic!("validator"))
        }));
        assert!(panic.is_err());
        assert_eq!(editor.project.name, "second");
        assert_eq!(editor.revision, 4);
    }

    #[test]
    fn checked_replacement_shares_batch_history_and_coalescing() {
        let original = Project::new("original");
        let mut editor = Editor::new(original.clone());
        editor.apply(EditCmd::RenameProject { name: "first".into() }, Some("gesture".into())).unwrap();
        let replacement = project();
        editor.replace_project_checked(replacement.clone(), Some("gesture".into()), |_| Ok(())).unwrap();
        assert_eq!(editor.project, replacement);
        assert_eq!(editor.revision, 2);
        assert_eq!(editor.last_key(), Some("gesture"));
        assert!(editor.undo());
        assert_eq!(editor.project, original);
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.project, replacement);
        assert_eq!(editor.revision, 4);
        editor.seal();
        let next = Project::new("next");
        editor.replace_project_checked(next.clone(), Some("gesture".into()), |_| Ok(())).unwrap();
        assert!(editor.undo());
        assert_eq!(editor.project, replacement);
        editor.replace_project_checked(next.clone(), None, |_| Ok(())).unwrap();
        assert!(!editor.can_redo());
        assert_eq!(editor.revision, 7);
        assert!(editor.undo());
        assert_eq!(editor.project, replacement);
        assert!(editor.redo());
        assert_eq!(editor.project, next);
    }

    #[test]
    fn checked_replacement_rejection_and_noop_preserve_history() {
        let original = Project::new("original");
        let mut editor = Editor::new(original.clone());
        editor.replace_project_checked(original.clone(), None, |_| Ok(())).unwrap();
        assert_eq!(editor.revision, 0);
        assert!(!editor.can_undo());
        editor.apply(EditCmd::RenameProject { name: "first".into() }, Some("gesture".into())).unwrap();
        let before = editor.project.clone();
        assert!(editor.replace_project_checked(project(), None, |_| bail!("invalid")).is_err());
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            editor.replace_project_checked(project(), None, |_| panic!("validator"))
        }));
        assert!(panic.is_err());
        assert_eq!(editor.project, before);
        assert_eq!(editor.revision, 1);
        editor.replace_project_checked(before, None, |_| Ok(())).unwrap();
        let replacement = project();
        editor.replace_project_checked(replacement.clone(), Some("gesture".into()), |_| Ok(())).unwrap();
        assert!(editor.undo());
        assert_eq!(editor.project, original);
        assert!(!editor.can_undo());
        assert!(editor.replace_project_checked(project(), None, |_| bail!("invalid")).is_err());
        editor.replace_project_checked(original, None, |_| Ok(())).unwrap();
        assert_eq!(editor.revision, 3);
        assert!(!editor.can_undo());
        assert!(editor.can_redo());
        assert!(editor.redo());
        assert_eq!(editor.project, replacement);
    }

    #[test]
    fn clips_append_to_main_track_back_to_back() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        assert_eq!(main_layout(&p), vec![(0, 5000), (5000, 3000)]);
    }

    #[test]
    fn the_video_ends_with_its_last_picture_and_music_past_it_is_cut() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(0), track_id: None }).unwrap();
        // Sound alone sets the length.
        assert_eq!(p.duration_us(), 20_000_000);
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        assert_eq!(p.duration_us(), 5_000_000);
    }

    #[test]
    fn music_stays_in_place_through_ripple_cuts_and_detached_sound_does_not() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(0), track_id: None }).unwrap();
        let video = p.tracks[0].clips[0].id.clone();
        p.apply(EditCmd::DetachAudio { clip_id: video }).unwrap();
        let music = p.tracks.iter().find(|t| t.keep_in_place).map(|t| t.id.clone()).unwrap();
        let sound =
            p.tracks.iter().find(|t| t.kind == TrackKind::Audio && !t.keep_in_place).map(|t| t.id.clone()).unwrap();
        assert_eq!(
            p.tracks.iter().filter(|t| t.kind == TrackKind::Audio).count(),
            2,
            "detached sound gets its own track"
        );
        p.apply(EditCmd::RippleDeleteRanges {
            ranges: vec![TimeRange { start_us: 0, end_us: 1_000_000 }],
            keep_track_ids: None,
        })
        .unwrap();
        let end = |p: &Project, id: &str| p.tracks.iter().find(|t| t.id == id).unwrap().clips[0].end_us();
        assert_eq!((end(&p, &music), end(&p, &sound)), (20_000_000, 4_000_000));
        p.apply(EditCmd::UpdateTrack {
            track_id: music.clone(),
            muted: None,
            hidden: None,
            keep_in_place: Some(false),
        })
        .unwrap();
        p.apply(EditCmd::RippleDeleteRanges {
            ranges: vec![TimeRange { start_us: 0, end_us: 1_000_000 }],
            keep_track_ids: None,
        })
        .unwrap();
        assert_eq!(end(&p, &music), 19_000_000);
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
        p.apply(EditCmd::TrimClip { clip_id: id.clone(), start_us: 0, duration_us: 60_000_000, source_in_us: None })
            .unwrap();
        assert_eq!(p.tracks[0].clips[0].duration_us, 3_000_000);
        // Left edge trim by one second keeps the right edge, then the main track packs.
        p.apply(EditCmd::TrimClip {
            clip_id: id,
            start_us: 1_000_000,
            duration_us: 2_000_000,
            source_in_us: Some(1_000_000),
        })
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
        p.apply(EditCmd::TrimClip { clip_id: a.clone(), start_us: 0, duration_us: 2_000_000, source_in_us: None })
            .unwrap();
        p.apply(EditCmd::TrimClip { clip_id: a.clone(), start_us: 0, duration_us: 5_000_000, source_in_us: None })
            .unwrap();
        assert_eq!(p.tracks[0].clips[0].id, a);
        assert_eq!(main_layout(&p), vec![(0, 5000), (5000, 3000)]);
    }

    #[test]
    fn rejected_edit_leaves_the_project_untouched() {
        let mut e = Editor::new(project());
        e.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(0), track_id: None }, None).unwrap();
        let track = e.project.tracks[1].id.clone();
        let first = e.project.tracks[1].clips[0].id.clone();
        e.apply(
            EditCmd::TrimClip { clip_id: first.clone(), start_us: 0, duration_us: 5_000_000, source_in_us: None },
            None,
        )
        .unwrap();
        e.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(10_000_000), track_id: Some(track) }, None)
            .unwrap();
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
        let EditCmd::UpdateClip { clip_id, transform, volume, text, style, adjust, fade_in_us, fade_out_us, .. } =
            update(&id)
        else {
            unreachable!()
        };
        p.apply(EditCmd::UpdateClip {
            clip_id,
            transform,
            volume,
            text,
            style,
            speed: Some(2.0),
            adjust,
            fade_in_us,
            fade_out_us,
        })
        .unwrap();
        assert_eq!(main_layout(&p), vec![(0, 2500)]);
        p.apply(EditCmd::SplitClip { clip_id: id, at_us: 1_000_000 }).unwrap();
        let ClipContent::Media { source_in_us, .. } = &p.tracks[0].clips[1].content else { panic!() };
        assert_eq!(*source_in_us, 2_000_000);
    }

    #[test]
    fn slowing_down_an_overlay_clip_cannot_overlap_its_neighbour() {
        let mut e = Editor::new(project());
        e.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(0), track_id: None }, None).unwrap();
        let track = e.project.tracks[1].id.clone();
        let first = e.project.tracks[1].clips[0].id.clone();
        e.apply(
            EditCmd::TrimClip { clip_id: first.clone(), start_us: 0, duration_us: 5_000_000, source_in_us: None },
            None,
        )
        .unwrap();
        e.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(5_000_000), track_id: Some(track) }, None)
            .unwrap();
        let before = e.project.clone();
        let EditCmd::UpdateClip { clip_id, transform, volume, text, style, adjust, fade_in_us, fade_out_us, .. } =
            update(&first)
        else {
            unreachable!()
        };
        let slower = EditCmd::UpdateClip {
            clip_id,
            transform,
            volume,
            text,
            style,
            speed: Some(0.5),
            adjust,
            fade_in_us,
            fade_out_us,
        };
        assert!(e.apply(slower, None).is_err());
        assert_eq!(e.project, before);
    }

    #[test]
    fn trim_at_high_speed_stays_inside_the_source() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        let id = p.tracks[0].clips[0].id.clone();
        let EditCmd::UpdateClip { clip_id, transform, volume, text, style, adjust, fade_in_us, fade_out_us, .. } =
            update(&id)
        else {
            unreachable!()
        };
        p.apply(EditCmd::UpdateClip {
            clip_id,
            transform,
            volume,
            text,
            style,
            speed: Some(10.0),
            adjust,
            fade_in_us,
            fade_out_us,
        })
        .unwrap();
        p.apply(EditCmd::TrimClip { clip_id: id, start_us: 0, duration_us: 33_334, source_in_us: Some(4_966_666) })
            .unwrap();
        let c = &p.tracks[0].clips[0];
        let ClipContent::Media { source_in_us, speed, .. } = &c.content else { panic!() };
        let source_end = source_in_us + (c.duration_us as f64 * *speed as f64).round() as i64;
        assert!(source_end <= 5_000_000, "source ends at {source_end}");
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
        let sound = p.apply(EditCmd::DetachAudio { clip_id: a.clone() }).unwrap().select[0].clone();
        let ClipContent::Media { volume, .. } = &p.tracks[0].clips[0].content else { panic!() };
        assert_eq!(*volume, 0.0);
        assert_eq!(p.tracks[1].kind, TrackKind::Audio);
        assert!(p.apply(EditCmd::DetachAudio { clip_id: a }).is_err());
        // The detached sound is audio: it cannot be detached again and moves along audio tracks.
        assert!(p.apply(EditCmd::DetachAudio { clip_id: sound.clone() }).is_err());
        p.apply(EditCmd::MoveClip { clip_id: sound.clone(), track_id: None, start_us: 1_000_000 }).unwrap();
        let track = p.tracks.iter().find(|t| t.clips.iter().any(|c| c.id == sound)).unwrap();
        assert_eq!(track.kind, TrackKind::Audio);
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
    fn duration_edits_reclamp_both_audio_fades_on_every_track() {
        for asset_id in ["a", "m"] {
            for operation in ["trim", "speed", "split", "ripple"] {
                let mut p = project();
                let id = p
                    .apply(EditCmd::AddClip { asset_id: asset_id.into(), start_us: None, track_id: None })
                    .unwrap()
                    .select[0]
                    .clone();
                let (ti, ci) = p.find_clip(&id).unwrap();
                let duration = p.tracks[ti].clips[ci].duration_us;
                let fade: EditCmd = serde_json::from_value(serde_json::json!({
                    "type":"updateClip", "clipId":id, "fadeInUs":duration / 2, "fadeOutUs":duration / 2
                }))
                .unwrap();
                p.apply(fade).unwrap();
                let cmd = match operation {
                    "trim" => {
                        EditCmd::TrimClip { clip_id: id, start_us: 0, duration_us: 1_000_000, source_in_us: None }
                    }
                    "speed" => {
                        serde_json::from_value(serde_json::json!({"type":"updateClip", "clipId":id, "speed":10.0}))
                            .unwrap()
                    }
                    "split" => EditCmd::SplitClip { clip_id: id, at_us: 1_000_000 },
                    _ => EditCmd::RippleDeleteRanges {
                        ranges: vec![TimeRange { start_us: 1_000_000, end_us: duration - 1_000_000 }],
                        keep_track_ids: Some(vec![]),
                    },
                };
                p.apply(cmd).unwrap();
                for clip in p.tracks.iter().flat_map(|track| &track.clips) {
                    let ClipContent::Media { fade_in_us, fade_out_us, .. } = &clip.content else { panic!() };
                    assert_eq!(
                        (*fade_in_us, *fade_out_us),
                        (clip.duration_us / 2, clip.duration_us / 2),
                        "{asset_id}: {operation}"
                    );
                }
            }
        }
    }

    #[test]
    fn duration_edits_reclamp_both_adjacent_transitions() {
        for operation in ["trim", "speed", "split_left", "split_right", "ripple"] {
            let mut p = project();
            for _ in 0..3 {
                p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
            }
            for ci in [1, 2] {
                p.apply(EditCmd::SetTransition {
                    clip_id: p.tracks[0].clips[ci].id.clone(),
                    transition: Some(Transition {
                        kind: crate::model::TransitionKind::Dissolve,
                        duration_us: 2_000_000,
                    }),
                })
                .unwrap();
            }
            let clip_id = p.tracks[0].clips[1].id.clone();
            let cmd = match operation {
                "trim" => EditCmd::TrimClip { clip_id, start_us: 5_000_000, duration_us: 500_000, source_in_us: None },
                "speed" => EditCmd::UpdateClip {
                    clip_id,
                    speed: Some(10.0),
                    transform: None,
                    volume: None,
                    text: None,
                    style: None,
                    adjust: None,
                    fade_in_us: None,
                    fade_out_us: None,
                },
                "split_left" => EditCmd::SplitClip { clip_id, at_us: 5_500_000 },
                "split_right" => EditCmd::SplitClip { clip_id, at_us: 9_500_000 },
                _ => EditCmd::RippleDeleteRanges {
                    ranges: vec![TimeRange { start_us: 5_500_000, end_us: 9_500_000 }],
                    keep_track_ids: None,
                },
            };
            p.apply(cmd).unwrap();
            let clips = &p.tracks[0].clips;
            for pair in clips.windows(2) {
                if let Some(transition) = pair[1].transition_in {
                    assert!(transition.duration_us <= pair[0].duration_us.min(pair[1].duration_us), "{operation}");
                }
            }
            assert!(clips.iter().any(|c| c.transition_in.is_some_and(|t| t.duration_us == 500_000)), "{operation}");
        }
    }

    #[test]
    fn outcome_reports_created_and_removed_clips() {
        let mut e = Editor::new(project());
        let added = e.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }, None).unwrap();
        let id = e.project.tracks[0].clips[0].id.clone();
        assert_eq!((added.created, added.removed), (vec![id.clone()], vec![]));
        let split = e.apply(EditCmd::SplitClip { clip_id: id.clone(), at_us: 1_000_000 }, None).unwrap();
        assert_eq!(split.created, split.select);
        let gone = e.apply(EditCmd::DeleteClips { clip_ids: vec![id.clone()] }, None).unwrap();
        assert_eq!(gone.removed, vec![id]);
    }

    #[test]
    fn batches_are_atomic_and_one_undo_step() {
        let mut e = Editor::new(project());
        e.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }, None).unwrap();
        let id = e.project.tracks[0].clips[0].id.clone();
        let split = |at_us| EditCmd::SplitClip { clip_id: id.clone(), at_us };
        let before = e.project.clone();
        // The second split is outside the clip, so nothing may change.
        assert!(e.apply_batch(vec![split(1_000_000), split(60_000_000)], None).is_err());
        assert_eq!(e.project, before);
        e.apply_batch(vec![split(1_000_000), split(500_000)], None).unwrap();
        assert_eq!(e.project.tracks[0].clips.len(), 3);
        assert!(e.undo());
        assert_eq!(e.project, before);
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
    fn a_run_is_one_sealed_step_that_can_be_dropped_without_redo() {
        let mut e = Editor::new(project());
        e.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }, Some("user".into()))
            .unwrap();
        let after_user = e.project.clone();
        e.seal();
        let run = "run:1";
        for asset in ["b", "a"] {
            e.apply(EditCmd::AddClip { asset_id: asset.into(), start_us: None, track_id: None }, Some(run.into()))
                .unwrap();
        }
        e.seal();
        assert_eq!(e.last_key(), Some(run));
        // After sealing, the same key starts a new step instead of joining the run.
        e.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }, Some(run.into())).unwrap();
        assert!(e.undo());
        assert_eq!(e.project.tracks[0].clips.len(), 3);
        assert!(!e.drop_last("run:2"));
        assert!(e.drop_last(run));
        assert_eq!(e.project, after_user);
        assert!(!e.can_redo());
        assert_eq!(e.last_key(), Some("user"));
        // A whole run comes back with one redo.
        e.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }, Some(run.into())).unwrap();
        e.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }, Some(run.into())).unwrap();
        assert!(e.undo());
        assert_eq!(e.project, after_user);
        assert!(e.redo());
        assert_eq!(e.project.tracks[0].clips.len(), 3);
        assert_eq!(e.last_key(), Some(run));
    }

    #[test]
    fn captions_replace_previous_caption_track() {
        let mut p = project();
        let style = TextStyle {
            font_family: None,
            font_size: 70.0,
            color: "#fff".into(),
            bold: true,
            stroke_width: 5.0,
            stroke_color: "#000".into(),
            background: None,
            max_width: None,
        };
        let seg = |s, e, t: &str| CaptionSegment { start_us: s, end_us: e, text: t.into() };
        p.apply(EditCmd::AddCaptions {
            segments: vec![seg(0, 1_200_000, "Ahoj"), seg(1_000_000, 2_000_000, "světe")],
            style: style.clone(),
        })
        .unwrap();
        let track = p.tracks.iter().find(|t| t.name == "Captions").unwrap().id.clone();
        p.apply(EditCmd::ReplaceCaptions {
            track_id: track,
            segments: vec![seg(0, 1_000_000, "Znovu")],
            style: style.clone(),
        })
        .unwrap();
        let caption_tracks: Vec<_> = p.tracks.iter().filter(|t| t.name == "Captions").collect();
        assert_eq!(caption_tracks.len(), 1);
        assert_eq!(caption_tracks[0].clips.len(), 1);
        // Adding never removes an existing captions track.
        p.apply(EditCmd::AddCaptions { segments: vec![seg(0, 1_000_000, "Druhá")], style: style.clone() }).unwrap();
        assert_eq!(p.tracks.iter().filter(|t| t.name == "Captions").count(), 2);
        // A title track is not a captions track, so its text is never replaced.
        p.apply(EditCmd::AddText { start_us: 0, text: "Title".into(), style: style.clone() }).unwrap();
        let titles = p.tracks.iter().find(|t| t.kind == TrackKind::Text && !t.is_captions()).unwrap().id.clone();
        let replace =
            EditCmd::ReplaceCaptions { track_id: titles.clone(), segments: vec![seg(0, 1_000_000, "X")], style };
        assert!(p.apply(replace).is_err());
        let title = &p.tracks.iter().find(|t| t.id == titles).unwrap().clips[0].content;
        assert!(matches!(title, ClipContent::Text { text, .. } if text == "Title"));
    }

    #[test]
    fn captions_with_equal_starts_merge_instead_of_overlapping() {
        let style = TextStyle {
            font_family: None,
            font_size: 40.0,
            color: "#fff".into(),
            bold: false,
            stroke_width: 0.0,
            stroke_color: "#000".into(),
            background: None,
            max_width: None,
        };
        let seg = |s, e, t: &str| CaptionSegment { start_us: s, end_us: e, text: t.into() };
        let clips = caption_clips(
            vec![seg(0, 1_000_000, "first"), seg(0, 2_000_000, "second"), seg(1_500_000, 3_000_000, "third")],
            &style,
            &Project::new("c").canvas,
            33_334,
        );
        let spans: Vec<_> = clips.iter().map(|c| (c.start_us, c.end_us())).collect();
        assert_eq!(spans, vec![(0, 1_500_000), (1_500_000, 3_000_000)]);
        let ClipContent::Text { text, .. } = &clips[0].content else { panic!() };
        assert_eq!(text, "first second");
    }

    #[test]
    fn ripple_delete_keeps_all_tracks_in_sync() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(0), track_id: None }).unwrap();
        let style = TextStyle {
            font_family: None,
            font_size: 40.0,
            color: "#fff".into(),
            bold: false,
            stroke_width: 0.0,
            stroke_color: "#000".into(),
            background: None,
            max_width: None,
        };
        p.apply(EditCmd::AddText { start_us: 2_000_000, text: "hi".into(), style }).unwrap();
        let music = p.tracks.iter().find(|t| t.kind == TrackKind::Audio).unwrap().id.clone();
        let ranges = vec![
            TimeRange { start_us: 3_000_000, end_us: 3_500_000 },
            TimeRange { start_us: 1_000_000, end_us: 2_000_000 },
            TimeRange { start_us: 1_500_000, end_us: 2_000_000 },
        ];
        p.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: Some(vec![music.clone()]) }).unwrap();
        // Main: 5 s minus 1.5 s, cut into three pieces with continuous source.
        assert_eq!(main_layout(&p), vec![(0, 1000), (1000, 1000), (2000, 1500)]);
        let sources: Vec<i64> = p.tracks[0]
            .clips
            .iter()
            .map(|c| match &c.content {
                ClipContent::Media { source_in_us, .. } => *source_in_us / 1000,
                _ => -1,
            })
            .collect();
        assert_eq!(sources, vec![0, 2000, 3500]);
        // The text clip [2 s, 5 s) loses [3, 3.5); its longer later part [3.5, 5) stays (one
        // copy of the text) and moves left by both cuts, staying over the same video frames.
        let text = p.tracks.iter().find(|t| t.kind == TrackKind::Text).unwrap();
        let spans: Vec<_> = text.clips.iter().map(|c| (c.start_us / 1000, c.end_us() / 1000)).collect();
        assert_eq!(spans, vec![(2000, 3500)]);
        // Kept music is untouched.
        let music = p.tracks.iter().find(|t| t.id == music).unwrap();
        assert_eq!((music.clips[0].start_us, music.clips[0].duration_us), (0, 20_000_000));
    }

    #[test]
    fn ripple_delete_inside_a_caption_keeps_one_copy_of_its_text() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        let style = TextStyle {
            font_family: None,
            font_size: 40.0,
            color: "#fff".into(),
            bold: false,
            stroke_width: 0.0,
            stroke_color: "#000".into(),
            background: None,
            max_width: None,
        };
        let seg = CaptionSegment { start_us: 1_000_000, end_us: 3_000_000, text: "jsem se".into() };
        p.apply(EditCmd::AddCaptions { segments: vec![seg], style }).unwrap();
        // Cutting [1.5, 2.2) leaves 0.5 s before and 0.8 s after: the later part stays.
        p.apply(EditCmd::RippleDeleteRanges {
            ranges: vec![TimeRange { start_us: 1_500_000, end_us: 2_200_000 }],
            keep_track_ids: Some(vec![]),
        })
        .unwrap();
        let captions = p.tracks.iter().find(|t| t.name == "Captions").unwrap();
        let spans: Vec<_> = captions.clips.iter().map(|c| (c.start_us, c.end_us())).collect();
        assert_eq!(spans, vec![(1_500_000, 2_300_000)]);
    }

    #[test]
    fn ripple_slivers_shift_captions_and_overlay_with_main() {
        for ranges in [
            vec![TimeRange { start_us: 10_000, end_us: 1_000_000 }],
            vec![TimeRange { start_us: 0, end_us: 500_000 }, TimeRange { start_us: 510_000, end_us: 1_000_000 }],
        ] {
            let mut p = project();
            p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
            let style = TextStyle {
                font_family: None,
                font_size: 40.0,
                color: "#fff".into(),
                bold: false,
                stroke_width: 0.0,
                stroke_color: "#000".into(),
                background: None,
                max_width: None,
            };
            p.apply(EditCmd::AddCaptions {
                segments: vec![CaptionSegment { start_us: 2_000_000, end_us: 3_000_000, text: "aligned".into() }],
                style,
            })
            .unwrap();
            let mut overlay = p.tracks[0].clone();
            overlay.id = "overlay".into();
            overlay.clips[0].id = "overlay-clip".into();
            overlay.clips[0].start_us = 2_000_000;
            overlay.clips[0].duration_us = 1_000_000;
            p.tracks.push(overlay);
            p.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: Some(vec![]) }).unwrap();
            assert_eq!(main_layout(&p), vec![(0, 4000)]);
            let ClipContent::Media { source_in_us, .. } = p.tracks[0].clips[0].content else { panic!() };
            assert_eq!(source_in_us, 1_000_000);
            for track in &p.tracks[1..] {
                assert_eq!(track.clips[0].start_us, 1_000_000, "{}", track.id);
            }
        }
    }

    #[test]
    fn ripple_delete_drops_slivers_shorter_than_a_frame() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        // Leaves 10 ms of the clip before the cut, less than one 30 fps frame.
        p.apply(EditCmd::RippleDeleteRanges {
            ranges: vec![TimeRange { start_us: 10_000, end_us: 1_000_000 }],
            keep_track_ids: Some(vec![]),
        })
        .unwrap();
        assert_eq!(main_layout(&p), vec![(0, 4000)]);
    }
}
