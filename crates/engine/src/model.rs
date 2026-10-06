//! Project file format. Everything the renderer needs to produce a frame lives here,
//! so the same JSON renders identically in the app and in the CLI.
//!
//! Times are integer microseconds on the timeline (`*_us`).

use serde::{Deserialize, Serialize};

pub const PROJECT_VERSION: u32 = 1;
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// `#rrggbb`
    pub background: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum AssetKind {
    Video,
    Audio,
    Image,
}

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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    Video,
    Audio,
    Text,
}

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
    pub clips: Vec<Clip>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    pub start_us: i64,
    pub duration_us: i64,
    pub content: ClipContent,
}

impl Clip {
    pub fn end_us(&self) -> i64 {
        self.start_us + self.duration_us
    }

    pub fn contains(&self, t_us: i64) -> bool {
        t_us >= self.start_us && t_us < self.end_us()
    }
}

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
    },
    #[serde(rename_all = "camelCase")]
    Text {
        text: String,
        style: TextStyle,
        #[serde(default)]
        transform: Transform,
    },
}

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextStyle {
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
            canvas: Canvas { width: 1080, height: 1920, fps: 30, background: "#000000".into() },
            assets: Vec::new(),
            tracks: vec![Track {
                id: "main".into(),
                kind: TrackKind::Video,
                name: "Main".into(),
                muted: false,
                hidden: false,
                clips: Vec::new(),
            }],
        }
    }

    pub fn asset(&self, id: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// End of the last clip on any track.
    pub fn duration_us(&self) -> i64 {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(Clip::end_us))
            .max()
            .unwrap_or(0)
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
            content: ClipContent::Text {
                text: "Ahoj světe".into(),
                style: TextStyle {
                    font_size: 80.0,
                    color: "#ffffff".into(),
                    bold: true,
                    stroke_width: 6.0,
                    stroke_color: "#000000".into(),
                    background: None,
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
