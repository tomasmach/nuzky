//! Word times on real Czech connected speech, against boundaries checked by hand.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use nuzky_analysis::{Aligner, AudioSource, Word, align_model, align_to_sound, transcribe_words_cancellable};
use nuzky_engine::media::probe;

/// How far past the hand-checked range a boundary may land. The ranges already hold the whole
/// closure or transition where cutting clips neither word. Deleting a word in connected speech
/// cuts at the boundary itself, because the cut margins shrink to half the gap between the
/// words. 40 ms past the range is two model frames: it can clip the edge of one consonant, never a
/// syllable. Whisper's estimates, even after `align_words`, miss by up to 270 ms here.
const TOLERANCE_US: i64 = 40_000;
/// Most boundaries must land in their range, not just near it.
const MEAN_US: i64 = 15_000;
/// Soft boundaries are vowel and glide transitions the eye cannot place exactly.
const SOFT_TOLERANCE_US: i64 = 60_000;

#[test]
#[ignore = "needs tmp-test/krysar-cs.wav and the large-v3-turbo-q5_0, Silero and Czech word timing models from scripts/fixtures.sh"]
fn connected_czech_speech_is_timed_to_its_word_boundaries() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize()?;
    let models = root.join("tmp-test/xdg/data/nuzky/models");
    let asset = probe(&root.join("tmp-test/krysar-cs.wav"), "krysar".into())?;
    let cache = cache_dir(&root)?;
    let recognised = transcribe_words_cancellable(
        AudioSource::Asset { asset: &asset, cache: &cache },
        &models.join("ggml-large-v3-turbo-q5_0.bin"),
        &models.join(nuzky_analysis::VAD_MODEL),
        "cs",
        || false,
    )?;
    let reference = Reference::read(&root.join("crates/analysis/tests/data/krysar-cs-boundaries.tsv"))?;

    let mut today = recognised.words.clone();
    align_to_sound(&mut today, &asset, &cache, None, || false)?;
    let before = reference.score(&today);
    eprintln!("Whisper + align_words: {before}");

    let aligner = Aligner::load(&models.join(align_model("cs").context("no Czech model")?.file))?;
    let mut words = recognised.words.clone();
    assert!(align_to_sound(&mut words, &asset, &cache, Some(&aligner), || false)?, "no word was measured");
    let after = reference.score(&words);
    eprintln!("word timing model: {after}");

    assert_eq!(
        words.iter().map(|w| &w.text).collect::<Vec<_>>(),
        recognised.words.iter().map(|w| &w.text).collect::<Vec<_>>()
    );
    assert!(words.windows(2).all(|p| p[0].start_us <= p[0].end_us && p[0].end_us <= p[1].start_us), "{words:?}");
    assert!(after.scored * 10 >= reference.sharp() * 9, "too few boundaries matched the recognised words: {after}");
    assert!(after.worst <= TOLERANCE_US && after.mean <= MEAN_US && after.soft_worst <= SOFT_TOLERANCE_US, "{after}");
    Ok(())
}

/// Stop during word timing ends it within a second or two, with nothing half done to keep.
#[test]
#[ignore = "needs tmp-test/krysar-cs.wav and the Czech word timing model from scripts/fixtures.sh"]
fn a_stop_ends_word_timing_within_seconds() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize()?;
    let models = root.join("tmp-test/xdg/data/nuzky/models");
    let aligner = Aligner::load(&models.join(align_model("cs").context("no Czech model")?.file))?;
    let asset = probe(&root.join("tmp-test/krysar-cs.wav"), "krysar".into())?;
    let cache = cache_dir(&root)?;
    // One long stretch of speech as words, so the model works on it for a long while.
    let mut words: Vec<Word> = (0..60)
        .map(|i| Word { start_us: i * 400_000, end_us: i * 400_000 + 350_000, text: "slovo".into(), probability: 0.9 })
        .collect();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let started = std::time::Instant::now();
    let asked = std::sync::Mutex::new(None);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            *asked.lock().unwrap() = Some(std::time::Instant::now());
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        align_to_sound(&mut words, &asset, &cache, Some(&aligner), || stop.load(std::sync::atomic::Ordering::Relaxed))
    });
    let asked = asked.lock().unwrap().context("the stop came after the work ended")?;
    let error = result.expect_err("the stop was ignored");
    assert!(error.to_string().starts_with("CANCELLED"), "{error:#}");
    assert!(asked.elapsed() < std::time::Duration::from_secs(2), "stopped {:?} after the request", asked.elapsed());
    eprintln!("stopped {:?} after the request, {:?} in", asked.elapsed(), started.elapsed());
    Ok(())
}

