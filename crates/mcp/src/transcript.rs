use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, ensure};
use capopen_analysis::{
    AudioSource, CaptionGrouping, Transcript, Word, group_words, transcribe_words,
};
use capopen_engine::{
    Project,
    audio::{Mixer, ensure_pcm, has_audio, us_to_samples},
    edit::{CaptionSegment, EditCmd},
    model::{ClipContent, TextStyle, TrackKind},
};
use serde_json::{Value, json};

use crate::{jobs::check_cancel, media::check_media};

const AUDIO_CHUNK_FRAMES: usize = 48_000;
const VAD_MODEL: &str = "ggml-silero-v5.1.2.bin";

#[derive(Clone)]
pub struct TranscriptRecord {
    pub target: String,
    pub transcript: Transcript,
    pub project: Project,
}

pub fn models(model: &str) -> Result<(PathBuf, PathBuf)> {
    let direct = PathBuf::from(model);
    let path = if direct.is_absolute() {
        direct
    } else {
        ensure!(
            !model.contains(['/', '\\']) && !model.contains(".."),
            "MODEL_MISSING: use an installed name or absolute path"
        );
        capopen_analysis::models_dir().join(format!("ggml-{model}.bin"))
    };
    ensure!(
        path.is_file(),
        "MODEL_MISSING: {}. Install a local Whisper model or pass its absolute path; no media is uploaded.",
        path.display()
    );
    let vad = path
        .parent()
        .context("Model has no parent directory")?
        .join(VAD_MODEL);
    ensure!(
        vad.is_file(),
        "MODEL_MISSING: voice detector {}",
        vad.display()
    );
    Ok((path, vad))
}

pub fn transcribe(
    project: Project,
    target: String,
    language: &str,
    model: &Path,
    vad: &Path,
    cache: &Path,
    cancel: &AtomicBool,
) -> Result<TranscriptRecord> {
    check_cancel(cancel)?;
    let transcript = if target == "timeline" {
        let audio = timeline_audio(&project, cache, cancel)?;
        check_cancel(cancel)?;
        transcribe_words(AudioSource::TimelineAudio(&audio), model, vad, language)?
    } else {
        let asset = project
            .asset(&target)
            .context("UNKNOWN_ASSET: transcription target")?;
        ensure!(
            Path::new(&asset.path).is_file(),
            "MEDIA_MISSING: {}",
            asset.path
        );
        transcribe_words(AudioSource::Asset { asset, cache }, model, vad, language)?
    };
    check_cancel(cancel)?;
    Ok(TranscriptRecord {
        target,
        transcript,
        project,
    })
}

fn timeline_audio(project: &Project, cache: &Path, cancel: &AtomicBool) -> Result<Vec<f32>> {
    check_media(project)?;
    for asset in project.assets.iter().filter(|a| has_audio(a)) {
        ensure_pcm(cache, asset, |_| {})?;
        check_cancel(cancel)?;
    }
    let frames = us_to_samples(project.duration_us());
    ensure!(frames > 0, "EMPTY_TIMELINE: nothing to transcribe");
    let capacity = usize::try_from(frames / 3).context("Timeline audio too large")?;
    let mut mono = Vec::new();
    mono.try_reserve(capacity)
        .context("Allocating timeline audio")?;
    let mut mixer = Mixer::new(cache.to_path_buf());
    let mut chunk = vec![0.0; AUDIO_CHUNK_FRAMES * 2];
    let mut start = 0;
    while start < frames {
        check_cancel(cancel)?;
        let count = (frames - start).min(AUDIO_CHUNK_FRAMES as i64) as usize;
        mixer.mix(project, start, &mut chunk[..count * 2]);
        mono.extend(
            chunk[..count * 2]
                .chunks(6)
                .map(|s| s.iter().sum::<f32>() / s.len() as f32),
        );
        start += count as i64;
    }
    Ok(mono)
}

