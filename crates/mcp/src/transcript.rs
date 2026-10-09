use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions, TryLockError};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, ensure};
use nuzky_analysis::{AudioSource, CaptionGrouping, group_words};
use nuzky_engine::{
    Project,
    edit::{EditCmd, TimeRange, merge_ranges},
    model::{Asset, ClipContent, MAX_CORRECTION_CHARS, TextStyle, WordCorrection},
    speech::{TimelineWord, Word, is_heard, map_words},
};
use nuzky_session::{
    jobs::check_cancel,
    transcripts::{Record, Segment, TranscriptStore, VERSION},
};
use serde::Serialize;
use serde_json::{Value, json};

use nuzky_analysis::VAD_MODEL;
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
        nuzky_analysis::models_dir().join(format!("ggml-{model}.bin"))
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
    if nuzky_analysis::models_dir().join("ggml-large-v3-turbo-q5_0.bin").is_file() {
        "large-v3-turbo-q5_0"
    } else {
        "small"
    }
}

/// What recognition is doing, for its job's progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    /// Another recognition holds the slot.
    Waiting,
    Recognising,
    /// The word timing model of the recognised language is downloading, 0..1.
    DownloadingAligner(f32),
    /// Measuring word times in the sound.
    Aligning,
}

/// Recognises the whole file in its own time and stores its words, replacing an older record.
/// Word times are measured with the word timing model of the recognised language, downloaded on
/// first use; without it, offline, or when it fails, they are Whisper's estimates aligned to pauses.
#[allow(clippy::too_many_arguments)]
pub fn recognise(
    store: &TranscriptStore,
    asset: &Asset,
    cache: &Path,
    model: &str,
    (model_path, vad): &(PathBuf, PathBuf),
    language: &str,
    cancel: &AtomicBool,
    stage: impl FnMut(Stage),
) -> Result<Record> {
    let source = |language: &str, stage: &mut dyn FnMut(Stage)| {
        let Some(model) = nuzky_analysis::align_model(language) else { return Ok(None) };
        Ok(aligner(model, &nuzky_analysis::models_dir(), cancel, stage)?.map(|aligner| (aligner, model)))
    };
    recognise_with(store, asset, cache, model, (model_path, vad), language, cancel, stage, &source)
}

type AlignerSource<'a> = dyn Fn(&str, &mut dyn FnMut(Stage)) -> Result<Option<(nuzky_analysis::Aligner, &'static nuzky_analysis::AlignModel)>>
    + 'a;

/// [`recognise`] with the word timing model for the recognised language from `aligner`.
#[allow(clippy::too_many_arguments)]
fn recognise_with(
    store: &TranscriptStore,
    asset: &Asset,
    cache: &Path,
    model: &str,
    (model_path, vad): (&PathBuf, &PathBuf),
    language: &str,
    cancel: &AtomicBool,
    mut stage: impl FnMut(Stage),
    aligner: &AlignerSource<'_>,
) -> Result<Record> {
    let _slot = recognition_slot(&nuzky_analysis::models_dir().join(".recognition.lock"), cancel, |waiting| {
        stage(if waiting { Stage::Waiting } else { Stage::Recognising })
    })?;
    let fingerprint = store.fingerprint(asset)?;
    let mut result = nuzky_analysis::transcribe_words_cancellable(
        AudioSource::Asset { asset, cache },
        model_path,
        vad,
        language,
        || cancel.load(Ordering::Relaxed),
    )?;
    check_cancel(cancel)?;
    let aligner = aligner(&result.language, &mut stage)?;
    stage(Stage::Aligning);
    // Cuts are planned from these times, so they follow the sound, not Whisper's estimates.
    let measured = measure(&mut result.words, asset, cache, aligner.as_ref().map(|(a, _)| a), cancel)?;
    // A stop asked for while the words were being refined still keeps nothing.
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
        alignment: aligner.filter(|_| measured).map(|(_, m)| m.id.to_owned()),
    };
    store.put(asset, &record)?;
    Ok(record)
}

/// The word timing `model` in `dir`, downloaded if it is missing. None when it cannot be had:
/// recognition then goes on as before, only a stop ends it.
fn aligner(
    model: &nuzky_analysis::AlignModel,
    dir: &Path,
    cancel: &AtomicBool,
    stage: &mut dyn FnMut(Stage),
) -> Result<Option<nuzky_analysis::Aligner>> {
    let integrity = crate::model_download::Integrity { size: model.size, sha256: model.sha256 };
    let loaded = crate::model_download::download(model.url, &dir.join(model.file), integrity, cancel, |done| {
        stage(Stage::DownloadingAligner(done))
    })
    .and_then(|path| {
        std::panic::catch_unwind(|| nuzky_analysis::Aligner::load(&path))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the word timing model crashed while loading")))
    });
    match loaded {
        Ok(aligner) => Ok(Some(aligner)),
        Err(error) => {
            check_cancel(cancel)?;
            log::warn!("Word times stay Whisper's estimates: {error:#}");
            Ok(None)
        }
    }
}

