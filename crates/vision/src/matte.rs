//! The person mattes of clips with a background (`nuzky_engine::matte`), made with MediaPipe's selfie
//! segmenter: the engine decodes, smooths and stores them, this runs the model.
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use nuzky_engine::{Project, matte, model::Asset};

use crate::models;
use crate::nets::{SELFIE_SIDE, Selfie, check_cancel};

/// Makes every matte the project's backgrounds show that is not made yet, file by file; `progress` gets 0..1.
/// Fails with `MODEL_MISSING` before any work when the person model is not installed.
pub fn prepare(
    project: &Project,
    cache_dir: &Path,
    models_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let missing = matte::missing(cache_dir, project);
    let files = missing.len() as f32;
    for (i, (asset, chunks)) in missing.iter().enumerate() {
        prepare_file(asset, chunks, cache_dir, models_dir, cancel, &mut |p| progress((i as f32 + p) / files))?;
    }
    Ok(())
}

/// Makes the `chunks` of one file's matte, as `nuzky_engine::matte::needed` names them.
pub fn prepare_file(
    asset: &Asset,
    chunks: &BTreeSet<i64>,
    cache_dir: &Path,
    models_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    debug_assert_eq!(SELFIE_SIDE, matte::SIDE as usize);
    models::require(models::BACKGROUND, models_dir)?;
    let mut selfie = Selfie::load(models_dir)?;
    matte::prepare(cache_dir, asset, chunks, &mut |rgba| selfie.probabilities(rgba, cancel), &mut |p| {
        check_cancel(cancel)?;
        progress(p);
        Ok(())
    })
}
