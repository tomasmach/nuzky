//! Where transcript words are heard on the timeline. Words belong to media files and are timed
//! in the file's own time; they are placed through the clips that play them, so cuts, speed
//! changes, undo and redo never make a transcript wrong.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::audio::has_audio;
use crate::model::{AssetKind, Clip, ClipContent, Project, Track};

/// A recognised word, timed in its media file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub start_us: i64,
    pub end_us: i64,
    pub text: String,
    /// Mean probability of the constituent text tokens, not calibrated confidence.
    pub probability: f32,
}

/// A word where it is heard on the timeline.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineWord {
    pub start_us: i64,
    pub end_us: i64,
    pub text: String,
    pub probability: f32,
    pub clip_id: String,
    pub asset_id: String,
    /// Where the word starts in its media file.
    pub source_start_us: i64,
}

/// The clip plays speech: a video's own sound on an unmuted track, not silenced. Detached sound
/// counts; the silenced video it came from and imported audio files such as music do not.
pub fn is_heard(project: &Project, track: &Track, clip: &Clip) -> bool {
    let ClipContent::Media { asset_id, volume, .. } = &clip.content else { return false };
    !track.muted && *volume > 0.0 && project.asset(asset_id).is_some_and(|a| a.kind == AssetKind::Video && has_audio(a))
}

fn heard_clips(project: &Project) -> impl Iterator<Item = &Clip> {
    project.tracks.iter().flat_map(move |t| t.clips.iter().filter(move |c| is_heard(project, t, c)))
}

/// Words of every heard clip in timeline order, from transcripts keyed by asset id. A word
/// belongs to the clip whose source range holds its midpoint, so a word cut in two stays with
/// the side that has most of it; its times are clamped to that clip.
pub fn map_words(project: &Project, transcripts: &HashMap<String, Vec<Word>>) -> Vec<TimelineWord> {
    let mut out = Vec::new();
    for clip in heard_clips(project) {
        let ClipContent::Media { asset_id, source_in_us, speed, .. } = &clip.content else { continue };
        let Some(words) = transcripts.get(asset_id) else { continue };
        let speed = *speed as f64;
        let source_end = *source_in_us as f64 + clip.duration_us as f64 * speed;
        let to_timeline = |t: i64| clip.start_us + ((t - source_in_us) as f64 / speed).round() as i64;
        for word in words {
            let mid = (word.start_us + word.end_us) as f64 / 2.0;
            if mid < *source_in_us as f64 || mid >= source_end {
                continue;
            }
            out.push(TimelineWord {
                start_us: to_timeline(word.start_us).max(clip.start_us),
                end_us: to_timeline(word.end_us).min(clip.end_us()),
                text: word.text.clone(),
                probability: word.probability,
                clip_id: clip.id.clone(),
                asset_id: asset_id.clone(),
                source_start_us: word.start_us,
            });
        }
    }
    // Stable, so words heard at the same moment keep the order of their tracks.
    out.sort_by_key(|w| (w.start_us, w.end_us));
    out
}

