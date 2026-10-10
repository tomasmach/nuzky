//! What a timeline cut in another editor teaches: every recording it hears speech from, in the
//! pieces the editor cut it into. The timeline says exactly where each piece came from, so nothing
//! is found by sound or picture and every piece matches fully. B-roll teaches no speech rules.

use std::path::Path;

use nuzky_interchange::{ImportedTimeline, MediaClip, TrackKind, Transform};

use super::{Alignment, Caption, Framing, Picture, Piece, Place, Source, ZoomChange, pauses_between};
use crate::Word;

/// Framing is read this often within a clip and this far from its ends, as in a finished video.
const SAMPLE_US: i64 = 500_000;
const EDGE_US: i64 = 200_000;
/// A smaller change of scale is not a zoom.
const MIN_ZOOM: f32 = 0.03;

/// A recording the timeline plays, with the words recognised in it.
pub struct Recording<'a> {
    /// As the timeline's clips name it.
    pub path: &'a Path,
    pub name: String,
    pub language: String,
    pub duration_us: i64,
    pub words: &'a [Word],
}

/// What one recording of a timeline teaches.
#[derive(Clone, Debug)]
pub struct Lesson {
    pub recording: String,
    pub cut: String,
    pub language: String,
    pub recording_us: i64,
    pub words: Vec<Word>,
    pub cut_words: Vec<Word>,
    pub alignment: Alignment,
    pub picture: Picture,
    pub places: Vec<Place>,
}

impl Lesson {
    pub fn source(&self) -> Source<'_> {
        Source {
            recording: self.recording.clone(),
            cut: self.cut.clone(),
            language: self.language.clone(),
            recording_us: self.recording_us,
            words: &self.words,
            cut_words: &self.cut_words,
            alignment: &self.alignment,
            picture: &self.picture,
            places: Some(&self.places),
            corrections: &[],
        }
    }
}

/// The media whose sound the timeline plays, in the order first heard: everything on its audio
/// tracks and on its lowest video track. Video above that lies over it, as B-roll does.
pub fn heard_media(timeline: &ImportedTimeline) -> Vec<&Path> {
    let lowest = timeline.tracks.iter().find(|t| t.kind == TrackKind::Video && !t.clips.is_empty());
    let mut clips: Vec<&MediaClip> = timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio || lowest.is_some_and(|l| std::ptr::eq(*t, l)))
        .flat_map(|t| &t.clips)
        .collect();
    clips.sort_by_key(|c| c.start_us);
    let mut out: Vec<&Path> = Vec::new();
    for path in clips.iter().filter_map(|c| c.path.as_deref()) {
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// The clips that play a recording's sound: on the lowest audio track that has it, as editors put a
/// camera's sound beside its picture, or else on the lowest video track. Other tracks with the same
/// recording only repeat it, such as its second channel.
pub fn speech_clips<'t>(timeline: &'t ImportedTimeline, path: &Path) -> Vec<&'t MediaClip> {
    let of = |kind| {
        timeline.tracks.iter().filter(move |t| t.kind == kind).find_map(|t| {
            let clips: Vec<&MediaClip> =
                t.clips.iter().filter(|c| c.path.as_deref() == Some(path) && c.speed > 0.0).collect();
            (!clips.is_empty()).then_some(clips)
        })
    };
    let mut clips = of(TrackKind::Audio).or_else(|| of(TrackKind::Video)).unwrap_or_default();
    clips.retain(|c| c.volume != Some(0.0));
    clips
}

