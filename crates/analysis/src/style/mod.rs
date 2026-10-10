//! Learns how a creator edits from their recordings and finished cuts, writes it down as
//! EDIT.md, and scores any cut of a recording against the creator's own.
mod align;
pub mod doc;
mod from_timeline;
mod learn;
mod picture;
mod score;

pub use align::{Alignment, Piece, Place, align};
pub use from_timeline::{Lesson, Recording, heard_media, lessons, speech_clips};
pub use learn::{Choice, Correction, Learned, Moment, RULES, Rule, SETTINGS, Source, learn, learned, settings_block};
pub use picture::{Caption, Framing, Picture, ZoomChange, picture};
pub use score::{Passage, Score, score};

use std::path::PathBuf;

use crate::{Range, Word};

/// Pauses shorter than this are gaps inside speech, as when sound is compared.
const MIN_PAUSE_US: i64 = 80_000;

/// Where the creator's style lives. MCP serves it as nuzky://style when it exists.
pub fn style_path() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("nuzky").join("EDIT.md")
}

/// Lowercase letters and digits only, so "Opus," and "opus" are the same word.
pub(crate) fn token(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The silences between words, where a timeline says exactly when each is heard.
pub fn pauses_between(words: &[Word]) -> Vec<Range> {
    words
        .windows(2)
        .filter(|p| p[1].start_us - p[0].end_us >= MIN_PAUSE_US)
        .map(|p| Range { start_us: p[0].end_us, end_us: p[1].start_us })
        .collect()
}
