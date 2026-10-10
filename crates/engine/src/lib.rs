//! Nuzky engine: project model, editing, decoding, compositing, mixing and export.
//! The desktop app and the `nuzky` CLI are thin layers over this crate.

pub mod audio;
pub mod credits;
pub mod edit;
pub mod effects;
pub mod export;
pub mod gpu;
pub mod loudness;
pub mod media;
pub mod model;
pub mod proxy;
pub mod render;
pub mod speech;
mod stretch;
pub mod text;
pub mod thumbnail;
pub mod voice;
pub mod worker;

pub use model::Project;
pub use render::{Renderer, Wait};
