//! Sentences said with emphasis, as moments for a punch-in.
//!
//! A creator punches in (scales the picture up by 15–30 %) on a key claim or a punchline, never on
//! every sentence. This proposes the sentences that stand out: louder than the median sentence of
//! the timeline, an exclamation, or the opening hook. Only sentences a viewer can take in count
//! (1.2–8 s, three words or more); they lie at least 5 s apart and there is about one per 12 s of
//! timeline, so the result stays subtle. Loudness is measured in the sound of each word's own file,
//! over the word's source time. The same words and sound always give the same result.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use nuzky_engine::{
    Project,
    audio::us_to_samples,
    model::CHANNELS,
    speech::{TimelineWord, Word},
};
use serde::Serialize;

/// A pause this long ends a sentence, as in get_transcript and the retake analysis.
const SENTENCE_GAP_US: i64 = 600_000;
const MIN_SENTENCE_US: i64 = 1_200_000;
const MAX_SENTENCE_US: i64 = 8_000_000;
const MIN_WORDS: usize = 3;
/// Silence between two punch-ins.
const MIN_SPACING_US: i64 = 5_000_000;
/// About one punch-in per this much timeline.
const TIMELINE_PER_ZOOM_US: i64 = 12_000_000;
/// A sentence this much louder than the median one gets the whole loudness share of the score.
const FULL_LOUDNESS_DB: f64 = 6.0;
/// Shares of the score; a sentence below `MIN_SCORE` is not proposed.
const LOUDNESS_WEIGHT: f64 = 0.6;
const EXCLAMATION_WEIGHT: f64 = 0.25;
const HOOK_WEIGHT: f64 = 0.15;
/// In hundredths of the score.
const MIN_SCORE: i64 = 10;
/// Scale of a punch-in from the lowest score to the highest, in ten-thousandths, so the scale of
/// a score is exact.
const MIN_SCALE: i64 = 11_500;
const MAX_SCALE: i64 = 13_000;

/// A proposed punch-in. Word indices are inclusive timeline word numbers, as in get_transcript;
/// the times are those of its first and last word.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Zoom {
    pub from: usize,
    pub to: usize,
    pub start_us: i64,
    pub end_us: i64,
    pub text: String,
    /// 0–1: how much the sentence stands out.
    pub score: f64,
    pub scale: f64,
}

/// The sound of one word: the sum of its squared samples and how many there are.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Energy {
    pub sum: f64,
    pub samples: u64,
}

/// Punch-ins for the timeline words of `project`, whose source words per asset are `sources`.
/// Reads the sound caches in `cache`, preparing a missing one.
pub fn emphasis(
    project: &Project,
    words: &[TimelineWord],
    sources: &HashMap<String, Vec<Word>>,
    cache: &Path,
) -> Result<Vec<Zoom>> {
    let energy = word_energy(project, words, sources, cache)?;
    Ok(select(words, &energy, project.duration_us()))
}

/// The sound of every word over its own source time, in its file.
pub fn word_energy(
    project: &Project,
    words: &[TimelineWord],
    sources: &HashMap<String, Vec<Word>>,
    cache: &Path,
) -> Result<Vec<Energy>> {
    let mut out = vec![Energy::default(); words.len()];
    let mut assets: Vec<&str> = words.iter().map(|w| w.asset_id.as_str()).collect();
    assets.sort_unstable();
    assets.dedup();
    for asset_id in assets {
        let Some(asset) = project.asset(asset_id) else { continue };
        let ends: HashMap<(i64, &str), i64> =
            sources.get(asset_id).into_iter().flatten().map(|w| ((w.start_us, w.text.as_str()), w.end_us)).collect();
        // Reading a word's level must never start decoding a whole file nobody can stop: the app
        // prepares sound in the background and recognition leaves it ready.
        anyhow::ensure!(
            nuzky_engine::audio::pcm_path(cache, asset).exists(),
            "AUDIO_NOT_READY: the sound of {} is still being prepared; try again in a moment",
            asset.name
        );
        let pcm = crate::audio::open_pcm(asset, cache, &|| false)?;
        let samples = pcm.samples();
        let frames = samples.len() / CHANNELS;
        for (word, energy) in words.iter().zip(&mut out).filter(|(w, _)| w.asset_id == asset_id) {
            let Some(&end) = ends.get(&(word.source_start_us, word.text.as_str())) else { continue };
            let from = (us_to_samples(word.source_start_us).max(0) as usize).min(frames);
            let to = (us_to_samples(end).max(0) as usize).clamp(from, frames);
            let span = &samples[from * CHANNELS..to * CHANNELS];
            *energy =
                Energy { sum: span.iter().map(|&s| f64::from(s) * f64::from(s)).sum(), samples: span.len() as u64 };
        }
    }
    Ok(out)
}

