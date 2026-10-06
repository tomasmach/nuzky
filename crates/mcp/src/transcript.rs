use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, ensure};
use capopen_analysis::{AudioSource, CaptionGrouping, group_words};
use capopen_engine::{
    Project,
    edit::{EditCmd, TimeRange, merge_ranges},
    model::{Asset, ClipContent, TextStyle, TrackKind},
    speech::{TimelineWord, Word, is_heard, map_words},
};
use capopen_session::{
    jobs::check_cancel,
    transcripts::{Record, Segment, TranscriptStore, VERSION},
};
use serde::Serialize;
use serde_json::{Value, json};

use capopen_analysis::VAD_MODEL;
pub const BEFORE_WORD_US: i64 = 80_000;
pub const AFTER_WORD_US: i64 = 120_000;
const SENTENCE_GAP_US: i64 = 600_000;
const PAUSE_GAP_US: i64 = 300_000;
pub const DEFAULT_PAUSE_US: i64 = 300_000;

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
    let vad = path.parent().context("Model has no parent directory")?.join(VAD_MODEL);
    ensure!(vad.is_file(), "MODEL_MISSING: voice detector {}", vad.display());
    Ok((path, vad))
}

pub fn best_model() -> &'static str {
    if capopen_analysis::models_dir().join("ggml-large-v3-turbo-q5_0.bin").is_file() {
        "large-v3-turbo-q5_0"
    } else {
        "small"
    }
}

/// Recognises the whole file in its own time and stores its words, replacing an older record.
pub fn recognise(
    store: &TranscriptStore,
    asset: &Asset,
    cache: &Path,
    model: &str,
    (model_path, vad): &(PathBuf, PathBuf),
    language: &str,
    cancel: &AtomicBool,
) -> Result<Record> {
    let fingerprint = store.fingerprint(asset)?;
    let result = capopen_analysis::transcribe_words_cancellable(
        AudioSource::Asset { asset, cache },
        model_path,
        vad,
        language,
        || cancel.load(Ordering::Relaxed),
    )?;
    check_cancel(cancel)?;
    let record = Record {
        version: VERSION,
        fingerprint,
        duration_us: asset.duration_us,
        model: model.into(),
        language: result.language,
        words: result.words,
        segments: result
            .segments
            .into_iter()
            .map(|s| Segment { start_us: s.start_us, end_us: s.end_us, text: s.text })
            .collect(),
    };
    store.put(asset, &record)?;
    Ok(record)
}

pub fn heard_assets(project: &Project) -> HashSet<String> {
    project
        .tracks
        .iter()
        .flat_map(|track| {
            track.clips.iter().filter_map(|clip| {
                if is_heard(project, track, clip)
                    && let ClipContent::Media { asset_id, .. } = &clip.content
                {
                    Some(asset_id.clone())
                } else {
                    None
                }
            })
        })
        .collect()
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
            Some(record) => {
                sources.insert(asset.id.clone(), record.words);
            }
            None => untranscribed.push(asset.id.clone()),
        }
    }
    Ok(Derived { words: map_words(project, &sources), sources, untranscribed })
}

/// Word indices also change on re-recognition, even when the timeline layout stays put.
pub fn word_key(project: &Project, words: &[TimelineWord]) -> String {
    let mut hash = DefaultHasher::new();
    for word in words {
        (&word.asset_id, word.source_start_us, word.start_us, word.end_us, &word.text).hash(&mut hash);
    }
    format!("{}:{:016x}", capopen_engine::speech::speech_layout_key(project), hash.finish())
}

