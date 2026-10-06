use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use capopen_analysis::{CaptionGrouping, group_words};
use capopen_engine::{
    Project,
    edit::{EditCmd, TimeRange},
    model::{ClipContent, TextStyle, TrackKind},
    speech::{TimelineWord, Word, is_heard, map_words},
};
use capopen_session::transcripts::TranscriptStore;
use serde_json::{Value, json};

const VAD_MODEL: &str = "ggml-silero-v5.1.2.bin";
pub const BEFORE_WORD_US: i64 = 80_000;
pub const AFTER_WORD_US: i64 = 120_000;
const SENTENCE_GAP_US: i64 = 600_000;
const PAUSE_GAP_US: i64 = 300_000;

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


pub fn best_model() -> &'static str {
    if capopen_analysis::models_dir().join("ggml-large-v3-turbo-q5_0.bin").is_file() {
        "large-v3-turbo-q5_0"
    } else {
        "small"
    }
}

pub fn heard_assets(project: &Project) -> HashSet<String> {
    project.tracks.iter().flat_map(|track| track.clips.iter().filter_map(|clip| {
        if is_heard(project, track, clip)
            && let ClipContent::Media { asset_id, .. } = &clip.content
        { Some(asset_id.clone()) } else { None }
    })).collect()
}

pub struct Derived {
    pub sources: HashMap<String, Vec<Word>>,
    pub words: Vec<TimelineWord>,
    pub untranscribed: Vec<String>,
}

pub fn derive(project: &Project, store: &TranscriptStore) -> Result<Derived> {
    let mut sources = HashMap::new();
    let mut untranscribed = Vec::new();
    let heard = heard_assets(project);
    for asset in project.assets.iter().filter(|a| heard.contains(&a.id)) {
        match store.get(asset)? {
            Some(record) => { sources.insert(asset.id.clone(), record.words); }
            None => untranscribed.push(asset.id.clone()),
        }
    }
    Ok(Derived { words: map_words(project, &sources), sources, untranscribed })
}