/// Measures `words` in the sound, with `aligner` when there is one; true when it measured any.
/// A failing model leaves Whisper's estimates aligned to pauses; only a stop ends recognition.
fn measure(
    words: &mut [Word],
    asset: &Asset,
    cache: &Path,
    aligner: Option<&nuzky_analysis::Aligner>,
    cancel: &AtomicBool,
) -> Result<bool> {
    let cancelled = || cancel.load(Ordering::Relaxed);
    if aligner.is_some() {
        let mut measured = words.to_vec();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            nuzky_analysis::align_to_sound(&mut measured, asset, cache, aligner, cancelled)
        }))
        .unwrap_or_else(|_| Err(anyhow::anyhow!("the word timing model crashed")));
        match result {
            Ok(any) => {
                words.clone_from_slice(&measured);
                return Ok(any);
            }
            Err(error) => {
                check_cancel(cancel)?;
                log::warn!("Word times stay Whisper's estimates: {error:#}");
            }
        }
    }
    nuzky_analysis::align_to_sound(words, asset, cache, None, cancelled)?;
    Ok(false)
}

/// One speech recognition at a time on this computer: the app and agents, attached or headless,
/// would otherwise each load a Whisper model. Later ones wait and stay cancellable. The slot is
/// best effort; recognition still runs when the lock file cannot be opened.
fn recognition_slot(path: &Path, cancel: &AtomicBool, mut waiting: impl FnMut(bool)) -> Result<Option<File>> {
    let lock = path
        .parent()
        .map(std::fs::create_dir_all)
        .transpose()
        .and_then(|_| OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path));
    let Ok(lock) = lock else { return Ok(None) };
    let mut waited = false;
    loop {
        check_cancel(cancel)?;
        match lock.try_lock() {
            Ok(()) => {
                if waited {
                    waiting(false);
                }
                return Ok(Some(lock));
            }
            Err(TryLockError::WouldBlock) => {
                if !std::mem::replace(&mut waited, true) {
                    waiting(true);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(TryLockError::Error(_)) => return Ok(None),
        }
    }
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

#[derive(Default)]
pub struct Derived {
    /// The words of each heard file, as the project's corrections read them.
    pub sources: HashMap<String, Vec<Word>>,
    pub words: Vec<TimelineWord>,
    pub untranscribed: Vec<String>,
    /// What corrected words were recognised as, by asset, start in the file and corrected text.
    pub originals: HashMap<(String, i64, String), String>,
}

impl Derived {
    /// The recognised words of `sources` with the project's corrections, placed on its timeline.
    /// A correction applies to the word with its start and recognised text only, never to
    /// another word, and is matched against what was recognised, so corrections never chain.
    pub fn new(project: &Project, mut sources: HashMap<String, Vec<Word>>, untranscribed: Vec<String>) -> Self {
        let corrections: HashMap<_, _> = project
            .word_corrections
            .iter()
            .map(|c| ((c.asset_id.as_str(), c.source_start_us, c.original.as_str()), c.text.as_str()))
            .collect();
        let mut originals = HashMap::new();
        if !corrections.is_empty() {
            for (asset, words) in &mut sources {
                for word in words {
                    if let Some(&text) = corrections.get(&(asset.as_str(), word.start_us, word.text.as_str())) {
                        let original = std::mem::replace(&mut word.text, text.to_owned());
                        originals.insert((asset.clone(), word.start_us, word.text.clone()), original);
                    }
                }
            }
        }
        Self { words: map_words(project, &sources), sources, untranscribed, originals }
    }

    /// What `word` was recognised as, when the user corrected it.
    pub fn original(&self, word: &TimelineWord) -> Option<&str> {
        self.originals.get(&(word.asset_id.clone(), word.source_start_us, word.text.clone())).map(String::as_str)
    }
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
    Ok(Derived::new(project, sources, untranscribed))
}

/// Word indices also change on re-recognition, even when the timeline layout stays put.
pub fn word_key(project: &Project, words: &[TimelineWord]) -> String {
    let mut hash = DefaultHasher::new();
    for word in words {
        (&word.asset_id, word.source_start_us, word.start_us, word.end_us, &word.text).hash(&mut hash);
    }
    format!("{}:{:016x}", nuzky_engine::speech::speech_layout_key(project), hash.finish())
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
    for &protected in &unnumbered {
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
    Ok((without_slivers(project, &bounds, &unnumbered, ranges), speech))
}

/// Where a take starts speaking at once, the silence a cut keeps before its first word lies at
/// the end of the take before it, cut off from that take's last word: a sliver of a clip on the
/// timeline. That silence moves next to the last word instead, so the pause keeps its length and
/// the takes meet at one cut; when the cut also removes words there, the sliver just goes. The
/// same holds the other way round, for a take that ends speaking at once.
fn without_slivers(
    project: &Project,
    words: &[TimelineWord],
    fragments: &[TimeRange],
    mut ranges: Vec<TimeRange>,
) -> Vec<TimeRange> {
    const SLIVER_US: i64 = 400_000;
    let joins: Vec<i64> = project
        .tracks
        .iter()
        .flat_map(|track| {
            let heard: Vec<_> = track.clips.iter().filter(|c| is_heard(project, track, c)).collect();
            heard
                .windows(2)
                .filter(|pair| pair[0].end_us() == pair[1].start_us)
                .map(|pair| pair[1].start_us)
                .collect::<Vec<_>>()
        })
        .collect();
    // Kept fragments of words count as speech like numbered words.
    let silent = |from: i64, to: i64| {
        words.iter().all(|w| w.end_us <= from || w.start_us >= to)
            && fragments.iter().all(|f| f.end_us <= from || f.start_us >= to)
    };
    let edges: Vec<(i64, i64)> = ranges.iter().map(|r| (r.start_us, r.end_us)).collect();
    for (i, range) in ranges.iter_mut().enumerate() {
        let next = edges.get(i + 1).map_or(i64::MAX, |r| r.0);
        let previous = i.checked_sub(1).map_or(i64::MIN, |p| edges[p].1);
        // A sliver after the cut, before a join.
        if let Some(&join) = joins.iter().filter(|&&t| t > range.end_us && t <= next).min() {
            let kept = join - range.end_us;
            if kept < SLIVER_US && silent(range.end_us, join) {
                if silent(range.start_us, range.start_us + kept) {
                    range.start_us += kept;
                }
                range.end_us = join;
                continue;
            }
        }
        // A sliver before the cut, after a join.
        if let Some(&join) = joins.iter().filter(|&&t| t < range.start_us && t >= previous).max() {
            let kept = range.start_us - join;
            if kept < SLIVER_US && silent(join, range.start_us) {
                if silent(range.end_us - kept, range.end_us) {
                    range.end_us -= kept;
                }
                range.start_us = join;
            }
        }
    }
    ranges
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
    check_word_key(project, derived, key)
}

/// Word indices from `key` still number the same words with the same text.
pub fn check_word_key(project: &Project, derived: &Derived, key: &str) -> Result<()> {
    ensure!(
        key == word_key(project, &derived.words),
        "SPEECH_CHANGED: speech changed or wrong key; use get_transcript's transcript_key (get_state's key is for apply_edits)"
    );
    Ok(())
}

/// What correcting words does, as one atomic batch.
#[derive(Debug)]
pub struct Correction {
    /// The corrections first, then the caption clips that show the words.
    pub edits: Vec<EditCmd>,
    /// Corrected words as `(index, recognised text, new text)`, for every place the word is heard.
    pub words: Vec<(usize, String, String)>,
    /// Caption clips whose text changes.
    pub captions: Vec<String>,
}

/// One line of text, as a recognised word is.
pub fn check_correction(text: &str) -> Result<String> {
    let text = text.trim();
    ensure!(
        !text.is_empty() && text.chars().count() <= MAX_CORRECTION_CHARS && !text.chars().any(char::is_control),
        "INVALID_CORRECTION: a word must be one line of 1 to {MAX_CORRECTION_CHARS} characters, not {text:?}"
    );
    Ok(text.to_owned())
}

/// Corrects the words at these `derived` indices to the given text. A word heard twice, such as
/// in a duplicated clip, is one recognised word: correcting it corrects every place it is heard.
pub fn plan_correction(project: &Project, derived: &Derived, fixes: &[(usize, String)]) -> Result<Correction> {
    ensure!(!fixes.is_empty(), "INVALID_ARGUMENTS: give at least one correction");
    let mut corrections: Vec<(usize, WordCorrection)> = Vec::new();
    for (i, text) in fixes {
        let word =
            derived.words.get(*i).with_context(|| format!("INVALID_WORD_INDEX: no word {i} in the transcript"))?;
        let text = check_correction(text)?;
        let original = derived.original(word).unwrap_or(&word.text).to_owned();
        let correction =
            WordCorrection { asset_id: word.asset_id.clone(), source_start_us: word.source_start_us, original, text };
        let same = |c: &WordCorrection| {
            (&c.asset_id, c.source_start_us, &c.original)
                == (&correction.asset_id, correction.source_start_us, &correction.original)
        };
        match corrections.iter().find(|(_, c)| same(c)) {
            Some((j, c)) => ensure!(
                c.text == correction.text,
                "INVALID_CORRECTION: words {j} and {i} are the same recognised word; give it one text"
            ),
            None if correction.text != word.text => corrections.push((*i, correction)),
            None => {}
        }
    }
    // Every place a corrected word is heard, with the text it shows now and the new text.
    let mut words = Vec::new();
    for (index, word) in derived.words.iter().enumerate() {
        let original = derived.original(word).unwrap_or(&word.text);
        if let Some((_, c)) = corrections.iter().find(|(_, c)| {
            c.asset_id == word.asset_id && c.source_start_us == word.source_start_us && c.original == original
        }) {
            words.push((index, word.text.clone(), c.text.clone()));
        }
    }
    let changes: Vec<(usize, &str)> = words.iter().map(|(i, _, text)| (*i, text.as_str())).collect();
    let captions = caption_corrections(project, derived, &changes);
    let mut edits = Vec::new();
    if !corrections.is_empty() {
        edits.push(EditCmd::CorrectWords { corrections: corrections.into_iter().map(|(_, c)| c).collect() });
    }
    let changed = captions.iter().map(|(id, _)| id.clone()).collect();
    edits.extend(captions.into_iter().map(|(clip_id, text)| EditCmd::UpdateClip {
        clip_id,
        transform: None,
        volume: None,
        text: Some(text),
        style: None,
        speed: None,
        adjust: None,
        fade_in_us: None,
        fade_out_us: None,
        clean_voice: None,
    }));
    Ok(Correction { edits, words, captions: changed })
}

/// Text compared without case, the punctuation around it and how wide its spaces are.
fn comparable(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

/// A caption clip's id, its text and the words to replace in it, as byte ranges and new text.
type Replacements<'a> = (&'a str, &'a str, Vec<(usize, usize, String)>);

/// The new text of caption clips that show corrected words: on the Captions track, a clip whose
/// time holds the middle of the word and whose text has the word, compared without case and the
/// punctuation around it, gets that word replaced. Its style and timing stay. `changes` are
/// `(index in derived.words, new text)`; a word twice in one caption is told apart by the words
/// heard before it there.
pub fn caption_corrections(project: &Project, derived: &Derived, changes: &[(usize, &str)]) -> Vec<(String, String)> {
    let middle = |w: &TimelineWord| w.start_us + (w.end_us - w.start_us) / 2;
    // Per clip: its text and the replacements by byte range, both from the text before any change.
    let mut planned: Vec<Replacements> = Vec::new();
    for &(index, text) in changes {
        let Some(word) = derived.words.get(index) else { continue };
        let wanted = comparable(&word.text);
        if wanted.is_empty() {
            continue;
        }
        // A corrected word may hold a space, so it may span several words of a caption.
        let length = word.text.split_whitespace().count();
        let at = middle(word);
        for clip in project.tracks.iter().filter(|t| t.is_captions()).flat_map(|t| &t.clips) {
            let ClipContent::Text { text: shown, words: timed, .. } = &clip.content else { continue };
            if !clip.contains(at) {
                continue;
            }
            let words = tokens(shown);
            let matching: Vec<(usize, usize)> = words
                .windows(length)
                .map(|run| (run[0].0, run[length - 1].0 + run[length - 1].1.len()))
                .filter(|&(start, end)| comparable(&shown[start..end]) == wanted)
                .collect();
            let before = derived.words[..index]
                .iter()
                .filter(|w| clip.contains(middle(w)) && comparable(&w.text) == wanted)
                .count();
            // A karaoke caption knows when each of its words is said, which also holds after its
            // start was trimmed off; other captions count the word's earlier repeats inside them.
            let spoken = nuzky_engine::model::spoken_word(shown, timed, at - clip.start_us)
                .and_then(|range| matching.iter().find(|&&(start, _)| start == range.start));
            let Some(&(start, end)) = spoken.or(matching.get(before)).or(matching.last()) else { continue };
            let entry = match planned.iter().position(|(id, ..)| *id == clip.id) {
                Some(found) => &mut planned[found],
                None => {
                    planned.push((&clip.id, shown, Vec::new()));
                    planned.last_mut().unwrap()
                }
            };
            if entry.2.iter().all(|&(a, b, _)| end <= a || start >= b) {
                entry.2.push((start, end, replaced(&shown[start..end], &word.text, text)));
            }
        }
    }
    planned
        .into_iter()
        .filter_map(|(id, shown, mut replacements)| {
            replacements.sort_by_key(|r| r.0);
            let mut out = String::with_capacity(shown.len());
            let mut last = 0;
            for (start, end, new) in replacements {
                out.push_str(&shown[last..start]);
                out.push_str(&new);
                last = end;
            }
            out.push_str(&shown[last..]);
            (out != shown).then(|| (id.to_owned(), out))
        })
        .collect()
}

/// Words of a caption text with their byte offsets.
fn tokens(text: &str) -> Vec<(usize, &str)> {
    let mut found = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices().chain([(text.len(), ' ')]) {
        match (c.is_whitespace(), start) {
            (true, Some(from)) => {
                found.push((from, &text[from..at]));
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    found
}

/// A caption word that reads as recognised takes the correction as it is. One edited by hand
/// keeps its own punctuation around the corrected word and its capital first letter.
fn replaced(token: &str, recognised: &str, text: &str) -> String {
    if token == recognised {
        return text.to_owned();
    }
    let edge = |c: char| !c.is_alphanumeric();
    let core = token.trim_matches(edge);
    let lead = &token[..token.len() - token.trim_start_matches(edge).len()];
    let trail = &token[lead.len() + core.len()..];
    let mut new = text.trim_matches(edge).to_owned();
    if core.starts_with(char::is_uppercase)
        && let Some(first) = new.chars().next()
        && first.is_lowercase()
    {
        new = first.to_uppercase().chain(new.chars().skip(1)).collect();
    }
    format!("{lead}{new}{trail}")
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
    nuzky_session::validate(&preview)?;
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
    let tracks: Vec<_> = project.tracks.iter().filter(|t| t.is_captions()).collect();
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
    use nuzky_engine::model::{Asset, AssetKind, TrackKind};

    pub fn fixture() -> (Project, HashMap<String, Vec<Word>>) {
        let mut project = Project::new("speech test");
        project
            .apply(EditCmd::AddAssets {
                assets: vec![Asset {
                    id: "talk".into(),
                    name: "talk".into(),
                    path: "/talk.mov".into(),
                    kind: AssetKind::Video,
                    duration_us: 10_000_000,
                    width: 1080,
                    height: 1920,
                    fps: 30.0,
                    has_audio: true,
                    rotation: 0,
                    mirror: false,
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
        let derived = Derived::new(&project, sources, vec![]);
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
                    path: "/broll.mov".into(),
                    kind: AssetKind::Video,
                    duration_us: 6_000_000,
                    width: 1080,
                    height: 1920,
                    fps: 30.0,
                    has_audio: true,
                    rotation: 0,
                    mirror: false,
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
        project.tracks.push(nuzky_engine::model::Track {
            id: "over".into(),
            kind: TrackKind::Video,
            name: "Overlay".into(),
            muted: false,
            hidden: false,
            keep_in_place: false,
            clips: vec![overlay],
        });
        sources.insert("broll".into(), vec![]);
        let derived = Derived::new(&project, sources, vec![]);
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

    /// A second take that speaks from its first moment: shortening the pause between the takes
    /// keeps its length next to the last word of the first take, not as a sliver of its end.
    #[test]
    fn a_take_that_speaks_at_once_leaves_no_sliver_of_the_take_before() {
        let (mut project, mut sources) = fixture();
        let mut next = project.assets[0].clone();
        (next.id, next.path, next.duration_us) = ("next".into(), "/next.mov".into(), 3_000_000);
        project.apply(EditCmd::AddAssets { assets: vec![next] }).unwrap();
        project.apply(EditCmd::AddClip { asset_id: "next".into(), start_us: None, track_id: None }).unwrap();
        let word =
            |start_us, text: &str| Word { start_us, end_us: start_us + 400_000, text: text.into(), probability: 1.0 };
        sources.insert("next".into(), vec![word(0, "next0"), word(1_000_000, "next1")]);
        let derived = Derived::new(&project, sources, vec![]);
        for (delete, last) in [(None, "word7"), (Some(&[[7, 7]][..]), "word6")] {
            let ranges = edit_ranges(&project, &derived, delete, None, Some(300_000)).unwrap();
            let cut = plan_cut(&project, &derived, ranges).unwrap();
            let main = &cut.preview.tracks[0].clips;
            assert!(main.iter().all(|c| c.duration_us >= 300_000), "{delete:?}: {main:?}");
            let words = map_words(&cut.preview, &derived.sources);
            let at = |text: &str| words.iter().find(|w| w.text == text).unwrap();
            assert_eq!(
                at("next0").start_us - at(last).end_us,
                if delete.is_none() { 300_000 } else { AFTER_WORD_US + BEFORE_WORD_US }
            );
        }
    }

    /// The kept start of a word whose middle lies past the end of its clip is speech too: moving
    /// a pause cut onto the join must not swallow it.
    #[test]
    fn a_kept_piece_of_a_word_at_the_end_of_a_take_is_not_taken_for_a_sliver() {
        let (mut project, mut sources) = fixture();
        project
            .apply(EditCmd::RippleDeleteRanges {
                ranges: vec![TimeRange { start_us: 1_000_000, end_us: 10_000_000 }],
                keep_track_ids: None,
            })
            .unwrap();
        let mut next = project.assets[0].clone();
        (next.id, next.path, next.duration_us) = ("next".into(), "/next.mov".into(), 3_000_000);
        project.apply(EditCmd::AddAssets { assets: vec![next] }).unwrap();
        project.apply(EditCmd::AddClip { asset_id: "next".into(), start_us: None, track_id: None }).unwrap();
        let word = |start_us, end_us, text: &str| Word { start_us, end_us, text: text.into(), probability: 1.0 };
        sources.insert("talk".into(), vec![word(200_000, 500_000, "first"), word(900_000, 1_300_000, "half")]);
        sources.insert("next".into(), vec![word(0, 400_000, "next0"), word(1_000_000, 1_400_000, "next1")]);
        let derived = Derived::new(&project, sources, vec![]);
        // "half" has its middle past the take's end, so only "first" is numbered in the take.
        assert_eq!(derived.words.iter().filter(|w| w.asset_id == "talk").count(), 1);
        let ranges = edit_ranges(&project, &derived, None, None, Some(300_000)).unwrap();
        let cut = plan_cut(&project, &derived, ranges).unwrap();
        let plays = |from: i64, to: i64| {
            cut.preview.tracks[0].clips.iter().any(|clip| {
                let ClipContent::Media { asset_id, source_in_us, .. } = &clip.content else { return false };
                asset_id == "talk" && *source_in_us <= from && source_in_us + clip.duration_us >= to
            })
        };
        assert!(plays(900_000, 1_000_000), "the kept start of \"half\" still plays: {:?}", cut.ranges);
    }

    /// "To je to." as a karaoke caption whose first word was trimmed off: correcting the last word
    /// changes the last word, not the first one still in the text.
    #[test]
    fn a_correction_finds_its_word_in_a_trimmed_karaoke_caption_by_time() {
        let (mut project, mut sources) = fixture();
        let word =
            |start_us, text: &str| Word { start_us, end_us: start_us + 400_000, text: text.into(), probability: 1.0 };
        sources.insert("talk".into(), vec![word(500_000, "To"), word(1_500_000, "je"), word(2_500_000, "to.")]);
        let timed = |text: &str, start_us| nuzky_engine::model::CaptionWord {
            text: text.into(),
            start_us,
            end_us: start_us + 400_000,
        };
        let segment = nuzky_engine::edit::CaptionSegment {
            start_us: 400_000,
            end_us: 3_000_000,
            text: "To je to.".into(),
            // Timeline times, as built from the transcript.
            words: vec![timed("To", 500_000), timed("je", 1_500_000), timed("to.", 2_500_000)],
        };
        project.apply(EditCmd::AddCaptions { segments: vec![segment], style: crate::params::reel_style() }).unwrap();
        let caption = project.tracks.iter().find(|t| t.is_captions()).unwrap().clips[0].id.clone();
        project
            .apply(EditCmd::TrimClip {
                clip_id: caption,
                start_us: 1_000_000,
                duration_us: 2_000_000,
                source_in_us: None,
            })
            .unwrap();
        let derived = Derived::new(&project, sources, vec![]);
        let last = derived.words.iter().position(|w| w.text == "to.").unwrap();
        let changed = caption_corrections(&project, &derived, &[(last, "tu.")]);
        assert_eq!(changed.iter().map(|(_, text)| text.as_str()).collect::<Vec<_>>(), ["To je tu."]);
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
            summary(&Derived { sources, words: words.clone(), ..Derived::default() }, Some([3_000_000, 5_000_000]))
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
    fn karaoke_captions_mark_each_word_where_it_is_heard() {
        let (mut project, sources) = fixture();
        project
            .apply(EditCmd::RippleDeleteRanges {
                ranges: vec![TimeRange { start_us: 3_000_000, end_us: 4_200_000 }],
                keep_track_ids: None,
            })
            .unwrap();
        let id = project.tracks[0].clips[1].id.clone();
        project.apply(serde_json::from_value(json!({"type":"updateClip","clipId":id,"speed":1.5})).unwrap()).unwrap();
        let words = map_words(&project, &sources);
        let style = nuzky_engine::edit::caption_preset("karaoke").unwrap().style.clone();
        let grouping = CaptionGrouping { max_words: 3, max_chars: 30, break_gap_us: 1_000_000 };
        let (edit, _) = caption_edit(&words, &project, style, grouping).unwrap();
        project.apply(edit).unwrap();
        let captions = &project.tracks.iter().find(|t| t.is_captions()).unwrap().clips;
        assert!(captions.iter().any(|c| matches!(&c.content, ClipContent::Text { words, .. } if words.len() > 1)));
        // In the middle of every word heard, its caption highlights that very word.
        for word in &words {
            let middle = (word.start_us + word.end_us) / 2;
            let clip = captions.iter().find(|c| c.contains(middle)).unwrap();
            let ClipContent::Text { text, words: spoken, .. } = &clip.content else { panic!() };
            let range = nuzky_engine::model::spoken_word(text, spoken, middle - clip.start_us).unwrap();
            assert_eq!(&text[range], word.text.trim(), "at {middle}");
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
                    let derived = Derived::new(&project, sources, vec![]);
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

    #[test]
    fn one_recognition_at_a_time_and_waiting_stays_cancellable() {
        use std::sync::{Arc, mpsc};
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("recognition-slot-{}", nuzky_engine::edit::new_id()));
        let lock = dir.join(".recognition.lock");
        let first = recognition_slot(&lock, &AtomicBool::new(false), |_| panic!("the slot was free")).unwrap();
        assert!(first.is_some());
        let waiter = |cancel: Arc<AtomicBool>| {
            let (tx, rx) = mpsc::channel();
            let lock = lock.clone();
            let worker = std::thread::spawn(move || {
                recognition_slot(&lock, &cancel, |waiting| tx.send(waiting).unwrap()).map(|slot| slot.is_some())
            });
            (rx, worker)
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (waiting, worker) = waiter(cancel.clone());
        assert!(waiting.recv_timeout(Duration::from_secs(2)).unwrap());
        cancel.store(true, Ordering::Relaxed);
        assert!(worker.join().unwrap().unwrap_err().to_string().starts_with("CANCELLED"));
        let (waiting, worker) = waiter(Arc::new(AtomicBool::new(false)));
        assert!(waiting.recv_timeout(Duration::from_secs(2)).unwrap());
        drop(first);
        assert!(!waiting.recv_timeout(Duration::from_secs(2)).unwrap());
        assert!(worker.join().unwrap().unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn correction(start_us: i64, original: &str, text: &str) -> EditCmd {
        EditCmd::CorrectWords {
            corrections: vec![WordCorrection {
                asset_id: "talk".into(),
                source_start_us: start_us,
                original: original.into(),
                text: text.into(),
            }],
        }
    }

    /// The words with their texts, where they are heard and what they were recognised as.
    fn read(project: &Project, sources: &HashMap<String, Vec<Word>>) -> Vec<(String, i64, Option<String>)> {
        let derived = Derived::new(project, sources.clone(), vec![]);
        derived.words.iter().map(|w| (w.text.clone(), w.start_us, derived.original(w).map(str::to_owned))).collect()
    }

    /// Plans the corrections as the app and the MCP tool do and applies them.
    fn correct(
        project: &mut Project,
        sources: &HashMap<String, Vec<Word>>,
        fixes: &[(usize, &str)],
    ) -> Result<Correction> {
        let derived = Derived::new(project, sources.clone(), vec![]);
        let fixes: Vec<_> = fixes.iter().map(|(i, text)| (*i, text.to_string())).collect();
        let plan = plan_correction(project, &derived, &fixes)?;
        for edit in plan.edits.clone() {
            project.apply(edit)?;
        }
        Ok(plan)
    }

    fn caption_texts(project: &Project) -> Vec<String> {
        project
            .tracks
            .iter()
            .filter(|t| t.is_captions())
            .flat_map(|t| &t.clips)
            .map(|c| match &c.content {
                ClipContent::Text { text, .. } => text.clone(),
                ClipContent::Media { .. } => unreachable!(),
            })
            .collect()
    }

    fn add_captions(project: &mut Project, segments: &[(i64, i64, &str)]) {
        let segments = segments
            .iter()
            .map(|&(start_us, end_us, text)| nuzky_engine::edit::CaptionSegment {
                start_us,
                end_us,
                text: text.into(),
                words: vec![],
            })
            .collect();
        project.apply(EditCmd::AddCaptions { segments, style: crate::params::reel_style() }).unwrap();
    }

    #[test]
    fn a_correction_follows_cuts_speed_and_duplicates_and_changes_the_key() {
        let (mut project, sources) = fixture();
        let key = |p: &Project| word_key(p, &Derived::new(p, sources.clone(), vec![]).words);
        let before = key(&project);
        project.apply(correction(3_500_000, "word3", "fixed")).unwrap();
        let words = read(&project, &sources);
        assert_eq!(words[3], ("fixed".into(), 3_500_000, Some("word3".into())));
        assert!(words.iter().enumerate().all(|(i, w)| i == 3 || (w.0.starts_with("word") && w.2.is_none())));
        assert_ne!(key(&project), before, "a correction changes the word indices' key");
        // Cut word1 and word2: the correction stays with its word, now at 1.5 s.
        project
            .apply(EditCmd::RippleDeleteRanges {
                ranges: vec![TimeRange { start_us: 1_000_000, end_us: 3_000_000 }],
                keep_track_ids: None,
            })
            .unwrap();
        let words = read(&project, &sources);
        assert_eq!(words[1], ("fixed".into(), 1_500_000, Some("word3".into())));
        // Double speed on the clip after the cut, then a copy of it: heard twice, corrected in both places.
        let clip = project.tracks[0].clips[1].id.clone();
        project
            .apply(serde_json::from_value(json!({"type":"updateClip","clipId":clip.clone(),"speed":2})).unwrap())
            .unwrap();
        project.apply(EditCmd::DuplicateClip { clip_id: clip }).unwrap();
        let fixed: Vec<_> = read(&project, &sources).into_iter().filter(|w| w.2.is_some()).collect();
        assert_eq!(
            fixed.iter().map(|w| (w.0.as_str(), w.1)).collect::<Vec<_>>(),
            [("fixed", 1_250_000), ("fixed", 4_750_000)]
        );
    }

    #[test]
    fn a_word_recognised_again_differently_leaves_the_correction_dormant() {
        let (mut project, mut sources) = fixture();
        project.apply(correction(3_500_000, "word3", "fixed")).unwrap();
        let words = sources.get_mut("talk").unwrap();
        // Another word now reads like the corrected one, and the corrected word reads differently.
        words[4].text = "word3".into();
        words[3].text = "other".into();
        let texts: Vec<_> = read(&project, &sources).into_iter().map(|w| w.0).collect();
        assert_eq!(texts[3..5], ["other", "word3"]);
        // The same text a little later is another word too.
        let words = sources.get_mut("talk").unwrap();
        (words[3].text, words[3].start_us) = ("word3".into(), 3_510_000);
        assert!(read(&project, &sources).iter().all(|w| w.0 != "fixed" && w.2.is_none()));
        // Recognised as before, it applies again.
        sources.get_mut("talk").unwrap()[3].start_us = 3_500_000;
        assert_eq!(read(&project, &sources)[3].0, "fixed");
    }

    #[test]
    fn correcting_a_word_updates_the_captions_that_show_it() {
        let (mut project, sources) = fixture();
        add_captions(
            &mut project,
            &[(0, 3_000_000, "word0 word1 word2."), (3_000_000, 6_000_000, "WORD3! word4 word5.")],
        );
        let before = project.tracks.last().unwrap().clips.clone();
        let plan = correct(&mut project, &sources, &[(3, "fixed"), (5, "five.")]).unwrap();
        assert_eq!(plan.words, [(3, "word3".into(), "fixed".into()), (5, "word5.".into(), "five.".into())]);
        assert_eq!(plan.captions, [before[1].id.clone()]);
        // A caption word edited by hand keeps its capital and its punctuation around the correction.
        assert_eq!(caption_texts(&project), ["word0 word1 word2.", "Fixed! word4 five."]);
        let after = &project.tracks.last().unwrap().clips[1];
        assert_eq!((after.start_us, after.duration_us), (before[1].start_us, before[1].duration_us));
        let look = |c: &nuzky_engine::model::Clip| match &c.content {
            ClipContent::Text { style, transform, .. } => (style.clone(), *transform),
            ClipContent::Media { .. } => unreachable!(),
        };
        assert_eq!(look(after), look(&before[1]));
        // A caption that no longer shows the word is left alone.
        let mut project = fixture().0;
        add_captions(&mut project, &[(3_000_000, 6_000_000, "something else")]);
        let plan = correct(&mut project, &sources, &[(3, "fixed")]).unwrap();
        assert!(plan.captions.is_empty());
        assert_eq!(caption_texts(&project), ["something else"]);
        assert_eq!(project.word_corrections.len(), 1);
    }

    #[test]
    fn a_word_twice_in_one_caption_is_told_apart_by_order() {
        let (mut project, mut sources) = fixture();
        for i in [3, 4] {
            sources.get_mut("talk").unwrap()[i].text = "to".into();
        }
        add_captions(&mut project, &[(3_000_000, 6_000_000, "to to word5.")]);
        correct(&mut project, &sources, &[(4, "two")]).unwrap();
        assert_eq!(caption_texts(&project), ["to two word5."]);
        correct(&mut project, &sources, &[(3, "Tu")]).unwrap();
        assert_eq!(caption_texts(&project), ["Tu two word5."]);
    }

    #[test]
    fn regenerated_captions_keep_the_correction() {
        let (mut project, sources) = fixture();
        let grouping = CaptionGrouping { max_words: 3, max_chars: 100, break_gap_us: 2_000_000 };
        let derived = Derived::new(&project, sources.clone(), vec![]);
        project
            .apply(caption_edit(&derived.words, &project, crate::params::reel_style(), grouping).unwrap().0)
            .unwrap();
        correct(&mut project, &sources, &[(3, "fixed")]).unwrap();
        let shown = caption_texts(&project);
        assert!(shown.iter().any(|t| t.split(' ').any(|w| w == "fixed")), "{shown:?}");
        let derived = Derived::new(&project, sources.clone(), vec![]);
        project
            .apply(caption_edit(&derived.words, &project, crate::params::reel_style(), grouping).unwrap().0)
            .unwrap();
        assert_eq!(caption_texts(&project), shown);
    }

    #[test]
    fn the_recognised_text_again_removes_the_correction() {
        let (mut project, sources) = fixture();
        add_captions(&mut project, &[(3_000_000, 4_000_000, "word3")]);
        correct(&mut project, &sources, &[(3, "fixed")]).unwrap();
        // Correcting the corrected word again replaces the correction rather than stacking one.
        correct(&mut project, &sources, &[(3, "fixed again")]).unwrap();
        assert_eq!(project.word_corrections.len(), 1);
        assert_eq!(read(&project, &sources)[3].2.as_deref(), Some("word3"));
        assert_eq!(caption_texts(&project), ["fixed again"]);
        let plan = correct(&mut project, &sources, &[(3, " word3 ")]).unwrap();
        assert!(matches!(&plan.edits[0], EditCmd::CorrectWords { .. }));
        assert!(project.word_corrections.is_empty());
        assert_eq!(caption_texts(&project), ["word3"]);
        // The text it already reads changes nothing.
        assert!(correct(&mut project, &sources, &[(3, "word3")]).unwrap().edits.is_empty());
    }

    #[test]
    fn a_word_heard_twice_gets_one_text() {
        let (mut project, sources) = fixture();
        let clip = project.tracks[0].clips[0].id.clone();
        project.apply(EditCmd::DuplicateClip { clip_id: clip }).unwrap();
        add_captions(&mut project, &[(3_000_000, 4_000_000, "word3"), (13_000_000, 14_000_000, "word3")]);
        let error = correct(&mut project.clone(), &sources, &[(3, "a"), (11, "b")]).unwrap_err().to_string();
        assert!(error.starts_with("INVALID_CORRECTION"), "{error}");
        let plan = correct(&mut project, &sources, &[(3, "fixed")]).unwrap();
        assert_eq!(plan.words.iter().map(|w| w.0).collect::<Vec<_>>(), [3, 11]);
        assert_eq!(caption_texts(&project), ["fixed", "fixed"]);
        let EditCmd::CorrectWords { corrections } = &plan.edits[0] else { panic!() };
        assert_eq!(corrections.len(), 1);
    }

    #[test]
    fn corrections_are_one_line_of_bounded_text_for_a_word_that_exists() {
        let (mut project, sources) = fixture();
        let long = "x".repeat(MAX_CORRECTION_CHARS + 1);
        for text in ["", "   ", "two\nlines", "tab\there", long.as_str()] {
            let error = correct(&mut project, &sources, &[(0, text)]).unwrap_err().to_string();
            assert!(error.starts_with("INVALID_CORRECTION"), "{text:?}: {error}");
        }
        let longest = format!(" {} ", "ž".repeat(MAX_CORRECTION_CHARS));
        assert_eq!(check_correction(&longest).unwrap().chars().count(), MAX_CORRECTION_CHARS);
        let error = correct(&mut project, &sources, &[(8, "late")]).unwrap_err().to_string();
        assert!(error.starts_with("INVALID_WORD_INDEX"), "{error}");
        assert!(correct(&mut project, &sources, &[]).unwrap_err().to_string().starts_with("INVALID_ARGUMENTS"));
        assert!(project.word_corrections.is_empty());
    }

    #[test]
    fn a_word_timing_model_that_cannot_be_had_leaves_recognition_going_and_a_stop_ends_it() {
        let dir = std::env::temp_dir().join(format!("nuzky-aligner-{}", nuzky_engine::edit::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url: &'static str =
            Box::leak(format!("http://{}/model.gguf", server.local_addr().unwrap()).into_boxed_str());
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for mut stream in server.incoming().flatten() {
                let _ = stream.read(&mut [0; 4096]);
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        });
        let model = nuzky_analysis::AlignModel {
            language: "cs",
            id: "test",
            file: "model.gguf",
            url,
            size: 1024,
            sha256: "00",
        };
        let missing = aligner(&model, &dir, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(missing.is_none());
        let stopped = aligner(&model, &dir, &AtomicBool::new(true), &mut |_| {}).map(|a| a.is_some()).unwrap_err();
        assert!(stopped.to_string().starts_with("CANCELLED"), "{stopped}");
        // A file that is not a word timing model, though its checksum matched once, is no model either.
        std::fs::write(dir.join("broken.gguf"), b"not a model").unwrap();
        assert!(nuzky_analysis::Aligner::load(&dir.join("broken.gguf")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "needs tmp-test/speech.wav and the small + Silero models from scripts/fixtures.sh"]
    fn without_a_word_timing_model_recognition_gives_the_words_it_gave_before() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        let models = root.join("tmp-test/xdg/data/nuzky/models");
        let dir = root.join("tmp-test/mcp-tests").join(format!("no-aligner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TranscriptStore::at(dir.join("transcripts")).unwrap();
        let cache = dir.join("cache");
        let asset = nuzky_engine::media::probe(&root.join("tmp-test/speech.wav"), "speech".into()).unwrap();
        let (whisper, vad) = (models.join("ggml-small.bin"), models.join(VAD_MODEL));
        let cancel = AtomicBool::new(false);
        let record =
            recognise_with(&store, &asset, &cache, "small", (&whisper, &vad), "en", &cancel, |_| {}, &|_, _| Ok(None))
                .unwrap();
        // What recognition stored before word timing models: Whisper's words aligned to pauses.
        let mut before =
            nuzky_analysis::transcribe_words(AudioSource::Asset { asset: &asset, cache: &cache }, &whisper, &vad, "en")
                .unwrap()
                .words;
        let pcm = nuzky_engine::audio::Pcm::open(&nuzky_engine::audio::ensure_pcm(&cache, &asset, |_| Ok(())).unwrap())
            .unwrap();
        nuzky_analysis::align_words(&mut before, pcm.samples());
        assert!(before.len() > 10);
        assert_eq!(record.words, before);
        assert_eq!((record.version, record.alignment.as_deref()), (VERSION, None));
        assert_eq!(store.get(&asset).unwrap(), Some(record));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