pub fn summary(derived: &Derived, range: Option<[i64; 2]>) -> Result<Value> {
    if let Some([start, end]) = range {
        ensure!(start >= 0 && end > start, "INVALID_RANGE: expected [start_us,end_us)");
    }
    let visible = |start, end| range.is_none_or(|[a, b]| start < b && end > a);
    let words = &derived.words;
    let numbered: Vec<_> = words
        .iter()
        .enumerate()
        .filter(|(_, w)| visible(w.start_us, w.end_us))
        .map(|(i, w)| json!({"i":i,"start_us":w.start_us,"end_us":w.end_us,"text":w.text,"p":w.probability}))
        .collect();
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
    let pauses: Vec<_> = words
        .windows(2)
        .enumerate()
        .filter_map(|(i, pair)| {
            let gap = pair[1].start_us - pair[0].end_us;
            (gap >= PAUSE_GAP_US && visible(pair[0].end_us, pair[1].start_us))
                .then(|| json!({"after_word":i,"gap_us":gap}))
        })
        .collect();
    Ok(json!({"words":numbered,"sentences":sentences,"pauses":pauses,"untranscribed":derived.untranscribed}))
}

struct WordFragment {
    timeline: TimeRange,
    source_start: f64,
    source_end: f64,
    owner: Option<usize>,
}

/// Midpoint ownership numbers a word once; editing must also preserve its other audible pieces.
/// Also returns where speech is: the clips playing part of a word. Cuts stay inside it, so
/// B-roll and other clips without words are never cut.
fn editing_bounds(project: &Project, derived: &Derived) -> (Vec<TimelineWord>, Vec<TimeRange>, Vec<TimeRange>) {
    let mut bounds = derived.words.clone();
    let mut unnumbered = Vec::new();
    let mut speech = Vec::new();
    let owners: HashMap<_, _> = derived
        .words
        .iter()
        .enumerate()
        .map(|(i, w)| ((w.clip_id.as_str(), w.source_start_us, w.text.as_str()), i))
        .collect();
    for (asset, words) in &derived.sources {
        for word in words {
            let mut fragments = Vec::new();
            for track in &project.tracks {
                for clip in track.clips.iter().filter(|c| is_heard(project, track, c)) {
                    let ClipContent::Media { asset_id, source_in_us, speed, .. } = &clip.content else { continue };
                    if asset_id != asset {
                        continue;
                    }
                    let speed = f64::from(*speed);
                    let source_start = (word.start_us as f64).max(*source_in_us as f64);
                    let source_end = (word.end_us as f64).min(*source_in_us as f64 + clip.duration_us as f64 * speed);
                    if source_end <= source_start {
                        continue;
                    }
                    let map = |t: f64| clip.start_us + ((t - *source_in_us as f64) / speed).round() as i64;
                    fragments.push(WordFragment {
                        timeline: TimeRange {
                            start_us: map(source_start).max(clip.start_us),
                            end_us: map(source_end).min(clip.end_us()),
                        },
                        source_start,
                        source_end,
                        owner: owners.get(&(clip.id.as_str(), word.start_us, word.text.as_str())).copied(),
                    });
                    speech.push(TimeRange { start_us: clip.start_us, end_us: clip.end_us() });
                }
            }
            fragments.sort_by_key(|f| (f.timeline.start_us, f.timeline.end_us));
            let mut from = 0;
            for i in 0..fragments.len() {
                let current = &fragments[i];
                if fragments.get(i + 1).is_none_or(|next| {
                    current.timeline.end_us != next.timeline.start_us || current.source_end != next.source_start
                }) {
                    let interval =
                        TimeRange { start_us: fragments[from].timeline.start_us, end_us: current.timeline.end_us };
                    let mut numbered = false;
                    for owner in fragments[from..=i].iter().filter_map(|f| f.owner) {
                        bounds[owner].start_us = interval.start_us;
                        bounds[owner].end_us = interval.end_us;
                        numbered = true;
                    }
                    if !numbered {
                        unnumbered.push(interval);
                    }
                    from = i + 1;
                }
            }
        }
    }
    (bounds, unnumbered, merge_ranges(speech))
}

/// Timeline ranges that delete or keep the inclusive word ranges and, with `pause`, shorten
/// longer pauses to it and trim the silence before the first and after the last word.
pub fn edit_ranges(
    project: &Project,
    derived: &Derived,
    delete: Option<&[[usize; 2]]>,
    keep: Option<&[[usize; 2]]>,
    pause: Option<i64>,
) -> Result<Vec<TimeRange>> {
    Ok(speech_cut(project, derived, delete, keep, pause)?.0)
}

