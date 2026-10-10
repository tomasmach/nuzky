use std::collections::HashSet;
use std::path::{Component, Path, Prefix};

use anyhow::{Context, Result, ensure};
use nuzky_engine::{
    Project,
    model::{
        Asset, AssetKind, Background, Clip, ClipContent, MAX_BORDER_WIDTH, MAX_CORRECTION_CHARS, MAX_FONT_HEIGHT_RATIO,
        MAX_REEL_HOOK_CHARS, MAX_REEL_TITLE_CHARS, MAX_REEL_WHY_CHARS, MAX_TEXT_WIDTH_RATIO, PROJECT_VERSION,
        ReelCandidate, ReelStatus, TextStyle, Thumbnail, TrackKind, Transform, max_stroke_width,
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
        if let Some(credit) = &asset.credit {
            sound_credit(credit).with_context(|| format!("INVALID_PROJECT: credit of asset {}", asset.id))?;
        }
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
    word_corrections(project)?;
    thumbnails(project)?;
    reel_candidates(project)
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

/// The app shows these and opens their links, and export writes them beside the video: short single
/// lines, and links only to web pages.
fn sound_credit(credit: &nuzky_engine::model::Credit) -> Result<()> {
    let text = |value: &str, max: usize| value.chars().count() <= max && !value.chars().any(char::is_control);
    let link = |value: &str| text(value, 2048) && value.starts_with("https://") && !value.contains(char::is_whitespace);
    ensure!(
        text(&credit.source, 32)
            && text(&credit.id, 200)
            && text(&credit.title, 300)
            && text(&credit.author, 300)
            && text(&credit.license_version, 16),
        "text too long or with control characters"
    );
    ensure!(link(&credit.license_url), "licence link must be an https address");
    ensure!(credit.url.is_empty() || link(&credit.url), "source link must be an https address");
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

/// Sizes are bounded by the picture the text is drawn on: the canvas, or a thumbnail.
fn text_style(style: &TextStyle, (width, height): (u32, u32)) -> Result<()> {
    ensure!(
        style.font_size.is_finite()
            && style.font_size > 0.0
            && style.font_size <= MAX_FONT_HEIGHT_RATIO * height as f32
            && style.stroke_width.is_finite()
            && style.stroke_width >= 0.0
            && style.stroke_width <= max_stroke_width(style.font_size)
            && style
                .max_width
                .is_none_or(|max| max.is_finite() && max > 0.0 && max <= MAX_TEXT_WIDTH_RATIO * width as f32),
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
    Ok(())
}

/// More text layers than any thumbnail needs, and more characters than one shows.
const MAX_THUMBNAIL_TEXTS: usize = 20;
const MAX_THUMBNAIL_CHARS: usize = 500;

/// A thumbnail's frame may lie past the end of the video after later edits; the renderer says so then.
fn thumbnails(project: &Project) -> Result<()> {
    let mut formats = HashSet::new();
    for thumbnail in &project.thumbnails {
        ensure!(formats.insert(thumbnail.format), "INVALID_PROJECT: two thumbnails of one format");
        self::thumbnail(thumbnail)?;
    }
    Ok(())
}

fn thumbnail(thumbnail: &Thumbnail) -> Result<()> {
    ensure!(thumbnail.time_us >= 0, "INVALID_PROJECT: a thumbnail's frame time must not be negative");
    transform(&thumbnail.frame)?;
    let background = &thumbnail.background;
    ensure!(
        (0.0..=1.0).contains(&background.blur) && (0.0..=1.0).contains(&background.dim),
        "INVALID_PROJECT: a thumbnail's background blur and dim go from 0 to 1"
    );
    color(&background.color, "thumbnail background")?;
    ensure!(
        thumbnail.texts.len() <= MAX_THUMBNAIL_TEXTS,
        "INVALID_PROJECT: a thumbnail has at most {MAX_THUMBNAIL_TEXTS} texts"
    );
    for text in &thumbnail.texts {
        ensure!(
            text.text.chars().count() <= MAX_THUMBNAIL_CHARS,
            "INVALID_PROJECT: a thumbnail text has at most {MAX_THUMBNAIL_CHARS} characters"
        );
        text_style(&text.style, thumbnail.format.size())?;
        transform(&text.transform)?;
    }
    if let Some(outline) = &thumbnail.outline {
        ensure!(
            outline.width > 0.0 && outline.width <= MAX_BORDER_WIDTH,
            "INVALID_PROJECT: a thumbnail's outline is wider than 0 and at most {MAX_BORDER_WIDTH} px"
        );
        color(&outline.color, "thumbnail outline")?;
    }
    Ok(())
}

/// More reel candidates than a long video has moments, so a project file cannot grow without bound.
const MAX_REEL_CANDIDATES: usize = 200;

/// Candidates not made yet never share words or time; a made one's words may have moved since. A made one names
/// its project, which the app may open, so that is an absolute path; texts are single lines the app shows as they are.
fn reel_candidates(project: &Project) -> Result<()> {
    let candidates = &project.reel_candidates;
    ensure!(
        candidates.len() <= MAX_REEL_CANDIDATES,
        "INVALID_PROJECT: more than {MAX_REEL_CANDIDATES} reel candidates"
    );
    unique(candidates.iter().map(|c| c.id.as_str()), "reel candidate")?;
    let line = |text: &str, max: usize| text.chars().count() <= max && !text.chars().any(char::is_control);
    for c in candidates {
        ensure!(line(&c.id, 64), "INVALID_PROJECT: a reel candidate's id is one line of up to 64 characters");
        ensure!(
            !c.title.trim().is_empty() && line(&c.title, MAX_REEL_TITLE_CHARS),
            "INVALID_PROJECT: a reel's title is one line of 1 to {MAX_REEL_TITLE_CHARS} characters"
        );
        ensure!(
            line(&c.why, MAX_REEL_WHY_CHARS) && line(&c.hook, MAX_REEL_HOOK_CHARS),
            "INVALID_PROJECT: a reel's reason is one line of up to {MAX_REEL_WHY_CHARS} characters, its hook of up to {MAX_REEL_HOOK_CHARS}"
        );
        ensure!(
            c.from <= c.to && c.start_us >= 0 && c.start_us < c.end_us && c.duration_us > 0,
            "INVALID_PROJECT: reel candidate {} has no words or time",
            c.id
        );
        ensure!(
            c.score.is_finite() && (0.0..=1.0).contains(&c.score),
            "INVALID_PROJECT: a reel's score goes from 0 to 1"
        );
        ensure!(
            (c.status == ReelStatus::Made) == c.project_path.is_some(),
            "INVALID_PROJECT: a made reel names its project, and only a made one"
        );
        if let Some(path) = &c.project_path {
            ensure!(
                line(path, 4096) && Path::new(path).is_absolute(),
                "INVALID_PROJECT: a reel's project must be an absolute path"
            );
        }
        if let Some(cover) = &c.thumbnail {
            thumbnail(cover)?;
        }
    }
    let mut open: Vec<&ReelCandidate> = candidates.iter().filter(|c| c.status != ReelStatus::Made).collect();
    open.sort_by_key(|c| c.start_us);
    for pair in open.windows(2) {
        ensure!(
            pair[0].to < pair[1].from && pair[0].end_us <= pair[1].start_us,
            "INVALID_PROJECT: reel candidates {} and {} overlap",
            pair[0].id,
            pair[1].id
        );
    }
    Ok(())
}

/// Only a drawn video or image clip has something behind its person; an image to show must be an image.
fn background_of(
    project: &Project,
    clip: &Clip,
    kind: TrackKind,
    asset: &Asset,
    background: &Background,
) -> Result<()> {
    if background.is_none() {
        return Ok(());
    }
    ensure!(
        kind == TrackKind::Video && asset.kind != AssetKind::Audio,
        "INVALID_PROJECT: only video and image clips have a background, not {}",
        clip.id
    );
    match background {
        Background::None => {}
        Background::Blur { strength } => ensure!(
            strength.is_finite() && *strength > 0.0 && *strength <= 1.0,
            "INVALID_PROJECT: the background blur of {} needs a strength above 0 and at most 1",
            clip.id
        ),
        Background::Color { color: value } => color(value, "background color")?,
        Background::Image { asset_id } => ensure!(
            project.asset(asset_id).is_some_and(|a| a.kind == AssetKind::Image),
            "INVALID_PROJECT: the background of {} must be an image of the project",
            clip.id
        ),
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
            background,
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
            background_of(project, clip, kind, asset, background)?;
        }
        ClipContent::Text { style, transform: t, words, .. } => {
            text_style(style, (project.canvas.width, project.canvas.height))?;
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
            credit: None,
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

    /// A shared project could carry links the app would open; only web pages pass.
    #[test]
    fn sound_credits_link_only_to_web_pages() {
        let mut project = Project::new("credits");
        project.assets.push(Asset {
            id: "song".into(),
            name: "Song".into(),
            path: std::env::temp_dir().join("song.mp3").to_string_lossy().into(),
            kind: AssetKind::Audio,
            duration_us: 1_000_000,
            width: 0,
            height: 0,
            fps: 0.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
            credit: Some(nuzky_engine::model::Credit {
                source: "openverse".into(),
                id: "b386828e".into(),
                title: "Lofy".into(),
                author: "macouno".into(),
                license: nuzky_engine::model::License::CcBy,
                license_version: "3.0".into(),
                license_url: "https://creativecommons.org/licenses/by/3.0/".into(),
                url: "https://www.jamendo.com/track/317391".into(),
            }),
        });
        validate(&project).unwrap();
        for bad in ["javascript:alert(1)", "file:///etc/passwd", "http://example.org", "https://a b"] {
            project.assets[0].credit.as_mut().unwrap().url = bad.into();
            let error = format!("{:#}", validate(&project).unwrap_err());
            assert!(error.starts_with("INVALID_PROJECT: credit of asset song"), "{bad}: {error}");
        }
        project.assets[0].credit.as_mut().unwrap().url = String::new();
        project.assets[0].credit.as_mut().unwrap().title = "line\nbreak".into();
        assert!(validate(&project).is_err());
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
            credit: None,
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
            credit: None,
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
    fn thumbnails_are_one_per_format_with_bounded_text_background_and_outline() {
        use nuzky_engine::model::{Thumbnail, ThumbnailFormat};
        let mut project = Project::new("covers");
        let presets: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../assets/presets/thumbnails.json")).unwrap();
        let texts: Vec<_> = presets
            .iter()
            .map(|p| serde_json::json!({"text": "HOOK", "style": p["style"], "behind": p["behind"]}))
            .collect();
        let cover = serde_json::json!({"format": "cover_9x16", "timeUs": 0, "texts": texts,
            "outline": {"color": "#ffffff", "width": 10}});
        project.thumbnails = vec![serde_json::from_value(cover).unwrap()];
        // Every preset is a valid style on a cover.
        validate(&project).unwrap();
        let youtube = |change: fn(&mut Thumbnail)| {
            let mut candidate = project.clone();
            let mut thumbnail = candidate.thumbnails[0].clone();
            thumbnail.format = ThumbnailFormat::Youtube16x9;
            change(&mut thumbnail);
            candidate.thumbnails.push(thumbnail);
            validate(&candidate)
        };
        youtube(|_| {}).unwrap();
        // Font sizes are bounded by the thumbnail, here twice the 720 px of a YouTube thumbnail.
        youtube(|t| t.texts[0].style.font_size = 1440.0).unwrap();
        type Break = fn(&mut Thumbnail);
        let broken: [(&str, Break); 12] = [
            ("format", |t| t.format = ThumbnailFormat::Cover9x16),
            ("time", |t| t.time_us = -1),
            ("blur", |t| t.background.blur = 1.5),
            ("dim", |t| t.background.dim = f32::NAN),
            ("colour", |t| t.background.color = "black".into()),
            ("font", |t| t.texts[0].style.font_size = 1441.0),
            ("text colour", |t| t.texts[1].style.background = Some("#€".into())),
            ("frame", |t| t.frame.scale = 0.0),
            ("outline", |t| t.outline.as_mut().unwrap().width = 101.0),
            ("outline colour", |t| t.outline.as_mut().unwrap().color = String::new()),
            ("texts", |t| t.texts = vec![t.texts[0].clone(); 21]),
            ("characters", |t| t.texts[0].text = "x".repeat(501)),
        ];
        for (what, change) in broken {
            let error = youtube(change).unwrap_err().to_string();
            assert!(error.starts_with("INVALID_PROJECT") || error.starts_with("INVALID_COLOR"), "{what}: {error}");
        }
    }

    /// A shared project can carry anything here, and the app shows these texts and may open the path.
    #[test]
    fn reel_candidates_stay_apart_with_single_line_texts_and_a_path_only_once_made() {
        use nuzky_engine::model::ReelCandidate;
        let candidate = |id: &str, from: usize, to: usize| -> ReelCandidate {
            serde_json::from_value(
                serde_json::json!({"id": id, "from": from, "to": to, "startUs": from as i64 * 1_000_000,
                "endUs": to as i64 * 1_000_000 + 500_000, "title": "Why sleep wins", "hook": "Sleep is a cheat code.",
                "why": "One claim and its proof.", "durationUs": 20_000_000, "score": 0.8, "status": "proposed"}),
            )
            .unwrap()
        };
        let mut project = Project::new("reels");
        project.reel_candidates = vec![candidate("b", 40, 60), candidate("a", 0, 39)];
        validate(&project).unwrap();
        let with = |change: fn(&mut Vec<ReelCandidate>)| {
            let mut changed = project.clone();
            change(&mut changed.reel_candidates);
            validate(&changed)
        };
        // A made one may overlap, since cuts may have moved its words.
        with(|c| {
            c[0].status = nuzky_engine::model::ReelStatus::Made;
            c[0].project_path = Some("/videos/talk-reel-1.nuzky".into());
            c[0].from = 30;
        })
        .unwrap();
        type Break = fn(&mut Vec<ReelCandidate>);
        let broken: [(&str, Break); 10] = [
            ("shared words", |c| c[0].from = 39),
            ("shared time", |c| c[0].start_us = 39_000_000),
            ("same id", |c| c[0].id = "a".into()),
            ("title", |c| c[0].title = " ".into()),
            ("two lines", |c| c[0].why = "One\nTwo".into()),
            ("score", |c| c[0].score = f32::NAN),
            ("no time", |c| c[0].end_us = c[0].start_us),
            ("path before made", |c| c[0].project_path = Some("/videos/x.nuzky".into())),
            ("made without path", |c| c[0].status = nuzky_engine::model::ReelStatus::Made),
            ("relative path", |c| {
                c[0].status = nuzky_engine::model::ReelStatus::Made;
                c[0].project_path = Some("x.nuzky".into());
            }),
        ];
        for (what, change) in broken {
            let error = with(change).unwrap_err().to_string();
            assert!(error.starts_with("INVALID_PROJECT"), "{what}: {error}");
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
                credit: None,
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