/// What each recording teaches, in their order, or why it teaches nothing. `cut` names the timeline.
pub fn lessons(timeline: &ImportedTimeline, cut: &str, recordings: &[Recording]) -> Vec<Result<Lesson, &'static str>> {
    let clips: Vec<Vec<&MediaClip>> = recordings.iter().map(|r| speech_clips(timeline, r.path)).collect();
    // Every recording word the timeline plays: its piece, the word in timeline time and its index.
    let heard: Vec<Vec<(usize, Word, usize)>> = recordings.iter().zip(&clips).map(|(r, c)| heard(r.words, c)).collect();
    let mut out = Vec::new();
    for (k, recording) in recordings.iter().enumerate() {
        let clips = &clips[k];
        let mut places: Vec<Place> = vec![None; recording.words.len()];
        for (piece, word, i) in &heard[k] {
            places[*i].get_or_insert((*piece, word.start_us, word.end_us));
        }
        if recording.words.is_empty() {
            out.push(Err("no speech was recognised in it"));
            continue;
        }
        if clips.is_empty() {
            out.push(Err("the timeline does not play its sound"));
            continue;
        }
        if heard[k].is_empty() {
            out.push(Err("none of its speech plays in the timeline"));
            continue;
        }
        if clips.len() == 1 && places.iter().all(Option::is_some) {
            out.push(Err("the timeline plays all of it, so nothing was cut"));
            continue;
        }
        let cut_words: Vec<Word> = heard[k].iter().map(|(_, w, _)| w.clone()).collect();
        // A silence of this recording is no pause while another one speaks in it.
        let others: Vec<&Word> =
            heard.iter().enumerate().filter(|(j, _)| *j != k).flat_map(|(_, h)| h.iter().map(|(_, w, _)| w)).collect();
        let cut_pauses = pauses_between(&cut_words)
            .into_iter()
            .filter(|p| !others.iter().any(|w| w.start_us < p.end_us && p.start_us < w.end_us))
            .collect();
        let pieces: Vec<Piece> = clips
            .iter()
            .map(|c| Piece { start_us: c.start_us, end_us: c.end_us(), offset_us: c.source_in_us - c.start_us })
            .collect();
        out.push(Ok(Lesson {
            recording: recording.name.clone(),
            cut: cut.to_owned(),
            language: recording.language.clone(),
            recording_us: recording.duration_us,
            words: recording.words.to_vec(),
            cut_words,
            alignment: Alignment {
                matched: 1.0,
                cut_duration_us: clips.iter().map(|c| c.duration_us).sum(),
                cut_pauses,
                recording_pauses: pauses_between(recording.words),
                pieces,
            },
            picture: picture(timeline, recording.path, clips),
            places,
        }));
    }
    out
}

/// The words each clip plays, moved to the timeline as `engine::speech::map_words` does: a word
/// is heard when its middle is.
fn heard(words: &[Word], clips: &[&MediaClip]) -> Vec<(usize, Word, usize)> {
    let mut out = Vec::new();
    for (piece, clip) in clips.iter().enumerate() {
        let to_timeline = |t: i64| clip.start_us + ((t - clip.source_in_us) as f64 / clip.speed).round() as i64;
        for (i, word) in words.iter().enumerate() {
            let middle = (word.start_us + word.end_us) / 2;
            if middle < clip.source_in_us || middle >= clip.source_out_us {
                continue;
            }
            let start_us = to_timeline(word.start_us).max(clip.start_us);
            let end_us = to_timeline(word.end_us).min(clip.end_us());
            out.push((piece, Word { start_us, end_us, ..word.clone() }, i));
        }
    }
    out.sort_by_key(|(_, w, _)| (w.start_us, w.end_us));
    out
}