/// The ranges of `edit_ranges` and where speech is.
fn speech_cut(
    project: &Project,
    derived: &Derived,
    delete: Option<&[[usize; 2]]>,
    keep: Option<&[[usize; 2]]>,
    pause: Option<i64>,
) -> Result<(Vec<TimeRange>, Vec<TimeRange>)> {
    let (bounds, unnumbered, speech) = editing_bounds(project, derived);
    let mut ranges = deletion_ranges(&bounds, &speech, delete, keep, pause)?;
    // A surviving fragment whose midpoint was cut away has no selectable index. Keep it audible.
    for protected in unnumbered {
        ranges = ranges
            .into_iter()
            .flat_map(|range| {
                if range.end_us <= protected.start_us || range.start_us >= protected.end_us {
                    return vec![range];
                }
                let mut pieces = Vec::new();
                if range.start_us < protected.start_us {
                    pieces.push(TimeRange { start_us: range.start_us, end_us: protected.start_us });
                }
                if range.end_us > protected.end_us {
                    pieces.push(TimeRange { start_us: protected.end_us, end_us: range.end_us });
                }
                pieces
            })
            .collect();
    }
    Ok((ranges, speech))
}

/// A silence in speech longer than the pause length; shortening it cuts `start_us..end_us`
/// out of the whole `gap_us`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pause {
    pub start_us: i64,
    pub end_us: i64,
    pub gap_us: i64,
}

/// What shortening every pause to `pause_us` cuts, in timeline order.
pub fn pauses(project: &Project, derived: &Derived, pause_us: i64) -> Result<Vec<Pause>> {
    let (ranges, speech) = speech_cut(project, derived, None, None, Some(pause_us))?;
    Ok(ranges
        .into_iter()
        .map(|r| {
            let span = speech.iter().find(|s| s.start_us <= r.start_us && r.end_us <= s.end_us).copied().unwrap_or(r);
            let from =
                derived.words.iter().map(|w| w.end_us).filter(|&t| t <= r.start_us).fold(span.start_us, i64::max);
            let to = derived.words.iter().map(|w| w.start_us).filter(|&t| t >= r.end_us).fold(span.end_us, i64::min);
            Pause { start_us: r.start_us, end_us: r.end_us, gap_us: to - from }
        })
        .collect())
}

