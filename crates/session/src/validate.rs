use std::collections::HashSet;
use std::path::{Component, Path, Prefix};

use anyhow::{Result, ensure};
use nuzky_engine::{
    Project,
    model::{
        AssetKind, Clip, ClipContent, MAX_BORDER_WIDTH, MAX_CORRECTION_CHARS, MAX_FONT_HEIGHT_RATIO,
        MAX_TEXT_WIDTH_RATIO, PROJECT_VERSION, TrackKind, Transform, max_stroke_width,
    },
};

/// More words than any caption shows; bounds the per-frame search for the spoken word.
const MAX_CAPTION_WORDS: usize = 1000;

/// Structural validity; missing media is reported by media tools, not by editing.
pub fn validate(project: &Project) -> Result<()> {
    ensure!(project.version == PROJECT_VERSION, "INVALID_PROJECT: unsupported version");
    ensure!(
        (16..=7680).contains(&project.canvas.width) && (16..=7680).contains(&project.canvas.height),
        "INVALID_PROJECT: canvas size"
    );
    ensure!((1..=240).contains(&project.canvas.fps), "INVALID_PROJECT: frame rate");
    ensure!(project.canvas.background_blur.is_finite(), "INVALID_PROJECT: background blur");
    color(&project.canvas.background, "canvas background")?;
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
    word_corrections(project)
}

/// More than a long recording has words, so a project file cannot grow without bound.
const MAX_WORD_CORRECTIONS: usize = 50_000;

