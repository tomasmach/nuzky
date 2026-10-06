use crate::{Range, Transcript, Word};

/// Conservative lexical proposals, not semantic judgements. cs: ehm/hmm/eee;
/// en: um/uh/erm. Ambiguous cs jakoby/prostě and en like/you know require pauses
/// (>=150 ms) or comma boundaries on BOTH sides. Immediate repeated words within
/// 250 ms propose removing earlier copies, keeping the final one. Intentional
/// repetition is indistinguishable from stuttering here: review before applying.
/// Unsupported languages return no proposals.
pub fn filler_words(transcript: &Transcript, language: &str) -> Vec<Range> {
    let language = if language == "auto" { transcript.language.as_str() } else { language };
    if !matches!(language, "cs" | "en") {
        return Vec::new();
    }
    let words = &transcript.words;
    let normalized: Vec<String> = words.iter().map(|w| normalize(&w.text)).collect();
    let mut ranges = Vec::new();
    for (index, word) in words.iter().enumerate() {
        let token = normalized[index].as_str();
        let simple = match language {
            "cs" => matches!(token, "ehm" | "hmm" | "eee"),
            _ => matches!(token, "um" | "uh" | "erm"),
        };
        let ambiguous = matches!((language, token), ("cs", "jakoby" | "prostě") | ("en", "like"));
        if simple || (ambiguous && isolated(words, index, index)) {
            ranges.push(Range { start_us: word.start_us, end_us: word.end_us });
        }
        if language == "en"
            && token == "you"
            && normalized.get(index + 1).is_some_and(|w| w == "know")
            && words[index + 1].start_us.saturating_sub(word.end_us) <= 250_000
            && isolated(words, index, index + 1)
        {
            ranges.push(Range { start_us: word.start_us, end_us: words[index + 1].end_us });
        }
        if !token.is_empty() && !word.text.ends_with(['.', '?', '!', ',', ';', ':']) {
            if let Some(next) = words.get(index + 1) {
                let gap = next.start_us.saturating_sub(word.end_us);
                if normalized[index + 1] == token && (0..=250_000).contains(&gap) {
                    ranges.push(Range { start_us: word.start_us, end_us: word.end_us });
                }
            }
        }
    }
    ranges.retain(|r| r.start_us >= 0 && r.end_us > r.start_us);
    ranges.sort_unstable_by_key(|r| (r.start_us, r.end_us));
    let mut merged: Vec<Range> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start_us <= last.end_us => last.end_us = last.end_us.max(range.end_us),
            _ => merged.push(range),
        }
    }
    merged
}

fn normalize(text: &str) -> String {
    text.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

fn isolated(words: &[Word], first: usize, last: usize) -> bool {
    let before = first == 0
        || words[first].start_us.saturating_sub(words[first - 1].end_us) >= 150_000
        || words[first - 1].text.ends_with(',');
    let after = last + 1 == words.len()
        || words[last + 1].start_us.saturating_sub(words[last].end_us) >= 150_000
        || words[last].text.ends_with(',');
    before && after
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(text: &str) -> Transcript {
        Transcript {
            language: "cs".into(),
            segments: vec![],
            words: text
                .split_whitespace()
                .enumerate()
                .map(|(i, text)| Word {
                    start_us: i as i64 * 200_000,
                    end_us: i as i64 * 200_000 + 180_000,
                    text: text.into(),
                    probability: 1.0,
                })
                .collect(),
        }
    }

    #[test]
    fn conservative_cs_and_stutter_keep_last_copy() {
        let t = transcript("Ehm je je je to prostě jednoduché jakoby nic hmm");
        let ranges = filler_words(&t, "cs");
        assert_eq!(ranges.iter().map(|r| r.start_us).collect::<Vec<_>>(), vec![0, 200_000, 400_000, 1_800_000]);
        assert_eq!(
            filler_words(&transcript("To, prostě, funguje"), "cs"),
            vec![Range { start_us: 200_000, end_us: 380_000 }]
        );
        assert!(filler_words(&transcript("Je. Je to tak"), "cs").is_empty());
    }

    #[test]
    fn english_phrases_preserve_literal_usage() {
        assert!(filler_words(&transcript("I like you and you know this"), "en").is_empty());
        let ranges = filler_words(&transcript("Um I, like, think, you know, uh yes"), "en");
        assert_eq!(ranges.len(), 4);
        assert_eq!(ranges[2], Range { start_us: 800_000, end_us: 1_180_000 });
        assert!(filler_words(&transcript("um"), "de").is_empty());
    }
}