pub fn summary(derived: &Derived, range: Option<[i64; 2]>) -> Result<Value> {
    if let Some([start, end]) = range {
        ensure!(start >= 0 && end > start, "INVALID_RANGE: expected [start_us,end_us)");
    }
    let visible = |start, end| range.is_none_or(|[a, b]| start < b && end > a);
    let words = &derived.words;
    let numbered: Vec<_> = words.iter().enumerate().filter(|(_, w)| visible(w.start_us, w.end_us))
        .map(|(i, w)| json!({"i":i,"start_us":w.start_us,"end_us":w.end_us,"text":w.text,"p":w.probability})).collect();
    let mut sentences = Vec::new();
    let mut from = 0;
    for (i, word) in words.iter().enumerate() {
        if word.text.trim_end().ends_with(['.', ',', '!', '?', ';', ':', '…'])
            || words.get(i + 1).is_none_or(|next| next.start_us - word.end_us >= SENTENCE_GAP_US)
        {
            let start = words[from].start_us;
            if visible(start, word.end_us) {
                sentences.push(json!({"from":from,"to":i,"start_us":start,"end_us":word.end_us,
                    "text":words[from..=i].iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ")}));
            }
            from = i + 1;
        }
    }
    let pauses: Vec<_> = words.windows(2).enumerate().filter_map(|(i, pair)| {
        let gap = pair[1].start_us - pair[0].end_us;
        (gap >= PAUSE_GAP_US && visible(pair[0].end_us, pair[1].start_us))
            .then(|| json!({"after_word":i,"gap_us":gap}))
    }).collect();
    Ok(json!({"words":numbered,"sentences":sentences,"pauses":pauses,"untranscribed":derived.untranscribed}))
}

pub fn deletion_ranges(
    words: &[TimelineWord], duration: i64, delete: Option<&[[usize; 2]]>,
    keep: Option<&[[usize; 2]]>, pause: i64,
) -> Result<Vec<TimeRange>> {
    ensure!(delete.is_none() || keep.is_none(), "INVALID_SELECTION: use delete OR keep");
    ensure!(pause >= 0, "INVALID_PAUSE: shorten_pauses_us must be nonnegative");
    ensure!(!words.is_empty(), "NO_WORDS: transcribe heard assets before editing");
    let mut kept = vec![keep.is_none(); words.len()];
    for &[from, to] in delete.or(keep).unwrap_or(&[]) {
        ensure!(from <= to && to < words.len(), "INVALID_WORD_RANGE: inclusive word indices out of bounds");
        kept[from..=to].fill(keep.is_some());
    }
    let mut ranges = Vec::new();
    let mut add = |start_us, end_us| {
        if end_us > start_us { ranges.push(TimeRange { start_us, end_us }); }
    };
    let retained: Vec<_> = kept.iter().enumerate().filter_map(|(i, keep)| keep.then_some(i)).collect();
    let Some(&first) = retained.first() else {
        return Ok(vec![TimeRange { start_us: 0, end_us: duration }]);
    };
    let leading = if first == 0 { BEFORE_WORD_US.min(pause) } else {
        BEFORE_WORD_US.min((words[first].start_us - words[first - 1].end_us).max(0) / 2)
    };
    add(0, words[first].start_us - leading);
    for pair in retained.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (end, start) = (words[a].end_us, words[b].start_us);
        if b == a + 1 {
            if start - end > pause {
                add(end + pause / 2, start - (pause - pause / 2));
            }
        } else {
            let after = AFTER_WORD_US.min((words[a + 1].start_us - end).max(0) / 2);
            let before = BEFORE_WORD_US.min((start - words[b - 1].end_us).max(0) / 2);
            add(end + after, start - before);
        }
    }
    if let Some(&last) = retained.last() {
        let trailing = if last + 1 == words.len() { AFTER_WORD_US.min(pause) } else {
            AFTER_WORD_US.min((words[last + 1].start_us - words[last].end_us).max(0) / 2)
        };
        add(words[last].end_us + trailing, duration);
    }
    // Simultaneous speakers cannot always be cut independently by a global ripple edit.
    for range in &ranges {
        ensure!(words.iter().all(|w| ![range.start_us, range.end_us].iter()
            .any(|t| w.start_us < *t && *t < w.end_us)),
            "OVERLAPPING_SPEECH: requested cut would land inside a word");
        ensure!(words.iter().zip(&kept).all(|(w, keep)| !keep || w.end_us <= range.start_us || w.start_us >= range.end_us),
            "OVERLAPPING_SPEECH: requested cut would remove retained speech");
    }
    for (word, keep) in words.iter().zip(&kept) {
        ensure!(*keep || ranges.iter().any(|r| r.start_us <= word.start_us && r.end_us >= word.end_us),
            "OVERLAPPING_SPEECH: removed word overlaps retained speech");
    }
    Ok(ranges)
}

pub fn caption_edit(
    words: &[TimelineWord], project: &Project, style: TextStyle, grouping: CaptionGrouping,
) -> Result<(EditCmd, usize)> {
    ensure!(grouping.max_words > 0 && grouping.max_chars > 0, "INVALID_GROUPING: caption limits must be positive");
    let mut segments = Vec::new();
    for clip in project.tracks.iter().flat_map(|t| &t.clips) {
        let clip_words: Vec<_> = words.iter().filter(|w| w.clip_id == clip.id).map(|w| Word {
            start_us: w.start_us, end_us: w.end_us, text: w.text.clone(), probability: w.probability,
        }).collect();
        let mut grouped = group_words(&clip_words, grouping);
        for segment in &mut grouped { segment.end_us = segment.end_us.min(clip.end_us()); }
        segments.extend(grouped.into_iter().filter(|s| s.end_us > s.start_us));
    }
    segments.sort_by_key(|s| s.start_us);
    let count = segments.len();
    ensure!(count > 0, "NO_CAPTIONS: no transcript words in retained media");
    let tracks: Vec<_> = project.tracks.iter().filter(|t| t.kind == TrackKind::Text && t.name == "Captions").collect();
    ensure!(tracks.len() <= 1, "AMBIGUOUS_CAPTIONS: multiple caption tracks; use explicit replaceCaptions");
    let edit = match tracks.first() {
        Some(track) => EditCmd::ReplaceCaptions { track_id: track.id.clone(), segments, style },
        None => EditCmd::AddCaptions { segments, style },
    };
    Ok((edit, count))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use capopen_engine::model::{Asset, AssetKind};

    pub fn fixture() -> (Project, HashMap<String, Vec<Word>>) {
        let mut project = Project::new("speech test");
        project.apply(EditCmd::AddAssets { assets: vec![Asset {
            id: "talk".into(), name: "talk".into(), path: "talk.mov".into(), kind: AssetKind::Video,
            duration_us: 10_000_000, width: 1080, height: 1920, fps: 30.0, has_audio: true, rotation: 0,
        }] }).unwrap();
        project.apply(EditCmd::AddClip { asset_id: "talk".into(), start_us: None, track_id: None }).unwrap();
        let words = (0..8).map(|i| Word {
            start_us: 500_000 + i * 1_000_000, end_us: 900_000 + i * 1_000_000,
            text: format!("word{i}{}", if i % 3 == 2 { "." } else { "" }), probability: 0.95,
        }).collect();
        (project, HashMap::from([("talk".into(), words)]))
    }

    #[test]
    fn every_selection_preserves_whole_words_and_keep_equals_delete() {
        let (project, sources) = fixture();
        let words = map_words(&project, &sources);
        for mask in 0..256 {
            let keep: Vec<_> = (0..8).filter(|i| mask & (1 << i) != 0).map(|i| [i,i]).collect();
            let delete: Vec<_> = (0..8).filter(|i| mask & (1 << i) == 0).map(|i| [i,i]).collect();
            let ranges = deletion_ranges(&words, project.duration_us(), None, Some(&keep), 300_000).unwrap();
            let other = deletion_ranges(&words, project.duration_us(), Some(&delete), None, 300_000).unwrap();
            assert_eq!(serde_json::to_value(&ranges).unwrap(), serde_json::to_value(other).unwrap());
            for range in &ranges {
                for boundary in [range.start_us, range.end_us] {
                    assert!(words.iter().all(|w| boundary <= w.start_us || boundary >= w.end_us));
                }
            }
            let mut result = project.clone();
            result.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: None }).unwrap();
            let actual = map_words(&result, &sources);
            assert_eq!(actual.iter().map(|w| &w.text).collect::<Vec<_>>(), keep.iter().map(|r| &words[r[0]].text).collect::<Vec<_>>());
            assert!(actual.iter().all(|w| w.end_us - w.start_us == 400_000));
        }
    }

    #[test]
    fn shorten_pauses_and_trim_edges() {
        let (mut project, sources) = fixture();
        let words = map_words(&project, &sources);
        let ranges = deletion_ranges(&words, project.duration_us(), None, None, 300_000).unwrap();
        project.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: None }).unwrap();
        let words = map_words(&project, &sources);
        assert_eq!(words[0].start_us, BEFORE_WORD_US);
        assert!(words.windows(2).all(|p| p[1].start_us - p[0].end_us == 300_000));
        assert_eq!(project.duration_us() - words.last().unwrap().end_us, AFTER_WORD_US);
    }

    #[test]
    fn get_transcript_and_captions_follow_cuts_speed_and_detached_sound() {
        let (mut project, sources) = fixture();
        project.apply(EditCmd::RippleDeleteRanges { ranges: vec![TimeRange { start_us: 3_000_000, end_us: 6_000_000 }], keep_track_ids: None }).unwrap();
        let id = project.tracks[0].clips[1].id.clone();
        project.apply(serde_json::from_value(json!({"type":"updateClip","clipId":id,"speed":2})).unwrap()).unwrap();
        project.apply(EditCmd::DetachAudio { clip_id: id }).unwrap();
        let words = map_words(&project, &sources);
        assert_eq!(words.len(), 5);
        assert_eq!(words[3].start_us, 3_250_000);
        let data = summary(&Derived { sources, words: words.clone(), untranscribed: vec![] }, Some([3_000_000,5_000_000])).unwrap();
        assert_eq!(data["words"][0]["i"], 3);
        assert_eq!(data["words"][0]["text"], "word6");
        let (edit, _) = caption_edit(&words, &project, crate::params::reel_style(), CaptionGrouping { max_words: 100, max_chars: 1000, ..CaptionGrouping::default() }).unwrap();
        let EditCmd::AddCaptions { segments, .. } = edit else { panic!() };
        for segment in segments {
            assert!(project.tracks.iter().flat_map(|t| &t.clips).any(|c| {
                words.iter().any(|w| w.clip_id == c.id) && segment.start_us >= c.start_us && segment.end_us <= c.end_us()
            }));
            assert!(!(segment.start_us < 3_000_000 && segment.end_us > 3_000_000));
        }
    }

    #[test]
    fn reject_overlapping_speech_instead_of_cutting_a_word() {
        let (project, sources) = fixture();
        let mut words = map_words(&project, &sources);
        words[0].end_us = words[1].start_us + 1;
        assert!(deletion_ranges(&words, project.duration_us(), Some(&[[1,1]]), None, 300_000).unwrap_err().to_string().contains("OVERLAPPING_SPEECH"));
    }
}
