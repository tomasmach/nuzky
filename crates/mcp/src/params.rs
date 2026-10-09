use capopen_engine::{
    edit::{EditCmd, TimeRange},
    export::{Delivery, Quality},
    model::TextStyle,
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
    pub expected_speech_layout_key: Option<String>,
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
    /// Reuse this id with the same paths to retry a failed save without importing twice.
    pub request_id: Option<String>,
}
/// Choices offered to the user; the CapOpen AI panel shows them as buttons.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SuggestOptions {
    /// The question, short, in the user's language.
    pub question: Option<String>,
    /// 2 to 6 choices. The label the user picks comes back as their next message.
    pub options: Vec<Choice>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    /// What the button says, at most 80 characters.
    pub label: String,
    /// One short line on what happens with this choice.
    pub detail: Option<String>,
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
    /// Restarted sentences and leading fillers over the whole timeline, returned at once.
    Retakes,
    /// Sentences said with emphasis, for a punch-in, over the whole timeline, returned at once.
    Emphasis,
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
    /// Required for silences, loudness, scenes and fillers. Omit for retakes and emphasis.
    pub asset_id: Option<String>,
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
    /// Required unless dry_run is true.
    pub run_id: Option<String>,
    /// Reuse this id with identical arguments to retry a failed save without cutting twice.
    pub request_id: Option<String>,
    pub transcript_key: String,
    /// Inclusive zero-based word indices [from,to].
    pub delete: Option<Vec<[usize; 2]>>,
    /// Inclusive zero-based word indices [from,to]. Mutually exclusive with delete.
    pub keep: Option<Vec<[usize; 2]>>,
    pub shorten_pauses_us: Option<i64>,
    #[serde(default)]
    pub dry_run: bool,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CorrectWords {
    pub run_id: String,
    /// Reuse this id with identical arguments to retry a failed save without correcting twice.
    pub request_id: Option<String>,
    pub transcript_key: String,
    pub corrections: Vec<WordFix>,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WordFix {
    /// get_transcript word index.
    pub i: usize,
    /// What the word should read, punctuation included: one line, at most 100 characters.
    pub text: String,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyZooms {
    pub run_id: String,
    /// Reuse this id with identical arguments to retry a failed save without zooming twice.
    pub request_id: Option<String>,
    /// get_transcript's transcript_key, also returned by analyze(kind: "emphasis").
    pub transcript_key: String,
    /// Punch-ins on INCLUSIVE word ranges, such as analyze(kind: "emphasis").zooms.
    pub zooms: Vec<crate::zooms::WordZoom>,
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
    /// A full caption style; leave it out to use style_preset or Reel.
    pub style: Option<TextStyle>,
    /// A caption preset by name: reel, outline, yellow, box, clean, karaoke or green_box.
    /// Karaoke and green_box highlight the word being spoken.
    pub style_preset: Option<String>,
    pub max_words: Option<usize>,
    pub max_chars: Option<usize>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Export {
    pub path: String,
    /// "reels": Instagram Reels and TikTok, 1080x1920 at 30 fps, loudness levelled to -14 LUFS
    /// with true peak at most -1 dBTP. Needs a 9:16 canvas.
    pub preset: Option<Delivery>,
    /// Short side in pixels, e.g. 1080 for a 1080x1920 reel. Required without preset; with a
    /// preset leave it out or give the preset's own value.
    pub resolution: Option<u32>,
    /// Required without preset; with a preset leave it out or give the preset's own value.
    pub fps: Option<u32>,
    /// Defaults to recommended.
    pub quality: Option<Quality>,
}

impl Captions {
    /// The style asked for: a full style or a preset, Reel when neither is given.
    pub fn style(&self) -> anyhow::Result<TextStyle> {
        match (&self.style, &self.style_preset) {
            (Some(_), Some(_)) => anyhow::bail!("INVALID_ARGUMENTS: give style or style_preset, not both"),
            (Some(style), None) => Ok(style.clone()),
            (None, Some(name)) => {
                capopen_engine::edit::caption_preset(name).map(|p| p.style.clone()).ok_or_else(|| {
                    let names: Vec<_> = capopen_engine::edit::caption_presets()
                        .iter()
                        .map(|p| p.name.to_lowercase().replace(' ', "_"))
                        .collect();
                    anyhow::anyhow!("INVALID_ARGUMENTS: unknown style_preset {name:?}; use one of {}", names.join(", "))
                })
            }
            (None, None) => Ok(reel_style()),
        }
    }

    pub fn grouping(&self) -> capopen_analysis::CaptionGrouping {
        let defaults = capopen_analysis::CaptionGrouping::default();
        capopen_analysis::CaptionGrouping {
            max_words: self.max_words.unwrap_or(defaults.max_words),
            max_chars: self.max_chars.unwrap_or(defaults.max_chars),
            ..defaults
        }
    }
}

pub fn reel_style() -> capopen_engine::model::TextStyle {
    capopen_engine::edit::caption_presets()[0].style.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions_omit_style_and_grouping_for_reel_defaults() {
        let args: Captions = serde_json::from_value(serde_json::json!({
            "run_id": "run"
        }))
        .unwrap();
        let defaults = capopen_analysis::CaptionGrouping::default();
        assert_eq!(args.grouping().max_words, defaults.max_words);
        assert_eq!(args.grouping().max_chars, defaults.max_chars);
        let style = args.style().unwrap();
        assert_eq!(
            serde_json::to_value(style).unwrap(),
            serde_json::json!({
                "fontFamily": null, "fontSize": 95.0, "color": "#ffffff", "bold": false,
                "strokeWidth": 7.5, "strokeColor": "#000000", "background": null
            })
        );
    }

    #[test]
    fn captions_pick_a_preset_by_name() {
        let captions = |value: serde_json::Value| {
            let mut args = serde_json::json!({"run_id": "run"});
            args.as_object_mut().unwrap().extend(value.as_object().unwrap().clone());
            serde_json::from_value::<Captions>(args).unwrap().style()
        };
        let karaoke = captions(serde_json::json!({"style_preset": "karaoke"})).unwrap();
        assert_eq!(karaoke.highlight.as_deref(), Some("#ffe14d"));
        assert_eq!(karaoke.font_size, reel_style().font_size);
        let boxed = captions(serde_json::json!({"style_preset": "Green_Box"})).unwrap();
        assert_eq!((boxed.highlight.as_deref(), boxed.background.is_some()), (Some("#4ade80"), true));
        let unknown = captions(serde_json::json!({"style_preset": "neon"})).unwrap_err().to_string();
        assert!(unknown.starts_with("INVALID_ARGUMENTS") && unknown.contains("karaoke, green_box"), "{unknown}");
        let both = captions(
            serde_json::json!({"style_preset": "karaoke", "style": serde_json::to_value(reel_style()).unwrap()}),
        );
        assert!(both.unwrap_err().to_string().starts_with("INVALID_ARGUMENTS"));
    }
}
