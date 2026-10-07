//! How close a cut is to the creator's own, word by word of the recording.

use serde::Serialize;

use crate::Word;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Score {
    /// Words recognised in the recording.
    pub words: usize,
    /// Words the creator kept, the cut being scored kept, and both kept.
    pub creator_kept: usize,
    pub cut_kept: usize,
    pub both_kept: usize,
    /// Share of the creator's words the cut kept.
    pub recall: f64,
    /// Share of the cut's words the creator kept too.
    pub precision: f64,
    /// Passages the creator kept and the cut dropped, then the other way round.
    pub missed: Vec<Passage>,
    pub extra: Vec<Passage>,
}

/// Consecutive recording words, timed in the recording.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Passage {
    pub start_us: i64,
    pub end_us: i64,
    pub text: String,
}

pub fn score(words: &[Word], creator: &[bool], cut: &[bool]) -> Score {
    let count = |keep: &dyn Fn(usize) -> bool| (0..words.len()).filter(|&i| keep(i)).count();
    let creator_kept = count(&|i| creator[i]);
    let cut_kept = count(&|i| cut[i]);
    let both_kept = count(&|i| creator[i] && cut[i]);
    let share = |part: usize, whole: usize| if whole == 0 { 0.0 } else { part as f64 / whole as f64 };
    Score {
        words: words.len(),
        creator_kept,
        cut_kept,
        both_kept,
        recall: share(both_kept, creator_kept),
        precision: share(both_kept, cut_kept),
        missed: passages(words, &|i| creator[i] && !cut[i]),
        extra: passages(words, &|i| cut[i] && !creator[i]),
    }
}

/// Runs of consecutive words that satisfy `pick`.
pub(crate) fn passages(words: &[Word], pick: &dyn Fn(usize) -> bool) -> Vec<Passage> {
    let mut out: Vec<Passage> = Vec::new();
    let mut previous = false;
    for (i, word) in words.iter().enumerate() {
        let picked = pick(i);
        if picked {
            match out.last_mut() {
                Some(last) if previous => {
                    last.end_us = word.end_us;
                    last.text.push(' ');
                    last.text.push_str(word.text.trim());
                }
                _ => out.push(Passage { start_us: word.start_us, end_us: word.end_us, text: word.text.trim().into() }),
            }
        }
        previous = picked;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_both_kept_and_names_the_differences() {
        let words: Vec<Word> = "a b c d e f"
            .split(' ')
            .enumerate()
            .map(|(i, t)| Word { start_us: i as i64 * 10, end_us: i as i64 * 10 + 5, text: t.into(), probability: 1.0 })
            .collect();
        let creator = [true, true, true, false, false, true];
        let cut = [false, true, true, true, true, true];
        let score = score(&words, &creator, &cut);
        assert_eq!((score.creator_kept, score.cut_kept, score.both_kept), (4, 5, 3));
        assert_eq!((score.recall, score.precision), (0.75, 0.6));
        assert_eq!(score.missed, [Passage { start_us: 0, end_us: 5, text: "a".into() }]);
        assert_eq!(score.extra, [Passage { start_us: 30, end_us: 45, text: "d e".into() }]);
        let none = super::score(&words, &[false; 6], &[false; 6]);
        assert_eq!((none.recall, none.precision), (0.0, 0.0));
    }
}
