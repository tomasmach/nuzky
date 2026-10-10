//! Timeline edits and undo history. The main track is magnetic like in CapCut:
//! its clips always sit back to back from zero, so deleting or moving closes gaps.

use anyhow::{Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::model::{
    Adjust, Animation, Asset, AssetKind, Background, CAPTIONS_TRACK, Canvas, CaptionWord, Clip, ClipContent, Ease,
    Keyframe, Project, Shape, TextStyle, Thumbnail, ThumbnailFormat, Track, TrackKind, Transform, Transition,
    WordCorrection,
};

pub const MAIN_TRACK: &str = "main";
const IMAGE_DURATION_US: i64 = 3_000_000;
const TEXT_DURATION_US: i64 = 3_000_000;
const UNDO_LIMIT: usize = 200;
pub const MIN_SPEED: f32 = 0.1;
pub const MAX_SPEED: f32 = 10.0;
pub const MAX_TRANSITION_US: i64 = 2_000_000;
pub const MAX_DUCK_DB: f32 = 40.0;
pub const CAPTION_Y: f32 = 0.15;

/// A caption style: its look, a font when it brings its own, the entry and exit animation of every caption
/// and how it marks key words. The built-in ones are in `assets/presets/captions.json`; the user's own live
/// beside the projects.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaptionPreset {
    pub name: String,
    /// Without `fontFamily` the captions keep the font they have.
    pub style: TextStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub anim_in: Option<Animation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub anim_out: Option<Animation>,
}

pub fn caption_presets() -> &'static [CaptionPreset] {
    static PRESETS: std::sync::LazyLock<Vec<CaptionPreset>> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../../assets/presets/captions.json")).expect("valid caption presets")
    });
    &PRESETS
}

/// The caption preset named `name`, ignoring case, with `_` or `-` for spaces: "green_box" is Green box.
pub fn caption_preset(name: &str) -> Option<&'static CaptionPreset> {
    let name = name.trim().replace(['_', '-'], " ");
    caption_presets().iter().find(|preset| preset.name.eq_ignore_ascii_case(&name))
}

/// Editing limits the engine enforces, sent to the UI at start so it never copies them.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub min_speed: f32,
    pub max_speed: f32,
    pub max_transition_us: i64,
    /// Vertical offset of generated captions from the canvas centre, as a fraction of its height.
    pub caption_y: f32,
    pub max_duck_db: f32,
    pub min_motion_strength: f64,
    pub max_motion_strength: f64,
}

