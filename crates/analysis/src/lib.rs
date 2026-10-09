//! Deterministic local editing analysis. Times are half-open microsecond ranges
//! relative to the media/container origin, or to the supplied timeline audio.
mod align;
mod audio;
mod boundaries;
mod captions;
mod emphasis;
mod fillers;
mod retakes;
mod scenes;
mod speech;
pub mod style;
mod wav2vec2;

pub use align::{ALIGN_MODELS, AlignModel, Aligner, align_model};
pub use audio::{
    ProgramLoudness, SilenceParams, loudness, loudness_cancellable, program_loudness, program_loudness_cancellable,
    silences, silences_cancellable,
};
pub use boundaries::{align_to_sound, align_words};
pub use captions::{CaptionGrouping, group_words};
pub use emphasis::{Energy, Zoom, emphasis, word_energy};
pub use fillers::filler_words;
pub use retakes::{Attempt, Filler, RetakeGroup, Retakes, Review, retakes};
pub use scenes::{SceneCut, SceneParams, scene_cuts, scene_cuts_cancellable};
pub use speech::{AudioSource, Segment, Transcript, Word, transcribe_words, transcribe_words_cancellable};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start_us: i64,
    pub end_us: i64,
}

pub fn models_dir() -> std::path::PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("nuzky").join("models")
}

/// Silero voice detector shared by desktop, MCP and the analysis CLI.
pub const VAD_MODEL: &str = "ggml-silero-v5.1.2.bin";
