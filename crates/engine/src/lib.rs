//! CapOpen engine: project model, editing, decoding, compositing, mixing and export.
//! The desktop app and the `capopen` CLI are thin layers over this crate.

pub mod audio;
pub mod edit;
pub mod effects;
pub mod export;
pub mod gpu;
pub mod media;
pub mod model;
pub mod render;
pub mod speech;
pub mod text;
pub mod worker;

pub use model::Project;
pub use render::{Renderer, Wait};