/// Changes whenever `map_words` could place a word differently, and stays the same through edits
/// that leave the heard clips alone, such as text, transforms, grading or volume above zero.
/// Comparable within one process only.
pub fn speech_layout_key(project: &Project) -> String {
    let mut hasher = DefaultHasher::new();
    for clip in heard_clips(project) {
        if let ClipContent::Media { asset_id, source_in_us, speed, .. } = &clip.content {
            (asset_id, clip.start_us, clip.end_us(), source_in_us, speed.to_bits()).hash(&mut hasher);
        }
    }
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{EditCmd, TimeRange};
    use crate::model::{Asset, TextStyle, Transform};

    const S: i64 = 1_000_000;

    fn asset(id: &str, kind: AssetKind) -> Asset {
        Asset {
            id: id.into(),
            name: id.into(),
            path: format!("/tmp/{id}"),
            kind,
            duration_us: 10 * S,
            width: 1080,
            height: 1920,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        }
    }

    /// A 10 s talking-head video with a word every second ("w0" at 0.2–0.6 s, "w1" at 1.2–1.6 s…).
    fn talk() -> (Project, HashMap<String, Vec<Word>>) {
        let mut p = Project::new("speech");
        p.apply(EditCmd::AddAssets { assets: vec![asset("v", AssetKind::Video), asset("song", AssetKind::Audio)] })
            .unwrap();
        p.apply(EditCmd::AddClip { asset_id: "v".into(), start_us: None, track_id: None }).unwrap();
        let words = (0..10)
            .map(|i| Word {
                start_us: i * S + 200_000,
                end_us: i * S + 600_000,
                text: format!("w{i}"),
                probability: 1.0,
            })
            .collect();
        (p, HashMap::from([("v".to_string(), words)]))
    }

    fn placed(p: &Project, words: &HashMap<String, Vec<Word>>) -> Vec<(String, i64)> {
        map_words(p, words).into_iter().map(|w| (w.text, w.start_us)).collect()
    }

    fn ripple(p: &mut Project, start_us: i64, end_us: i64) {
        p.apply(EditCmd::RippleDeleteRanges {
            ranges: vec![TimeRange { start_us, end_us }],
            keep_track_ids: Some(vec![]),
        })
        .unwrap();
    }

    fn main_id(p: &Project, i: usize) -> String {
        p.tracks[0].clips[i].id.clone()
    }

    #[test]
    fn words_follow_cuts_and_speed() {
        let (mut p, words) = talk();
        ripple(&mut p, 3 * S, 6 * S);
        let second = main_id(&p, 1);
        p.apply(EditCmd::UpdateClip {
            clip_id: second,
            transform: None,
            volume: None,
            text: None,
            style: None,
            speed: Some(2.0),
            adjust: None,
            fade_in_us: None,
            fade_out_us: None,
        })
        .unwrap();
        let got = placed(&p, &words);
        assert_eq!(got[..3], [("w0".into(), 200_000), ("w1".into(), 1_200_000), ("w2".into(), 2_200_000)]);
        // w6 starts 0.2 s into the second clip at source 6 s; at 2x it plays 0.1 s after the cut.
        assert_eq!(got[3], ("w6".into(), 3 * S + 100_000));
        assert_eq!(got.len(), 7);
    }

    #[test]
    fn a_dropped_sliver_moves_every_later_word_with_it() {
        let (mut p, words) = talk();
        // Cutting [20 ms, 1 s) leaves a 20 ms piece, shorter than a frame, which the engine drops.
        ripple(&mut p, 20_000, S);
        assert_eq!(p.tracks[0].clips[0].start_us, 0);
        assert_eq!(placed(&p, &words)[0], ("w1".into(), 200_000));
    }

    #[test]
    fn detached_sound_is_heard_once_and_moves_its_words() {
        let (mut p, words) = talk();
        let video = main_id(&p, 0);
        let sound = p.apply(EditCmd::DetachAudio { clip_id: video }).unwrap().select[0].clone();
        assert_eq!(placed(&p, &words).len(), 10);
        let before = speech_layout_key(&p);
        p.apply(EditCmd::MoveClip { clip_id: sound.clone(), track_id: None, start_us: S }).unwrap();
        assert_eq!(placed(&p, &words)[0], ("w0".into(), 1_200_000));
        assert_ne!(speech_layout_key(&p), before);
        assert!(map_words(&p, &words).iter().all(|w| w.clip_id == sound));
    }

    #[test]
    fn muted_silenced_and_music_clips_say_nothing() {
        let (mut p, words) = talk();
        p.apply(EditCmd::AddClip { asset_id: "song".into(), start_us: Some(0), track_id: None }).unwrap();
        let song_words = HashMap::from([("song".to_string(), words["v"].clone())]);
        assert!(map_words(&p, &song_words).is_empty());
        p.apply(EditCmd::UpdateTrack { track_id: "main".into(), muted: Some(true), hidden: None, keep_in_place: None })
            .unwrap();
        assert!(map_words(&p, &words).is_empty());
    }

    #[test]
    fn duplicated_speech_is_heard_twice() {
        let (mut p, words) = talk();
        let video = main_id(&p, 0);
        p.apply(EditCmd::DuplicateClip { clip_id: video }).unwrap();
        let got = placed(&p, &words);
        assert_eq!(got.len(), 20);
        assert_eq!(got[10], ("w0".into(), 10 * S + 200_000));
    }

    #[test]
    fn the_key_changes_only_when_speech_moves() {
        let (mut p, _) = talk();
        let key = speech_layout_key(&p);
        let style = TextStyle {
            font_family: None,
            font_size: 64.0,
            color: "#ffffff".into(),
            bold: false,
            stroke_width: 0.0,
            stroke_color: "#000000".into(),
            background: None,
            max_width: None,
        };
        p.apply(EditCmd::AddText { start_us: 0, text: "Title".into(), style }).unwrap();
        let video = main_id(&p, 0);
        let zoom = Transform { scale: 1.2, ..Transform::default() };
        p.apply(EditCmd::UpdateClip {
            clip_id: video,
            transform: Some(zoom),
            volume: Some(0.5),
            text: None,
            style: None,
            speed: None,
            adjust: None,
            fade_in_us: None,
            fade_out_us: None,
        })
        .unwrap();
        assert_eq!(speech_layout_key(&p), key);
        ripple(&mut p, 0, S);
        assert_ne!(speech_layout_key(&p), key);
    }
}
