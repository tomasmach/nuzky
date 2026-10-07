use std::collections::HashSet;
use std::path::{Component, Path, Prefix};

use anyhow::{Result, ensure};
use capopen_engine::{
    Project,
    model::{
        AssetKind, Clip, ClipContent, MAX_FONT_HEIGHT_RATIO, MAX_TEXT_WIDTH_RATIO, PROJECT_VERSION, TrackKind,
        Transform, max_stroke_width,
    },
};

/// Structural validity; missing media is reported by media tools, not by editing.
pub fn validate(project: &Project) -> Result<()> {
    ensure!(project.version == PROJECT_VERSION, "INVALID_PROJECT: unsupported version");
    ensure!(
        (16..=7680).contains(&project.canvas.width) && (16..=7680).contains(&project.canvas.height),
        "INVALID_PROJECT: canvas size"
    );
    ensure!((1..=240).contains(&project.canvas.fps), "INVALID_PROJECT: frame rate");
    ensure!(project.canvas.background_blur.is_finite(), "INVALID_PROJECT: background blur");
    unique(project.assets.iter().map(|a| a.id.as_str()), "asset")?;
    unique(project.tracks.iter().map(|t| t.id.as_str()), "track")?;
    unique(project.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.id.as_str())), "clip")?;
    ensure!(
        project.tracks.first().is_some_and(|t| t.id == "main" && t.kind == TrackKind::Video),
        "INVALID_PROJECT: first track must be main video"
    );
    for asset in &project.assets {
        ensure!(
            asset.duration_us >= 0 && asset.fps.is_finite() && asset.fps >= 0.0,
            "INVALID_PROJECT: asset {} timing",
            asset.id
        );
        local_media_path(&asset.path)?;
    }
    for track in &project.tracks {
        let mut clips: Vec<_> = track.clips.iter().collect();
        clips.sort_by_key(|c| c.start_us);
        let mut end = 0;
        for clip in clips {
            ensure!(clip.start_us >= end, "INVALID_PROJECT: overlapping clips on {}", track.id);
            if track.id == "main" {
                ensure!(clip.start_us == end, "INVALID_PROJECT: main track is magnetic and must have no gaps");
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

/// FFmpeg reads prefixes such as `http:`, `concat:` or `pipe:` as protocols and relative paths
/// depend on the working directory, so media paths must be absolute and local. Missing files
/// stay valid so the project can be relinked.
pub fn local_media_path(path: &str) -> Result<()> {
    let local = Path::new(path);
    let device = matches!(
        local.components().next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::DeviceNS(_))
    );
    ensure!(
        !path.contains('\0') && local.is_absolute() && !device,
        "INVALID_ASSET_PATH: media must be an absolute local file path, not {path:?}"
    );
    Ok(())
}

fn unique<'a>(ids: impl Iterator<Item = &'a str>, kind: &str) -> Result<()> {
    let mut seen = HashSet::new();
    for id in ids {
        ensure!(!id.is_empty() && seen.insert(id), "INVALID_PROJECT: empty or duplicate {kind} id {id}");
    }
    Ok(())
}

fn transform(value: &Transform) -> Result<()> {
    ensure!(
        [value.x, value.y, value.scale, value.rotation, value.opacity].iter().all(|v| v.is_finite()),
        "INVALID_PROJECT: non-finite transform"
    );
    ensure!(value.scale > 0.0 && (0.0..=1.0).contains(&value.opacity), "INVALID_PROJECT: transform scale or opacity");
    Ok(())
}

fn validate_clip(project: &Project, clip: &Clip, kind: TrackKind) -> Result<()> {
    ensure!(clip.start_us >= 0 && clip.duration_us > 0, "INVALID_PROJECT: clip {} timing", clip.id);
    match &clip.content {
        ClipContent::Media { asset_id, source_in_us, speed, volume, transform: t, fade_in_us, fade_out_us, adjust } => {
            let asset =
                project.asset(asset_id).ok_or_else(|| anyhow::anyhow!("INVALID_PROJECT: missing asset {asset_id}"))?;
            ensure!(
                kind != TrackKind::Text && (kind != TrackKind::Video || asset.kind != AssetKind::Audio),
                "INVALID_PROJECT: media on incompatible track"
            );
            ensure!(
                speed.is_finite() && (0.1..=10.0).contains(speed) && volume.is_finite() && *volume >= 0.0,
                "INVALID_PROJECT: clip {} speed/volume",
                clip.id
            );
            ensure!(*source_in_us >= 0, "INVALID_PROJECT: negative source range");
            // AddClip extends short sources to one whole canvas frame.
            let source_end = *source_in_us as f64 + clip.duration_us as f64 * *speed as f64;
            let minimum_clip_duration_us = project.frame_duration_us().ceil();
            let tolerance = minimum_clip_duration_us * *speed as f64;
            ensure!(
                asset.kind == AssetKind::Image || source_end <= asset.duration_us as f64 + tolerance,
                "INVALID_PROJECT: clip {} exceeds source duration",
                clip.id
            );
            ensure!(*fade_in_us >= 0 && *fade_out_us >= 0, "INVALID_PROJECT: negative fade");
            ensure!(
                [
                    adjust.exposure,
                    adjust.tint,
                    adjust.highlights,
                    adjust.shadows,
                    adjust.fade,
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
        ClipContent::Text { style, transform: t, .. } => {
            ensure!(
                style.font_size.is_finite()
                    && style.font_size > 0.0
                    && style.font_size <= MAX_FONT_HEIGHT_RATIO * project.canvas.height as f32
                    && style.stroke_width.is_finite()
                    && style.stroke_width >= 0.0
                    && style.stroke_width <= max_stroke_width(style.font_size)
                    && style.max_width.is_none_or(|width| width.is_finite()
                        && width > 0.0
                        && width <= MAX_TEXT_WIDTH_RATIO * project.canvas.width as f32),
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
        ensure!(animation.duration_us >= 0, "INVALID_PROJECT: negative animation duration");
    }
    if let Some(transition) = clip.transition_in {
        ensure!(transition.duration_us >= 0, "INVALID_PROJECT: negative transition duration");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use capopen_engine::{
        edit::EditCmd,
        model::{Adjust, Asset},
    };

    #[test]
    fn text_styles_require_finite_canvas_relative_bounds() {
        let style: capopen_engine::model::TextStyle = serde_json::from_value(serde_json::json!({
            "fontSize":95.0,"color":"#ffffff","strokeWidth":7.5
        }))
        .unwrap();
        let mut project = Project::new("text bounds");
        project.apply(EditCmd::AddText { start_us: 0, text: "Title".into(), style: style.clone() }).unwrap();
        for field in ["font", "stroke", "width"] {
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, f32::MAX] {
                let mut bad = style.clone();
                match field {
                    "font" => bad.font_size = value,
                    "stroke" => bad.stroke_width = value,
                    _ => bad.max_width = Some(value),
                }
                let ClipContent::Text { style, .. } = &mut project.tracks[1].clips[0].content else { panic!() };
                *style = bad;
                assert!(validate(&project).is_err(), "{field}: {value}");
            }
        }
        for (font, stroke, width, valid) in [
            (3840.0, 3840.0, Some(4320.0), true),
            (3841.0, 0.0, None, false),
            (95.0, 96.0, None, false),
            (12.0, 20.0, None, true),
            (12.0, 20.5, None, false),
            (95.0, 0.0, Some(4321.0), false),
            (95.0, 0.0, Some(0.0), false),
            (0.0, 0.0, None, false),
        ] {
            let ClipContent::Text { style, .. } = &mut project.tracks[1].clips[0].content else { panic!() };
            style.font_size = font;
            style.stroke_width = stroke;
            style.max_width = width;
            assert_eq!(validate(&project).is_ok(), valid, "font {font}, stroke {stroke}, width {width:?}");
        }
    }

    #[test]
    fn asset_paths_must_be_absolute_and_local_but_may_be_missing() {
        let mut project = Project::new("paths");
        project.assets.push(Asset {
            id: "clip".into(),
            name: "Clip".into(),
            path: "/missing/clip.mp4".into(),
            kind: AssetKind::Video,
            duration_us: 1_000_000,
            width: 1080,
            height: 1920,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        validate(&project).unwrap();
        for path in [
            "http://host/x.mp4",
            "https://host/x.mp4",
            "concat:/a.mp4|/b.mp4",
            "pipe:0",
            "file:/tmp/x.mp4",
            "clip.mp4",
            "./clip.mp4",
            "",
            "/tmp/a\0http://host/x.mp4",
        ] {
            project.assets[0].path = path.into();
            let error = validate(&project).unwrap_err().to_string();
            assert!(error.starts_with("INVALID_ASSET_PATH:"), "{path:?}: {error}");
        }
        let before = Project::new("edit");
        let mut editor = capopen_engine::edit::Editor::new(before.clone());
        let assets = project.assets.clone();
        assert!(editor.apply_batch_checked(vec![EditCmd::AddAssets { assets }], None, validate).is_err());
        assert_eq!(editor.project, before);
    }

    #[test]
    fn new_color_adjustments_reject_non_finite_values() {
        let mut project = Project::new("color validation");
        project.assets.push(Asset {
            id: "ramp".into(),
            name: "Ramp".into(),
            path: "/ramp.ppm".into(),
            kind: AssetKind::Image,
            duration_us: 0,
            width: 256,
            height: 16,
            fps: 0.0,
            has_audio: false,
            rotation: 0,
        });
        project.apply(EditCmd::AddClip { asset_id: "ramp".into(), start_us: None, track_id: None }).unwrap();
        validate(&project).unwrap();
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, 0.5, 1.0] {
            for candidate in [
                Adjust { exposure: value, ..Adjust::default() },
                Adjust { tint: value, ..Adjust::default() },
                Adjust { highlights: value, ..Adjust::default() },
                Adjust { shadows: value, ..Adjust::default() },
                Adjust { fade: value, ..Adjust::default() },
            ] {
                let ClipContent::Media { adjust, .. } = &mut project.tracks[0].clips[0].content else { panic!() };
                *adjust = candidate;
                let result = validate(&project);
                assert_eq!(result.is_ok(), value.is_finite(), "{candidate:?}: {result:?}");
                if let Err(error) = result {
                    assert!(error.to_string().contains("non-finite adjustment"));
                }
            }
        }
    }
}
