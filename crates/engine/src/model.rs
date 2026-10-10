//! Project file format. Everything the renderer needs to produce a frame lives here,
//! so the same JSON renders identically in the app and in the CLI.
//!
//! Times are integer microseconds on the timeline (`*_us`).

use serde::{Deserialize, Serialize};

pub const PROJECT_VERSION: u32 = 1;
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub canvas: Canvas,
    pub assets: Vec<Asset>,
    /// Compositing order: index 0 is the bottom layer (the main track).
    pub tracks: Vec<Track>,
    /// Recognised words the user corrected. Transcripts belong to the media files and are
    /// shared by every project, so the corrections live here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub word_corrections: Vec<WordCorrection>,
    /// Covers and thumbnails of the video, at most one per format.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thumbnails: Vec<Thumbnail>,
    /// Moments of a long video that could each be a reel of their own, in timeline order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reel_candidates: Vec<ReelCandidate>,
}

/// A moment an agent proposed as a reel. It is the transcript's words `from` to `to`, inclusive, which played
/// from `start_us` to `end_us` when it was proposed; a reel is made of it only while they still do.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReelCandidate {
    pub id: String,
    pub from: usize,
    pub to: usize,
    pub start_us: i64,
    pub end_us: i64,
    pub title: String,
    /// The sentence the reel opens with.
    pub hook: String,
    /// Why it works on its own.
    pub why: String,
    /// How long the reel is once made: the words, the silence kept around them and pauses shortened as cuts do.
    pub duration_us: i64,
    /// 0 to 1, higher is better.
    pub score: f32,
    pub status: ReelStatus,
    /// The reel's own project, once made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    /// The reel's cover; nothing makes it yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<Thumbnail>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReelStatus {
    Proposed,
    Picked,
    Rejected,
    /// Its project exists at `project_path`.
    Made,
}

/// The longest title, reason and hook of a reel candidate, in characters.
pub const MAX_REEL_TITLE_CHARS: usize = 100;
pub const MAX_REEL_WHY_CHARS: usize = 300;
pub const MAX_REEL_HOOK_CHARS: usize = 1000;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ThumbnailFormat {
    /// Reels, TikTok and Shorts cover, 1080 × 1920.
    #[serde(rename = "cover_9x16")]
    Cover9x16,
    /// YouTube thumbnail, 1280 × 720.
    #[serde(rename = "youtube_16x9")]
    Youtube16x9,
}

impl ThumbnailFormat {
    pub fn size(self) -> (u32, u32) {
        match self {
            ThumbnailFormat::Cover9x16 => (1080, 1920),
            ThumbnailFormat::Youtube16x9 => (1280, 720),
        }
    }
}

/// A still made from one frame of the video: a background, the person cut out of the frame, an
/// outline around them and text, some of it behind them. Sizes are in thumbnail pixels.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Thumbnail {
    pub format: ThumbnailFormat,
    /// The frame, as the video shows it at this timeline time without its text tracks.
    pub time_us: i64,
    /// Where the frame sits, as for clips: scale 1 fits the whole frame inside the thumbnail, x and y move its
    /// centre by fractions of the thumbnail, crop cuts its edges without moving the rest.
    #[serde(default)]
    pub frame: Transform,
    #[serde(default)]
    pub background: ThumbnailBackground,
    /// Bottom to top.
    #[serde(default)]
    pub texts: Vec<ThumbnailText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<ThumbnailOutline>,
}