/// Picks the punch-ins from the words, the sound of each and the timeline's length.
pub fn select(words: &[TimelineWord], energy: &[Energy], timeline_us: i64) -> Vec<Zoom> {
    let sentences = sentences(words);
    let level = |&(from, to): &(usize, usize)| {
        let (sum, samples) = energy[from..=to].iter().fold((0.0, 0), |(s, n), e| (s + e.sum, n + e.samples));
        (samples > 0).then(|| 10.0 * (sum / samples as f64).max(1e-12).log10())
    };
    let levels: Vec<Option<f64>> = sentences.iter().map(level).collect();
    let mut sorted: Vec<f64> = levels.iter().flatten().copied().collect();
    sorted.sort_by(f64::total_cmp);
    let median = sorted.get(sorted.len().saturating_sub(1) / 2).copied();
    let fits = |&(from, to): &(usize, usize)| {
        let length = words[to].end_us - words[from].start_us;
        let said = words[from..=to].iter().filter(|w| w.text.chars().any(char::is_alphanumeric)).count();
        (MIN_SENTENCE_US..=MAX_SENTENCE_US).contains(&length) && said >= MIN_WORDS
    };
    let hook = sentences.iter().position(fits);
    let mut candidates: Vec<Zoom> = sentences
        .iter()
        .zip(&levels)
        .enumerate()
        .filter(|(_, (sentence, _))| fits(sentence))
        .map(|(index, (&(from, to), level))| {
            let louder = match (level, median) {
                (Some(level), Some(median)) => ((level - median) / FULL_LOUDNESS_DB).clamp(0.0, 1.0),
                _ => 0.0,
            };
            let exclaims = words[to].text.trim_end().ends_with('!');
            let score = LOUDNESS_WEIGHT * louder
                + if exclaims { EXCLAMATION_WEIGHT } else { 0.0 }
                + if hook == Some(index) { HOOK_WEIGHT } else { 0.0 };
            // In whole hundredths, so the result does not hang on the last bits of a float.
            let score = (score * 100.0).round() as i64;
            let scale = (MIN_SCALE + (MAX_SCALE - MIN_SCALE) * score / 100 + 50) / 100;
            Zoom {
                from,
                to,
                start_us: words[from].start_us,
                end_us: words[to].end_us,
                text: words[from..=to].iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" "),
                score: score as f64 / 100.0,
                scale: scale as f64 / 100.0,
            }
        })
        .filter(|zoom| (zoom.score * 100.0).round() as i64 >= MIN_SCORE)
        .collect();
    // The ones that stand out most first, the earlier of two equal ones.
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.start_us.cmp(&b.start_us)));
    let most = ((timeline_us + TIMELINE_PER_ZOOM_US / 2) / TIMELINE_PER_ZOOM_US).max(1) as usize;
    let mut picked: Vec<Zoom> = Vec::new();
    for zoom in candidates {
        if picked.len() == most {
            break;
        }
        let apart = |other: &Zoom| {
            zoom.start_us - other.end_us >= MIN_SPACING_US || other.start_us - zoom.end_us >= MIN_SPACING_US
        };
        if picked.iter().all(apart) {
            picked.push(zoom);
        }
    }
    picked.sort_by_key(|zoom| zoom.start_us);
    picked
}

