//! Groups transcript words into short on-screen captions, the way reels show them.

use capopen_engine::edit::CaptionSegment;
use serde::{Deserialize, Serialize};

use crate::Word;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptionGrouping {
    /// Most words on screen at once.
    pub max_words: usize,
    /// Most characters per caption, spaces included; a single longer word still gets its own caption.
    pub max_chars: usize,
    /// A pause at least this long between words starts a new caption.
    pub break_gap_us: i64,
}

impl Default for CaptionGrouping {
    /// Reel captions: one or two words, fifteen characters. A hand-made reel of 191 captions
    /// had 110 with two words, 67 with one and 14 with three.
    fn default() -> Self {
        Self { max_words: 2, max_chars: 15, break_gap_us: 300_000 }
    }
}

/// A caption may stay on screen this long after its last word if nothing follows.
const TAIL_US: i64 = 150_000;

/// Captions in time order. A caption ends where the next one starts when they are close,
/// so text does not flicker off between words, otherwise shortly after its last word.
pub fn group_words(words: &[Word], grouping: CaptionGrouping) -> Vec<CaptionSegment> {
    let mut groups: Vec<Vec<&Word>> = Vec::new();
    for word in words.iter().filter(|w| !w.text.trim().is_empty()) {
        match groups.last_mut() {
            Some(group) if !hard_break(group, word, grouping) && fits(group, word, grouping) => group.push(word),
            // "Opus 5.5": a number starts its caption together with the word it belongs to.
            Some(group) if !hard_break(group, word, grouping) && starts_with_digit(word) && group.len() > 1
                && fits(&group[group.len() - 1..], word, grouping) =>
            {
                let carried = group.pop();
                groups.push(carried.into_iter().chain([word]).collect());
            }
            _ => groups.push(vec![word]),
        }
        if ends_phrase(&word.text) {
            balance_phrase_end(&mut groups, grouping);
        }
    }
    let starts: Vec<i64> = groups.iter().map(|g| g[0].start_us).collect();
    groups
        .iter()
        .enumerate()
        .map(|(i, group)| {
            let last_end = group[group.len() - 1].end_us;
            let end = match starts.get(i + 1) {
                Some(&next) if next - last_end < grouping.break_gap_us => next,
                Some(&next) => (last_end + TAIL_US).min(next),
                None => last_end + TAIL_US,
            };
            let text = group.iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ");
            CaptionSegment { start_us: group[0].start_us, end_us: end.max(group[0].start_us + 1), text }
        })
        .collect()
}

/// A phrase ends or the speaker pauses before `next`, so it never shares a caption with `group`.
fn hard_break(group: &[&Word], next: &Word, grouping: CaptionGrouping) -> bool {
    let last = group[group.len() - 1];
    ends_phrase(&last.text) || next.start_us - last.end_us >= grouping.break_gap_us
}

fn fits(group: &[&Word], next: &Word, grouping: CaptionGrouping) -> bool {
    let chars = group.iter().map(|w| w.text.trim().chars().count() + 1).sum::<usize>() + next.text.trim().chars().count();
    group.len() < grouping.max_words.max(1) && chars <= grouping.max_chars
}

/// A phrase does not end on a lone word when the caption before can give it one:
/// "programuje fakt | skvěle." becomes "programuje | fakt skvěle.".
fn balance_phrase_end(groups: &mut [Vec<&Word>], grouping: CaptionGrouping) {
    let [.., before, last] = groups else { return };
    if last.len() != 1 || before.len() < 2 || hard_break(before, last[0], grouping) {
        return;
    }
    if let Some(&moved) = before.last()
        && fits(&[moved], last[0], grouping)
    {
        before.pop();
        last.insert(0, moved);
    }
}

fn starts_with_digit(word: &Word) -> bool {
    word.text.trim_start().starts_with(|c: char| c.is_ascii_digit())
}

fn ends_phrase(text: &str) -> bool {
    text.trim_end().ends_with(['.', ',', '!', '?', ';', ':', '…'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start_ms: i64, end_ms: i64, text: &str) -> Word {
        Word { start_us: start_ms * 1000, end_us: end_ms * 1000, text: text.into(), probability: 1.0 }
    }

    fn texts(words: &[Word], grouping: CaptionGrouping) -> Vec<String> {
        group_words(words, grouping).into_iter().map(|c| c.text).collect()
    }

    #[test]
    fn breaks_on_word_count_characters_punctuation_and_pauses() {
        let words = [
            word(0, 200, "Opus"),
            word(200, 400, "5.5"),
            word(420, 600, "za"),
            word(600, 800, "mě"),
            word(800, 1300, "programuje"),
            word(1300, 1500, "fakt"),
            word(1500, 1900, "skvěle."),
            word(1950, 2300, "Největší"),
            word(2300, 2700, "rozdíl"),
            word(3200, 3500, "je"),
        ];
        assert_eq!(
            texts(&words, CaptionGrouping::default()),
            ["Opus 5.5", "za mě", "programuje", "fakt skvěle.", "Největší rozdíl", "je"]
        );
        let one = CaptionGrouping { max_words: 1, ..CaptionGrouping::default() };
        assert_eq!(texts(&words[..2], one), ["Opus", "5.5"]);
        let three = CaptionGrouping { max_words: 3, ..CaptionGrouping::default() };
        assert_eq!(texts(&words[..5], three), ["Opus 5.5 za", "mě programuje"]);
    }

    #[test]
    fn numbers_stay_with_their_word_and_phrases_do_not_end_on_a_lone_word() {
        let words = [word(0, 300, "přišel"), word(300, 600, "Opus"), word(600, 900, "5.5"), word(900, 1100, "a"), word(1100, 1500, "konec.")];
        assert_eq!(texts(&words, CaptionGrouping::default()), ["přišel", "Opus 5.5", "a konec."]);
        // Across a pause the lone word keeps its own caption.
        let paused = [word(0, 300, "fakt"), word(300, 600, "dobře"), word(1200, 1500, "jo.")];
        assert_eq!(texts(&paused, CaptionGrouping::default()), ["fakt dobře", "jo."]);
    }

    #[test]
    fn captions_bridge_short_gaps_and_end_after_long_ones() {
        let words = [word(0, 400, "Ahoj"), word(420, 800, "světe."), word(2000, 2400, "Konec")];
        let captions = group_words(&words, CaptionGrouping::default());
        // "Ahoj světe." ends at a full stop; the next caption is 1.2 s later, so it ends after a short tail.
        assert_eq!((captions[0].start_us, captions[0].end_us), (0, 950_000));
        assert_eq!((captions[1].start_us, captions[1].end_us), (2_000_000, 2_550_000));
        let close = [word(0, 400, "jsem"), word(500, 900, "se"), word(950, 1300, "sakra,"), word(1350, 1700, "snažit")];
        let captions = group_words(&close, CaptionGrouping::default());
        assert_eq!(captions[0].end_us, captions[1].start_us);
    }

    #[test]
    fn a_long_word_still_gets_its_own_caption() {
        let words = [word(0, 900, "nejneobhospodařovávatelnějšími"), word(900, 1200, "lidmi")];
        assert_eq!(texts(&words, CaptionGrouping::default()), ["nejneobhospodařovávatelnějšími", "lidmi"]);
    }
}