impl Thumbnail {
    /// The person is cut out of the frame whenever something goes between them and the rest of the
    /// picture: text behind them, an outline, or a background that is not the frame as it is.
    pub fn needs_mask(&self) -> bool {
        let b = &self.background;
        self.texts.iter().any(|t| t.behind) || self.outline.is_some() || !b.picture || b.blur > 0.0 || b.dim > 0.0
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ThumbnailBackground {
    /// A copy of the frame behind the person: the frame as placed, enlarged about its centre until it covers
    /// the thumbnail. False shows only `color`.
    pub picture: bool,
    /// Blur of that copy, 0 (sharp) to 1 (strongest).
    pub blur: f32,
    /// How much darker the background gets, 0 (unchanged) to 1 (black).
    pub dim: f32,
    /// `#rrggbb` under everything.
    pub color: String,
}

impl Default for ThumbnailBackground {
    fn default() -> Self {
        Self { picture: true, blur: 0.0, dim: 0.0, color: "#000000".into() }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailText {
    pub text: String,
    /// Font size and outline in thumbnail pixels; lines wrap at 90% of the thumbnail width by default.
    pub style: TextStyle,
    #[serde(default)]
    pub transform: Transform,
    /// Drawn behind the person, so their head can cover part of it.
    #[serde(default)]
    pub behind: bool,
}

/// A solid line around the person, like a sticker's edge.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailOutline {
    /// `#rrggbb` or `#rrggbbaa`
    pub color: String,
    /// Thumbnail pixels, up to 100.
    pub width: f32,
}

/// Longest corrected word, in characters.
pub const MAX_CORRECTION_CHARS: usize = 100;

/// A word as the user corrected it. It applies only while the file's transcript has a word that
/// starts at `source_start_us` with the `original` text: recognised again differently, the
/// correction does nothing, and it never moves to another word.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WordCorrection {
    pub asset_id: String,
    /// Where the word starts in its media file.
    pub source_start_us: i64,
    /// The word as recognised.
    pub original: String,
    pub text: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// `#rrggbb`
    pub background: String,
    /// 0 keeps the solid background. Above 0 a blurred, canvas-filling copy of the
    /// main-track frame shows behind it (CapCut "Canvas: Blur"); 1 is the strongest blur.
    #[serde(default)]
    pub background_blur: f32,
}

/// Taller than this is a vertical video, shown in Reels and TikTok under their interface.
const VERTICAL_MIN_RATIO: f32 = 1.7;

/// Where Instagram Reels and TikTok draw nothing over a vertical video, in canvas pixels: clear of
/// the top bar, the like and comment rail on the right and the caption and buttons at the bottom.
/// Conservative values that suit both apps, from 250 / 180 / 500 / 60 px of 1080×1920.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SafeArea {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl SafeArea {
    /// The widest text centred on the canvas that stays inside.
    pub fn centered_width(&self, canvas_width: f32) -> f32 {
        let center = canvas_width / 2.0;
        2.0 * (center - self.left).min(self.right - center)
    }
}

impl Canvas {
    /// The app interface's free area on vertical videos; other formats have nothing over them.
    pub fn safe_area(&self) -> Option<SafeArea> {
        let (w, h) = (self.width as f32, self.height as f32);
        (h >= w * VERTICAL_MIN_RATIO).then(|| SafeArea {
            left: w * 60.0 / 1080.0,
            top: h * 250.0 / 1920.0,
            right: w - w * 180.0 / 1080.0,
            bottom: h - h * 500.0 / 1920.0,
        })
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum AssetKind {
    Video,
    Audio,
    Image,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    pub name: String,
    pub path: String,
    pub kind: AssetKind,
    /// 0 for still images.
    pub duration_us: i64,
    /// Display size, after applying rotation metadata.
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_audio: bool,
    /// Clockwise rotation from container metadata (0, 90, 180, 270).
    #[serde(default)]
    pub rotation: u32,
    /// Mirrored left to right after the rotation, as front-camera photos can ask in EXIF.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mirror: bool,
    /// Where a sound from the sound library came from, so the video can credit it. None for the
    /// user's own files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<Credit>,
}

/// The licences the sound library offers: both allow monetized videos.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum License {
    /// Public domain dedication; no credit needed.
    #[serde(rename = "cc0")]
    Cc0,
    /// Creative Commons Attribution: the video must credit the author.
    #[serde(rename = "by")]
    CcBy,
}

/// A sound from the library: its source, author and licence.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Credit {
    /// "nuzky" for the sounds built into Nuzky, "openverse" or "freesound".
    pub source: String,
    /// The sound's id at its source.
    pub id: String,
    pub title: String,
    pub author: String,
    pub license: License,
    /// Such as "4.0" or "1.0".
    pub license_version: String,
    /// The licence's page at Creative Commons.
    pub license_url: String,
    /// The sound's page at its source; empty for built-in sounds.
    #[serde(default)]
    pub url: String,
}

impl Credit {
    /// "CC BY 4.0".
    pub fn license_name(&self) -> String {
        match self.license {
            License::Cc0 => format!("CC0 {}", self.license_version),
            License::CcBy => format!("CC BY {}", self.license_version),
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    Video,
    Audio,
    Text,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub kind: TrackKind,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub hidden: bool,
    /// Ripple cuts leave this track alone, so music keeps playing across them. On for tracks
    /// made for audio files, off for everything else, including sound detached from a video.
    #[serde(default)]
    pub keep_in_place: bool,
    pub clips: Vec<Clip>,
}

/// Generated captions live on text tracks with this name; other text tracks hold titles.
pub const CAPTIONS_TRACK: &str = "Captions";

impl Track {
    pub fn is_captions(&self) -> bool {
        self.kind == TrackKind::Text && self.name == CAPTIONS_TRACK
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    pub start_us: i64,
    pub duration_us: i64,
    pub content: ClipContent,
    /// Entry animation over the first `duration_us` of the clip.
    #[serde(default)]
    pub anim_in: Option<Animation>,
    /// Exit animation over the last `duration_us` of the clip.
    #[serde(default)]
    pub anim_out: Option<Animation>,
    /// Transform keyframes, `t_us` relative to the clip start. When present they replace
    /// the content transform and are interpolated along each keyframe's `ease`; outside the
    /// range the nearest keyframe holds.
    #[serde(default)]
    pub keyframes: Vec<Keyframe>,
    /// Main track only: transition from the previous clip, centred on the cut.
    #[serde(default)]
    pub transition_in: Option<Transition>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AnimationKind {
    Fade,
    ZoomIn,
    ZoomOut,
    SlideUp,
    SlideDown,
    SlideLeft,
    SlideRight,
    Pop,
    /// Text only; media clips treat it as Fade.
    Typewriter,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Animation {
    pub kind: AnimationKind,
    pub duration_us: i64,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Keyframe {
    pub t_us: i64,
    pub transform: Transform,
    /// How the values move on to the next keyframe.
    #[serde(default)]
    pub ease: Ease,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Ease {
    /// At an even pace.
    #[default]
    Linear,
    /// Starts and ends gently, like a camera move (smoothstep).
    Smooth,
}

impl Ease {
    /// The share of the way to the next keyframe at `p` of the time between them, both 0..=1.
    pub fn at(self, p: f32) -> f32 {
        match self {
            Ease::Linear => p,
            Ease::Smooth => p * p * (3.0 - 2.0 * p),
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TransitionKind {
    Dissolve,
    FadeBlack,
    FadeWhite,
    SlideLeft,
    SlideUp,
    ZoomIn,
    WipeLeft,
    Blur,
}

/// Plays across [cut - d/2, cut + d/2]. The outgoing clip continues past its out point
/// (holding its last frame when the source ends) and the incoming clip starts early
/// (holding its first frame), so the timeline length does not change. Audio crossfades.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub kind: TransitionKind,
    pub duration_us: i64,
}

/// Colour adjustments, all 0 by default (no change). Ranges are -1..=1 except fade and vignette 0..=1.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Adjust {
    /// Linear-light exposure, -2 to +2 stops.
    pub exposure: f32,
    /// Negative shifts green, positive shifts magenta.
    pub tint: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub fade: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    /// Negative cools (blue), positive warms (orange).
    pub temperature: f32,
    pub vignette: f32,
}

impl Clip {
    pub fn new(id: String, start_us: i64, duration_us: i64, content: ClipContent) -> Self {
        Self {
            id,
            start_us,
            duration_us,
            content,
            anim_in: None,
            anim_out: None,
            keyframes: Vec::new(),
            transition_in: None,
        }
    }

    pub fn end_us(&self) -> i64 {
        self.start_us + self.duration_us
    }

    pub fn contains(&self, t_us: i64) -> bool {
        t_us >= self.start_us && t_us < self.end_us()
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClipContent {
    #[serde(rename_all = "camelCase")]
    Media {
        asset_id: String,
        source_in_us: i64,
        #[serde(default = "one")]
        volume: f32,
        #[serde(default)]
        transform: Transform,
        /// Playback speed; the clip covers `duration_us * speed` of source. Sound follows it.
        #[serde(default = "one")]
        speed: f32,
        /// Sound at another speed keeps its pitch; off, it gets higher when faster and lower
        /// when slower. Projects from before it have it off, so they sound as they did.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        keep_pitch: bool,
        #[serde(default)]
        adjust: Adjust,
        /// Audio fades at the clip edges.
        #[serde(default)]
        fade_in_us: i64,
        #[serde(default)]
        fade_out_us: i64,
        /// Clean voice: a high-pass against rumble and hum, gentle noise reduction and a de-esser,
        /// for speech recorded on a phone. Only clips with sound. Playback uses the original sound
        /// until the cleaned sound is prepared; export always has the cleaned sound.
        #[serde(default)]
        clean_voice: bool,
        /// Rounded corners, a border and a shadow around the picture, as for a picture in picture.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shape: Option<Shape>,
        /// Ducking, for music under speech: how many dB the clip goes down while someone speaks in
        /// a video's own sound, coming back in the pauses. 0 turns it off.
        #[serde(default, skip_serializing_if = "is_zero")]
        duck_db: f32,
        /// Blur or replace what is behind the person (video and image clips).
        #[serde(default, skip_serializing_if = "Background::is_none")]
        background: Background,
    },
    #[serde(rename_all = "camelCase")]
    Text {
        text: String,
        style: TextStyle,
        #[serde(default)]
        transform: Transform,
        /// The spoken words of a generated caption, in order, so `style.highlight` can mark the one
        /// being said. They count only while `text` is exactly their texts joined by single spaces;
        /// after a manual edit of the text nothing is highlighted.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        words: Vec<CaptionWord>,
    },
}

/// One spoken word of a caption. Times are relative to the clip start, like keyframes, so moving
/// the clip keeps them; a trim may leave some outside the clip, where they never show.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaptionWord {
    pub text: String,
    pub start_us: i64,
    pub end_us: i64,
}

/// Byte range in `text` of the word spoken at `t_us` (clip time), or `None` in a gap, outside the
/// words, for zero-length words and when `text` is not the words joined by single spaces.
pub fn spoken_word(text: &str, words: &[CaptionWord], t_us: i64) -> Option<std::ops::Range<usize>> {
    let mut at = 0;
    let mut spoken = None;
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            at += text.get(at..)?.strip_prefix(' ').map(|_| 1)?;
        }
        let end = at + word.text.len();
        if text.get(at..end)? != word.text {
            return None;
        }
        if word.start_us <= t_us && t_us < word.end_us {
            spoken = Some(at..end);
        }
        at = end;
    }
    (at == text.len()).then_some(spoken).flatten()
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transform {
    /// Offset of the layer centre from the canvas centre, as a fraction of canvas width.
    pub x: f32,
    /// Offset of the layer centre from the canvas centre, as a fraction of canvas height.
    pub y: f32,
    /// 1.0 fits media inside the canvas; text is drawn at its font size.
    pub scale: f32,
    /// Degrees, clockwise.
    pub rotation: f32,
    pub opacity: f32,
    /// Edges cut off the layer. Part of the transform, so keyframes can animate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<Crop>,
}

impl Default for Transform {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, scale: 1.0, rotation: 0.0, opacity: 1.0, crop: None }
    }
}

/// How much of each edge is cut off, as a fraction of the layer's width (left, right) or height (top,
/// bottom), on the picture as it shows after rotation and mirroring. The rest stays where it was.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Crop {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Crop {
    /// The part left visible as left, top, right and bottom edges in 0..1.
    pub fn visible(crop: Option<Crop>) -> [f32; 4] {
        let c = crop.unwrap_or_default();
        [c.left, c.top, 1.0 - c.right, 1.0 - c.bottom]
    }
}

/// The edge of a video or image layer.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Shape {
    /// Corner radius as a share of half the shorter visible side, 0 to 1. 1 rounds the shorter sides
    /// fully: a circle when the crop leaves a square.
    pub radius: f32,
    /// Canvas pixels, drawn outside the edge; 0 draws no border.
    pub border_width: f32,
    /// `#rrggbb` or `#rrggbbaa`
    pub border_color: String,
    /// A soft shadow under the layer, 0 (none) to 1.
    pub shadow: f32,
}

impl Default for Shape {
    fn default() -> Self {
        Self { radius: 0.0, border_width: 0.0, border_color: "#ffffff".into(), shadow: 0.0 }
    }
}

/// Widest border, in canvas pixels.
pub const MAX_BORDER_WIDTH: f32 = 100.0;

/// What shows behind the person in a video or image clip. Anything but `None` cuts the person out of
/// every frame with an outline found once per file and fills the rest of the clip's picture.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Background {
    /// The picture as recorded.
    #[default]
    None,
    /// The clip's own picture blurred behind the person, 0 (lightest) to 1 (strongest).
    Blur { strength: f32 },
    /// `#rrggbb` behind the person.
    Color { color: String },
    /// An image of the project behind the person, covering the clip's picture.
    #[serde(rename_all = "camelCase")]
    Image { asset_id: String },
}

impl Background {
    pub fn is_none(&self) -> bool {
        *self == Background::None
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextStyle {
    /// Font family name; `None` uses the bundled default, Inter.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub font_family: Option<String>,
    /// Pixels at canvas resolution.
    pub font_size: f32,
    /// `#rrggbb`
    pub color: String,
    #[serde(default)]
    pub bold: bool,
    /// Outline width in canvas pixels, 0 disables it.
    #[serde(default)]
    pub stroke_width: f32,
    #[serde(default = "black")]
    pub stroke_color: String,
    /// `#rrggbbaa` box behind the text.
    #[serde(default)]
    pub background: Option<String>,
    /// Lines wrap at this width in canvas pixels; `None` wraps at 90% of the canvas width. Generated captions on
    /// vertical videos keep to the Reels and TikTok safe area.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<f32>,
    /// `#rrggbb` fill of the word being spoken (karaoke captions); `None` draws every word in `color`.
    /// Needs the clip's `words`; outline, box, size and wrapping stay the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight: Option<String>,
}

/// Oversized titles may extend beyond the canvas; these bounds still reject unbounded allocations.
pub const MAX_FONT_HEIGHT_RATIO: f32 = 2.0;
pub const MAX_TEXT_WIDTH_RATIO: f32 = 4.0;

/// Outlines may be as wide as the text is tall, and up to the inspector's 20 px on any size.
pub fn max_stroke_width(font_size: f32) -> f32 {
    font_size.max(20.0)
}

impl TextStyle {
    /// Older project files can bypass session validation when rendered by the CLI.
    pub fn bounded(&self, canvas: &Canvas) -> Self {
        let mut style = self.clone();
        let finite = |value: f32, fallback: f32| if value.is_finite() { value } else { fallback };
        style.font_size = finite(style.font_size, 1.0).clamp(1.0, MAX_FONT_HEIGHT_RATIO * canvas.height.max(1) as f32);
        style.stroke_width = finite(style.stroke_width, 0.0).clamp(0.0, max_stroke_width(style.font_size));
        style.max_width = style.max_width.map(|width| {
            finite(width, canvas.width as f32 * 0.9).clamp(1.0, MAX_TEXT_WIDTH_RATIO * canvas.width.max(1) as f32)
        });
        style
    }
}

fn one() -> f32 {
    1.0
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

fn black() -> String {
    "#000000".into()
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: PROJECT_VERSION,
            name: name.into(),
            canvas: Canvas { width: 1080, height: 1920, fps: 30, background: "#000000".into(), background_blur: 0.0 },
            assets: Vec::new(),
            tracks: vec![Track {
                id: "main".into(),
                kind: TrackKind::Video,
                name: "Main".into(),
                muted: false,
                hidden: false,
                keep_in_place: false,
                clips: Vec::new(),
            }],
            word_corrections: Vec::new(),
            thumbnails: Vec::new(),
            reel_candidates: Vec::new(),
        }
    }

    pub fn thumbnail(&self, format: ThumbnailFormat) -> Option<&Thumbnail> {
        self.thumbnails.iter().find(|t| t.format == format)
    }

    pub fn asset(&self, id: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// End of the last clip on any track.
    /// Where the video ends: at the last picture or text, or at the last sound when there is
    /// no picture. Music running past the picture is cut, as in CapCut, so an export never ends
    /// on black.
    pub fn duration_us(&self) -> i64 {
        let end = |audio: bool| {
            self.tracks
                .iter()
                .filter(|t| (t.kind == TrackKind::Audio) == audio)
                .flat_map(|t| t.clips.iter().map(Clip::end_us))
                .max()
        };
        end(false).or(end(true)).unwrap_or(0)
    }

    pub fn frame_duration_us(&self) -> f64 {
        1_000_000.0 / self.canvas.fps.max(1) as f64
    }
}

/// Parses `#rgb`, `#rrggbb` or `#rrggbbaa` into straight RGBA in 0..1. Anything else,
/// including text with non-hex characters, is opaque white.
pub fn parse_color(s: &str) -> [f32; 4] {
    let hex = s.trim().trim_start_matches('#');
    let digits: Option<Vec<u8>> = hex.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect();
    let digits = digits.unwrap_or_default();
    let byte = |i: usize| digits[i] * 16 + digits[i + 1];
    let (r, g, b, a) = match digits.len() {
        3 => (digits[0] * 17, digits[1] * 17, digits[2] * 17, 255),
        6 => (byte(0), byte(2), byte(4), 255),
        8 => (byte(0), byte(2), byte(4), byte(6)),
        _ => (255, 255, 255, 255),
    };
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_round_trips_through_json() {
        let mut p = Project::new("Test");
        p.tracks[0].clips.push(Clip {
            id: "c1".into(),
            start_us: 0,
            duration_us: 2_000_000,
            anim_in: Some(Animation { kind: AnimationKind::Pop, duration_us: 400_000 }),
            anim_out: None,
            keyframes: Vec::new(),
            transition_in: None,
            content: ClipContent::Text {
                text: "Ahoj světe".into(),
                style: TextStyle {
                    font_family: None,
                    font_size: 80.0,
                    color: "#ffffff".into(),
                    bold: true,
                    stroke_width: 6.0,
                    stroke_color: "#000000".into(),
                    background: None,
                    max_width: None,
                    highlight: Some("#ffe14d".into()),
                },
                transform: Transform::default(),
                words: vec![
                    CaptionWord { text: "Ahoj".into(), start_us: 0, end_us: 400_000 },
                    CaptionWord { text: "světe".into(), start_us: 450_000, end_us: 900_000 },
                ],
            },
        });
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains(r##""highlight":"#ffe14d""##) && json.contains(r#""words":[{"text":"Ahoj","startUs":0"#));
        assert_eq!(serde_json::from_str::<Project>(&json).unwrap(), p);
    }

    #[test]
    fn text_without_words_or_highlight_saves_as_before() {
        let old = r##"{"type":"text","text":"Ahoj","style":{"fontFamily":null,"fontSize":95.0,"color":"#ffffff","bold":false,"strokeWidth":7.5,"strokeColor":"#000000","background":null},"transform":{"x":0.0,"y":0.15,"scale":1.0,"rotation":0.0,"opacity":1.0}}"##;
        let content: ClipContent = serde_json::from_str(old).unwrap();
        let ClipContent::Text { words, style, .. } = &content else { panic!() };
        assert!(words.is_empty() && style.highlight.is_none());
        assert_eq!(serde_json::to_string(&content).unwrap(), old);
    }

    #[test]
    fn media_without_crop_or_shape_saves_as_before() {
        let old = r#"{"type":"media","assetId":"a","sourceInUs":0,"volume":1.0,"transform":{"x":0.0,"y":0.0,"scale":1.0,"rotation":0.0,"opacity":1.0},"speed":1.0,"adjust":{"exposure":0.0,"tint":0.0,"highlights":0.0,"shadows":0.0,"fade":0.0,"brightness":0.0,"contrast":0.0,"saturation":0.0,"temperature":0.0,"vignette":0.0},"fadeInUs":0,"fadeOutUs":0,"cleanVoice":false}"#;
        let content: ClipContent = serde_json::from_str(old).unwrap();
        assert_eq!(serde_json::to_string(&content).unwrap(), old);
        let ClipContent::Media { mut transform, .. } = content else { panic!() };
        transform.crop = Some(Crop { left: 0.25, ..Crop::default() });
        let shape = Some(Shape { radius: 1.0, border_width: 8.0, border_color: "#ff0000".into(), shadow: 0.5 });
        let json = serde_json::json!({"transform": transform, "shape": shape});
        assert_eq!(
            json["transform"]["crop"],
            serde_json::json!({"left": 0.25, "top": 0.0, "right": 0.0, "bottom": 0.0})
        );
        assert_eq!(serde_json::from_value::<Option<Shape>>(json["shape"].clone()).unwrap(), shape);
    }

    #[test]
    fn spoken_word_follows_the_words_and_ignores_edited_text() {
        let word = |text: &str, start_us, end_us| CaptionWord { text: text.into(), start_us, end_us };
        let words = [word("Příliš", 0, 300), word("žluťoučký", 400, 800), word("kůň", 800, 800)];
        let text = "Příliš žluťoučký kůň";
        let at = |t| spoken_word(text, &words, t).map(|r| &text[r]);
        assert_eq!(at(0), Some("Příliš"));
        assert_eq!(at(299), Some("Příliš"));
        // The gap, the end of a word, a zero-length word and the time outside the words.
        for t in [-1, 300, 399, 800, 801, 5_000] {
            assert_eq!(at(t), None, "{t}");
        }
        assert_eq!(at(400), Some("žluťoučký"));
        assert_eq!(at(799), Some("žluťoučký"));
        // A manual edit, a cut inside a character, a double space or a missing word: nothing.
        for edited in
            ["Příliš žluťoučký kůň!", "Prilis zlutoucky kun", "Příliš  žluťoučký kůň", "Příliš žluťoučký", "Pří"]
        {
            assert_eq!(spoken_word(edited, &words, 500), None, "{edited}");
        }
        assert_eq!(spoken_word("", &[], 0), None);
    }

    #[test]
    fn parses_colors() {
        assert_eq!(parse_color("#ff0000"), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(parse_color("#00000080")[3], 128.0 / 255.0);
        assert_eq!(parse_color("#fff"), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(parse_color(" #0a0 "), [0.0, 170.0 / 255.0, 0.0, 1.0]);
    }

    #[test]
    fn malformed_colors_fall_back_to_white_without_panicking() {
        for color in ["#€", "€", "#é1", "#ffé", "#ff00zz", "#+f+f+f", "#ff00€0", "", "#", "#ff00", "red"] {
            assert_eq!(parse_color(color), [1.0; 4], "{color:?}");
        }
    }
}