/// Inclusive word ranges of sentences. They end at . ! ? …, at a pause of `SENTENCE_GAP_US` and
/// where the timeline moves to another file.
fn sentences(words: &[TimelineWord]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, word) in words.iter().enumerate() {
        let ends = word.text.trim_end().ends_with(['.', '!', '?', '…'])
            || words
                .get(i + 1)
                .is_none_or(|next| next.start_us - word.end_us >= SENTENCE_GAP_US || next.asset_id != word.asset_id);
        if ends {
            out.push((from, i));
            from = i + 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sentences back to back in one file: (text, loudness of its words in dBFS, pause after it in
    /// microseconds). Words are 300 ms long with 100 ms between them.
    fn timeline(sentences: &[(&str, f64, i64)]) -> (Vec<TimelineWord>, Vec<Energy>) {
        let (mut words, mut energy) = (Vec::new(), Vec::new());
        let mut t = 500_000;
        for &(text, db, pause) in sentences {
            for word in text.split(' ') {
                words.push(TimelineWord {
                    start_us: t,
                    end_us: t + 300_000,
                    text: format!(" {word}"),
                    probability: 0.9,
                    clip_id: "clip".into(),
                    asset_id: "take".into(),
                    source_start_us: t,
                });
                // 0.3 s of samples at this RMS level.
                let samples = 14_400 * CHANNELS as u64;
                energy.push(Energy { sum: 10f64.powf(db / 10.0) * samples as f64, samples });
                t += 400_000;
            }
            t += pause - 100_000;
        }
        (words, energy)
    }

    fn texts(zooms: &[Zoom]) -> Vec<&str> {
        zooms.iter().map(|z| z.text.as_str()).collect()
    }

    const CALM: f64 = -30.0;

    #[test]
    fn louder_sentences_exclamations_and_the_hook_stand_out() {
        let (words, energy) = timeline(&[
            ("Dneska vám ukážu jak natočit video.", CALM, 900_000),
            ("Mikrofon dejte co nejblíž k puse.", CALM, 900_000),
            ("Potom řešíte světlo z okna.", CALM - 1.0, 900_000),
            ("Kamera musí stát pevně na stativu.", CALM, 900_000),
            ("Tohle je ta nejdůležitější věc vůbec.", CALM + 6.0, 900_000),
            ("Hotové video nahrajte na Instagram.", CALM, 900_000),
            ("Napište mi co chcete vidět příště.", CALM, 900_000),
            ("Celé to zabere jen deset minut!", CALM, 900_000),
            ("Hodně štěstí s prvním videem.", CALM, 900_000),
            ("Ahoj u dalšího dílu.", CALM, 900_000),
        ]);
        let duration = words.last().unwrap().end_us + 500_000;
        let zooms = select(&words, &energy, duration);
        // 31 s: up to three, at least 5 s apart. The hook opens, the loud claim scores highest.
        assert_eq!(
            texts(&zooms),
            [
                "Dneska vám ukážu jak natočit video.",
                "Tohle je ta nejdůležitější věc vůbec.",
                "Celé to zabere jen deset minut!"
            ]
        );
        assert_eq!(
            zooms.iter().map(|z| (z.score, z.scale)).collect::<Vec<_>>(),
            [(0.15, 1.17), (0.6, 1.24), (0.25, 1.19)]
        );
        let loud = &zooms[1];
        assert_eq!((loud.from, loud.to), (23, 28));
        assert_eq!((loud.start_us, loud.end_us), (words[23].start_us, words[28].end_us));
        // The same input gives byte-identical output.
        let json = serde_json::to_string(&zooms).unwrap();
        assert_eq!(json, serde_json::to_string(&select(&words, &energy, duration)).unwrap());
    }

    #[test]
    fn zooms_stay_apart_and_few() {
        // Every sentence loud and exclaimed, 2.7 s apart: only every other one is 5 s away from the last.
        let loud: Vec<(&str, f64, i64)> = (0..12)
            .map(|i| ("Tohle je opravdu důležité!", if i % 2 == 0 { CALM } else { CALM + 3.0 }, 1_100_000))
            .collect();
        let (words, energy) = timeline(&loud);
        let duration = words.last().unwrap().end_us;
        let zooms = select(&words, &energy, duration);
        assert!(zooms.windows(2).all(|p| p[1].start_us - p[0].end_us >= MIN_SPACING_US), "{zooms:#?}");
        // 30 s of timeline allows three.
        assert_eq!(duration / 1_000_000, 30);
        assert_eq!(zooms.len(), 3, "{zooms:#?}");
        // A short video still gets its one punch-in.
        let (words, energy) = timeline(&loud[..2]);
        assert_eq!(select(&words, &energy, 4_000_000).len(), 1);
    }

    #[test]
    fn only_sentences_of_a_readable_length_qualify() {
        let (words, energy) = timeline(&[
            ("Ano!", CALM + 10.0, 900_000),
            ("Tak jo!", CALM + 10.0, 900_000),
            (
                "Tohle je ta nejdůležitější a taky nejdelší věta celého videa kterou snad nikdo neudrží v hlavě ani když ji uslyší třikrát za sebou!",
                CALM + 10.0,
                900_000,
            ),
            ("Potom řešíte světlo z okna.", CALM, 900_000),
            ("Kamera musí stát.", CALM, 900_000),
        ]);
        // One word, two words, 9.1 s and 1.1 s are out; the hook is the first sentence that fits.
        let zooms = select(&words, &energy, 30_000_000);
        assert_eq!(texts(&zooms), ["Potom řešíte světlo z okna."]);
        assert_eq!(zooms[0].score, 0.15);
    }

    #[test]
    fn a_calm_monotone_talk_gets_only_its_hook() {
        let calm: Vec<(&str, f64, i64)> = (0..6).map(|_| ("Potom řešíte světlo z okna.", CALM, 900_000)).collect();
        let (words, energy) = timeline(&calm);
        let zooms = select(&words, &energy, 30_000_000);
        assert_eq!((zooms.len(), zooms[0].from), (1, 0), "{zooms:#?}");
        // No words, no zooms.
        assert!(select(&[], &[], 30_000_000).is_empty());
    }

    /// Without the file's sound cache there is nothing to measure, and nothing starts decoding the
    /// whole file behind the caller's back: it says to try again once the sound is ready.
    #[test]
    fn words_of_a_file_whose_sound_is_not_ready_ask_to_wait() {
        let (words, _) = timeline(&[("Tohle je důležité!", -20.0, 900_000)]);
        let mut project = Project::new("emphasis");
        project
            .apply(nuzky_engine::edit::EditCmd::AddAssets {
                assets: vec![nuzky_engine::model::Asset {
                    id: words[0].asset_id.clone(),
                    name: "take.mov".into(),
                    path: "/nonexistent/take.mov".into(),
                    kind: nuzky_engine::model::AssetKind::Video,
                    duration_us: 5_000_000,
                    width: 2,
                    height: 2,
                    fps: 30.0,
                    has_audio: true,
                    rotation: 0,
                    mirror: false,
                }],
            })
            .unwrap();
        let cache = std::env::temp_dir().join(format!("nuzky-emphasis-{}", nuzky_engine::edit::new_id()));
        let error = word_energy(&project, &words, &HashMap::new(), &cache).unwrap_err().to_string();
        assert!(error.starts_with("AUDIO_NOT_READY") && error.contains("take.mov"), "{error}");
        assert!(!cache.exists(), "nothing was decoded");
    }

    #[test]
    fn sentences_end_at_punctuation_long_pauses_and_another_file() {
        let (mut words, _) = timeline(&[("Jedna dvě tři", CALM, 700_000), ("čtyři pět šest. sedm osm", CALM, 300_000)]);
        words[7].asset_id = "other".into();
        assert_eq!(sentences(&words), [(0, 2), (3, 5), (6, 6), (7, 7)]);
    }
}
