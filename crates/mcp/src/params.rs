use capopen_engine::{
    edit::{EditCmd, TimeRange},
    model::TextStyle,
    export::Quality,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub range: Option<TimeRange>,
    pub clip_ids: Option<Vec<String>>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Begin {
    pub label: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Apply {
    pub run_id: String,
    pub request_id: String,
    pub edits: Vec<EditCmd>,
    pub expected_revision: Option<u64>,
    pub expected_speech_key: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndAction {
    Keep,
    Discard,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct End {
    pub run_id: String,
    pub action: EndAction,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    Keep,
    Restore,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    pub action: RecoveryAction,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Undo {
    pub run_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Import {
    pub run_id: String,
    pub paths: Vec<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inspect {
    pub times_us: Vec<i64>,
    pub width: Option<u32>,
    #[serde(default)]
    pub safe_area: bool,
}
#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisKind {
    Silences,
    Loudness,
    Scenes,
    Fillers,
}
#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnalysisParams {
    pub threshold_db: Option<f32>,
    pub min_silence_us: Option<i64>,
    pub pad_us: Option<i64>,
    pub window_us: Option<i64>,
    pub threshold: Option<f32>,
    pub min_gap_us: Option<i64>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Analyze {
    pub kind: AnalysisKind,
    pub asset_id: String,
    #[serde(default)]
    pub params: AnalysisParams,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transcribe {
    pub asset_ids: Option<Vec<String>>,
    pub language: Option<String>,
    /// Installed model name or absolute local .bin path. Defaults to best installed.
    pub model: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetTranscript {
    /// Half-open timeline interval; word indices remain global.
    pub range_us: Option<[i64; 2]>,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditTranscript {
    pub run_id: String,
    /// Reuse this id with identical arguments to retry a failed save without cutting twice.
    pub request_id: Option<String>,
    pub speech_key: String,
    /// Inclusive zero-based word indices [from,to].
    pub delete: Option<Vec<[usize; 2]>>,
    /// Inclusive zero-based word indices [from,to]. Mutually exclusive with delete.
    pub keep: Option<Vec<[usize; 2]>>,
    pub shorten_pauses_us: Option<i64>,
    #[serde(default)]
    pub dry_run: bool,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobAction {
    Get,
    Cancel,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub job_id: String,
    pub action: JobAction,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Captions {
    pub run_id: String,
    pub style: Option<TextStyle>,
    pub max_words: Option<usize>,
    pub max_chars: Option<usize>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Export {
    pub path: String,
    /// Short side in pixels, e.g. 1080 for a 1080x1920 reel.
    pub resolution: u32,
    pub fps: u32,
    pub quality: Quality,
}

impl Captions {
    pub fn grouping(&self) -> capopen_analysis::CaptionGrouping {
        let defaults = capopen_analysis::CaptionGrouping::default();
        capopen_analysis::CaptionGrouping {
            max_words: self.max_words.unwrap_or(defaults.max_words),
            max_chars: self.max_chars.unwrap_or(defaults.max_chars),
            ..defaults
        }
    }
}

pub fn reel_style() -> TextStyle {
    TextStyle {
        font_family: None,
        font_size: 95.0,
        color: "#ffffff".into(),
        bold: false,
        stroke_width: 7.5,
        stroke_color: "#000000".into(),
        background: None,
        max_width: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions_omit_style_and_grouping_for_reel_defaults() {
        let args: Captions = serde_json::from_value(serde_json::json!({
            "run_id": "run"
        })).unwrap();
        let defaults = capopen_analysis::CaptionGrouping::default();
        assert_eq!(args.grouping().max_words, defaults.max_words);
        assert_eq!(args.grouping().max_chars, defaults.max_chars);
        let style = args.style.unwrap_or_else(reel_style);
        assert_eq!(serde_json::to_value(style).unwrap(), serde_json::json!({
            "fontFamily": null, "fontSize": 95.0, "color": "#ffffff", "bold": false,
            "strokeWidth": 7.5, "strokeColor": "#000000", "background": null
        }));
    }
}
