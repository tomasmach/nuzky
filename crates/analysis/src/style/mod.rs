//! Learns how a creator edits from their recordings and finished cuts, writes it down as
//! EDIT.md, and scores any cut of a recording against the creator's own.
mod align;
pub mod doc;
mod learn;
mod picture;
mod score;

pub use align::{Alignment, Piece, Place, align};
pub use learn::{Choice, Correction, Learned, Moment, RULES, Rule, SETTINGS, Source, learn, learned, settings_block};
pub use picture::{Caption, Framing, Picture, ZoomChange, picture};
pub use score::{Passage, Score, score};

use std::path::PathBuf;

/// Where the creator's style lives. MCP serves it as nuzky://style when it exists.
pub fn style_path() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("nuzky").join("EDIT.md")
}

/// Lowercase letters and digits only, so "Opus," and "opus" are the same word.
pub(crate) fn token(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}