fn audio_layout(project: &Project) -> Value {
    json!(project.tracks.iter().filter(|t| t.kind != TrackKind::Text).map(|t| {
        json!({"id": t.id, "muted": t.muted, "clips": t.clips.iter().filter_map(|c| {
            if let ClipContent::Media { asset_id, source_in_us, speed, volume, fade_in_us, fade_out_us, .. } = &c.content {
                Some(json!([asset_id, project.asset(asset_id), c.start_us, c.duration_us, source_in_us, speed, volume, fade_in_us, fade_out_us, c.transition_in]))
            } else { None }
        }).collect::<Vec<_>>()})
    }).collect::<Vec<_>>())
}

pub fn caption_edit(
    record: &TranscriptRecord,
    project: &Project,
    style: TextStyle,
    grouping: CaptionGrouping,
) -> Result<(EditCmd, usize)> {
    ensure!(
        grouping.max_words > 0 && grouping.max_chars > 0,
        "Caption limits must be positive"
    );
    let mut segments = if record.target == "timeline" {
        ensure!(
            audio_layout(&record.project) == audio_layout(project),
            "STALE_TRANSCRIPT: timeline audio changed; transcribe again or use an asset transcript"
        );
        group_words(&record.transcript.words, grouping)
    } else {
        asset_captions(record, project, grouping)?
    };
    for segment in &mut segments {
        segment.end_us = segment.end_us.min(project.duration_us());
    }
    segments.retain(|segment| segment.end_us > segment.start_us);
    let count = segments.len();
    ensure!(
        count > 0,
        "NO_CAPTIONS: no transcript words in retained media"
    );
    let tracks: Vec<_> = project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Text && t.name == "Captions")
        .collect();
    ensure!(
        tracks.len() <= 1,
        "AMBIGUOUS_CAPTIONS: multiple caption tracks; use explicit ReplaceCaptions through apply_edits"
    );
    let edit = match tracks.first() {
        Some(track) => EditCmd::ReplaceCaptions {
            track_id: track.id.clone(),
            segments,
            style,
        },
        None => EditCmd::AddCaptions { segments, style },
    };
    Ok((edit, count))
}

