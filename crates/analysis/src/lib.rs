//! Deterministic local editing analysis. Times are half-open microsecond ranges
//! relative to the media/container origin, or to the supplied timeline audio.
mod audio;
mod captions;
mod fillers;
mod scenes;
mod speech;

pub use audio::{SilenceParams, integrated_lufs, loudness, silences};
pub use captions::{CaptionGrouping, group_words};
pub use fillers::filler_words;
pub use scenes::{SceneCut, SceneParams, scene_cuts};
pub use speech::{AudioSource, Segment, Transcript, Word, transcribe_words, transcribe_words_cancellable};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start_us: i64,
    pub end_us: i64,
}

pub fn models_dir() -> std::path::PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("capopen").join("models")
}

/// Silero voice detector shared by desktop, MCP and the analysis CLI.
pub const VAD_MODEL: &str = "ggml-silero-v5.1.2.bin";
