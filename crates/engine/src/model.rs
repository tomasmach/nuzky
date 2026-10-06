//! Project file format. Everything the renderer needs to produce a frame lives here,
//! so the same JSON renders identically in the app and in the CLI.
//!
//! Times are integer microseconds on the timeline (`*_us`).

use serde::{Deserialize, Serialize};

pub const PROJECT_VERSION: u32 = 1;
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub canvas: Canvas,
    pub assets: Vec<Asset>,
    /// Compositing order: index 0 is the bottom layer (the main track).
    pub tracks: Vec<Track>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum AssetKind {
    Video,
    Audio,
    Image,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    Video,
    Audio,
    Text,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// the content transform and are interpolated linearly; outside the range the nearest
    /// keyframe holds.
    #[serde(default)]
    pub keyframes: Vec<Keyframe>,
    /// Main track only: transition from the previous clip, centred on the cut.
    #[serde(default)]
    pub transition_in: Option<Transition>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Animation {
    pub kind: AnimationKind,
    pub duration_us: i64,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Keyframe {
    pub t_us: i64,
    pub transform: Transform,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub kind: TransitionKind,
    pub duration_us: i64,
}

/// Colour adjustments, all 0 by default (no change). Ranges are -1..=1 except fade and vignette 0..=1.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
        /// Playback speed; the clip covers `duration_us * speed` of source. Audio follows
        /// (pitch changes with speed).
        #[serde(default = "one")]
        speed: f32,
        #[serde(default)]
        adjust: Adjust,
        /// Audio fades at the clip edges.
        #[serde(default)]
        fade_in_us: i64,
        #[serde(default)]
        fade_out_us: i64,
    },
    #[serde(rename_all = "camelCase")]
    Text {
        text: String,
        style: TextStyle,
        #[serde(default)]
        transform: Transform,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
}

impl Default for Transform {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, scale: 1.0, rotation: 0.0, opacity: 1.0 }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextStyle {
    /// Font family name; `None` uses the default sans-serif.
    #[serde(default)]
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
    /// Lines wrap at this width in canvas pixels; `None` wraps at 90% of the canvas width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<f32>,
}

/// Oversized titles may extend beyond the canvas; these bounds still reject unbounded allocations.
pub const MAX_FONT_HEIGHT_RATIO: f32 = 2.0;
pub const MAX_STROKE_FONT_RATIO: f32 = 1.0;
pub const MAX_TEXT_WIDTH_RATIO: f32 = 4.0;

impl TextStyle {
    /// Older project files can bypass session validation when rendered by the CLI.
    pub fn bounded(&self, canvas: &Canvas) -> Self {
        let mut style = self.clone();
        let finite = |value: f32, fallback: f32| if value.is_finite() { value } else { fallback };
        style.font_size = finite(style.font_size, 1.0).clamp(1.0, MAX_FONT_HEIGHT_RATIO * canvas.height.max(1) as f32);
        style.stroke_width = finite(style.stroke_width, 0.0).clamp(0.0, MAX_STROKE_FONT_RATIO * style.font_size);
        style.max_width = style.max_width.map(|width| {
            finite(width, canvas.width as f32 * 0.9).clamp(1.0, MAX_TEXT_WIDTH_RATIO * canvas.width.max(1) as f32)
        });
        style
    }
}

fn one() -> f32 {
    1.0
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
        }
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

/// Parses `#rgb`, `#rrggbb` or `#rrggbbaa` into straight RGBA in 0..1.
pub fn parse_color(s: &str) -> [f32; 4] {
    let hex = s.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255);
    let (r, g, b, a) = match hex.len() {
        3 => {
            let n = |i: usize| u8::from_str_radix(&hex[i..i + 1].repeat(2), 16).unwrap_or(255);
            (n(0), n(1), n(2), 255)
        }
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
                },
                transform: Transform::default(),
            },
        });
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert_eq!(serde_json::from_str::<Project>(&json).unwrap(), p);
    }

    #[test]
    fn parses_colors() {
        assert_eq!(parse_color("#ff0000"), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(parse_color("#00000080")[3], 128.0 / 255.0);
        assert_eq!(parse_color("#fff"), [1.0, 1.0, 1.0, 1.0]);
    }
}