fn asset_captions(
    record: &TranscriptRecord,
    project: &Project,
    grouping: CaptionGrouping,
) -> Result<Vec<CaptionSegment>> {
    ensure!(
        record.project.asset(&record.target) == project.asset(&record.target),
        "STALE_TRANSCRIPT: source asset changed"
    );
    let main = project.tracks.first().context("No main track")?;
    let mut segments = Vec::new();
    for clip in &main.clips {
        let ClipContent::Media {
            asset_id,
            source_in_us,
            speed,
            ..
        } = &clip.content
        else {
            continue;
        };
        if asset_id != &record.target {
            continue;
        }
        let source_end = *source_in_us as f64 + clip.duration_us as f64 * *speed as f64;
        let words: Vec<_> = record
            .transcript
            .words
            .iter()
            .filter_map(|w| {
                let midpoint = (w.start_us as f64 + w.end_us as f64) / 2.0;
                if midpoint < *source_in_us as f64 || midpoint >= source_end {
                    return None;
                }
                let map = |t| {
                    clip.start_us
                        + ((t as f64 - *source_in_us as f64) / *speed as f64).round() as i64
                };
                Some(Word {
                    start_us: map(w.start_us).max(clip.start_us),
                    end_us: map(w.end_us).min(clip.end_us()),
                    ..w.clone()
                })
            })
            .collect();
        let mut grouped = group_words(&words, grouping);
        for caption in &mut grouped {
            caption.end_us = caption.end_us.min(clip.end_us());
        }
        segments.extend(grouped.into_iter().filter(|s| s.end_us > s.start_us));
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use capopen_engine::{
        edit::TimeRange,
        model::{Asset, AssetKind},
    };

    fn record() -> TranscriptRecord {
        let mut p = Project::new("test");
        p.apply(EditCmd::AddAssets {
            assets: vec![Asset {
                id: "a".into(),
                name: "a".into(),
                path: "a.mov".into(),
                kind: AssetKind::Video,
                duration_us: 10_000_000,
                width: 1080,
                height: 1920,
                fps: 30.0,
                has_audio: true,
                rotation: 0,
            }],
        })
        .unwrap();
        p.apply(EditCmd::AddClip {
            asset_id: "a".into(),
            start_us: None,
            track_id: None,
        })
        .unwrap();
        let words = [
            (1_000_000, "First"),
            (4_000_000, "Removed"),
            (7_000_000, "Last"),
        ]
        .into_iter()
        .map(|(t, text)| Word {
            start_us: t,
            end_us: t + 400_000,
            text: text.into(),
            probability: 1.0,
        })
        .collect();
        TranscriptRecord {
            target: "a".into(),
            transcript: Transcript {
                language: "en".into(),
                words,
                segments: vec![],
            },
            project: p,
        }
    }
    fn style() -> TextStyle {
        TextStyle {
            font_family: None,
            font_size: 64.0,
            color: "#ffffff".into(),
            bold: true,
            stroke_width: 4.0,
            stroke_color: "#000000".into(),
            background: None,
        }
    }
    #[test]
    fn source_captions_follow_ripple_and_speed_without_crossing_cuts() {
        let r = record();
        let mut p = r.project.clone();
        p.apply(EditCmd::RippleDeleteRanges {
            ranges: vec![TimeRange {
                start_us: 3_000_000,
                end_us: 6_000_000,
            }],
            keep_track_ids: Some(vec![]),
        })
        .unwrap();
        let id = p.tracks[0].clips[1].id.clone();
        let cmd: EditCmd =
            serde_json::from_value(json!({"type":"updateClip","clipId":id,"speed":2})).unwrap();
        p.apply(cmd).unwrap();
        let (edit, count) = caption_edit(&r, &p, style(), CaptionGrouping::default()).unwrap();
        assert_eq!(count, 2);
        let EditCmd::AddCaptions { segments, .. } = edit else {
            panic!()
        };
        assert_eq!(segments[0].text, "First");
        assert_eq!(segments[1].text, "Last");
        assert_eq!(segments[1].start_us, 3_500_000);
        assert!(segments[0].end_us <= 3_000_000);
    }
    #[test]
    fn caption_replacement_preserves_other_text_and_stale_timeline_is_rejected() {
        let mut r = record();
        let mut p = r.project.clone();
        p.apply(EditCmd::AddText {
            start_us: 0,
            text: "Title".into(),
            style: style(),
        })
        .unwrap();
        let (edit, _) = caption_edit(&r, &p, style(), CaptionGrouping::default()).unwrap();
        p.apply(edit).unwrap();
        let (edit, _) = caption_edit(&r, &p, style(), CaptionGrouping::default()).unwrap();
        assert!(matches!(edit, EditCmd::ReplaceCaptions { .. }));
        p.apply(edit).unwrap();
        assert_eq!(p.tracks.iter().filter(|t| t.name == "Captions").count(), 1);
        assert!(
            p.tracks
                .iter()
                .flat_map(|t| &t.clips)
                .any(|c| matches!(&c.content, ClipContent::Text { text, .. } if text == "Title"))
        );
        r.target = "timeline".into();
        assert!(caption_edit(&r, &p, style(), CaptionGrouping::default()).is_ok());
        p.apply(EditCmd::RippleDeleteRanges {
            ranges: vec![TimeRange {
                start_us: 0,
                end_us: 1_000_000,
            }],
            keep_track_ids: Some(vec![]),
        })
        .unwrap();
        assert!(
            caption_edit(&r, &p, style(), CaptionGrouping::default())
                .unwrap_err()
                .to_string()
                .contains("STALE_TRANSCRIPT")
        );
    }
    #[test]
    fn timeline_caption_tail_does_not_extend_the_video() {
        let mut r = record();
        r.target = "timeline".into();
        r.transcript.words = vec![Word {
            start_us: 9_900_000,
            end_us: 10_000_000,
            text: "End".into(),
            probability: 1.0,
        }];
        let (edit, _) = caption_edit(&r, &r.project, style(), CaptionGrouping::default()).unwrap();
        let EditCmd::AddCaptions { segments, .. } = edit else {
            panic!()
        };
        assert_eq!(segments[0].end_us, r.project.duration_us());
    }
}
