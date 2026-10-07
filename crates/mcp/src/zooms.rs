//! Punch-ins on emphasis, shared by agents (`analyze(kind: "emphasis")`, `apply_zooms`) and the
//! Transcript panel: which sentences to zoom, and where on the timeline a zoom of words starts and ends.

use std::path::Path;

use anyhow::{Result, ensure};
use capopen_engine::{
    Project,
    edit::{MAIN_TRACK, ZoomRange},
    speech::TimelineWord,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::transcript::Derived;

/// How far a punch-in reaches into the silence before its first word and after its last.
pub const EDGE_US: i64 = 150_000;

/// A punch-in on INCLUSIVE timeline word numbers, as get_transcript numbers them.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WordZoom {
    pub from: usize,
    pub to: usize,
    /// Multiplies the picture's scale, 1.15–1.3 for a subtle punch-in.
    pub scale: f64,
}

/// Sentences of the timeline to punch in on, from the stored words and the sound of the files.
pub fn suggest(project: &Project, derived: &Derived, cache: &Path) -> Result<Vec<capopen_analysis::Zoom>> {
    ensure!(
        derived.untranscribed.is_empty(),
        "TRANSCRIPT_MISSING: transcribe every heard asset first, untranscribed: {}",
        derived.untranscribed.join(", ")
    );
    capopen_analysis::emphasis(project, &derived.words, &derived.sources, cache)
}

/// Timeline ranges of word zooms. Each starts midway into the silence before its first word and
/// ends midway into the silence after its last, at most `EDGE_US` from the word, so the picture
/// changes between words. Where no word lies between that edge and the cut of its main-track clip,
/// the zoom reaches the cut: a piece of a take with only silence in it would count as a clip
/// without speech, whose pause the transcript can no longer remove. The engine then moves an edge
/// to a nearby cut rather than leave a sliver.
pub fn ranges(project: &Project, words: &[TimelineWord], zooms: &[WordZoom]) -> Result<Vec<ZoomRange>> {
    ensure!(!zooms.is_empty(), "INVALID_ARGUMENTS: give at least one zoom");
    let mut sorted = zooms.to_vec();
    sorted.sort_by_key(|z| z.from);
    for z in &sorted {
        ensure!(
            z.from <= z.to && z.to < words.len(),
            "INVALID_WORD_RANGE: inclusive word indices [{}, {}] out of bounds for {} words",
            z.from,
            z.to,
            words.len()
        );
    }
    ensure!(sorted.windows(2).all(|pair| pair[0].to < pair[1].from), "INVALID_WORD_RANGE: zooms overlap");
    let end = project.duration_us();
    let main = project.tracks.iter().find(|t| t.id == MAIN_TRACK).map_or(&[][..], |t| &t.clips[..]);
    let clip_at = |t: i64| main.iter().find(|c| c.start_us < t && t < c.end_us());
    let silent = |from: i64, to: i64| words.iter().all(|w| w.end_us <= from || w.start_us >= to);
    Ok(sorted
        .iter()
        .map(|z| {
            let (first, last) = (words[z.from].start_us, words[z.to].end_us);
            let before = z.from.checked_sub(1).map_or(0, |i| words[i].end_us);
            let after = words.get(z.to + 1).map_or(end, |w| w.start_us);
            let mut start_us = (first - ((first - before).max(0) / 2).min(EDGE_US)).max(0);
            let mut end_us = last + ((after - last).max(0) / 2).min(EDGE_US);
            if let Some(clip) = clip_at(start_us).filter(|c| silent(c.start_us, start_us)) {
                start_us = clip.start_us;
            }
            if let Some(clip) = clip_at(end_us).filter(|c| silent(end_us, c.end_us())) {
                end_us = clip.end_us();
            }
            ZoomRange { start_us, end_us, scale: z.scale }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use capopen_engine::speech::map_words;

    #[test]
    fn edges_sit_midway_into_the_silence_and_at_most_150_ms_out() {
        // Words at 0.5–0.9 s, 1.5–1.9 s and so on: 0.6 s of silence between them.
        let (project, sources) = crate::transcript::tests::fixture();
        let mut words = map_words(&project, &sources);
        let zooms = [WordZoom { from: 3, to: 5, scale: 1.2 }, WordZoom { from: 1, to: 2, scale: 1.25 }];
        let found = ranges(&project, &words, &zooms).unwrap();
        assert_eq!(
            found,
            [
                ZoomRange { start_us: 1_350_000, end_us: 3_050_000, scale: 1.25 },
                ZoomRange { start_us: 3_350_000, end_us: 6_050_000, scale: 1.2 },
            ]
        );
        // Closer words: midway into a 0.1 s gap.
        words[2].end_us = words[3].start_us - 100_000;
        assert_eq!(ranges(&project, &words, &zooms[..1]).unwrap()[0].start_us, 3_450_000);
        for bad in [[WordZoom { from: 2, to: 1, scale: 1.2 }], [WordZoom { from: 7, to: 8, scale: 1.2 }]] {
            assert!(ranges(&project, &words, &bad).unwrap_err().to_string().starts_with("INVALID_WORD_RANGE"));
        }
        let overlapping = [WordZoom { from: 0, to: 3, scale: 1.2 }, WordZoom { from: 3, to: 4, scale: 1.2 }];
        assert!(ranges(&project, &words, &overlapping).unwrap_err().to_string().contains("overlap"));
    }

    /// The first and the last sentence of a take: the silence before the first word and after the
    /// last zooms with them, so no piece of the take is left without words, which the transcript
    /// would take for B-roll and never shorten. Every pause stays as long as it was.
    #[test]
    fn zooms_at_the_edges_of_a_take_reach_the_cut_and_keep_every_pause() {
        use crate::transcript::{Derived, pauses};
        let (mut project, sources) = crate::transcript::tests::fixture();
        let derived = |project: &Project| Derived::new(project, sources.clone(), vec![]);
        let before = derived(&project);
        let zooms = [WordZoom { from: 0, to: 1, scale: 1.2 }, WordZoom { from: 6, to: 7, scale: 1.2 }];
        let found = ranges(&project, &before.words, &zooms).unwrap();
        assert_eq!(
            found.iter().map(|r| (r.start_us, r.end_us)).collect::<Vec<_>>(),
            [(0, 2_050_000), (6_350_000, 10_000_000)]
        );
        let gaps = |project: &Project, derived: &Derived| {
            pauses(project, derived, 500_000).unwrap().iter().map(|p| p.gap_us).collect::<Vec<_>>()
        };
        let kept = gaps(&project, &before);
        project.apply(capopen_engine::edit::EditCmd::ZoomRanges { ranges: found }).unwrap();
        assert_eq!(project.tracks[0].clips.len(), 3);
        assert_eq!(gaps(&project, &derived(&project)), kept);
    }
}