pub const LIMITS: Limits = Limits {
    min_speed: MIN_SPEED,
    max_speed: MAX_SPEED,
    max_transition_us: MAX_TRANSITION_US,
    caption_y: CAPTION_Y,
    max_duck_db: MAX_DUCK_DB,
    min_motion_strength: *MOTION_STRENGTHS.start(),
    max_motion_strength: *MOTION_STRENGTHS.end(),
};

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptionSegment {
    pub start_us: i64,
    pub end_us: i64,
    pub text: String,
    /// The spoken words, with TIMELINE times, for karaoke styles; their texts joined by single spaces
    /// must be `text`. The clip stores them relative to its start.
    #[serde(default)]
    pub words: Vec<CaptionWord>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
    /// Puts a video or image in a small window with rounded corners and a soft shadow in the top right
    /// corner, inside the Reels and TikTok safe area on vertical videos, on an overlay track above the
    /// main track. `duration_us` defaults to the whole video, or 3 s of an image.
    #[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
    AddPictureInPicture {
        asset_id: String,
        start_us: i64,
        duration_us: Option<i64>,
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
    #[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
    UpdateClip {
        clip_id: String,
        transform: Option<Transform>,
        volume: Option<f32>,
        text: Option<String>,
        style: Option<TextStyle>,
        /// Changing speed keeps the source range, so the clip gets shorter or longer.
        speed: Option<f32>,
        /// Sound at another speed keeps its pitch. Moving a clip off 1x turns it on unless the
        /// same command sets it.
        keep_pitch: Option<bool>,
        adjust: Option<Adjust>,
        fade_in_us: Option<i64>,
        fade_out_us: Option<i64>,
        /// Clean voice on clips with sound: less rumble, hum, background noise and harsh s sounds.
        clean_voice: Option<bool>,
        /// Video and image clips: corners, border and shadow. The default shape removes them.
        shape: Option<Shape>,
        /// Ducking: how many dB the clip goes down while a video's own sound has speech, up to 40;
        /// 0 turns it off. 12 suits music under speech.
        duck_db: Option<f32>,
        /// Video and image clips: blur what is behind the person, or put a colour or an image of the
        /// project there; `{"type": "none"}` shows the picture as recorded. The person's outline is made
        /// once per file in the background.
        background: Option<Box<Background>>,
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
    #[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
    UpdateTrack {
        track_id: String,
        muted: Option<bool>,
        hidden: Option<bool>,
        keep_in_place: Option<bool>,
    },
    #[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
    SetCanvas {
        width: u32,
        height: u32,
        background: Option<String>,
        background_blur: Option<f32>,
    },
    /// Adds a new captions track; existing tracks are left alone. Every caption gets the animations.
    #[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
    AddCaptions {
        segments: Vec<CaptionSegment>,
        style: TextStyle,
        #[serde(default)]
        anim_in: Option<Animation>,
        #[serde(default)]
        anim_out: Option<Animation>,
    },
    /// Replaces the clips of an existing captions track.
    #[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
    ReplaceCaptions {
        track_id: String,
        segments: Vec<CaptionSegment>,
        style: TextStyle,
        #[serde(default)]
        anim_in: Option<Animation>,
        #[serde(default)]
        anim_out: Option<Animation>,
    },
    /// Cuts the timeline ranges out of every track except `keep_track_ids` (by default the
    /// tracks kept in place) and closes the gaps, so video, overlays, audio and captions stay in sync.
    #[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
    RippleDeleteRanges {
        ranges: Vec<TimeRange>,
        #[serde(default)]
        keep_track_ids: Option<Vec<String>>,
    },
    RenameProject {
        name: String,
    },
    /// Sets how recognised words read, each found by its media file, its start in the file and
    /// its recognised text. Setting a word back to its recognised text removes the correction.
    CorrectWords {
        corrections: Vec<WordCorrection>,
    },
    /// Punch-ins on the main track: the picture of each range is scaled by its `scale`, which
    /// multiplies the clip's own scale and keeps its position and rotation. Clips are split at the
    /// range edges. An edge that would leave a piece shorter than 0.3 s, or shorter than a
    /// transition, animation or fade the piece carries, moves to the clip's edge. Clips with
    /// keyframes are left alone and listed in the outcome's `skipped`. Ranges must not overlap.
    ZoomRanges {
        ranges: Vec<ZoomRange>,
    },
    /// A slow camera move on video and image clips, written as two keyframes on a smooth curve
    /// from the clip's own transform, so they stay editable. The zoom centres on the middle of the
    /// Reels and TikTok safe area on vertical canvases, else on the canvas centre, so what is
    /// framed there stays put. `clip_id` moves one clip on any track, over the part of it inside
    /// `range` or its whole length; without it every main-track video and image clip under
    /// `range` moves, as one continuous motion. Clips with keyframes are left alone and listed in
    /// the outcome's `skipped`.
    #[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
    ApplyMotion {
        clip_id: Option<String>,
        range: Option<TimeRange>,
        kind: MotionKind,
        /// How far it zooms, as a fraction: 0.06 is subtle, 0.15 strong.
        strength: f64,
    },
    /// Sets the thumbnail of its format, replacing the one there.
    SetThumbnail {
        thumbnail: Thumbnail,
    },
    RemoveThumbnail {
        format: ThumbnailFormat,
    },
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MotionKind {
    /// Zooms in.
    PushIn,
    /// Starts zoomed in and ends on the clip's own framing.
    PullOut,
    /// Zooms in while drifting to the right.
    KenBurns,
}

/// Motion strengths an `ApplyMotion` may have.
pub const MOTION_STRENGTHS: std::ops::RangeInclusive<f64> = 0.02..=0.5;

/// The canvas point a motion zooms about, as the transform's fractions of the canvas from its
/// centre: the middle of the safe area on vertical canvases, else the centre. Ken Burns zooms
/// about the right edge of that area, so the picture drifts right as it grows.
fn motion_centre(canvas: &Canvas, kind: MotionKind) -> (f32, f32) {
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    let (left, top, right, bottom) =
        canvas.safe_area().map_or((0.0, 0.0, w, h), |area| (area.left, area.top, area.right, area.bottom));
    let x = if kind == MotionKind::KenBurns { right } else { (left + right) / 2.0 };
    (x / w - 0.5, (top + bottom) / 2.0 / h - 0.5)
}

/// A punch-in over the timeline range `[start_us, end_us)`.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ZoomRange {
    pub start_us: i64,
    pub end_us: i64,
    /// Multiplies the clip's scale; 1.15–1.3 is a subtle punch-in.
    pub scale: f64,
}

/// Zoom factors a range may have.
pub const ZOOM_SCALES: std::ops::RangeInclusive<f64> = 0.25..=4.0;
/// The shortest piece a zoom leaves of a clip, so it never flashes for a few frames.
pub const MIN_ZOOM_PIECE_US: i64 = 300_000;

/// Timeline range `[start_us, end_us)`.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TimeRange {
    pub start_us: i64,
    pub end_us: i64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
    /// Clips the edit left alone on purpose, such as clips with keyframes inside a zoom range.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
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

fn media(asset_id: String, transform: Transform, shape: Option<Shape>) -> ClipContent {
    ClipContent::Media {
        asset_id,
        source_in_us: 0,
        volume: 1.0,
        transform,
        speed: 1.0,
        keep_pitch: false,
        adjust: Adjust::default(),
        fade_in_us: 0,
        fade_out_us: 0,
        clean_voice: false,
        shape,
        duck_db: 0.0,
        background: Default::default(),
    }
}

/// The longer side of a picture in picture, as a share of the canvas's shorter side.
const PIP_SIZE: f32 = 0.45;
/// Gap to the canvas edges where no safe area applies, as a share of the canvas's shorter side.
const PIP_MARGIN: f32 = 0.04;
const PIP_RADIUS: f32 = 0.15;
const PIP_SHADOW: f32 = 0.5;

/// The asset scaled down into the top right corner: inside the safe area on vertical videos, a small gap
/// from the edges on the others.
fn picture_in_picture(canvas: &Canvas, asset: &Asset) -> Transform {
    let (cw, ch) = (canvas.width as f32, canvas.height as f32);
    let fit = (cw / asset.width as f32).min(ch / asset.height as f32);
    let (w, h) = (asset.width as f32 * fit, asset.height as f32 * fit);
    let scale = PIP_SIZE * cw.min(ch) / w.max(h);
    let margin = PIP_MARGIN * cw.min(ch);
    let (right, top) = canvas.safe_area().map_or((cw - margin, margin), |area| (area.right, area.top));
    Transform {
        x: (right - w * scale / 2.0) / cw - 0.5,
        y: (top + h * scale / 2.0) / ch - 0.5,
        scale,
        ..Transform::default()
    }
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
    /// Captions tracks only take generated captions, so regenerating them never removes a title.
    fn free_track(&mut self, kind: TrackKind, start: i64, end: i64, keep_in_place: bool) -> usize {
        let found = self
            .tracks
            .iter()
            .enumerate()
            .skip(1)
            .find(|(i, t)| {
                t.kind == kind
                    && !t.is_captions()
                    && t.keep_in_place == keep_in_place
                    && self.is_free(*i, start, end, None)
            })
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
        if let ClipContent::Media { source_in_us, speed, fade_in_us, .. } = &mut second.content {
            *source_in_us += (offset as f64 * *speed as f64).round() as i64;
            *fade_in_us = 0;
        }
        // The first half keeps the entry animation and fade, the second the exit ones.
        if let ClipContent::Media { fade_out_us, .. } = &mut clip.content {
            *fade_out_us = 0;
        }
        clip.anim_out = None;
        second.anim_in = None;
        second.transition_in = None;
        for k in &mut second.keyframes {
            k.t_us -= offset;
        }
        shift_words(&mut second, offset);
        clip.duration_us = offset;
        self.tracks[ti].clips.insert(ci + 1, second);
        ci + 1
    }

    /// Removes `range` from one track: clips are cut at its edges, the inside goes, later
    /// clips move left. Pieces shorter than `min` are dropped rather than kept as slivers.
    fn ripple_delete_track(&mut self, ti: usize, range: TimeRange, min: i64) {
        // Backwards, so a caption split in two leaves the clips still to visit where they were.
        for ci in (0..self.tracks[ti].clips.len()).rev() {
            let c = &self.tracks[ti].clips[ci];
            if !matches!(c.content, ClipContent::Text { .. })
                || c.end_us() <= range.start_us
                || range.end_us <= c.start_us
            {
                continue;
            }
            let (from, to) = (range.start_us - c.start_us, range.end_us - c.start_us);
            let inside = 0 < from && to < c.duration_us;
            let longer_before = from >= c.duration_us - to;
            match heard_sides(c, from, to) {
                // A generated caption shows only the words still heard; with none left, it goes.
                Some((before, after)) if before.is_empty() && after.is_empty() => {
                    self.tracks[ti].clips[ci].duration_us = 0
                }
                Some((before, after)) if inside && !before.is_empty() && !after.is_empty() => {
                    set_words(&mut self.tracks[ti].clips[ci], after);
                    self.split_clip(ti, ci, range.end_us);
                    set_words(&mut self.tracks[ti].clips[ci], before);
                }
                Some((before, after)) => {
                    let keep_before = !before.is_empty();
                    set_words(&mut self.tracks[ti].clips[ci], [before, after].concat());
                    if inside {
                        keep_side(&mut self.tracks[ti].clips[ci], keep_before, range);
                    }
                }
                // Splitting other text would show its whole text twice; it keeps only the longer side.
                None if inside => keep_side(&mut self.tracks[ti].clips[ci], longer_before, range),
                None => {}
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
            .ok_or_else(|| anyhow!("Unknown captions track: replaceCaptions needs a text track named Captions"))
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
            | EditCmd::AddPictureInPicture { .. }
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
            EditCmd::CorrectWords { corrections } => self.correct_words(corrections)?,
            EditCmd::ZoomRanges { ranges } => self.zoom_ranges(ranges, &mut out)?,
            EditCmd::ApplyMotion { clip_id, range, kind, strength } => {
                self.apply_motion(clip_id, range, kind, strength, &mut out)?
            }
            EditCmd::SetThumbnail { thumbnail } => {
                self.thumbnails.retain(|t| t.format != thumbnail.format);
                self.thumbnails.push(thumbnail);
                self.thumbnails.sort_by_key(|t| t.format as u8);
            }
            EditCmd::RemoveThumbnail { format } => self.thumbnails.retain(|t| t.format != format),
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
                    // An image behind a person goes with the image; the person stays.
                    for c in &mut t.clips {
                        if let ClipContent::Media { background, .. } = &mut c.content
                            && matches!(background, Background::Image { asset_id: a } if *a == asset_id)
                        {
                            *background = Background::None;
                        }
                    }
                }
            }
            EditCmd::AddClip { asset_id, start_us, track_id } => {
                let asset = self.asset(&asset_id).ok_or_else(|| anyhow!("Unknown media"))?.clone();
                let kind = track_kind_for(&asset);
                let keep = asset.kind == AssetKind::Audio;
                let duration =
                    if asset.kind == AssetKind::Image { IMAGE_DURATION_US } else { asset.duration_us.max(min) };
                let clip = Clip::new(new_id(), 0, duration, media(asset_id, Transform::default(), None));
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
            EditCmd::AddPictureInPicture { asset_id, start_us, duration_us } => {
                let asset = self.asset(&asset_id).ok_or_else(|| anyhow!("Unknown media"))?.clone();
                ensure!(
                    asset.kind != AssetKind::Audio && asset.width > 0 && asset.height > 0,
                    "Only a video or an image can be a picture in picture"
                );
                let duration = match (asset.kind, duration_us) {
                    (AssetKind::Image, d) => d.unwrap_or(IMAGE_DURATION_US).max(min),
                    (_, d) => d.unwrap_or(asset.duration_us).clamp(min, asset.duration_us.max(min)),
                };
                let start = start_us.max(0);
                let shape = Shape { radius: PIP_RADIUS, shadow: PIP_SHADOW, ..Shape::default() };
                let content = media(asset_id, picture_in_picture(&self.canvas, &asset), Some(shape));
                let clip = Clip::new(new_id(), start, duration, content);
                let track = self.free_track(TrackKind::Video, start, start + duration, false);
                out.select.push(clip.id.clone());
                self.tracks[track].clips.push(clip);
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
                        words: Vec::new(),
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
                if let ClipContent::Media { transform, adjust, shape, background, .. } = &mut sound.content {
                    *transform = Transform::default();
                    *adjust = Adjust::default();
                    *shape = None;
                    *background = Background::None;
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
                // Keyframes and spoken words stay attached to the content when the left edge moves.
                let shift = start - old_start;
                for k in &mut clip.keyframes {
                    k.t_us -= shift;
                }
                shift_words(clip, shift);
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
            EditCmd::UpdateClip {
                clip_id,
                transform,
                volume,
                text,
                style,
                speed,
                keep_pitch,
                adjust,
                fade_in_us,
                fade_out_us,
                clean_voice,
                shape,
                duck_db,
                background,
            } => {
                let (ti, ci) = self.find_clip(&clip_id).ok_or_else(|| anyhow!("Unknown clip"))?;
                let changes_length = speed.is_some();
                ensure!(
                    shape.is_none() || matches!(self.tracks[ti].clips[ci].content, ClipContent::Media { .. }),
                    "Only video and image clips have a shape"
                );
                if let Some(background) = background.as_ref().filter(|b| !b.is_none()) {
                    let picture = match &self.tracks[ti].clips[ci].content {
                        ClipContent::Media { asset_id, .. } => {
                            self.tracks[ti].kind == TrackKind::Video
                                && self.asset(asset_id).is_some_and(|a| a.kind != AssetKind::Audio)
                        }
                        ClipContent::Text { .. } => false,
                    };
                    ensure!(picture, "Only video and image clips have a background");
                    if let Background::Image { asset_id } = &**background {
                        ensure!(
                            self.asset(asset_id).is_some_and(|a| a.kind == AssetKind::Image),
                            "The background must be an image of the project"
                        );
                    }
                }
                if clean_voice == Some(true) {
                    let sound = match &self.tracks[ti].clips[ci].content {
                        ClipContent::Media { asset_id, .. } => {
                            self.asset(asset_id).is_some_and(crate::audio::has_audio)
                        }
                        ClipContent::Text { .. } => false,
                    };
                    ensure!(sound, "This clip has no sound to clean");
                }
                let limit = self.source_limit(&self.tracks[ti].clips[ci]);
                let clip = &mut self.tracks[ti].clips[ci];
                let half = clip.duration_us / 2;
                match &mut clip.content {
                    ClipContent::Media {
                        source_in_us: src,
                        transform: tr,
                        volume: v,
                        speed: sp,
                        keep_pitch: kp,
                        adjust: adj,
                        fade_in_us: fi,
                        fade_out_us: fo,
                        clean_voice: cv,
                        shape: sh,
                        duck_db: duck,
                        background: bg,
                        ..
                    } => {
                        if let Some(x) = transform {
                            *tr = x;
                        }
                        if let Some(x) = shape {
                            *sh = Some(x).filter(|x| *x != Shape::default());
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
                        if let Some(x) = clean_voice {
                            *cv = x;
                        }
                        if let Some(x) = duck_db {
                            *duck = x.clamp(0.0, MAX_DUCK_DB);
                        }
                        if let Some(x) = background {
                            *bg = *x;
                        }
                        if let Some(x) = keep_pitch {
                            *kp = x;
                        }
                        if let Some(x) = speed {
                            let x = x.clamp(MIN_SPEED, MAX_SPEED);
                            if *sp == 1.0 && keep_pitch.is_none() {
                                *kp = true;
                            }
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
                    ClipContent::Text { transform: tr, text: tx, style: st, words } => {
                        if let Some(x) = transform {
                            *tr = x;
                        }
                        if let Some(x) = text {
                            retext_words(words, tx, &x);
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
                    // Keeping place across cuts is for music: a kept video or text track would drift
                    // out of sync with the picture by every cut.
                    ensure!(!k || t.kind == TrackKind::Audio, "Only audio tracks can keep their place while cutting");
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
            EditCmd::AddCaptions { segments, style, anim_in, anim_out } => {
                let clips = caption_clips(segments, &style, [anim_in, anim_out], &self.canvas, min)?;
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
            EditCmd::ReplaceCaptions { track_id, segments, style, anim_in, anim_out } => {
                let ti = self.caption_track(&track_id)?;
                let clips = caption_clips(segments, &style, [anim_in, anim_out], &self.canvas, min)?;
                out.select = clips.iter().map(|c| c.id.clone()).take(1).collect();
                self.tracks[ti].clips = clips;
            }
            EditCmd::RippleDeleteRanges { ranges, keep_track_ids } => {
                // A mistyped id would silently cut the track the caller meant to keep.
                if let Some(unknown) = keep_track_ids.iter().flatten().find(|id| self.track_index(id).is_none()) {
                    bail!("Unknown track in keepTrackIds: {unknown}");
                }
                let kept: Vec<bool> = match &keep_track_ids {
                    Some(ids) => self.tracks.iter().map(|t| ids.contains(&t.id)).collect(),
                    // Only audio keeps its place, also in projects written before that was checked.
                    None => self.tracks.iter().map(|t| t.keep_in_place && t.kind == TrackKind::Audio).collect(),
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
                    // A thumbnail's frame moves with the picture, so it keeps showing the same moment.
                    for thumbnail in &mut self.thumbnails {
                        let t = thumbnail.time_us;
                        thumbnail.time_us -= ranges.iter().map(|r| (t.min(r.end_us) - r.start_us).max(0)).sum::<i64>();
                    }
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

    fn correct_words(&mut self, corrections: Vec<WordCorrection>) -> Result<()> {
        for correction in corrections {
            ensure!(self.asset(&correction.asset_id).is_some(), "Unknown media {}", correction.asset_id);
            let same = |c: &WordCorrection| {
                c.asset_id == correction.asset_id
                    && c.source_start_us == correction.source_start_us
                    && c.original == correction.original
            };
            self.word_corrections.retain(|c| !same(c));
            let text = correction.text.trim().to_string();
            if text != correction.original {
                self.word_corrections.push(WordCorrection { text, ..correction });
            }
        }
        Ok(())
    }

    /// Splits main-track clips at the range edges and scales the pieces inside. Each piece keeps
    /// what it carries whole: the first piece of a clip its transition, entry animation and fade
    /// in, the last its exit animation, fade out and the next clip's transition. Split halves play
    /// their sound on as one, so the sound does not change.
    fn zoom_ranges(&mut self, mut ranges: Vec<ZoomRange>, out: &mut EditOutcome) -> Result<()> {
        ensure!(!ranges.is_empty(), "Give at least one range to zoom");
        for r in &ranges {
            ensure!(
                r.start_us >= 0 && r.end_us > r.start_us,
                "A zoom range must start at 0 or later and end after it starts"
            );
            ensure!(
                r.scale.is_finite() && ZOOM_SCALES.contains(&r.scale),
                "Zoom must be between {}x and {}x",
                ZOOM_SCALES.start(),
                ZOOM_SCALES.end()
            );
        }
        ranges.sort_by_key(|r| r.start_us);
        ensure!(ranges.windows(2).all(|pair| pair[0].end_us <= pair[1].start_us), "Zoom ranges must not overlap");
        let Some(main) = self.track_index(MAIN_TRACK) else { return Ok(()) };
        // Where the previous range's zoom ended, so a piece is never zoomed twice.
        let mut zoomed_until = 0;
        for range in ranges {
            let start = range.start_us.max(zoomed_until);
            let ids: Vec<String> = self.tracks[main]
                .clips
                .iter()
                .filter(|c| c.start_us < range.end_us && c.end_us() > start)
                .map(|c| c.id.clone())
                .collect();
            for id in ids {
                let Some(ci) = self.tracks[main].clips.iter().position(|c| c.id == id) else { continue };
                let clips = &self.tracks[main].clips;
                let clip = &clips[ci];
                if !matches!(clip.content, ClipContent::Media { .. }) {
                    continue;
                }
                if !clip.keyframes.is_empty() {
                    if !out.skipped.contains(&id) {
                        out.skipped.push(id);
                    }
                    continue;
                }
                let (c0, c1) = (clip.start_us, clip.end_us());
                let (fade_in, fade_out) = match &clip.content {
                    ClipContent::Media { fade_in_us, fade_out_us, .. } => (*fade_in_us, *fade_out_us),
                    ClipContent::Text { .. } => (0, 0),
                };
                let length = |a: Option<Animation>| a.map_or(0, |a| a.duration_us);
                let transition = |c: Option<&Clip>| c.and_then(|c| c.transition_in).map_or(0, |t| t.duration_us);
                let head = length(clip.anim_in).max(transition(Some(clip))).max(2 * fade_in);
                let tail = length(clip.anim_out).max(transition(clips.get(ci + 1))).max(2 * fade_out);
                let (mut a, mut b) = (start.max(c0), range.end_us.min(c1));
                if a > c0 && a - c0 < MIN_ZOOM_PIECE_US.max(head) {
                    a = c0;
                }
                if b < c1 && c1 - b < MIN_ZOOM_PIECE_US.max(tail) {
                    b = c1;
                }
                // A sliver of a range at the edge of a clip is not worth a cut.
                let whole = a == c0 && b == c1;
                let needs = MIN_ZOOM_PIECE_US.max(if a == c0 { head } else { 0 }).max(if b == c1 { tail } else { 0 });
                if !whole && b - a < needs {
                    continue;
                }
                if b < c1 {
                    self.split_clip(main, ci, b);
                }
                let zi = if a > c0 { self.split_clip(main, ci, a) } else { ci };
                if let ClipContent::Media { transform, .. } = &mut self.tracks[main].clips[zi].content {
                    transform.scale = (f64::from(transform.scale) * range.scale) as f32;
                }
                zoomed_until = zoomed_until.max(b);
            }
        }
        Ok(())
    }

    /// Two keyframes on each clip the motion reaches, at the ends of the range.
    fn apply_motion(
        &mut self,
        clip_id: Option<String>,
        range: Option<TimeRange>,
        kind: MotionKind,
        strength: f64,
        out: &mut EditOutcome,
    ) -> Result<()> {
        ensure!(
            strength.is_finite() && MOTION_STRENGTHS.contains(&strength),
            "Motion strength must be between {} and {}; 0.1 zooms 10 %",
            MOTION_STRENGTHS.start(),
            MOTION_STRENGTHS.end()
        );
        let picture = |p: &Project, c: &Clip| match &c.content {
            ClipContent::Media { asset_id, .. } => p.asset(asset_id).is_some_and(|a| a.kind != AssetKind::Audio),
            ClipContent::Text { .. } => false,
        };
        let targets: Vec<(usize, usize)> = match &clip_id {
            Some(id) => {
                let (ti, ci) = self.find_clip(id).ok_or_else(|| anyhow!("Unknown clip"))?;
                ensure!(picture(self, &self.tracks[ti].clips[ci]), "Motion moves video and image clips only");
                vec![(ti, ci)]
            }
            None => {
                let r = range.ok_or_else(|| anyhow!("Give a clip or a range for the motion"))?;
                let ti = self.track_index(MAIN_TRACK).ok_or_else(|| anyhow!("No main track"))?;
                let under = |c: &Clip| c.start_us < r.end_us && c.end_us() > r.start_us && picture(self, c);
                (0..self.tracks[ti].clips.len())
                    .filter(|&ci| under(&self.tracks[ti].clips[ci]))
                    .map(|ci| (ti, ci))
                    .collect()
            }
        };
        ensure!(!targets.is_empty(), "No video or image clip on the main track under the range");
        let (start, end) = range.map_or_else(
            || {
                let c = &self.tracks[targets[0].0].clips[targets[0].1];
                (c.start_us, c.end_us())
            },
            |r| (r.start_us, r.end_us),
        );
        // A clip keeps the range of a motion that reached beyond it, even once the clips before it are gone.
        ensure!(
            end > start && (start >= 0 || clip_id.is_some()),
            "A motion range must start at 0 or later and end after it starts"
        );
        let min = min_duration(self);
        let (fx, fy) = motion_centre(&self.canvas, kind);
        let zoom = 1.0 + strength as f32;
        for (ti, ci) in targets {
            let clip = &mut self.tracks[ti].clips[ci];
            let (a, b) = (start.max(clip.start_us), end.min(clip.end_us()));
            if b - a < min {
                ensure!(clip_id.is_none(), "The motion range lies outside the clip");
                continue;
            }
            if !clip.keyframes.is_empty() {
                out.skipped.push(clip.id.clone());
                continue;
            }
            let base = match &clip.content {
                ClipContent::Media { transform, .. } | ClipContent::Text { transform, .. } => *transform,
            };
            // Scaled about (fx, fy), so the picture there stays where it is.
            let (x, y) = (base.x + (zoom - 1.0) * (base.x - fx), base.y + (zoom - 1.0) * (base.y - fy));
            let zoomed = Transform { x, y, scale: base.scale * zoom, ..base };
            let (from, to) = if kind == MotionKind::PullOut { (zoomed, base) } else { (base, zoomed) };
            // At the ends of the whole range, outside the clip where it is only a part, so the clips
            // under a range follow one curve.
            let start_us = clip.start_us;
            let at = |t: i64, transform| Keyframe { t_us: t - start_us, transform, ease: Ease::Smooth };
            clip.keyframes = vec![at(start, from), at(end, to)];
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
fn caption_clips(
    mut segments: Vec<CaptionSegment>,
    style: &TextStyle,
    [anim_in, anim_out]: [Option<Animation>; 2],
    canvas: &Canvas,
    min: i64,
) -> Result<Vec<Clip>> {
    let mut style = style.clone();
    if style.max_width.is_none() {
        style.max_width = canvas.safe_area().map(|area| area.centered_width(canvas.width as f32));
    }
    segments.retain(|s| s.end_us > s.start_us && !s.text.trim().is_empty());
    segments.sort_by_key(|s| s.start_us);
    let mut merged: Vec<CaptionSegment> = Vec::with_capacity(segments.len());
    for s in segments {
        let text = s.text.trim().to_string();
        // A highlight following other words than the ones shown would mark the wrong word.
        ensure!(
            s.words.is_empty() || s.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ") == text,
            "Caption words must be the caption text split at single spaces: {text:?}"
        );
        let start_us = s.start_us.max(0);
        match merged.last_mut() {
            Some(prev) if start_us < prev.start_us + min => {
                prev.text = format!("{} {text}", prev.text);
                prev.end_us = prev.end_us.max(s.end_us);
                // Both or neither keep their words, so the merged text still matches them.
                if prev.words.is_empty() || s.words.is_empty() {
                    prev.words.clear();
                } else {
                    prev.words.extend(s.words);
                }
            }
            Some(prev) => {
                prev.end_us = prev.end_us.min(start_us);
                merged.push(CaptionSegment { start_us, end_us: s.end_us, text, words: s.words });
            }
            None => merged.push(CaptionSegment { start_us, end_us: s.end_us, text, words: s.words }),
        }
    }
    Ok(merged
        .into_iter()
        .map(|s| {
            // Where CapCut puts auto captions on a reel: a bit below the middle.
            let transform = Transform { y: CAPTION_Y, ..Transform::default() };
            let words = s
                .words
                .into_iter()
                .map(|w| CaptionWord {
                    start_us: w.start_us.saturating_sub(s.start_us),
                    end_us: w.end_us.saturating_sub(s.start_us),
                    ..w
                })
                .collect();
            let content = ClipContent::Text { text: s.text, style: style.clone(), transform, words };
            Clip { anim_in, anim_out, ..Clip::new(new_id(), s.start_us, (s.end_us - s.start_us).max(min), content) }
        })
        .collect())
}

/// A caption corrected word for word keeps its timing: when its old text was its words and the new text
/// has as many words (split at single spaces), each word takes its new text and keeps its times. A
/// corrected word may hold a space ("na pivo"), so each takes as many as it had. Any other edit leaves
/// the words as they were, so they no longer match and nothing is highlighted.
fn retext_words(words: &mut [CaptionWord], old: &str, new: &str) {
    let joined = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    if words.is_empty() || joined != old || new.split(' ').count() != old.split(' ').count() {
        return;
    }
    let mut tokens = new.split(' ');
    for word in words {
        let size = word.text.split(' ').count();
        word.text = tokens.by_ref().take(size).collect::<Vec<_>>().join(" ");
    }
}

/// The words of a generated caption heard in it before `from` and from `to` on (clip time), each where its
/// middle is; None for other text and for a caption whose text no longer is its words. Words a split or a
/// trim left outside the clip are not heard there, so they go.
fn heard_sides(clip: &Clip, from: i64, to: i64) -> Option<(Vec<CaptionWord>, Vec<CaptionWord>)> {
    let ClipContent::Text { text, words, .. } = &clip.content else { return None };
    if words.is_empty() || words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ") != *text {
        return None;
    }
    let heard: Vec<_> =
        words.iter().filter(|w| (0..clip.duration_us).contains(&((w.start_us + w.end_us) / 2))).collect();
    let side = |keep: &dyn Fn(i64) -> bool| {
        heard.iter().filter(|w| keep((w.start_us + w.end_us) / 2)).map(|&w| w.clone()).collect()
    };
    Some((side(&|middle| middle < from), side(&|middle| middle >= to)))
}

/// Makes a generated caption say exactly `kept`.
fn set_words(clip: &mut Clip, kept: Vec<CaptionWord>) {
    if let ClipContent::Text { text, words, .. } = &mut clip.content {
        *text = kept.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
        *words = kept;
    }
}

/// Trims a text clip that `range` lies inside to the part before or after it.
fn keep_side(clip: &mut Clip, before: bool, range: TimeRange) {
    if before {
        clip.duration_us = range.start_us - clip.start_us;
        return;
    }
    // Keyframes and spoken words stay on the kept text, as when a split drops the first half.
    for k in &mut clip.keyframes {
        k.t_us -= range.end_us - clip.start_us;
    }
    shift_words(clip, range.end_us - clip.start_us);
    clip.duration_us = clip.end_us() - range.end_us;
    clip.start_us = range.end_us;
}

/// Keeps a text clip's spoken words on the same moments when its start moves `by` later.
fn shift_words(clip: &mut Clip, by: i64) {
    if let ClipContent::Text { words, .. } = &mut clip.content {
        for word in words {
            word.start_us = word.start_us.saturating_sub(by);
            word.end_us = word.end_us.saturating_sub(by);
        }
    }
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
                let step = self.project.apply(cmd)?;
                outcome.select.extend(step.select);
                outcome.skipped.extend(step.skipped);
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

    /// The key an edit needs to merge into the newest undo step; none after a seal, undo or redo.
    pub fn coalescing(&self) -> Option<&str> {
        self.coalesce.as_deref()
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
            mirror: false,
            credit: None,
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

    #[test]
    fn thumbnails_stay_one_per_format_and_keep_their_frame_through_cuts() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        let thumbnail = |format, time_us| EditCmd::SetThumbnail {
            thumbnail: Thumbnail {
                format,
                time_us,
                frame: Transform::default(),
                background: Default::default(),
                texts: Vec::new(),
                outline: None,
            },
        };
        p.apply(thumbnail(ThumbnailFormat::Youtube16x9, 1_000_000)).unwrap();
        p.apply(thumbnail(ThumbnailFormat::Cover9x16, 4_500_000)).unwrap();
        p.apply(thumbnail(ThumbnailFormat::Cover9x16, 4_000_000)).unwrap();
        let times = |p: &Project| p.thumbnails.iter().map(|t| (t.format, t.time_us)).collect::<Vec<_>>();
        assert_eq!(times(&p), [(ThumbnailFormat::Cover9x16, 4_000_000), (ThumbnailFormat::Youtube16x9, 1_000_000)]);
        // A second before the cover's frame goes and half a second ends at it; the earlier frame stays.
        let ranges = vec![
            TimeRange { start_us: 2_000_000, end_us: 3_000_000 },
            TimeRange { start_us: 3_500_000, end_us: 4_200_000 },
        ];
        p.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: None }).unwrap();
        assert_eq!(times(&p), [(ThumbnailFormat::Cover9x16, 2_500_000), (ThumbnailFormat::Youtube16x9, 1_000_000)]);
        p.apply(EditCmd::RemoveThumbnail { format: ThumbnailFormat::Youtube16x9 }).unwrap();
        assert_eq!(times(&p), [(ThumbnailFormat::Cover9x16, 2_500_000)]);
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

    /// Only music keeps its place: a kept main track would leave the cut picture out of sync.
    #[test]
    fn only_audio_tracks_keep_their_place_through_ripple_cuts() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        let keep = |id: &str| EditCmd::UpdateTrack {
            track_id: id.into(),
            muted: None,
            hidden: None,
            keep_in_place: Some(true),
        };
        let error = p.apply(keep(MAIN_TRACK)).unwrap_err();
        assert!(error.to_string().contains("Only audio tracks"), "{error:#}");
        // A project written before that was checked still cuts its main track.
        p.tracks[0].keep_in_place = true;
        p.apply(EditCmd::RippleDeleteRanges {
            ranges: vec![TimeRange { start_us: 0, end_us: 1_000_000 }],
            keep_track_ids: None,
        })
        .unwrap();
        assert_eq!(p.tracks[0].clips[0].duration_us, 4_000_000);
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
    fn picture_in_picture_sits_in_the_top_right_corner_above_the_main_track() {
        let mut p = project();
        let wide = Asset { width: 1920, height: 1080, ..asset("wide", AssetKind::Video, 4) };
        let photo = Asset { width: 1000, height: 1000, ..asset("photo", AssetKind::Image, 0) };
        p.apply(EditCmd::AddAssets { assets: vec![wide, photo] }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        let pip = |p: &mut Project, asset: &str, start_us, duration_us| {
            let cmd = EditCmd::AddPictureInPicture { asset_id: asset.into(), start_us, duration_us };
            let id = p.apply(cmd).unwrap().select[0].clone();
            let (ti, ci) = p.find_clip(&id).unwrap();
            let clip = &p.tracks[ti].clips[ci];
            let ClipContent::Media { shape, .. } = &clip.content else { panic!() };
            assert_eq!(shape.as_ref().map(|s| (s.radius, s.shadow)), Some((PIP_RADIUS, PIP_SHADOW)));
            let bounds = crate::render::layer_bounds(p, clip.start_us, &mut crate::text::TextRenderer::new());
            let corners = bounds.iter().find(|b| b.0 == id).unwrap().1;
            (ti, clip.start_us, clip.duration_us, corners)
        };
        // Vertical: inside the Reels and TikTok safe area, against its top right corner.
        let (track, start, duration, [tl, tr, br, _]) = pip(&mut p, "wide", 1_000_000, None);
        let area = p.canvas.safe_area().unwrap();
        assert_eq!((track, start, duration), (1, 1_000_000, 4_000_000));
        assert!((tr[0] - area.right).abs() < 0.01 && (tr[1] - area.top).abs() < 0.01, "{tr:?}");
        assert!(
            ((tr[0] - tl[0]) - 0.45 * 1080.0).abs() < 0.01
                && ((br[1] - tr[1]) - 0.45 * 1080.0 * 9.0 / 16.0).abs() < 0.01
        );
        // Another one over the first goes on a new track above it; an image lasts 3 s or as asked.
        assert_eq!(pip(&mut p, "photo", 2_000_000, None).0, 2);
        assert_eq!(pip(&mut p, "photo", 6_000_000, Some(500_000)).2, 500_000);
        // A video is never longer than its file.
        assert_eq!(pip(&mut p, "wide", 9_000_000, Some(60_000_000)).2, 4_000_000);
        // 16:9: a small gap from the top and right edges of the canvas.
        p.apply(EditCmd::SetCanvas { width: 1920, height: 1080, background: None, background_blur: None }).unwrap();
        let (.., [_, tr, ..]) = pip(&mut p, "photo", 20_000_000, None);
        assert!((tr[0] - (1920.0 - 0.04 * 1080.0)).abs() < 0.01 && (tr[1] - 0.04 * 1080.0).abs() < 0.01, "{tr:?}");
        let sound = EditCmd::AddPictureInPicture { asset_id: "m".into(), start_us: 0, duration_us: None };
        assert!(p.apply(sound).is_err());
    }

    #[test]
    fn a_default_shape_is_no_shape_and_text_has_none() {
        let mut p = project();
        let id = p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap().select[0]
            .clone();
        let shape = |p: &Project| match &p.tracks[0].clips[0].content {
            ClipContent::Media { shape, .. } => shape.clone(),
            ClipContent::Text { .. } => unreachable!(),
        };
        let set = |clip_id: &str, shape: Shape| {
            serde_json::from_value::<EditCmd>(
                serde_json::json!({"type": "updateClip", "clipId": clip_id, "shape": shape}),
            )
            .unwrap()
        };
        let round = Shape { radius: 1.0, border_width: 6.0, ..Shape::default() };
        p.apply(set(&id, round.clone())).unwrap();
        assert_eq!(shape(&p), Some(round));
        p.apply(set(&id, Shape::default())).unwrap();
        assert_eq!(shape(&p), None);
        let style: TextStyle =
            serde_json::from_value(serde_json::json!({"fontSize": 60.0, "color": "#ffffff"})).unwrap();
        let text = p.apply(EditCmd::AddText { start_us: 0, text: "Hi".into(), style }).unwrap().select[0].clone();
        assert!(p.apply(set(&text, Shape::default())).is_err());
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
            keep_pitch: None,
            adjust: None,
            fade_in_us: None,
            fade_out_us: None,
            clean_voice: None,
            shape: None,
            duck_db: None,
            background: None,
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
            keep_pitch: None,
            adjust,
            fade_in_us,
            fade_out_us,
            clean_voice: None,
            shape: None,
            duck_db: None,
            background: None,
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
            keep_pitch: None,
            adjust,
            fade_in_us,
            fade_out_us,
            clean_voice: None,
            shape: None,
            duck_db: None,
            background: None,
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
            keep_pitch: None,
            adjust,
            fade_in_us,
            fade_out_us,
            clean_voice: None,
            shape: None,
            duck_db: None,
            background: None,
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
    fn cuts_keep_the_fade_in_on_the_first_piece_and_the_fade_out_on_the_last() {
        let fades = |p: &Project, ti: usize| {
            p.tracks[ti]
                .clips
                .iter()
                .map(|c| match &c.content {
                    ClipContent::Media { fade_in_us, fade_out_us, .. } => (*fade_in_us / 1000, *fade_out_us / 1000),
                    ClipContent::Text { .. } => panic!(),
                })
                .collect::<Vec<_>>()
        };
        // Music with 2 s fades split in the middle keeps playing through the split.
        let mut p = project();
        let id = p.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(0), track_id: None }).unwrap().select
            [0]
        .clone();
        let fade: EditCmd = serde_json::from_value(serde_json::json!({
            "type":"updateClip", "clipId":id, "fadeInUs":2_000_000, "fadeOutUs":2_000_000
        }))
        .unwrap();
        p.apply(fade).unwrap();
        p.apply(EditCmd::SplitClip { clip_id: id, at_us: 10_000_000 }).unwrap();
        assert_eq!(fades(&p, 1), vec![(2000, 0), (0, 2000)]);
        // Silence cuts on a clip with short fades do not fade every piece in and out.
        let mut p = project();
        let id = p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap().select[0]
            .clone();
        let fade: EditCmd = serde_json::from_value(serde_json::json!({
            "type":"updateClip", "clipId":id, "fadeInUs":300_000, "fadeOutUs":300_000
        }))
        .unwrap();
        p.apply(fade).unwrap();
        p.apply(EditCmd::RippleDeleteRanges {
            ranges: vec![
                TimeRange { start_us: 1_000_000, end_us: 1_500_000 },
                TimeRange { start_us: 3_000_000, end_us: 3_500_000 },
            ],
            keep_track_ids: None,
        })
        .unwrap();
        assert_eq!(fades(&p, 0), vec![(300, 0), (0, 0), (0, 300)]);
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
                // Cut pieces lose the fade at the cut and keep the clamped fade at the outer edge.
                let clips: Vec<_> = p.tracks.iter().flat_map(|track| &track.clips).collect();
                for (index, clip) in clips.iter().enumerate() {
                    let ClipContent::Media { fade_in_us, fade_out_us, .. } = &clip.content else { panic!() };
                    let half = clip.duration_us / 2;
                    assert_eq!(
                        (*fade_in_us, *fade_out_us),
                        (if index == 0 { half } else { 0 }, if index == clips.len() - 1 { half } else { 0 }),
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
                    keep_pitch: None,
                    transform: None,
                    volume: None,
                    text: None,
                    style: None,
                    adjust: None,
                    fade_in_us: None,
                    fade_out_us: None,
                    clean_voice: None,
                    shape: None,
                    duck_db: None,
                    background: None,
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
                keep_pitch: None,
                adjust: None,
                fade_in_us: None,
                fade_out_us: None,
                clean_voice: None,
                shape: None,
                duck_db: None,
                background: None,
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
            highlight: None,
            keywords: None,
        };
        let seg = |s, e, t: &str| CaptionSegment { start_us: s, end_us: e, text: t.into(), words: Vec::new() };
        p.apply(EditCmd::AddCaptions {
            segments: vec![seg(0, 1_200_000, "Ahoj"), seg(1_000_000, 2_000_000, "světe")],
            style: style.clone(),
            anim_in: None,
            anim_out: None,
        })
        .unwrap();
        let track = p.tracks.iter().find(|t| t.name == "Captions").unwrap().id.clone();
        p.apply(EditCmd::ReplaceCaptions {
            track_id: track,
            segments: vec![seg(0, 1_000_000, "Znovu")],
            style: style.clone(),
            anim_in: None,
            anim_out: None,
        })
        .unwrap();
        let caption_tracks: Vec<_> = p.tracks.iter().filter(|t| t.name == "Captions").collect();
        assert_eq!(caption_tracks.len(), 1);
        assert_eq!(caption_tracks[0].clips.len(), 1);
        // Adding never removes an existing captions track.
        p.apply(EditCmd::AddCaptions {
            segments: vec![seg(0, 1_000_000, "Druhá")],
            style: style.clone(),
            anim_in: None,
            anim_out: None,
        })
        .unwrap();
        assert_eq!(p.tracks.iter().filter(|t| t.name == "Captions").count(), 2);
        // A title added where a captions track is free still gets its own track, so replacing
        // the captions never removes it.
        p.apply(EditCmd::AddText { start_us: 5_000_000, text: "Title".into(), style: style.clone() }).unwrap();
        let titles = p.tracks.iter().find(|t| t.kind == TrackKind::Text && !t.is_captions()).unwrap().id.clone();
        let replace = EditCmd::ReplaceCaptions {
            track_id: titles.clone(),
            segments: vec![seg(0, 1_000_000, "X")],
            style: style.clone(),
            anim_in: None,
            anim_out: None,
        };
        assert!(p.apply(replace).is_err());
        let captions = p.tracks.iter().find(|t| t.is_captions()).unwrap().id.clone();
        p.apply(EditCmd::ReplaceCaptions {
            track_id: captions,
            segments: vec![seg(0, 1_000_000, "Y")],
            style,
            anim_in: None,
            anim_out: None,
        })
        .unwrap();
        let title = &p.tracks.iter().find(|t| t.id == titles).unwrap().clips[0].content;
        assert!(matches!(title, ClipContent::Text { text, .. } if text == "Title"));
    }

    #[test]
    fn every_caption_gets_the_animations_of_its_style() {
        let style: TextStyle =
            serde_json::from_value(serde_json::json!({"fontSize": 95.0, "color": "#ffffff"})).unwrap();
        let seg = |s, e, t: &str| CaptionSegment { start_us: s, end_us: e, text: t.into(), words: Vec::new() };
        let pop = Some(Animation { kind: crate::model::AnimationKind::Pop, duration_us: 200_000 });
        let clips = caption_clips(
            vec![seg(0, 1_000_000, "first"), seg(1_000_000, 2_000_000, "second")],
            &style,
            [pop, None],
            &Project::new("c").canvas,
            33_334,
        )
        .unwrap();
        assert!(clips.iter().all(|c| c.anim_in == pop && c.anim_out.is_none()));
        // An older command without animations reads as none.
        let old: EditCmd = serde_json::from_value(serde_json::json!({
            "type": "addCaptions", "segments": [], "style": style
        }))
        .unwrap();
        assert!(matches!(old, EditCmd::AddCaptions { anim_in: None, anim_out: None, .. }));
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
            highlight: None,
            keywords: None,
        };
        let seg = |s, e, t: &str| CaptionSegment { start_us: s, end_us: e, text: t.into(), words: Vec::new() };
        let clips = caption_clips(
            vec![seg(0, 1_000_000, "first"), seg(0, 2_000_000, "second"), seg(1_500_000, 3_000_000, "third")],
            &style,
            [None, None],
            &Project::new("c").canvas,
            33_334,
        )
        .unwrap();
        let spans: Vec<_> = clips.iter().map(|c| (c.start_us, c.end_us())).collect();
        assert_eq!(spans, vec![(0, 1_500_000), (1_500_000, 3_000_000)]);
        let ClipContent::Text { text, .. } = &clips[0].content else { panic!() };
        assert_eq!(text, "first second");
    }

    #[test]
    fn ripple_delete_rejects_unknown_kept_tracks() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        let before = p.clone();
        let cut = EditCmd::RippleDeleteRanges {
            ranges: vec![TimeRange { start_us: 0, end_us: 500_000 }],
            keep_track_ids: Some(vec!["musci".into()]),
        };
        assert!(p.apply(cut).unwrap_err().to_string().contains("Unknown track in keepTrackIds: musci"));
        assert_eq!(p, before);
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
            highlight: None,
            keywords: None,
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
            highlight: None,
            keywords: None,
        };
        let seg = CaptionSegment { start_us: 1_000_000, end_us: 3_000_000, text: "jsem se".into(), words: Vec::new() };
        p.apply(EditCmd::AddCaptions { segments: vec![seg], style, anim_in: None, anim_out: None }).unwrap();
        let clip_id = p.tracks.iter().find(|t| t.is_captions()).unwrap().clips[0].id.clone();
        // A keyframe at 2.5 s on the timeline, inside the part that stays.
        let keyframes = vec![Keyframe { t_us: 1_500_000, transform: Transform::default(), ease: Ease::Linear }];
        p.apply(EditCmd::SetKeyframes { clip_id, keyframes }).unwrap();
        // Cutting [1.5, 2.2) leaves 0.5 s before and 0.8 s after: the later part stays.
        p.apply(EditCmd::RippleDeleteRanges {
            ranges: vec![TimeRange { start_us: 1_500_000, end_us: 2_200_000 }],
            keep_track_ids: Some(vec![]),
        })
        .unwrap();
        let captions = p.tracks.iter().find(|t| t.is_captions()).unwrap();
        let spans: Vec<_> = captions.clips.iter().map(|c| (c.start_us, c.end_us())).collect();
        assert_eq!(spans, vec![(1_500_000, 2_300_000)]);
        // The keyframe moved with the cut to 1.8 s, 0.3 s into the kept text.
        assert_eq!(captions.clips[0].keyframes.iter().map(|k| k.t_us).collect::<Vec<_>>(), vec![300_000]);
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
                highlight: None,
                keywords: None,
            };
            p.apply(EditCmd::AddCaptions {
                segments: vec![CaptionSegment {
                    start_us: 2_000_000,
                    end_us: 3_000_000,
                    text: "aligned".into(),
                    words: Vec::new(),
                }],
                style,
                anim_in: None,
                anim_out: None,
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

    /// The word a karaoke caption marks at timeline time `t_us`.
    fn spoken_at(p: &Project, t_us: i64) -> Option<String> {
        let clip = p.tracks.iter().filter(|t| t.is_captions()).flat_map(|t| &t.clips).find(|c| c.contains(t_us))?;
        let ClipContent::Text { text, words, .. } = &clip.content else { return None };
        crate::model::spoken_word(text, words, t_us - clip.start_us).map(|r| text[r].to_string())
    }

    /// Five seconds of video with one karaoke caption over [1 s, 4 s): "Dneska vám ukážu jak", the words
    /// spoken at 1.0–1.4, 1.5–1.9, 2.4–2.9 and 3.2–3.8 s.
    fn karaoke() -> (Project, String) {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        let word = |text: &str, start_ms: i64, end_ms: i64| CaptionWord {
            text: text.into(),
            start_us: start_ms * 1000,
            end_us: end_ms * 1000,
            key: false,
        };
        let style: TextStyle = serde_json::from_value(serde_json::json!({
            "fontSize": 95.0, "color": "#ffffff", "strokeWidth": 7.5, "highlight": "#ffe14d"
        }))
        .unwrap();
        let segment = CaptionSegment {
            start_us: 1_000_000,
            end_us: 4_000_000,
            text: "Dneska vám ukážu jak".into(),
            words: vec![
                word("Dneska", 1000, 1400),
                word("vám", 1500, 1900),
                word("ukážu", 2400, 2900),
                word("jak", 3200, 3800),
            ],
        };
        let out =
            p.apply(EditCmd::AddCaptions { segments: vec![segment], style, anim_in: None, anim_out: None }).unwrap();
        (p, out.select[0].clone())
    }

    fn spoken(p: &Project) -> Vec<Option<String>> {
        (0..5_000_000).step_by(50_000).map(|t| spoken_at(p, t)).collect()
    }

    #[test]
    fn karaoke_words_stay_on_their_moments_through_split_trim_duplicate_and_cuts() {
        let (p, id) = karaoke();
        let before = spoken(&p);
        assert_eq!(before[20].as_deref(), Some("Dneska"));
        assert_eq!(before[50].as_deref(), Some("ukážu"));
        assert_eq!(before[42], None, "the gap between vám and ukážu");
        // A split keeps every word where it was, in both halves.
        let mut split = p.clone();
        split.apply(EditCmd::SplitClip { clip_id: id.clone(), at_us: 2_200_000 }).unwrap();
        assert_eq!(split.tracks[1].clips.len(), 2);
        assert_eq!(spoken(&split), before, "split");
        // Trimming the left edge keeps the words over the same moments.
        let mut trimmed = p.clone();
        let trim =
            EditCmd::TrimClip { clip_id: id.clone(), start_us: 1_450_000, duration_us: 2_550_000, source_in_us: None };
        trimmed.apply(trim).unwrap();
        let expected: Vec<_> =
            before.iter().enumerate().map(|(i, w)| if i * 50_000 < 1_450_000 { None } else { w.clone() }).collect();
        assert_eq!(spoken(&trimmed), expected, "left trim");
        // A copy says the same words at the same offsets.
        let mut copied = p.clone();
        let copy = copied.apply(EditCmd::DuplicateClip { clip_id: id }).unwrap().select[0].clone();
        let copy_start = copied.tracks[1].clips.iter().find(|c| c.id == copy).unwrap().start_us;
        for offset in (0..3_000_000).step_by(50_000) {
            assert_eq!(spoken_at(&copied, copy_start + offset), spoken_at(&p, 1_000_000 + offset), "copy at {offset}");
        }
        // Ripple cuts inside the caption and across its start: every word left on the timeline moves with the
        // picture, in whichever caption keeps it.
        for (cut_start, cut_end) in [(1_300_000, 2_000_000), (3_000_000, 3_500_000), (500_000, 1_600_000)] {
            let mut cut = p.clone();
            let ranges = vec![TimeRange { start_us: cut_start, end_us: cut_end }];
            cut.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: None }).unwrap();
            let length = cut_end - cut_start;
            for (i, word) in before.iter().enumerate() {
                let t = i as i64 * 50_000;
                let at = if t < cut_start {
                    t
                } else if t >= cut_end {
                    t - length
                } else {
                    continue;
                };
                if cut.tracks[1].clips.iter().any(|c| c.contains(at)) {
                    assert_eq!(spoken_at(&cut, at), *word, "cut [{cut_start}, {cut_end}) at {t}");
                }
            }
        }
    }

    #[test]
    fn a_cut_takes_its_words_out_of_the_captions() {
        let (p, id) = karaoke();
        let cut = |p: &Project, start_ms: i64, end_ms: i64| -> Vec<(i64, i64, String)> {
            let mut cut = p.clone();
            let ranges = vec![TimeRange { start_us: start_ms * 1000, end_us: end_ms * 1000 }];
            cut.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: None }).unwrap();
            let captions = cut.tracks.iter().filter(|t| t.is_captions()).flat_map(|t| &t.clips);
            captions
                .map(|c| {
                    let ClipContent::Text { text, .. } = &c.content else { panic!() };
                    (c.start_us / 1000, c.end_us() / 1000, text.clone())
                })
                .collect()
        };
        // A cut inside the caption leaves a caption on each side, each saying its own words.
        assert_eq!(cut(&p, 1300, 2000), [(1000, 1300, "Dneska".into()), (1300, 3300, "ukážu jak".into())]);
        // Across its start, the words after the cut are left; a word cut in the middle goes.
        assert_eq!(cut(&p, 500, 1950), [(500, 2550, "ukážu jak".into())]);
        // With every word cut, the caption goes too, though a moment of it is left after the cut.
        assert_eq!(cut(&p, 900, 3900), []);
        // After a split each half carries every word, but a half a cut reaches says only those heard in it;
        // the other half keeps the whole text, as the split left it.
        let mut split = p.clone();
        split.apply(EditCmd::SplitClip { clip_id: id.clone(), at_us: 2_200_000 }).unwrap();
        let halves = [(1000, 1950, "Dneska vám".into()), (2000, 3800, "Dneska vám ukážu jak".into())];
        assert_eq!(cut(&split, 1950, 2150), halves);
        // Typed text is no longer the words: it keeps its longer side whole, as before.
        let mut typed = p.clone();
        typed
            .apply(
                serde_json::from_value(serde_json::json!({"type": "updateClip", "clipId": id, "text": "Ahoj"}))
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(cut(&typed, 1300, 2000), [(1300, 3300, "Ahoj".into())]);
    }

    #[test]
    fn a_caption_corrected_word_for_word_keeps_its_highlight() {
        let (mut p, id) = karaoke();
        let before = spoken(&p);
        let retext = |text: &str| -> EditCmd {
            serde_json::from_value(serde_json::json!({"type": "updateClip", "clipId": id, "text": text})).unwrap()
        };
        // A word correction replaces one word: the corrected word lights up exactly when the old one did.
        p.apply(retext("Dneska vám ukážu kam")).unwrap();
        let corrected: Vec<_> =
            before.iter().map(|w| w.as_ref().map(|w| if w == "jak" { "kam".into() } else { w.clone() })).collect();
        assert_eq!(spoken(&p), corrected);
        // Typing in the inspector passes through a trailing space: still four words, the last one empty for now.
        p.apply(retext("Dneska vám ukážu ")).unwrap();
        p.apply(retext("Dneska vám ukážu, jak")).unwrap();
        assert_eq!(spoken_at(&p, 2_600_000).as_deref(), Some("ukážu,"));
        assert_eq!(spoken_at(&p, 3_500_000).as_deref(), Some("jak"));
        // A word more or less: the words no longer match, so nothing lights up, and nothing guesses.
        p.apply(retext("Dneska vám ukážu, jak na to")).unwrap();
        assert!(spoken(&p).iter().all(Option::is_none));
        let ClipContent::Text { words, .. } = &p.tracks[1].clips[0].content else { panic!() };
        assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["Dneska", "vám", "ukážu,", "jak"]);
        // Back to the words they were: lit again.
        p.apply(retext("Dneska vám ukážu, jak")).unwrap();
        assert_eq!(spoken_at(&p, 2_600_000).as_deref(), Some("ukážu,"));
        // A word corrected into two before captions were made stays one timed word, and fixing
        // another word keeps the caption lit.
        let timed = |text: &str, start_us: i64| CaptionWord {
            text: text.into(),
            start_us,
            end_us: start_us + 200_000,
            key: false,
        };
        let mut words = vec![timed("na pivo", 0), CaptionWord { key: true, ..timed("teď", 300_000) }];
        retext_words(&mut words, "na pivo teď", "na pivo hned");
        assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["na pivo", "hned"]);
        // A corrected key word stays key.
        assert_eq!(words.iter().map(|w| w.key).collect::<Vec<_>>(), [false, true]);
        assert_eq!(crate::model::spoken_word("na pivo hned", &words, 350_000), Some(8..12));
    }

    #[test]
    fn caption_words_must_spell_the_caption_and_survive_merging() {
        let word = |text: &str, start_us: i64| CaptionWord {
            text: text.into(),
            start_us,
            end_us: start_us + 200_000,
            key: false,
        };
        let segment = |start_us: i64, text: &str, words: Vec<CaptionWord>| CaptionSegment {
            start_us,
            end_us: start_us + 500_000,
            text: text.into(),
            words,
        };
        let style: TextStyle =
            serde_json::from_value(serde_json::json!({"fontSize": 95.0, "color": "#ffffff", "highlight": "#ffe14d"}))
                .unwrap();
        let mut p = project();
        let wrong = segment(0, "Ahoj světe", vec![word("Ahoj", 0), word("svete", 250_000)]);
        let error = p
            .apply(EditCmd::AddCaptions { segments: vec![wrong], style: style.clone(), anim_in: None, anim_out: None })
            .unwrap_err();
        assert!(error.to_string().contains("Caption words must be the caption text"), "{error}");
        // Segments starting within a frame merge; the words of both stay, at their times.
        let merged = vec![
            segment(0, " Ahoj ", vec![word("Ahoj", 0)]),
            segment(10_000, "světe", vec![word("světe", 250_000)]),
            segment(1_000_000, "bez", vec![word("bez", 1_000_000)]),
            segment(1_010_000, "slov", Vec::new()),
        ];
        p.apply(EditCmd::AddCaptions { segments: merged, style, anim_in: None, anim_out: None }).unwrap();
        let captions = &p.tracks.iter().find(|t| t.is_captions()).unwrap().clips;
        let ClipContent::Text { text, words, .. } = &captions[0].content else { panic!() };
        assert_eq!(
            (text.as_str(), words.iter().map(|w| w.start_us).collect::<Vec<_>>()),
            ("Ahoj světe", vec![0, 250_000])
        );
        assert_eq!(spoken_at(&p, 300_000).as_deref(), Some("světe"));
        // A merged caption with only some of its words cannot highlight the right one, so it highlights none.
        let ClipContent::Text { text, words, .. } = &captions[1].content else { panic!() };
        assert_eq!((text.as_str(), words.len()), ("bez slov", 0));
    }

    #[test]
    fn correct_words_sets_replaces_and_removes_and_undoes_in_one_step() {
        let fix = |start_us: i64, original: &str, text: &str| WordCorrection {
            asset_id: "a".into(),
            source_start_us: start_us,
            original: original.into(),
            text: text.into(),
        };
        let mut editor = Editor::new(project());
        let before = editor.project.clone();
        editor
            .apply(
                EditCmd::CorrectWords { corrections: vec![fix(500_000, "oka", " okna "), fix(900_000, "to", "tu")] },
                None,
            )
            .unwrap();
        assert_eq!(editor.project.word_corrections, [fix(500_000, "oka", "okna"), fix(900_000, "to", "tu")]);
        // The same word again replaces its correction; its recognised text removes it.
        editor.apply(EditCmd::CorrectWords { corrections: vec![fix(500_000, "oka", "okno")] }, None).unwrap();
        editor.apply(EditCmd::CorrectWords { corrections: vec![fix(900_000, "to", "to")] }, None).unwrap();
        assert_eq!(editor.project.word_corrections, [fix(500_000, "oka", "okno")]);
        // Another word with the same start is another correction.
        editor.apply(EditCmd::CorrectWords { corrections: vec![fix(500_000, "okno", "okna")] }, None).unwrap();
        assert_eq!(editor.project.word_corrections.len(), 2);
        let unknown = WordCorrection { asset_id: "gone".into(), ..fix(0, "a", "b") };
        let kept = editor.project.clone();
        assert!(editor.apply(EditCmd::CorrectWords { corrections: vec![unknown] }, None).is_err());
        assert_eq!(editor.project, kept);
        for _ in 0..4 {
            editor.undo();
        }
        assert_eq!(editor.project, before);
    }

    #[test]
    fn projects_without_corrections_read_and_write_as_before() {
        let mut p = project();
        let json = serde_json::to_value(&p).unwrap();
        assert!(json.get("wordCorrections").is_none());
        assert_eq!(serde_json::from_value::<Project>(json).unwrap(), p);
        p.apply(EditCmd::CorrectWords {
            corrections: vec![WordCorrection {
                asset_id: "a".into(),
                source_start_us: 1,
                original: "oka".into(),
                text: "okna".into(),
            }],
        })
        .unwrap();
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(
            json["wordCorrections"],
            serde_json::json!([{"assetId": "a", "sourceStartUs": 1, "original": "oka", "text": "okna"}])
        );
        assert_eq!(serde_json::from_value::<Project>(json).unwrap(), p);
    }

    /// Main-track pieces as (start ms, end ms, scale).
    fn zoom_layout(p: &Project) -> Vec<(i64, i64, f32)> {
        p.tracks[0]
            .clips
            .iter()
            .map(|c| {
                let ClipContent::Media { transform, .. } = &c.content else { panic!() };
                (c.start_us / 1000, c.end_us() / 1000, (transform.scale * 1000.0).round() / 1000.0)
            })
            .collect()
    }

    fn changes(id: &str, fields: serde_json::Value) -> EditCmd {
        let mut cmd = serde_json::json!({"type": "updateClip", "clipId": id});
        cmd.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        serde_json::from_value(cmd).unwrap()
    }

    fn zoom(ranges: &[(i64, i64, f64)]) -> EditCmd {
        EditCmd::ZoomRanges {
            ranges: ranges.iter().map(|&(start_us, end_us, scale)| ZoomRange { start_us, end_us, scale }).collect(),
        }
    }

    #[test]
    fn zoom_ranges_split_at_the_edges_and_scale_only_inside() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(0), track_id: None }).unwrap();
        let style = TextStyle {
            font_family: None,
            font_size: 64.0,
            color: "#fff".into(),
            bold: false,
            stroke_width: 0.0,
            stroke_color: "#000".into(),
            background: None,
            max_width: None,
            highlight: None,
            keywords: None,
        };
        p.apply(EditCmd::AddText { start_us: 500_000, text: "Title".into(), style }).unwrap();
        let first = p.tracks[0].clips[0].id.clone();
        let framed = Transform { x: 0.1, y: -0.05, scale: 1.1, rotation: 5.0, ..Transform::default() };
        p.apply(changes(&first, serde_json::json!({"transform": framed}))).unwrap();
        let before = p.clone();
        // The second range starts 0.2 s before the cut: too little of the first clip to zoom, so
        // the punch-in starts at the cut.
        p.apply(zoom(&[(1_000_000, 2_500_000, 1.2), (4_800_000, 6_000_000, 1.25)])).unwrap();
        assert_eq!(
            zoom_layout(&p),
            [(0, 1000, 1.1), (1000, 2500, 1.32), (2500, 5000, 1.1), (5000, 6000, 1.25), (6000, 8000, 1.0)]
        );
        let ClipContent::Media { transform, .. } = &p.tracks[0].clips[1].content else { panic!() };
        assert_eq!((transform.x, transform.y, transform.rotation), (0.1, -0.05, 5.0));
        // Nothing but the main track changed, and its pieces play the same source back to back.
        assert_eq!(p.tracks[1..], before.tracks[1..]);
        let sources: Vec<i64> = p.tracks[0]
            .clips
            .iter()
            .map(|c| match &c.content {
                ClipContent::Media { source_in_us, .. } => *source_in_us,
                ClipContent::Text { .. } => panic!(),
            })
            .collect();
        assert_eq!(sources, [0, 1_000_000, 2_500_000, 0, 1_000_000]);
    }

    #[test]
    fn zoom_edges_move_to_the_clip_edge_instead_of_leaving_a_sliver() {
        let mut p = project();
        for _ in 0..2 {
            p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        }
        let (first, second) = (p.tracks[0].clips[0].id.clone(), p.tracks[0].clips[1].id.clone());
        let dissolve = Transition { kind: crate::model::TransitionKind::Dissolve, duration_us: 500_000 };
        p.apply(EditCmd::SetTransition { clip_id: second, transition: Some(dissolve) }).unwrap();
        p.apply(changes(&first, serde_json::json!({"fadeInUs": 1_000_000}))).unwrap();
        let fade = |p: &Project| match p.tracks[0].clips[0].content {
            ClipContent::Media { fade_in_us, .. } => fade_in_us,
            ClipContent::Text { .. } => panic!(),
        };
        // 0.1 s after a clip starts, and 0.2 s before it ends: both edges go to the cuts.
        let mut q = p.clone();
        q.apply(zoom(&[(5_100_000, 9_800_000, 1.2)])).unwrap();
        assert_eq!(zoom_layout(&q), [(0, 5000, 1.0), (5000, 10000, 1.2)]);
        // 1.5 s into a clip that fades in over 1 s would halve the fade, and 0.4 s before a 0.5 s
        // dissolve would shorten it: the zoom takes those pieces in.
        let mut q = p.clone();
        q.apply(zoom(&[(1_500_000, 4_600_000, 1.2)])).unwrap();
        assert_eq!(zoom_layout(&q), [(0, 5000, 1.2), (5000, 10000, 1.0)]);
        assert_eq!(fade(&q), 1_000_000);
        assert_eq!(q.tracks[0].clips[1].transition_in.unwrap().duration_us, 500_000);
        // A range that barely reaches into a clip leaves that clip alone.
        let mut q = p.clone();
        q.apply(zoom(&[(2_000_000, 5_150_000, 1.2)])).unwrap();
        assert_eq!(zoom_layout(&q), [(0, 2000, 1.0), (2000, 5000, 1.2), (5000, 10000, 1.0)]);
        assert_eq!(fade(&q), 1_000_000);
    }

    #[test]
    fn zoom_leaves_clips_with_keyframes_alone_and_names_them() {
        let mut e = Editor::new(project());
        for _ in 0..2 {
            e.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }, None).unwrap();
        }
        let keyed = e.project.tracks[0].clips[0].id.clone();
        let keyframes = vec![
            Keyframe { t_us: 0, transform: Transform::default(), ease: Ease::Linear },
            Keyframe {
                t_us: 4_000_000,
                transform: Transform { scale: 1.5, ..Transform::default() },
                ease: Ease::Linear,
            },
        ];
        e.apply(EditCmd::SetKeyframes { clip_id: keyed.clone(), keyframes }, None).unwrap();
        let before = e.project.clone();
        let outcome = e.apply(zoom(&[(1_000_000, 2_000_000, 1.2), (6_000_000, 8_000_000, 1.2)]), None).unwrap();
        assert_eq!(outcome.skipped, std::slice::from_ref(&keyed));
        assert_eq!(e.project.tracks[0].clips[0], before.tracks[0].clips[0]);
        assert_eq!(zoom_layout(&e.project), [(0, 5000, 1.0), (5000, 6000, 1.0), (6000, 8000, 1.2), (8000, 10000, 1.0)]);
        assert_eq!(serde_json::to_value(&outcome).unwrap()["skipped"], serde_json::json!([keyed]));
        // Two ranges, three new pieces: one undo step takes all of it back.
        assert!(e.undo());
        assert_eq!(e.project, before);
    }

    #[test]
    fn zoom_rejects_overlapping_or_unusable_ranges_and_changes_nothing() {
        let mut e = Editor::new(project());
        e.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }, None).unwrap();
        let before = e.project.clone();
        for bad in [
            zoom(&[]),
            zoom(&[(1_000_000, 3_000_000, 1.2), (2_000_000, 4_000_000, 1.2)]),
            zoom(&[(2_000_000, 1_000_000, 1.2)]),
            zoom(&[(-1, 1_000_000, 1.2)]),
            zoom(&[(1_000_000, 2_000_000, f64::NAN)]),
            zoom(&[(1_000_000, 2_000_000, 0.0)]),
            zoom(&[(1_000_000, 2_000_000, 9.0)]),
        ] {
            assert!(e.apply(bad.clone(), None).is_err(), "{bad:?}");
            assert_eq!(e.project, before);
        }
        assert!(!e.can_redo() && e.can_undo());
    }

    fn motion(clip_id: Option<&str>, range: Option<(i64, i64)>, kind: MotionKind, strength: f64) -> EditCmd {
        let range = range.map(|(start_us, end_us)| TimeRange { start_us, end_us });
        EditCmd::ApplyMotion { clip_id: clip_id.map(Into::into), range, kind, strength }
    }

    /// (time, zoom against the clip's own scale, x, y) of each keyframe of main-track clip `i`.
    fn motion_keys(p: &Project, i: usize) -> Vec<(i64, f32, f32, f32)> {
        let clip = &p.tracks[0].clips[i];
        assert!(clip.keyframes.iter().all(|k| k.ease == Ease::Smooth));
        let ClipContent::Media { transform: base, .. } = &clip.content else { panic!() };
        clip.keyframes.iter().map(|k| (k.t_us, k.transform.scale / base.scale, k.transform.x, k.transform.y)).collect()
    }

    fn assert_keys(found: Vec<(i64, f32, f32, f32)>, expected: [(i64, f32, f32, f32); 2]) {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(
            found.len() == 2
                && found
                    .iter()
                    .zip(&expected)
                    .all(|(f, e)| f.0 == e.0 && close(f.1, e.1) && close(f.2, e.2) && close(f.3, e.3)),
            "{found:?} != {expected:?}"
        );
    }

    #[test]
    fn motion_zooms_about_the_safe_area_centre_on_a_smooth_curve_and_skips_keyed_clips() {
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        let (first, second) = (p.tracks[0].clips[0].id.clone(), p.tracks[0].clips[1].id.clone());
        let framed = Transform { x: 0.1, y: -0.05, scale: 1.2, rotation: 5.0, opacity: 0.9, crop: None };
        p.apply(changes(&first, serde_json::json!({"transform": framed}))).unwrap();

        p.apply(motion(Some(&first), None, MotionKind::PushIn, 0.1)).unwrap();
        let keys = &p.tracks[0].clips[0].keyframes;
        assert_eq!(keys.iter().map(|k| k.t_us).collect::<Vec<_>>(), [0, 5_000_000]);
        assert_eq!(keys[0].transform, framed);
        let end = keys[1].transform;
        assert!((end.scale - 1.32).abs() < 1e-6 && end.rotation == 5.0 && end.opacity == 0.9);
        // The middle of the 1080×1920 safe area (60–900 px across, 250–1420 px down) stays where
        // it is: the same point of the picture lies there before and after the zoom.
        let (fx, fy) = (480.0 / 1080.0 - 0.5, 835.0 / 1920.0 - 0.5);
        let under = |t: Transform| ((fx - t.x) / t.scale, (fy - t.y) / t.scale);
        let (before, after) = (under(framed), under(end));
        assert!((before.0 - after.0).abs() < 1e-6 && (before.1 - after.1).abs() < 1e-6, "{before:?} {after:?}");

        // A clip with keyframes keeps them and is named.
        let keyed = p.tracks[0].clips[0].keyframes.clone();
        let out = p.apply(motion(Some(&first), None, MotionKind::KenBurns, 0.15)).unwrap();
        assert_eq!((out.skipped, &p.tracks[0].clips[0].keyframes), (vec![first.clone()], &keyed));

        // Pull out over part of a clip: zoomed in at the start of the range, the clip's own framing at its end.
        p.apply(motion(Some(&second), Some((6_000_000, 7_000_000)), MotionKind::PullOut, 0.15)).unwrap();
        assert_keys(motion_keys(&p, 1), [(1_000_000, 1.15, fx * -0.15, fy * -0.15), (2_000_000, 1.0, 0.0, 0.0)]);

        // Without a clip, every main-track clip under the range moves as one motion: both get
        // keyframes at the ends of the range, so halfway through it, at the cut, the first clip
        // ends half zoomed in and the second starts there. Ken Burns zooms about the right edge
        // of the safe area.
        let mut p = project();
        p.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }).unwrap();
        p.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: None, track_id: None }).unwrap();
        let out = p.apply(motion(None, Some((4_000_000, 6_000_000)), MotionKind::KenBurns, 0.1)).unwrap();
        assert!(out.skipped.is_empty());
        let right = 900.0 / 1080.0 - 0.5;
        let zoomed = (1.1, right * -0.1, fy * -0.1);
        assert_keys(motion_keys(&p, 0), [(4_000_000, 1.0, 0.0, 0.0), (6_000_000, zoomed.0, zoomed.1, zoomed.2)]);
        assert_keys(motion_keys(&p, 1), [(-1_000_000, 1.0, 0.0, 0.0), (1_000_000, zoomed.0, zoomed.1, zoomed.2)]);
        let (last, first) = (&p.tracks[0].clips[0], &p.tracks[0].clips[1]);
        let at = |clip: &Clip, t: i64| crate::effects::transform_at(clip, t).0;
        let (ending, starting) = (at(last, last.end_us() - 1), at(first, first.start_us));
        assert!((ending.scale - 1.05).abs() < 1e-4 && (starting.scale - 1.05).abs() < 1e-4, "{ending:?} {starting:?}");
        assert!((at(last, 4_500_000).scale - 1.015625).abs() < 1e-6, "one smooth curve over the whole range");
        // Once the clip before is gone, the second clip's range starts before 0 and still rewrites.
        let id = first.id.clone();
        p.apply(EditCmd::DeleteClips { clip_ids: vec![last.id.clone()] }).unwrap();
        p.apply(EditCmd::SetKeyframes { clip_id: id.clone(), keyframes: vec![] }).unwrap();
        p.apply(motion(Some(&id), Some((-1_000_000, 1_000_000)), MotionKind::PushIn, 0.06)).unwrap();
        assert_eq!(p.tracks[0].clips[0].keyframes.iter().map(|k| k.t_us).collect::<Vec<_>>(), [-1_000_000, 1_000_000]);

        // Other canvases have no safe area: the canvas centre, and its right edge for Ken Burns.
        p.apply(EditCmd::SetCanvas { width: 1920, height: 1080, background: None, background_blur: None }).unwrap();
        p.apply(EditCmd::SetKeyframes { clip_id: id.clone(), keyframes: vec![] }).unwrap();
        p.apply(motion(Some(&id), None, MotionKind::KenBurns, 0.1)).unwrap();
        assert_keys(motion_keys(&p, 0), [(0, 1.0, 0.0, 0.0), (3_000_000, 1.1, -0.05, 0.0)]);
    }

    #[test]
    fn motion_refuses_text_sound_and_bad_ranges_and_changes_nothing() {
        let mut e = Editor::new(project());
        e.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }, None).unwrap();
        e.apply(EditCmd::AddClip { asset_id: "m".into(), start_us: Some(0), track_id: None }, None).unwrap();
        let style: TextStyle = serde_json::from_value(serde_json::json!({"fontSize":64.0,"color":"#ffffff"})).unwrap();
        e.apply(EditCmd::AddText { start_us: 0, text: "Title".into(), style }, None).unwrap();
        let id =
            |e: &Editor, kind: TrackKind| e.project.tracks.iter().find(|t| t.kind == kind).unwrap().clips[0].id.clone();
        let (video, sound, text) = (id(&e, TrackKind::Video), id(&e, TrackKind::Audio), id(&e, TrackKind::Text));
        let before = e.project.clone();
        for bad in [
            motion(Some(&text), None, MotionKind::PushIn, 0.1),
            motion(Some(&sound), None, MotionKind::PushIn, 0.1),
            motion(Some("missing"), None, MotionKind::PushIn, 0.1),
            motion(Some(&video), Some((6_000_000, 7_000_000)), MotionKind::PushIn, 0.1),
            motion(Some(&video), Some((2_000_000, 1_000_000)), MotionKind::PushIn, 0.1),
            motion(None, None, MotionKind::PushIn, 0.1),
            motion(None, Some((6_000_000, 7_000_000)), MotionKind::PushIn, 0.1),
            motion(Some(&video), None, MotionKind::PushIn, 0.0),
            motion(Some(&video), None, MotionKind::PushIn, 10.0),
            motion(Some(&video), None, MotionKind::PushIn, f64::NAN),
        ] {
            assert!(e.apply(bad.clone(), None).is_err(), "{bad:?}");
            assert_eq!(e.project, before);
        }
    }

    /// Split halves play on from each other, so zooming changes no sample of the mix, fades,
    /// volume and the dissolve into the next clip included.
    #[test]
    fn zoom_keeps_the_sound_sample_for_sample() {
        use crate::audio::{Mixer, pcm_path};
        use crate::model::CHANNELS;
        let cache = std::env::temp_dir().join(format!("nuzky-zoom-sound-{}", new_id()));
        std::fs::create_dir_all(cache.join("pcm")).unwrap();
        let mut p = project();
        for id in ["a", "b"] {
            let asset = p.asset(id).unwrap().clone();
            let frames = (asset.duration_us * 48 / 1000) as usize;
            // A chirp, so any shift or step between pieces shows.
            let samples: Vec<f32> = (0..frames)
                .flat_map(|i| {
                    let t = i as f32 / 48_000.0;
                    [0.4 * (t * (300.0 + 200.0 * t) * std::f32::consts::TAU).sin(); CHANNELS]
                })
                .collect();
            std::fs::write(pcm_path(&cache, &asset), bytemuck::cast_slice(&samples)).unwrap();
            p.apply(EditCmd::AddClip { asset_id: id.into(), start_us: None, track_id: None }).unwrap();
        }
        let (first, second) = (p.tracks[0].clips[0].id.clone(), p.tracks[0].clips[1].id.clone());
        p.apply(changes(&first, serde_json::json!({"volume": 0.8, "fadeInUs": 300_000, "fadeOutUs": 200_000})))
            .unwrap();
        let dissolve = Transition { kind: crate::model::TransitionKind::Dissolve, duration_us: 400_000 };
        p.apply(EditCmd::SetTransition { clip_id: second, transition: Some(dissolve) }).unwrap();
        let mix = |p: &Project| {
            let mut out = vec![0.0; 8 * 48_000 * CHANNELS];
            Mixer::new(cache.clone()).mix(p, 0, &mut out);
            out
        };
        let before = mix(&p);
        p.apply(zoom(&[(1_234_567, 2_345_678, 1.2), (4_100_000, 5_900_000, 1.3), (6_500_000, 7_000_000, 1.15)]))
            .unwrap();
        assert_eq!(p.tracks[0].clips.len(), 8, "{:?}", zoom_layout(&p));
        let after = mix(&p);
        std::fs::remove_dir_all(&cache).unwrap();
        let difference = before.iter().zip(&after).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(before.iter().any(|s| s.abs() > 0.1), "the mix is silent");
        assert!(difference < 1e-6, "zooming changed the sound by {difference}");
    }
}