pub fn deletion_ranges(
    words: &[TimelineWord],
    speech: &[TimeRange],
    delete: Option<&[[usize; 2]]>,
    keep: Option<&[[usize; 2]]>,
    pause: Option<i64>,
) -> Result<Vec<TimeRange>> {
    ensure!(delete.is_none() || keep.is_none(), "INVALID_SELECTION: use delete OR keep");
    ensure!(pause.is_none_or(|p| p >= 0), "INVALID_PAUSE: shorten_pauses_us must be nonnegative");
    let (Some(first_span), Some(last_span)) = (speech.first(), speech.last()) else {
        anyhow::bail!("NO_WORDS: transcribe heard assets before editing");
    };
    ensure!(!words.is_empty(), "NO_WORDS: transcribe heard assets before editing");
    let (start, end) = (first_span.start_us, last_span.end_us);
    let mut kept = vec![keep.is_none(); words.len()];
    for &[from, to] in delete.or(keep).unwrap_or(&[]) {
        ensure!(from <= to && to < words.len(), "INVALID_WORD_RANGE: inclusive word indices out of bounds");
        kept[from..=to].fill(keep.is_some());
    }
    let mut ranges = Vec::new();
    let mut add = |start_us, end_us| {
        if end_us > start_us {
            ranges.push(TimeRange { start_us, end_us });
        }
    };
    let retained: Vec<_> = kept.iter().enumerate().filter_map(|(i, keep)| keep.then_some(i)).collect();
    let (Some(&first), Some(&last)) = (retained.first(), retained.last()) else {
        return Ok(speech.to_vec());
    };
    if first > 0 {
        add(
            start,
            words[first].start_us - BEFORE_WORD_US.min((words[first].start_us - words[first - 1].end_us).max(0) / 2),
        );
    } else if let Some(pause) = pause
        && words[0].start_us - start > pause
    {
        add(start, words[0].start_us - BEFORE_WORD_US.min(pause));
    }
    for pair in retained.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (gap_start, gap_end) = (words[a].end_us, words[b].start_us);
        if b > a + 1 {
            let after = AFTER_WORD_US.min((words[a + 1].start_us - gap_start).max(0) / 2);
            let before = BEFORE_WORD_US.min((gap_end - words[b - 1].end_us).max(0) / 2);
            add(gap_start + after, gap_end - before);
        } else if let Some(pause) = pause
            && gap_end - gap_start > pause
        {
            add(gap_start + pause / 2, gap_end - (pause - pause / 2));
        }
    }
    if last + 1 < words.len() {
        add(words[last].end_us + AFTER_WORD_US.min((words[last + 1].start_us - words[last].end_us).max(0) / 2), end);
    } else if let Some(pause) = pause
        && end - words[last].end_us > pause
    {
        add(words[last].end_us + AFTER_WORD_US.min(pause), end);
    }
    let ranges: Vec<_> = ranges
        .into_iter()
        .flat_map(|r| {
            speech.iter().filter_map(move |s| {
                let (start_us, end_us) = (r.start_us.max(s.start_us), r.end_us.min(s.end_us));
                (end_us > start_us).then_some(TimeRange { start_us, end_us })
            })
        })
        .collect();
    // Simultaneous speakers cannot always be cut independently by a global ripple edit.
    for range in &ranges {
        ensure!(
            words.iter().all(|w| ![range.start_us, range.end_us].iter().any(|t| w.start_us < *t && *t < w.end_us)),
            "OVERLAPPING_SPEECH: requested cut would land inside a word"
        );
        ensure!(
            words.iter().zip(&kept).all(|(w, keep)| !keep || w.end_us <= range.start_us || w.start_us >= range.end_us),
            "OVERLAPPING_SPEECH: requested cut would remove retained speech"
        );
    }
    for (word, keep) in words.iter().zip(&kept) {
        ensure!(
            *keep || ranges.iter().any(|r| r.start_us <= word.start_us && r.end_us >= word.end_us),
            "OVERLAPPING_SPEECH: removed word overlaps retained speech"
        );
    }
    Ok(ranges)
}

/// A cut planned from `key` must still match what is heard, with every heard file recognised.
pub fn check_key(project: &Project, derived: &Derived, key: &str) -> Result<()> {
    ensure!(derived.untranscribed.is_empty(), "TRANSCRIPT_MISSING: transcribe all heard assets before cutting");
    ensure!(
        key == word_key(project, &derived.words),
        "SPEECH_CHANGED: speech changed or wrong key; use get_transcript's transcript_key (get_state's key is for apply_edits)"
    );
    Ok(())
}

pub struct Cut {
    pub edit: EditCmd,
    pub ranges: Vec<TimeRange>,
    /// The project once cut, and where its words are then.
    pub preview: Project,
    pub words: Vec<TimelineWord>,
}

/// One ripple edit removing `ranges`, checked to remove exactly the words they cover.
pub fn plan_cut(project: &Project, derived: &Derived, ranges: Vec<TimeRange>) -> Result<Cut> {
    let edit = EditCmd::RippleDeleteRanges { ranges: ranges.clone(), keep_track_ids: None };
    let mut preview = project.clone();
    preview.apply(edit.clone()).context("EDIT_REJECTED: preview failed")?;
    capopen_session::validate(&preview)?;
    let words = map_words(&preview, &derived.sources);
    let identities = |words: &[TimelineWord]| {
        let mut ids: Vec<_> = words.iter().map(|w| (w.asset_id.clone(), w.source_start_us, w.text.clone())).collect();
        ids.sort();
        ids
    };
    let retained: Vec<_> = derived
        .words
        .iter()
        .filter(|w| !ranges.iter().any(|r| r.start_us <= w.start_us && r.end_us >= w.end_us && w.start_us < r.end_us))
        .cloned()
        .collect();
    ensure!(
        identities(&words) == identities(&retained),
        "UNSAFE_CUT: kept tracks or sub-frame fragments would change the selected words"
    );
    Ok(Cut { edit, ranges, preview, words })
}