fn cache_dir(root: &Path) -> Result<PathBuf> {
    let dir = root.join("tmp-test/analysis").join(format!("alignment-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

struct Boundary {
    kind: Kind,
    word: usize,
    from_us: i64,
    to_us: i64,
    sharp: bool,
}

#[derive(PartialEq)]
enum Kind {
    Onset,
    Offset,
    Join,
}

struct Reference {
    words: Vec<String>,
    boundaries: Vec<Boundary>,
}

struct Score {
    scored: usize,
    mean: i64,
    worst: i64,
    soft_worst: i64,
    misses: Vec<String>,
}

impl std::fmt::Display for Score {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} sharp boundaries, mean {} ms, worst {} ms past their range; soft worst {} ms; over 30 ms: {:?}",
            self.scored,
            self.mean / 1000,
            self.worst / 1000,
            self.soft_worst / 1000,
            self.misses
        )
    }
}

impl Reference {
    fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let mut reference = Self { words: Vec::new(), boundaries: Vec::new() };
        for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let fields: Vec<&str> = line.split('\t').collect();
            let us = |s: &str| -> Result<i64> { Ok((s.parse::<f64>()? * 1e6).round() as i64) };
            match fields[..] {
                ["word", _, text] => reference.words.push(text.into()),
                [kind, word, from, to, class] => reference.boundaries.push(Boundary {
                    kind: match kind {
                        "onset" => Kind::Onset,
                        "offset" => Kind::Offset,
                        _ => Kind::Join,
                    },
                    word: word.parse()?,
                    from_us: us(from)?,
                    to_us: us(to)?,
                    sharp: class == "sharp",
                }),
                _ => anyhow::bail!("bad reference line {line:?}"),
            }
        }
        Ok(reference)
    }

    fn sharp(&self) -> usize {
        self.boundaries.iter().filter(|b| b.sharp).count()
    }

    /// How far each boundary of `words` lands past its range. A join is where the cut between the
    /// two words falls: the middle of the gap between them. Recognised words are matched to the
    /// reference by their spelling, so a misheard letter or two still matches.
    fn score(&self, words: &[Word]) -> Score {
        let matched = match_words(&self.words, words);
        let mut score = Score { scored: 0, mean: 0, worst: 0, soft_worst: 0, misses: Vec::new() };
        let mut total = 0;
        for b in &self.boundaries {
            let at = |i: usize| matched[i];
            let time = match b.kind {
                Kind::Onset => at(b.word).map(|w| words[w].start_us),
                Kind::Offset => at(b.word).map(|w| words[w].end_us),
                Kind::Join => match (at(b.word - 1), at(b.word)) {
                    (Some(l), Some(r)) if r == l + 1 => Some((words[l].end_us + words[r].start_us) / 2),
                    _ => None,
                },
            };
            let Some(time) = time else { continue };
            let past = (b.from_us - time).max(time - b.to_us).max(0);
            if b.sharp {
                score.scored += 1;
                total += past;
                score.worst = score.worst.max(past);
            } else {
                score.soft_worst = score.soft_worst.max(past);
            }
            if past > 30_000 {
                let kind = match b.kind {
                    Kind::Onset => "start of",
                    Kind::Offset => "end of",
                    Kind::Join => "before",
                };
                score.misses.push(format!(
                    "{kind} {} {:+} ms",
                    self.words[b.word],
                    (time - time.clamp(b.from_us, b.to_us)) / 1000
                ));
            }
        }
        score.mean = total / score.scored.max(1) as i64;
        score
    }
}

/// For each reference word, the recognised word that says it, in order: the longest common
/// subsequence of words alike in their first three letters, without case or diacritics.
fn match_words(reference: &[String], words: &[Word]) -> Vec<Option<usize>> {
    let key = |text: &str| -> String {
        use unicode_normalization::UnicodeNormalization;
        text.nfd().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).take(3).collect()
    };
    let a: Vec<String> = reference.iter().map(|w| key(w)).collect();
    let b: Vec<String> = words.iter().map(|w| key(&w.text)).collect();
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] { table[i + 1][j + 1] + 1 } else { table[i + 1][j].max(table[i][j + 1]) };
        }
    }
    let mut matched = vec![None; a.len()];
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            matched[i] = Some(j);
            (i, j) = (i + 1, j + 1);
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    matched
}
