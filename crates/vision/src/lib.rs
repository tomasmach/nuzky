//! Local face and subject analysis of the rendered timeline, for covers and thumbnails: which
//! frames are worth one and where the person is. Runs ONNX models on the CPU with ONNX Runtime;
//! the models are downloaded by the app or the CLI, never here.
mod frame;
pub mod framing;
pub mod mask;
pub mod matte;
pub mod models;
mod nets;
pub mod runtime;
pub mod thumbnails;
mod timeline;

pub use framing::Format;
pub use mask::{Mask, cached_alpha, segment_subject, subject_alpha};
pub use thumbnails::{Candidate, thumbnail_frames};