pub fn caption_edit(
    words: &[TimelineWord],
    project: &Project,
    style: TextStyle,
    grouping: CaptionGrouping,
) -> Result<(EditCmd, usize)> {
    ensure!(grouping.max_words > 0 && grouping.max_chars > 0, "INVALID_GROUPING: caption limits must be positive");
    let mut segments = Vec::new();
    let min_caption_us = project.frame_duration_us().ceil() as i64;
    let mut by_clip: HashMap<&str, Vec<Word>> = HashMap::new();
    for word in words {
        by_clip.entry(&word.clip_id).or_default().push(Word {
            start_us: word.start_us,
            end_us: word.end_us,
            text: word.text.clone(),
            probability: word.probability,
        });
    }
    for clip in project.tracks.iter().flat_map(|t| &t.clips).filter(|c| c.duration_us >= min_caption_us) {
        let Some(clip_words) = by_clip.get(clip.id.as_str()) else { continue };
        let mut grouped = group_words(clip_words, grouping);
        for segment in &mut grouped {
            segment.end_us = segment.end_us.min(clip.end_us());
            // The engine enforces a one-frame minimum; borrow time before a short tail word.
            segment.start_us = segment.start_us.min(segment.end_us - min_caption_us).max(clip.start_us);
        }
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
        project
            .apply(EditCmd::AddAssets {
                assets: vec![Asset {
                    id: "talk".into(),
                    name: "talk".into(),
                    path: "talk.mov".into(),
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
        project.apply(EditCmd::AddClip { asset_id: "talk".into(), start_us: None, track_id: None }).unwrap();
        let words = (0..8)
            .map(|i| Word {
                start_us: 500_000 + i * 1_000_000,
                end_us: 900_000 + i * 1_000_000,
                text: format!("word{i}{}", if i % 3 == 2 { "." } else { "" }),
                probability: 0.95,
            })
            .collect();
        (project, HashMap::from([("talk".into(), words)]))
    }

    fn whole(project: &Project) -> [TimeRange; 1] {
        [TimeRange { start_us: 0, end_us: project.duration_us() }]
    }

    #[test]
    fn cutting_words_alone_keeps_the_other_pauses_and_the_edges() {
        let (project, sources) = fixture();
        let words = map_words(&project, &sources);
        let ranges = deletion_ranges(&words, &whole(&project), Some(&[[3, 4]]), None, None).unwrap();
        assert_eq!(
            ranges,
            vec![TimeRange { start_us: words[2].end_us + AFTER_WORD_US, end_us: words[5].start_us - BEFORE_WORD_US }]
        );
        assert!(deletion_ranges(&words, &whole(&project), None, None, None).unwrap().is_empty());
    }

    #[test]
    fn pauses_report_the_whole_silence_they_shorten() {
        let (project, sources) = fixture();
        let derived = Derived { words: map_words(&project, &sources), sources, untranscribed: vec![] };
        let found = pauses(&project, &derived, 300_000).unwrap();
        // Before the first word, the seven gaps between words and after the last one.
        assert_eq!(found.len(), 9);
        assert_eq!(found[0], Pause { start_us: 0, end_us: 500_000 - BEFORE_WORD_US, gap_us: 500_000 });
        assert_eq!(found[1], Pause { start_us: 1_050_000, end_us: 1_350_000, gap_us: 600_000 });
        assert_eq!(found[8].gap_us, 10_000_000 - 7_900_000);
        // Silence no longer than the pause stays, at the edges too.
        assert_eq!(pauses(&project, &derived, 600_000).unwrap().len(), 1);
    }

    #[test]
    fn pauses_and_cuts_never_touch_clips_without_words() {
        let (mut project, mut sources) = fixture();
        project
            .apply(EditCmd::AddAssets {
                assets: vec![Asset {
                    id: "broll".into(),
                    name: "broll".into(),
                    path: "broll.mov".into(),
                    kind: AssetKind::Video,
                    duration_us: 6_000_000,
                    width: 1080,
                    height: 1920,
                    fps: 30.0,
                    has_audio: true,
                    rotation: 0,
                }],
            })
            .unwrap();
        // B-roll with ambient sound between two takes, and again on an overlay over the end.
        let take = project.tracks[0].clips[0].id.clone();
        project.apply(EditCmd::SplitClip { clip_id: take, at_us: 5_000_000 }).unwrap();
        project
            .apply(EditCmd::AddClip { asset_id: "broll".into(), start_us: Some(5_000_000), track_id: None })
            .unwrap();
        let mut overlay = project.tracks[0].clips[1].clone();
        (overlay.id, overlay.start_us) = ("overlay".into(), 14_000_000);
        project.tracks.push(capopen_engine::model::Track {
            id: "over".into(),
            kind: TrackKind::Video,
            name: "Overlay".into(),
            muted: false,
            hidden: false,
            keep_in_place: false,
            clips: vec![overlay],
        });
        sources.insert("broll".into(), vec![]);
        let derived = Derived { words: map_words(&project, &sources), sources, untranscribed: vec![] };
        let main = |p: &Project| p.tracks[0].clips.iter().map(|c| (c.start_us, c.end_us())).collect::<Vec<_>>();
        assert_eq!(main(&project), [(0, 5_000_000), (5_000_000, 11_000_000), (11_000_000, 16_000_000)]);
        let found = pauses(&project, &derived, 300_000).unwrap();
        assert!(
            found.iter().all(|p| p.end_us <= 5_000_000 || p.start_us >= 11_000_000 && p.end_us <= 16_000_000),
            "{found:?}"
        );
        // The take after the B-roll starts with 0.5 s of silence, not the 6.6 s since the last word.
        assert!(found.contains(&Pause { start_us: 11_000_000, end_us: 11_350_000, gap_us: 500_000 }));
        for ranges in [
            found.iter().map(|p| TimeRange { start_us: p.start_us, end_us: p.end_us }).collect(),
            edit_ranges(&project, &derived, Some(&[[4, 5]]), None, None).unwrap(),
        ] {
            let cut = plan_cut(&project, &derived, ranges).unwrap();
            let main = &cut.preview.tracks[0].clips;
            let broll: Vec<_> = main
                .iter()
                .filter(|c| matches!(&c.content, ClipContent::Media { asset_id, .. } if asset_id == "broll"))
                .map(|c| c.duration_us)
                .collect();
            assert_eq!(broll, [6_000_000]);
            let over = cut.preview.tracks[1].clips.last().unwrap();
            assert_eq!(over.end_us() - main.last().unwrap().end_us(), 4_000_000);
        }
    }

    #[test]
    fn every_selection_preserves_whole_words_and_keep_equals_delete() {
        let (project, sources) = fixture();
        let words = map_words(&project, &sources);
        for mask in 0..256 {
            let keep: Vec<_> = (0..8).filter(|i| mask & (1 << i) != 0).map(|i| [i, i]).collect();
            let delete: Vec<_> = (0..8).filter(|i| mask & (1 << i) == 0).map(|i| [i, i]).collect();
            let ranges = deletion_ranges(&words, &whole(&project), None, Some(&keep), Some(300_000)).unwrap();
            let other = deletion_ranges(&words, &whole(&project), Some(&delete), None, Some(300_000)).unwrap();
            assert_eq!(serde_json::to_value(&ranges).unwrap(), serde_json::to_value(other).unwrap());
            for range in &ranges {
                for boundary in [range.start_us, range.end_us] {
                    assert!(words.iter().all(|w| boundary <= w.start_us || boundary >= w.end_us));
                }
            }
            let mut result = project.clone();
            result.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: None }).unwrap();
            let actual = map_words(&result, &sources);
            assert_eq!(
                actual.iter().map(|w| &w.text).collect::<Vec<_>>(),
                keep.iter().map(|r| &words[r[0]].text).collect::<Vec<_>>()
            );
            assert!(actual.iter().all(|w| w.end_us - w.start_us == 400_000));
        }
    }

    #[test]
    fn shorten_pauses_and_trim_edges() {
        let (mut project, sources) = fixture();
        let words = map_words(&project, &sources);
        let ranges = deletion_ranges(&words, &whole(&project), None, None, Some(300_000)).unwrap();
        project.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: None }).unwrap();
        let words = map_words(&project, &sources);
        assert_eq!(words[0].start_us, BEFORE_WORD_US);
        assert!(words.windows(2).all(|p| p[1].start_us - p[0].end_us == 300_000));
        assert_eq!(project.duration_us() - words.last().unwrap().end_us, AFTER_WORD_US);
    }

    #[test]
    fn get_transcript_and_captions_follow_cuts_speed_and_detached_sound() {
        let (mut project, sources) = fixture();
        project
            .apply(EditCmd::RippleDeleteRanges {
                ranges: vec![TimeRange { start_us: 3_000_000, end_us: 6_000_000 }],
                keep_track_ids: None,
            })
            .unwrap();
        let id = project.tracks[0].clips[1].id.clone();
        project.apply(serde_json::from_value(json!({"type":"updateClip","clipId":id,"speed":2})).unwrap()).unwrap();
        project.apply(EditCmd::DetachAudio { clip_id: id }).unwrap();
        let words = map_words(&project, &sources);
        assert_eq!(words.len(), 5);
        assert_eq!(words[3].start_us, 3_250_000);
        let data =
            summary(&Derived { sources, words: words.clone(), untranscribed: vec![] }, Some([3_000_000, 5_000_000]))
                .unwrap();
        assert_eq!(data["words"][0]["i"], 3);
        assert_eq!(data["words"][0]["text"], "word6");
        let (edit, _) = caption_edit(
            &words,
            &project,
            crate::params::reel_style(),
            CaptionGrouping { max_words: 100, max_chars: 1000, ..CaptionGrouping::default() },
        )
        .unwrap();
        let EditCmd::AddCaptions { segments, .. } = edit else { panic!() };
        for segment in segments {
            assert!(project.tracks.iter().flat_map(|t| &t.clips).any(|c| {
                words.iter().any(|w| w.clip_id == c.id)
                    && segment.start_us >= c.start_us
                    && segment.end_us <= c.end_us()
            }));
            assert!(!(segment.start_us < 3_000_000 && segment.end_us > 3_000_000));
        }
    }

    #[test]
    fn protect_word_fragments_at_clip_boundaries_and_speed() {
        for speed in [1.0, 0.5, 2.0] {
            for (word_start, word_end) in [(4_800_000, 5_400_000), (4_600_000, 5_200_000)] {
                for edge in [false, true] {
                    let (mut project, mut sources) = fixture();
                    let id = project.tracks[0].clips[0].id.clone();
                    project.apply(EditCmd::SplitClip { clip_id: id, at_us: 5_000_000 }).unwrap();
                    for clip in project.tracks[0].clips.clone() {
                        project
                            .apply(
                                serde_json::from_value(json!({"type":"updateClip","clipId":clip.id,"speed":speed}))
                                    .unwrap(),
                            )
                            .unwrap();
                    }
                    let mut words =
                        vec![Word { start_us: word_start, end_us: word_end, text: "whole".into(), probability: 1.0 }];
                    if !edge {
                        words.insert(
                            0,
                            Word { start_us: 4_000_000, end_us: 4_400_000, text: "before".into(), probability: 1.0 },
                        );
                    }
                    sources.insert("talk".into(), words);
                    let derived = Derived { words: map_words(&project, &sources), sources, untranscribed: vec![] };
                    if word_start == 4_800_000 {
                        assert_eq!(derived.words.last().unwrap().start_us, (5_000_000.0 / speed) as i64);
                    }
                    let ranges = edit_ranges(&project, &derived, None, None, Some(DEFAULT_PAUSE_US)).unwrap();
                    let protected = TimeRange {
                        start_us: (word_start as f64 / speed) as i64,
                        end_us: (word_end as f64 / speed) as i64,
                    };
                    assert!(
                        ranges.iter().all(|r| r.end_us <= protected.start_us || r.start_us >= protected.end_us),
                        "{speed} {edge}: {ranges:?}"
                    );
                    project.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: None }).unwrap();
                    let audible_us: i64 = project.tracks[0]
                        .clips
                        .iter()
                        .map(|c| {
                            let ClipContent::Media { source_in_us, speed, .. } = c.content else { return 0 };
                            let start = source_in_us.max(word_start);
                            let end =
                                (source_in_us + (c.duration_us as f64 * f64::from(speed)).round() as i64).min(word_end);
                            (end - start).max(0)
                        })
                        .sum();
                    assert_eq!(audible_us, 600_000);
                }
            }
        }
    }

    #[test]
    fn subframe_caption_tails_stay_on_their_side_of_a_cut() {
        let (mut project, mut sources) = fixture();
        let id = project.tracks[0].clips[0].id.clone();
        project.apply(EditCmd::SplitClip { clip_id: id, at_us: 5_000_000 }).unwrap();
        sources.get_mut("talk").unwrap()[0].start_us = 4_999_000;
        sources.get_mut("talk").unwrap()[0].end_us = 5_000_000;
        sources.get_mut("talk").unwrap().truncate(1);
        let words = map_words(&project, &sources);
        let (edit, _) =
            caption_edit(&words, &project, crate::params::reel_style(), CaptionGrouping::default()).unwrap();
        project.apply(edit).unwrap();
        let caption = &project.tracks.iter().find(|t| t.name == "Captions").unwrap().clips[0];
        assert_eq!(caption.end_us(), 5_000_000);
    }

    #[test]
    fn captions_group_by_clip_and_skip_subframe_clips() {
        let (mut project, _) = fixture();
        let mut short = project.tracks[0].clips[0].clone();
        short.id = "short".into();
        short.duration_us = 10_000;
        project.tracks[0].clips[0].start_us = short.end_us();
        project.tracks[0].clips.insert(0, short);
        let words: Vec<_> = project.tracks[0]
            .clips
            .iter()
            .map(|clip| TimelineWord {
                start_us: clip.start_us,
                end_us: clip.start_us + 5_000,
                text: clip.id.clone(),
                probability: 1.0,
                clip_id: clip.id.clone(),
                asset_id: "talk".into(),
                source_start_us: 0,
            })
            .collect();
        let (edit, count) =
            caption_edit(&words, &project, crate::params::reel_style(), CaptionGrouping::default()).unwrap();
        assert_eq!(count, 1);
        let EditCmd::AddCaptions { segments, .. } = &edit else { panic!() };
        assert_eq!(segments[0].text, words[1].text);
        assert!(segments[0].start_us >= 10_000);
        project.apply(edit).unwrap();
        let captions = &project.tracks.iter().find(|t| t.name == "Captions").unwrap().clips;
        assert_eq!(captions.len(), 1);
        assert!(captions[0].start_us >= 10_000);
        assert!(
            caption_edit(&words[..1], &project, crate::params::reel_style(), CaptionGrouping::default())
                .unwrap_err()
                .to_string()
                .contains("NO_CAPTIONS")
        );
    }

    #[test]
    fn reject_overlapping_speech_instead_of_cutting_a_word() {
        let (project, sources) = fixture();
        let mut words = map_words(&project, &sources);
        words[0].end_us = words[1].start_us + 1;
        assert!(
            deletion_ranges(&words, &whole(&project), Some(&[[1, 1]]), None, Some(300_000))
                .unwrap_err()
                .to_string()
                .contains("OVERLAPPING_SPEECH")
        );
    }
}
