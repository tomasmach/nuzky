use std::collections::HashSet;

use anyhow::{Result, ensure};
use capopen_engine::{
    Project,
    model::{AssetKind, Clip, ClipContent, PROJECT_VERSION, TrackKind, Transform},
};

/// Structural validity; missing media is reported by media tools, not by editing.
pub fn validate(project: &Project) -> Result<()> {
    ensure!(
        project.version == PROJECT_VERSION,
        "INVALID_PROJECT: unsupported version"
    );
    ensure!(
        (16..=7680).contains(&project.canvas.width) && (16..=7680).contains(&project.canvas.height),
        "INVALID_PROJECT: canvas size"
    );
    ensure!(
        (1..=240).contains(&project.canvas.fps),
        "INVALID_PROJECT: frame rate"
    );
    ensure!(
        project.canvas.background_blur.is_finite(),
        "INVALID_PROJECT: background blur"
    );
    unique(project.assets.iter().map(|a| a.id.as_str()), "asset")?;
    unique(project.tracks.iter().map(|t| t.id.as_str()), "track")?;
    unique(
        project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id.as_str())),
        "clip",
    )?;
    ensure!(
        project
            .tracks
            .first()
            .is_some_and(|t| t.id == "main" && t.kind == TrackKind::Video),
        "INVALID_PROJECT: first track must be main video"
    );
    for asset in &project.assets {
        ensure!(
            asset.duration_us >= 0 && asset.fps.is_finite() && asset.fps >= 0.0,
            "INVALID_PROJECT: asset {} timing",
            asset.id
        );
    }
    for track in &project.tracks {
        let mut clips: Vec<_> = track.clips.iter().collect();
        clips.sort_by_key(|c| c.start_us);
        let mut end = 0;
        for clip in clips {
            ensure!(
                clip.start_us >= end,
                "INVALID_PROJECT: overlapping clips on {}",
                track.id
            );
            if track.id == "main" {
                ensure!(
                    clip.start_us == end,
                    "INVALID_PROJECT: main track is magnetic and must have no gaps"
                );
            }
            validate_clip(project, clip, track.kind)?;
            end = clip
                .start_us
                .checked_add(clip.duration_us)
                .ok_or_else(|| anyhow::anyhow!("INVALID_PROJECT: clip time overflow"))?;
        }
    }
    Ok(())
}

fn unique<'a>(ids: impl Iterator<Item = &'a str>, kind: &str) -> Result<()> {
    let mut seen = HashSet::new();
    for id in ids {
        ensure!(
            !id.is_empty() && seen.insert(id),
            "INVALID_PROJECT: empty or duplicate {kind} id {id}"
        );
    }
    Ok(())
}

fn transform(value: &Transform) -> Result<()> {
    ensure!(
        [value.x, value.y, value.scale, value.rotation, value.opacity]
            .iter()
            .all(|v| v.is_finite()),
        "INVALID_PROJECT: non-finite transform"
    );
    ensure!(
        value.scale > 0.0 && (0.0..=1.0).contains(&value.opacity),
        "INVALID_PROJECT: transform scale or opacity"
    );
    Ok(())
}

fn validate_clip(project: &Project, clip: &Clip, kind: TrackKind) -> Result<()> {
    ensure!(
        clip.start_us >= 0 && clip.duration_us > 0,
        "INVALID_PROJECT: clip {} timing",
        clip.id
    );
    match &clip.content {
        ClipContent::Media {
            asset_id,
            source_in_us,
            speed,
            volume,
            transform: t,
            fade_in_us,
            fade_out_us,
            adjust,
        } => {
            let asset = project
                .asset(asset_id)
                .ok_or_else(|| anyhow::anyhow!("INVALID_PROJECT: missing asset {asset_id}"))?;
            ensure!(
                kind != TrackKind::Text
                    && (kind != TrackKind::Video || asset.kind != AssetKind::Audio),
                "INVALID_PROJECT: media on incompatible track"
            );
            ensure!(
                speed.is_finite()
                    && (0.1..=10.0).contains(speed)
                    && volume.is_finite()
                    && *volume >= 0.0,
                "INVALID_PROJECT: clip {} speed/volume",
                clip.id
            );
            ensure!(*source_in_us >= 0, "INVALID_PROJECT: negative source range");
            // AddClip extends short sources to one whole canvas frame.
            let source_end = *source_in_us as f64 + clip.duration_us as f64 * *speed as f64;
            let minimum_clip_duration_us = project.frame_duration_us().ceil();
            let tolerance = minimum_clip_duration_us * *speed as f64;
            ensure!(
                asset.kind == AssetKind::Image
                    || source_end <= asset.duration_us as f64 + tolerance,
                "INVALID_PROJECT: clip {} exceeds source duration",
                clip.id
            );
            ensure!(
                *fade_in_us >= 0 && *fade_out_us >= 0,
                "INVALID_PROJECT: negative fade"
            );
            ensure!(
                [
                    adjust.brightness,
                    adjust.contrast,
                    adjust.saturation,
                    adjust.temperature,
                    adjust.vignette
                ]
                .iter()
                .all(|v| v.is_finite()),
                "INVALID_PROJECT: non-finite adjustment"
            );
            transform(t)?;
        }
        ClipContent::Text {
            style,
            transform: t,
            ..
        } => {
            ensure!(
                style.font_size.is_finite()
                    && style.font_size > 0.0
                    && style.stroke_width.is_finite()
                    && style.stroke_width >= 0.0,
                "INVALID_PROJECT: text style"
            );
            transform(t)?;
        }
    }
    // Trimming deliberately preserves out-of-clip keys to preserve interpolation.
    let mut last_key = None;
    for key in &clip.keyframes {
        ensure!(
            last_key.is_none_or(|last| key.t_us.checked_sub(last).is_some_and(|span| span > 0)),
            "INVALID_PROJECT: keyframe order or time overflow"
        );
        transform(&key.transform)?;
        last_key = Some(key.t_us);
    }
    for animation in [clip.anim_in, clip.anim_out].into_iter().flatten() {
        ensure!(
            animation.duration_us >= 0,
            "INVALID_PROJECT: negative animation duration"
        );
    }
    if let Some(transition) = clip.transition_in {
        ensure!(
            transition.duration_us >= 0,
            "INVALID_PROJECT: negative transition duration"
        );
    }
    Ok(())
}