/// Captions said over the recording and how its picture is framed, as far as the timeline says.
fn picture(timeline: &ImportedTimeline, path: &Path, heard: &[&MediaClip]) -> Picture {
    let spoken = |t: i64| heard.iter().any(|c| c.start_us <= t && t < c.end_us());
    let texts: Vec<_> =
        timeline.tracks.iter().flat_map(|t| &t.texts).filter(|c| spoken(c.start_us + c.duration_us / 2)).collect();
    let mut captions: Vec<Caption> = texts
        .iter()
        .map(|c| Caption {
            start_us: c.start_us,
            end_us: c.start_us + c.duration_us,
            words: c.text.split_whitespace().count(),
        })
        .collect();
    captions.sort_by_key(|c| c.start_us);
    let mut heights: Vec<f32> = texts.iter().filter_map(|c| c.transform).map(|t| 0.5 + t.y).collect();
    heights.sort_by(f32::total_cmp);
    let caption_band = heights.get(heights.len() / 2).map(|&middle| (middle, middle));
    // The recording's picture: its clips on the lowest video track that shows it.
    let pictured: Vec<&MediaClip> = timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .map(|t| t.clips.iter().filter(|c| c.path.as_deref() == Some(path)).collect::<Vec<_>>())
        .find(|c| !c.is_empty())
        .unwrap_or_default();
    let framed = pictured.iter().any(|c| c.transform.is_some() || !c.keyframes.is_empty());
    if caption_band.is_none() && !framed {
        let skipped = Some("the timeline does not say where its captions sit or how its picture is framed".into());
        return Picture { skipped, captions, ..Picture::default() };
    }
    let mut framing = Vec::new();
    let mut zooms = Vec::new();
    for (i, clip) in pictured.iter().enumerate() {
        let mut t = clip.start_us + EDGE_US;
        while t < clip.end_us() - EDGE_US {
            let at = transform_at(clip, t);
            framing.push(Framing { time_us: t, scale: at.scale, x: at.x, y: at.y });
            t += SAMPLE_US;
        }
        if let Some(next) = pictured.get(i + 1).filter(|n| n.start_us == clip.end_us()) {
            let (from, to) = (transform_at(clip, clip.end_us() - 1).scale, transform_at(next, next.start_us).scale);
            if (to - from).abs() >= MIN_ZOOM {
                zooms.push(ZoomChange { start_us: next.start_us, end_us: next.start_us, from, to, at_cut: true });
            }
        }
        for pair in clip.keyframes.windows(2) {
            let (from, to) = (pair[0].transform.scale, pair[1].transform.scale);
            if (to - from).abs() >= MIN_ZOOM {
                let (start_us, end_us) = (clip.start_us + pair[0].at_us, clip.start_us + pair[1].at_us);
                zooms.push(ZoomChange { start_us, end_us, from, to, at_cut: false });
            }
        }
    }
    zooms.sort_by_key(|z| z.start_us);
    Picture { skipped: None, captions, caption_band, framing, zooms }
}