/// A corrected word is one line of text, like the recognised word it replaces. Corrections of
/// media no longer in the project stay valid: they apply to nothing.
fn word_corrections(project: &Project) -> Result<()> {
    ensure!(
        project.word_corrections.len() <= MAX_WORD_CORRECTIONS,
        "INVALID_PROJECT: more than {MAX_WORD_CORRECTIONS} corrected words"
    );
    let mut seen = HashSet::new();
    for correction in &project.word_corrections {
        for text in [&correction.text, &correction.original] {
            ensure!(
                !text.trim().is_empty()
                    && text.chars().count() <= MAX_CORRECTION_CHARS
                    && !text.chars().any(char::is_control),
                "INVALID_PROJECT: a corrected word must be one line of 1 to {MAX_CORRECTION_CHARS} characters, not {text:?}"
            );
        }
        ensure!(
            !correction.asset_id.is_empty() && correction.source_start_us >= 0,
            "INVALID_PROJECT: corrected word without its media or start"
        );
        ensure!(
            seen.insert((&correction.asset_id, correction.source_start_us, &correction.original)),
            "INVALID_PROJECT: the word {:?} is corrected twice",
            correction.original
        );
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

/// The renderer parses colours by byte offset; anything but ASCII hex must not reach it.
fn color(value: &str, field: &str) -> Result<()> {
    let hex = value.strip_prefix('#').unwrap_or_default();
    ensure!(
        matches!(hex.len(), 3 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "INVALID_COLOR: {field} must be #rgb, #rrggbb or #rrggbbaa, not {value:?}"
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
    if let Some(c) = value.crop {
        ensure!(
            [c.left, c.top, c.right, c.bottom].iter().all(|v| v.is_finite() && *v >= 0.0)
                && c.left + c.right < 1.0
                && c.top + c.bottom < 1.0,
            "INVALID_PROJECT: a crop must leave part of the layer visible"
        );
    }
    Ok(())
}

fn validate_clip(project: &Project, clip: &Clip, kind: TrackKind) -> Result<()> {
    ensure!(clip.start_us >= 0 && clip.duration_us > 0, "INVALID_PROJECT: clip {} timing", clip.id);
    match &clip.content {
        ClipContent::Media {
            asset_id,
            source_in_us,
            speed,
            keep_pitch: _,
            volume,
            transform: t,
            fade_in_us,
            fade_out_us,
            adjust,
            clean_voice,
            shape,
            duck_db,
        } => {
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
                (0.0..=nuzky_engine::edit::MAX_DUCK_DB).contains(duck_db),
                "INVALID_PROJECT: clip {} ducking",
                clip.id
            );
            ensure!(
                !clean_voice || nuzky_engine::audio::has_audio(asset),
                "INVALID_PROJECT: clip {} cleans the voice of media without sound",
                clip.id
            );
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
            if let Some(shape) = shape {
                ensure!(
                    (0.0..=1.0).contains(&shape.radius)
                        && (0.0..=MAX_BORDER_WIDTH).contains(&shape.border_width)
                        && (0.0..=1.0).contains(&shape.shadow),
                    "INVALID_PROJECT: shape of {} needs a radius and shadow from 0 to 1 and a border up to {MAX_BORDER_WIDTH} px",
                    clip.id
                );
                color(&shape.border_color, "border color")?;
            }
        }
        ClipContent::Text { style, transform: t, words, .. } => {
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
            color(&style.color, "text color")?;
            color(&style.stroke_color, "outline color")?;
            if let Some(background) = &style.background {
                color(background, "text background")?;
            }
            if let Some(highlight) = &style.highlight {
                color(highlight, "highlight color")?;
            }
            // Words that no longer spell the text are kept but ignored, so only their times are checked.
            ensure!(
                words.len() <= MAX_CAPTION_WORDS && words.iter().all(|w| w.start_us <= w.end_us),
                "INVALID_PROJECT: caption words of {} must be at most {MAX_CAPTION_WORDS}, each ending after it starts",
                clip.id
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
    use nuzky_engine::{
        edit::EditCmd,
        model::{Adjust, Asset},
    };

    #[test]
    fn text_styles_require_finite_canvas_relative_bounds() {
        let style: nuzky_engine::model::TextStyle = serde_json::from_value(serde_json::json!({
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
    fn caption_words_need_ordered_times_but_may_differ_from_the_text() {
        let style: nuzky_engine::model::TextStyle = serde_json::from_value(serde_json::json!({
            "fontSize":95.0,"color":"#ffffff","highlight":"#ffe14d"
        }))
        .unwrap();
        let mut project = Project::new("words");
        project.apply(EditCmd::AddText { start_us: 0, text: "Ahoj".into(), style }).unwrap();
        let word = |start_us, end_us| nuzky_engine::model::CaptionWord { text: "Jinak".into(), start_us, end_us };
        for (words, valid) in [
            (vec![word(0, 0), word(-500, 100)], true),
            (vec![word(i64::MIN, i64::MAX)], true),
            (vec![word(100, 99)], false),
            (vec![word(0, 1); MAX_CAPTION_WORDS + 1], false),
        ] {
            let ClipContent::Text { words: stored, .. } = &mut project.tracks[1].clips[0].content else { panic!() };
            *stored = words;
            assert_eq!(validate(&project).is_ok(), valid, "{:?}", validate(&project));
        }
    }

    #[test]
    fn asset_paths_must_be_absolute_and_local_but_may_be_missing() {
        let mut project = Project::new("paths");
        project.assets.push(Asset {
            id: "clip".into(),
            name: "Clip".into(),
            path: std::env::temp_dir().join("missing/clip.mp4").to_string_lossy().into(),
            kind: AssetKind::Video,
            duration_us: 1_000_000,
            width: 1080,
            height: 1920,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
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
        let mut editor = nuzky_engine::edit::Editor::new(before.clone());
        let assets = project.assets.clone();
        assert!(editor.apply_batch_checked(vec![EditCmd::AddAssets { assets }], None, validate).is_err());
        assert_eq!(editor.project, before);
    }

    #[test]
    fn every_color_field_requires_ascii_hex() {
        let style: nuzky_engine::model::TextStyle = serde_json::from_value(serde_json::json!({
            "fontSize":95.0,"color":"#ffffff","strokeWidth":7.5,"background":"#00000080"
        }))
        .unwrap();
        let mut project = Project::new("colors");
        project.apply(EditCmd::AddText { start_us: 0, text: "Title".into(), style }).unwrap();
        validate(&project).unwrap();
        for field in 0..5 {
            for (value, valid) in [
                ("#fff", true),
                ("#FFAA00", true),
                ("#00000080", true),
                ("#€", false),
                ("#ab€", false),
                ("white", false),
                ("#ggg", false),
                ("#12345", false),
                ("fff", false),
                ("#fff ", false),
                ("", false),
            ] {
                let mut candidate = project.clone();
                let ClipContent::Text { style, .. } = &mut candidate.tracks[1].clips[0].content else { panic!() };
                match field {
                    0 => candidate.canvas.background = value.into(),
                    1 => style.color = value.into(),
                    2 => style.stroke_color = value.into(),
                    3 => style.background = Some(value.into()),
                    _ => style.highlight = Some(value.into()),
                }
                let result = validate(&candidate);
                assert_eq!(result.is_ok(), valid, "field {field}: {value:?}");
                if let Err(error) = result {
                    assert!(error.to_string().starts_with("INVALID_COLOR:"), "{error}");
                }
            }
        }
        let mut editor = nuzky_engine::edit::Editor::new(project.clone());
        let set =
            EditCmd::SetCanvas { width: 1080, height: 1920, background: Some("#€".into()), background_blur: None };
        let error = editor.apply_batch_checked(vec![set], None, validate).unwrap_err();
        assert!(format!("{error:#}").contains("INVALID_COLOR"), "{error:#}");
        assert_eq!(editor.project, project);
    }

    #[test]
    fn word_corrections_are_one_line_of_bounded_text_once_per_word() {
        use nuzky_engine::model::WordCorrection;
        let fix = |original: &str, text: &str| WordCorrection {
            asset_id: "clip".into(),
            source_start_us: 500_000,
            original: original.into(),
            text: text.into(),
        };
        let mut project = Project::new("corrections");
        project.word_corrections = vec![fix("oka", "okna"), fix("to", "tu")];
        // Media removed later leaves its corrections valid: they apply to nothing.
        validate(&project).unwrap();
        let long = "ž".repeat(MAX_CORRECTION_CHARS + 1);
        for bad in [
            fix("oka", ""),
            fix("oka", "  "),
            fix("oka", "two\nlines"),
            fix("oka", "a\u{7}"),
            fix("oka", &long),
            fix("", "okna"),
            WordCorrection { source_start_us: -1, ..fix("x", "y") },
            WordCorrection { asset_id: String::new(), ..fix("x", "y") },
            fix("oka", "okno"),
        ] {
            let mut candidate = project.clone();
            candidate.word_corrections.push(bad.clone());
            let error = validate(&candidate).unwrap_err().to_string();
            assert!(error.starts_with("INVALID_PROJECT:"), "{bad:?}: {error}");
        }
        project.assets.push(Asset {
            id: "clip".into(),
            name: "Clip".into(),
            path: std::env::temp_dir().join("clip.mp4").to_string_lossy().into(),
            kind: AssetKind::Video,
            duration_us: 1_000_000,
            width: 1080,
            height: 1920,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
        });
        let mut editor = nuzky_engine::edit::Editor::new(project.clone());
        let correct = |text: &str| vec![EditCmd::CorrectWords { corrections: vec![fix("word", text)] }];
        let error = editor.apply_batch_checked(correct("two\nlines"), None, validate).unwrap_err().to_string();
        assert!(error.contains("one line"), "{error}");
        assert_eq!(editor.project, project);
        editor.apply_batch_checked(correct("words"), None, validate).unwrap();
        assert_eq!(editor.project.word_corrections.len(), 3);
    }

    #[test]
    fn new_color_adjustments_reject_non_finite_values() {
        let mut project = Project::new("color validation");
        project.assets.push(Asset {
            id: "ramp".into(),
            name: "Ramp".into(),
            path: std::env::temp_dir().join("ramp.ppm").to_string_lossy().into(),
            kind: AssetKind::Image,
            duration_us: 0,
            width: 256,
            height: 16,
            fps: 0.0,
            has_audio: false,
            rotation: 0,
            mirror: false,
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

    #[test]
    fn clean_voice_needs_a_clip_with_sound() {
        let mut project = Project::new("clean voice");
        for (id, kind, sound) in [("photo", AssetKind::Image, false), ("take", AssetKind::Video, true)] {
            project.assets.push(Asset {
                id: id.into(),
                name: id.into(),
                path: std::env::temp_dir().join(id).to_string_lossy().into(),
                kind,
                duration_us: if sound { 2_000_000 } else { 0 },
                width: 16,
                height: 16,
                fps: 30.0,
                has_audio: sound,
                rotation: 0,
                mirror: false,
            });
            project.apply(EditCmd::AddClip { asset_id: id.into(), start_us: None, track_id: None }).unwrap();
        }
        let clean = |project: &mut Project, index: usize| {
            let ClipContent::Media { clean_voice, .. } = &mut project.tracks[0].clips[index].content else { panic!() };
            *clean_voice = true;
        };
        let mut sound = project.clone();
        clean(&mut sound, 1);
        validate(&sound).unwrap();
        clean(&mut project, 0);
        let error = validate(&project).unwrap_err().to_string();
        assert!(error.starts_with("INVALID_PROJECT:") && error.contains("without sound"), "{error}");
    }
}