/// The clip's framing at timeline time `t`, moving straight from one keyframe to the next.
fn transform_at(clip: &MediaClip, t: i64) -> Transform {
    let base = clip.transform.unwrap_or(Transform { scale: 1.0, x: 0.0, y: 0.0 });
    let t = t - clip.start_us;
    let next = clip.keyframes.iter().position(|k| k.at_us > t);
    match (next, clip.keyframes.last()) {
        (_, None) => base,
        (None, Some(last)) => last.transform,
        (Some(0), _) => clip.keyframes[0].transform,
        (Some(n), _) => {
            let (a, b) = (clip.keyframes[n - 1], clip.keyframes[n]);
            let f = (t - a.at_us) as f32 / (b.at_us - a.at_us) as f32;
            let mix = |x: f32, y: f32| x + (y - x) * f;
            Transform {
                scale: mix(a.transform.scale, b.transform.scale),
                x: mix(a.transform.x, b.transform.x),
                y: mix(a.transform.y, b.transform.y),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use nuzky_interchange::{Keyframe, TextClip, Track};

    use super::*;

    const S: i64 = 1_000_000;

    fn clip(path: &str, start_us: i64, source_in_us: i64, duration_us: i64, speed: f64) -> MediaClip {
        MediaClip {
            name: path.into(),
            media: path.into(),
            path: Some(PathBuf::from(path)),
            source_in_us,
            source_out_us: source_in_us + (duration_us as f64 * speed) as i64,
            start_us,
            duration_us,
            speed,
            volume: None,
            transform: None,
            keyframes: Vec::new(),
            transition_in: None,
        }
    }

    fn track(kind: TrackKind, clips: Vec<MediaClip>) -> Track {
        Track { name: String::new(), kind, clips, texts: Vec::new() }
    }

    /// A word every second, "w0" at 0.2 to 0.6 s, "w1" at 1.2 to 1.6 s and so on.
    fn words(prefix: &str, count: i64) -> Vec<Word> {
        (0..count)
            .map(|i| Word {
                start_us: i * S + 200_000,
                end_us: i * S + 600_000,
                text: format!("{prefix}{i}"),
                probability: 1.0,
            })
            .collect()
    }

    fn recording<'a>(path: &'a str, words: &'a [Word]) -> Recording<'a> {
        Recording { path: Path::new(path), name: path.into(), language: "en".into(), duration_us: 20 * S, words }
    }

    #[test]
    fn pieces_and_words_come_straight_from_the_cut() {
        let talk = words("w", 20);
        let timeline = ImportedTimeline {
            tracks: vec![
                // The camera's picture cuts away early, to B-roll above it.
                track(
                    TrackKind::Video,
                    vec![clip("talk.mov", 0, 2 * S, 2 * S, 1.0), clip("talk.mov", 4 * S, 10 * S, 2 * S, 2.0)],
                ),
                track(TrackKind::Video, vec![clip("broll.mov", 2 * S, 0, 2 * S, 1.0)]),
                // Its sound goes on under the B-roll: what is heard is cut here. The second channel repeats it.
                track(
                    TrackKind::Audio,
                    vec![clip("talk.mov", 0, 2 * S, 3 * S, 1.0), clip("talk.mov", 4 * S, 10 * S, 2 * S, 2.0)],
                ),
                track(TrackKind::Audio, vec![clip("talk.mov", 0, 0, 20 * S, 1.0)]),
            ],
            ..ImportedTimeline::default()
        };
        assert_eq!(heard_media(&timeline), [Path::new("talk.mov")], "B-roll above the main track is not heard");
        let broll: Vec<Word> = Vec::new();
        let taught = lessons(&timeline, "Reel", &[recording("talk.mov", &talk), recording("broll.mov", &broll)]);
        let lesson = taught[0].as_ref().unwrap();
        assert_eq!(taught[1].as_ref().unwrap_err(), &"no speech was recognised in it");
        assert_eq!(
            lesson.alignment.pieces,
            [
                Piece { start_us: 0, end_us: 3 * S, offset_us: 2 * S },
                Piece { start_us: 4 * S, end_us: 6 * S, offset_us: 6 * S }
            ]
        );
        assert_eq!((lesson.alignment.matched, lesson.alignment.cut_duration_us), (1.0, 5 * S));
        let kept: Vec<&str> = lesson.cut_words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(kept, ["w2", "w3", "w4", "w10", "w11", "w12", "w13"], "the second piece plays 4 s of talk in 2 s");
        // "w11" is said 11.2 to 11.6 s into the recording: 0.6 s into a piece played twice as fast from 10 s at 4 s.
        assert_eq!(lesson.places[11], Some((1, 4_600_000, 4_800_000)));
        assert_eq!(lesson.places[5], None);
        assert_eq!(lesson.cut_words.len(), lesson.places.iter().flatten().count());
        assert!(lesson.alignment.recording_pauses.len() == 19 && !lesson.alignment.cut_pauses.is_empty());
        assert!(lesson.picture.skipped.is_some(), "the timeline does not say how the picture is framed");
    }

    #[test]
    fn each_recording_is_its_own_source_and_another_voice_is_no_pause() {
        let (a, b) = (words("a", 10), words("b", 10));
        let timeline = ImportedTimeline {
            tracks: vec![track(
                TrackKind::Video,
                vec![
                    clip("a.mov", 0, 0, 2 * S, 1.0),
                    clip("b.mov", 2 * S, 0, 2 * S, 1.0),
                    clip("a.mov", 4 * S, 5 * S, 2 * S, 1.0),
                ],
            )],
            ..ImportedTimeline::default()
        };
        let taught = lessons(&timeline, "Talk", &[recording("a.mov", &a), recording("b.mov", &b)]);
        let (first, second) = (taught[0].as_ref().unwrap(), taught[1].as_ref().unwrap());
        assert_eq!(first.alignment.pieces.len(), 2);
        assert_eq!(second.alignment.pieces, [Piece { start_us: 2 * S, end_us: 4 * S, offset_us: -2 * S }]);
        // a1 ends at 1.6 s and a5 starts at 4.2 s, while b speaks between them.
        assert!(
            first.alignment.cut_pauses.iter().all(|p| p.end_us - p.start_us < S),
            "{:?}",
            first.alignment.cut_pauses
        );
        assert_eq!(second.cut.as_str(), "Talk");
    }

    #[test]
    fn a_recording_played_whole_teaches_no_cutting() {
        let talk = words("w", 5);
        let timeline = ImportedTimeline {
            tracks: vec![track(TrackKind::Video, vec![clip("t.mov", 0, 0, 5 * S, 1.0)])],
            ..Default::default()
        };
        let taught = lessons(&timeline, "Reel", &[recording("t.mov", &talk), recording("gone.mov", &talk)]);
        assert_eq!(taught[0].as_ref().unwrap_err(), &"the timeline plays all of it, so nothing was cut");
        assert_eq!(taught[1].as_ref().unwrap_err(), &"the timeline does not play its sound");
    }

    #[test]
    fn captions_and_zoom_when_the_timeline_says_them() {
        let talk = words("w", 20);
        let framed = |scale: f32| Some(Transform { scale, x: 0.0, y: 0.0 });
        let mut first = clip("t.mov", 0, 0, 2 * S, 1.0);
        first.transform = framed(1.0);
        let mut second = clip("t.mov", 2 * S, 5 * S, 2 * S, 1.0);
        second.transform = framed(1.25);
        let mut third = clip("t.mov", 4 * S, 10 * S, 2 * S, 1.0);
        third.keyframes = vec![
            Keyframe { at_us: 0, transform: Transform { scale: 1.0, x: 0.0, y: 0.0 } },
            Keyframe { at_us: S, transform: Transform { scale: 1.2, x: 0.0, y: 0.1 } },
        ];
        let text = |start_us, text: &str| TextClip {
            text: text.into(),
            start_us,
            duration_us: 500_000,
            transform: Some(Transform { scale: 1.0, x: 0.0, y: 0.15 }),
        };
        let mut titles = track(TrackKind::Video, Vec::new());
        titles.texts = vec![text(0, "two words"), text(S, "three short words"), text(9 * S, "after the cut")];
        let timeline = ImportedTimeline {
            tracks: vec![track(TrackKind::Video, vec![first, second, third]), titles],
            ..Default::default()
        };
        let lesson = lessons(&timeline, "Reel", &[recording("t.mov", &talk)]).remove(0).unwrap();
        let picture = &lesson.picture;
        assert_eq!(picture.skipped, None);
        assert_eq!(
            picture.captions.iter().map(|c| c.words).collect::<Vec<_>>(),
            [2, 3],
            "only captions said over the recording"
        );
        let (top, bottom) = picture.caption_band.unwrap();
        assert!((top - 0.65).abs() < 1e-6 && top == bottom, "{top}");
        let scales: Vec<(i64, f32)> = picture.framing.iter().map(|f| (f.time_us, f.scale)).collect();
        assert_eq!(
            &scales[..5],
            [(200_000, 1.0), (700_000, 1.0), (1_200_000, 1.0), (1_700_000, 1.0), (2_200_000, 1.25)]
        );
        let between = scales.iter().find(|(t, _)| *t == 4_700_000).unwrap().1;
        assert!((between - 1.14).abs() < 1e-5, "70% of the way between keyframes: {scales:?}");
        let zooms: Vec<_> = picture.zooms.iter().map(|z| (z.start_us, z.end_us, z.at_cut)).collect();
        assert_eq!(zooms, [(2 * S, 2 * S, true), (4 * S, 4 * S, true), (4 * S, 5 * S, false)]);
    }
}
