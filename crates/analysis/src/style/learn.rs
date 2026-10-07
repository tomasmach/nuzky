//! Turns recordings, their finished cuts and what was measured in them into EDIT.md: rules in
//! numbers, each shown on moments of the recordings. The same input always gives the same text.

use std::fmt::Write as _;

use crate::Word;

use super::{Alignment, Caption, Picture, token};

/// Rules need at least this many moments from the recordings to be written down.
const MIN_EXAMPLES: usize = 3;
/// Examples shown for one rule.
const SHOWN: usize = 4;
/// A pause this long ends a sentence even without punctuation.
const SENTENCE_GAP_US: i64 = 700_000;
/// Calls to action are looked for in the last part of a recording.
const ENDING_SHARE: f64 = 0.7;

const FILLERS_CS: &[&str] =
    &["ehm", "hmm", "hm", "eee", "ee", "em", "mmm", "no", "jako", "jakoby", "prostě", "vlastně", "teda", "takže", "jo"];
const FILLERS_EN: &[&str] =
    &["um", "uh", "erm", "er", "ah", "hmm", "like", "so", "well", "basically", "actually", "literally", "okay"];
/// Word beginnings of asking viewers to comment, follow, like or share.
const CALLS: &[&str] = &[
    "koment", "napiš", "napišt", "sleduj", "odebír", "sdílej", "lajk", "follow", "odkaz", "link", "bio", "ulož",
    "comment", "subscrib", "like", "share", "save",
];

/// One recording, its finished cut and what was measured between them.
pub struct Source<'a> {
    pub recording: String,
    pub cut: String,
    pub language: String,
    pub recording_us: i64,
    /// Words recognised in the recording, in its own time, and in the cut, in the cut's time.
    pub words: &'a [Word],
    pub cut_words: &'a [Word],
    pub alignment: &'a Alignment,
    pub picture: &'a Picture,
}

/// A recording word with where it plays in the cut.
struct Spoken<'a> {
    word: &'a Word,
    token: String,
    /// Piece and cut time, when the cut kept it.
    cut: Option<(usize, i64, i64)>,
}

struct Edit<'a> {
    index: usize,
    source: &'a Source<'a>,
    words: Vec<Spoken<'a>>,
}

impl<'a> Edit<'a> {
    fn new(index: usize, source: &'a Source<'a>) -> Self {
        let places = source.alignment.places(source.words, source.cut_words);
        let words =
            source.words.iter().zip(places).map(|(word, cut)| Spoken { word, token: token(&word.text), cut }).collect();
        Self { index, source, words }
    }

    fn kept(&self, i: usize) -> bool {
        self.words[i].cut.is_some()
    }

    /// Kept words in cut order with their cut times.
    fn cut_words(&self) -> Vec<(i64, i64, &Word)> {
        let mut out: Vec<_> = self.words.iter().filter_map(|w| w.cut.map(|(_, s, e)| (s, e, w.word))).collect();
        out.sort_by_key(|(s, e, _)| (*s, *e));
        out
    }

    fn text(&self, from: usize, to: usize) -> String {
        self.words[from..=to].iter().map(|w| w.word.text.trim()).collect::<Vec<_>>().join(" ")
    }
}

/// A moment of one recording, quoted.
#[derive(Clone)]
struct Example {
    source: usize,
    time_us: i64,
    line: String,
}

pub fn learn(sources: &[Source]) -> String {
    let edits: Vec<Edit> = sources.iter().enumerate().map(|(i, s)| Edit::new(i, s)).collect();
    let many = edits.len() > 1;
    let names: Vec<&str> = sources.iter().map(|s| s.recording.as_str()).collect();
    let say = |examples: &[Example]| -> String {
        let mut out = String::new();
        for e in spread(examples) {
            let at = if many { format!("{} {}", names[e.source], clock(e.time_us)) } else { clock(e.time_us) };
            let _ = writeln!(out, "- {at} {}", e.line);
        }
        out
    };
    let mut settings: Vec<(String, String)> = Vec::new();
    // Kinds seen too rarely for a rule, with what was seen.
    let mut rare: Vec<String> = Vec::new();
    let mut body = String::new();

    // What gets cut.
    let words: usize = edits.iter().map(|e| e.words.len()).sum();
    let kept: usize = edits.iter().map(|e| (0..e.words.len()).filter(|&i| e.kept(i)).count()).sum();
    let recorded: i64 = sources.iter().map(|s| s.recording_us).sum();
    let cut_us: i64 = sources.iter().map(|s| s.alignment.cut_duration_us).sum();
    let _ = writeln!(body, "## What gets cut\n");
    let _ = writeln!(
        body,
        "The finished cuts kept {kept} of {words} recognised words ({}) and {} of {} recorded ({}). That share is what the rules below produced, not a goal: never cut a sentence only to get closer to it.\n",
        percent(kept, words),
        clock(cut_us),
        clock(recorded),
        percent(cut_us as usize, recorded as usize)
    );
    let removals = removals(&edits);
    if let Some(section) = said_once(&edits, &removals, &say) {
        body.push_str(&section);
    }
    for kind in Kind::ALL {
        let found: Vec<&Removal> = removals.iter().filter(|r| r.kind == kind).collect();
        if found.is_empty() {
            continue;
        }
        let examples: Vec<Example> = found.iter().map(|r| r.example.clone()).collect();
        if examples.len() < MIN_EXAMPLES {
            let seen: Vec<String> = spread(&examples).iter().map(|e| e.line.clone()).collect();
            rare.push(format!("{}, {} seen: {}", kind.title(), examples.len(), seen.join("; ")));
            continue;
        }
        let seconds: i64 = found.iter().map(|r| r.duration_us).sum();
        let _ = writeln!(body, "### {}\n", kind.title());
        let _ = writeln!(body, "{}", kind.rule(found.len(), seconds, &removals, &edits));
        let _ = writeln!(body, "\n{}", say(&examples));
    }
    if let Some((section, setting)) = fillers(&edits, &say) {
        body.push_str(&section);
        settings.extend(setting);
    }

    // Pauses, pace, cuts, captions and zoom.
    let pauses = pauses(&edits, &say);
    if let Some((section, setting)) = pauses {
        body.push_str(&section);
        settings.push(setting);
    }
    if let Some(section) = pace(&edits, &say) {
        body.push_str(&section);
    }
    if let Some((section, setting)) = cuts(&edits, &say) {
        body.push_str(&section);
        settings.push(setting);
    }
    match captions(&edits, &say) {
        Some((section, rows)) => {
            body.push_str(&section);
            settings.extend(rows);
        }
        None => rare.push("Burned-in captions: none found".into()),
    }
    match zoom(&edits, &say) {
        Some((section, rows)) => {
            body.push_str(&section);
            settings.extend(rows);
        }
        None => rare.push("Zoom: no framing could be measured".into()),
    }

    let mut doc = String::new();
    let _ = writeln!(doc, "# Editing style\n");
    let _ = writeln!(
        doc,
        "CapOpen measured how these recordings became their finished cuts. An agent editing through CapOpen follows the rules and numbers here instead of the general defaults in capopen://guide, and keeps the defaults for anything this file does not cover. Edit this file by hand to change the style; delete it to go back to the defaults.\n"
    );
    let _ = writeln!(doc, "| Recording | Language | Length | Finished cut | Length |\n|---|---|---|---|---|");
    for s in sources {
        let _ = writeln!(
            doc,
            "| {} | {} | {} | {} | {} |",
            s.recording,
            s.language,
            clock(s.recording_us),
            s.cut,
            clock(s.alignment.cut_duration_us)
        );
    }
    if !settings.is_empty() {
        let _ = writeln!(doc, "\n## Settings\n\n| Setting | Value |\n|---|---|");
        for (name, value) in &settings {
            let _ = writeln!(doc, "| {name} | {value} |");
        }
    }
    doc.push('\n');
    doc.push_str(&body);
    if !rare.is_empty() {
        let _ = writeln!(
            doc,
            "## Seen too rarely for a rule\n\nFewer than {MIN_EXAMPLES} times, so these are not rules and the guide's defaults apply. They only show what else happened.\n"
        );
        for line in &rare {
            let _ = writeln!(doc, "- {line}");
        }
    }
    plain(&doc)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Restart,
    Repeat,
    Filler,
    Slip,
    Call,
    LeadIn,
    Ending,
    Dropped,
}

impl Kind {
    const ALL: [Kind; 8] =
        [Kind::Restart, Kind::Repeat, Kind::Dropped, Kind::Slip, Kind::Filler, Kind::LeadIn, Kind::Ending, Kind::Call];

    fn title(self) -> &'static str {
        match self {
            Kind::Restart => "Restarted sentences",
            Kind::Repeat => "Repeats of something already kept",
            Kind::Filler => "Filler sounds",
            Kind::Slip => "Slips and stray words",
            Kind::Call => "Closing calls to action",
            Kind::LeadIn => "Talk before the start",
            Kind::Ending => "Talk after the end",
            Kind::Dropped => "Dropped passages",
        }
    }

    fn rule(self, count: usize, duration_us: i64, all: &[Removal], edits: &[Edit]) -> String {
        let total = format!("{count} cut, {} in total", seconds(duration_us));
        match self {
            Kind::Restart => {
                let repeats = all.iter().filter(|r| r.kind == Kind::Repeat).count();
                let shorter = all.iter().filter(|r| r.kind == Kind::Restart && r.longer_than_retake).count();
                let mut rule = format!(
                    "An attempt at a sentence that the creator then said again: {total}. The creator kept the latest attempt {count} of {} times",
                    count + repeats
                );
                if shorter > 0 {
                    let times = if shorter == 1 { "once".to_owned() } else { format!("{shorter} times") };
                    rule.push_str(&format!(", {times} even though an earlier attempt had more words"));
                }
                rule.push_str(". Cut every earlier attempt, including half sentences that trail off");
                rule.push_str(if shorter > 0 {
                    ", and keep the latest one even when an earlier attempt is longer or more complete."
                } else {
                    ", and keep the latest one."
                });
                rule
            }
            Kind::Repeat => format!(
                "Something said again after a take of it was already kept: {total}. Here the earlier take was kept, so cut the repeat."
            ),
            Kind::Filler => format!("Sounds without meaning: {total}. Cut them."),
            Kind::Slip => {
                format!("One to three words between kept speech that are not part of the sentence: {total}. Cut them.")
            }
            Kind::Call => {
                let heard = edits.iter().map(|e| calls(e).len()).sum::<usize>();
                format!(
                    "Asking viewers to comment, follow or share near the end: {total}, of {heard} such sentences recorded. Cut them unless the user asks for one."
                )
            }
            Kind::LeadIn => {
                format!("Anything before the first kept sentence: {total}. Start the cut at the first real sentence.")
            }
            Kind::Ending => {
                format!("Anything after the last kept sentence: {total}. End the cut on the last point made.")
            }
            Kind::Dropped => format!(
                "Whole sentences said only once and still cut, such as side remarks and points the video does not need: {total}, typically {} each. Weigh every sentence against the point of the video; these examples show what this creator leaves out.",
                seconds(duration_us / count.max(1) as i64)
            ),
        }
    }
}

/// A cut-out piece of one recording, one sentence at most.
struct Removal {
    kind: Kind,
    duration_us: i64,
    /// A restart whose kept retake has fewer words than this attempt.
    longer_than_retake: bool,
    /// Recording and inclusive word range.
    words: (usize, usize, usize),
    example: Example,
}

/// How often the creator kept a sentence said only once: neither an attempt said again, nor a
/// slip or a filler. Next to the rules about what to cut, this says how much else stays.
fn said_once(edits: &[Edit], removals: &[Removal], say: &dyn Fn(&[Example]) -> String) -> Option<String> {
    let mut kept = Vec::new();
    let mut total = 0;
    for edit in edits {
        let retaken = |k: usize| {
            removals.iter().any(|r| {
                let (source, from, to) = r.words;
                source == edit.index
                    && (from..=to).contains(&k)
                    && matches!(r.kind, Kind::Restart | Kind::Repeat | Kind::Filler | Kind::Slip)
            })
        };
        for (from, to) in sentences(edit, 0, edit.words.len().saturating_sub(1)) {
            let words = to - from + 1;
            if edit.words.is_empty() || (from..=to).filter(|&k| retaken(k)).count() * 2 >= words {
                continue;
            }
            total += 1;
            if (from..=to).filter(|&k| edit.kept(k)).count() * 2 > words {
                let line = format!("kept \"{}\"", edit.text(from, to));
                kept.push(Example { source: edit.index, time_us: edit.words[from].word.start_us, line });
            }
        }
    }
    if kept.len() < MIN_EXAMPLES {
        return None;
    }
    Some(format!(
        "### Sentences said once\n\nOf {total} sentences said only once, the creator kept {} ({}). When unsure whether to cut a sentence said once, keep it.\n\n{}\n",
        kept.len(),
        percent(kept.len(), total),
        say(&kept)
    ))
}

/// Every cut-out run of words, split into sentences and sorted into kinds.
fn removals(edits: &[Edit]) -> Vec<Removal> {
    let mut out = Vec::new();
    for edit in edits {
        let n = edit.words.len();
        let first_kept = (0..n).find(|&i| edit.kept(i));
        let last_kept = (0..n).rev().find(|&i| edit.kept(i));
        let mut i = 0;
        while i < n {
            if edit.kept(i) {
                i += 1;
                continue;
            }
            let start = i;
            while i < n && !edit.kept(i) {
                i += 1;
            }
            let end = i - 1;
            let after: Vec<usize> = (i..n).filter(|&k| edit.kept(k)).take(25).collect();
            let mut before: Vec<usize> = (0..start).rev().filter(|&k| edit.kept(k)).take(25).collect();
            before.reverse();
            let tokens_of =
                |indices: &[usize]| indices.iter().map(|&k| edit.words[k].token.as_str()).collect::<Vec<_>>();
            let (after_tokens, before_tokens) = (tokens_of(&after), tokens_of(&before));
            for (from, to) in sentences(edit, start, end) {
                let tokens: Vec<&str> =
                    (from..=to).map(|k| edit.words[k].token.as_str()).filter(|t| !t.is_empty()).collect();
                let (first, last) = (&edit.words[from].word, &edit.words[to].word);
                let leading = first_kept.is_none_or(|f| to < f);
                let trailing = last_kept.is_none_or(|l| from > l);
                let late = first.start_us as f64 >= edit.source.recording_us as f64 * ENDING_SHARE;
                let retake = shares(&tokens, &after_tokens).map(|k| after[k]);
                let kind = if !tokens.is_empty() && tokens.iter().all(|t| is_filler(t, &edit.source.language)) {
                    Kind::Filler
                } else if retake.is_some() {
                    Kind::Restart
                } else if (late || trailing) && tokens.iter().any(|t| is_call(t)) {
                    Kind::Call
                } else if shares(&tokens, &before_tokens).is_some() {
                    Kind::Repeat
                } else if leading {
                    Kind::LeadIn
                } else if trailing {
                    Kind::Ending
                } else if tokens.len() <= 3 && last.end_us - first.start_us <= 1_500_000 {
                    Kind::Slip
                } else {
                    Kind::Dropped
                };
                let text = edit.text(from, to);
                // The retake from the start to the end of its sentence.
                let retake = retake.filter(|_| kind == Kind::Restart).map(|mut k| {
                    while k > 0
                        && edit.kept(k - 1)
                        && !ends_sentence(&edit.words[k - 1].word.text)
                        && edit.words[k].word.start_us - edit.words[k - 1].word.end_us < SENTENCE_GAP_US
                    {
                        k -= 1;
                    }
                    let mut end = k;
                    while end + 1 < n
                        && edit.kept(end + 1)
                        && !ends_sentence(&edit.words[end].word.text)
                        && edit.words[end + 1].word.start_us - edit.words[end].word.end_us < SENTENCE_GAP_US
                    {
                        end += 1;
                    }
                    (k, end)
                });
                let line = match retake {
                    Some((k, end)) => format!("cut \"{text}\", kept the retake \"{}\"", edit.text(k, end.min(k + 11))),
                    None if kind == Kind::Restart => format!("cut \"{text}\""),
                    None => format!("cut \"{text}\" ({})", seconds(last.end_us - first.start_us)),
                };
                out.push(Removal {
                    kind,
                    duration_us: last.end_us - first.start_us,
                    longer_than_retake: retake.is_some_and(|(k, end)| end - k < to - from),
                    words: (edit.index, from, to),
                    example: Example { source: edit.index, time_us: first.start_us, line },
                });
            }
        }
    }
    out
}

/// Inclusive word ranges of sentences within `start..=end`.
fn sentences(edit: &Edit, start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if end >= edit.words.len() || start > end {
        return out;
    }
    let mut from = start;
    for k in start..=end {
        let word = edit.words[k].word;
        let gap = edit.words.get(k + 1).map_or(0, |next| next.word.start_us - word.end_us);
        if k == end || ends_sentence(&word.text) || gap >= SENTENCE_GAP_US {
            out.push((from, k));
            from = k + 1;
        }
    }
    out
}

/// Where `b` first repeats wording of `a`: three words in a row, or two whose letters add up to
/// six or more.
fn shares(a: &[&str], b: &[&str]) -> Option<usize> {
    let long = |w: &[&str]| w.len() == 3 || w[0].chars().count() + w[1].chars().count() >= 6;
    (0..b.len()).find(|&k| {
        [2, 3].iter().any(|&n| k + n <= b.len() && long(&b[k..k + n]) && a.windows(n).any(|w| w == &b[k..k + n]))
    })
}

fn fillers_for(language: &str) -> &'static [&'static str] {
    match language {
        "cs" | "sk" => FILLERS_CS,
        _ => FILLERS_EN,
    }
}

fn is_filler(token: &str, language: &str) -> bool {
    fillers_for(language).contains(&token)
}

fn is_call(token: &str) -> bool {
    CALLS.iter().any(|stem| token.starts_with(stem))
}

/// Sentences that ask viewers for something near the end of a recording.
fn calls(edit: &Edit) -> Vec<(usize, usize)> {
    sentences(edit, 0, edit.words.len().saturating_sub(1))
        .into_iter()
        .filter(|&(from, to)| {
            !edit.words.is_empty()
                && edit.words[from].word.start_us as f64 >= edit.source.recording_us as f64 * ENDING_SHARE
                && (from..=to).any(|k| is_call(&edit.words[k].token))
        })
        .collect()
}

/// How often each filler word is cut where it is heard.
fn fillers(edits: &[Edit], say: &dyn Fn(&[Example]) -> String) -> Option<(String, Vec<(String, String)>)> {
    let mut rows: Vec<(String, usize, usize, Vec<Example>)> = Vec::new();
    for edit in edits {
        for (i, w) in edit.words.iter().enumerate() {
            if !is_filler(&w.token, &edit.source.language) {
                continue;
            }
            let from = i.saturating_sub(3);
            let to = (i + 3).min(edit.words.len() - 1);
            let line = format!(
                "\"{}\": {}",
                edit.text(from, to),
                if edit.kept(i) {
                    format!("\"{}\" kept", w.word.text.trim())
                } else {
                    format!("\"{}\" cut", w.word.text.trim())
                }
            );
            let example = Example { source: edit.index, time_us: w.word.start_us, line };
            match rows.iter_mut().find(|r| r.0 == w.token) {
                Some(row) => {
                    row.1 += 1;
                    row.2 += usize::from(!edit.kept(i));
                    row.3.push(example);
                }
                None => rows.push((w.token.clone(), 1, usize::from(!edit.kept(i)), vec![example])),
            }
        }
    }
    rows.retain(|r| r.1 >= MIN_EXAMPLES);
    if rows.is_empty() {
        return None;
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out = String::from(
        "### Filler words\n\nHow often each word that is often filler was cut where it was said. Cut a word the creator usually cuts, keep one the creator usually keeps.\n\n| Word | Heard | Cut |\n|---|---|---|\n",
    );
    for (word, heard, cut, _) in &rows {
        let _ = writeln!(out, "| {word} | {heard} | {cut} ({}) |", percent(*cut, *heard));
    }
    out.push('\n');
    for (_, _, _, examples) in &rows {
        out.push_str(&say(examples));
    }
    out.push('\n');
    let usually: Vec<&str> = rows.iter().filter(|r| r.2 * 2 > r.1).map(|r| r.0.as_str()).collect();
    let setting = (!usually.is_empty()).then(|| ("Filler words to cut".to_owned(), usually.join(", ")));
    Some((out, setting.into_iter().collect()))
}

/// Silences heard between speech in the recordings and in their cuts.
fn pauses(edits: &[Edit], say: &dyn Fn(&[Example]) -> String) -> Option<(String, (String, String))> {
    let (mut recorded, mut kept, mut examples) = (Vec::new(), Vec::new(), Vec::new());
    let (mut recorded_minutes, mut cut_minutes) = (0.0, 0.0);
    for edit in edits {
        let alignment = edit.source.alignment;
        recorded_minutes += edit.source.recording_us as f64 / 60e6;
        cut_minutes += alignment.cut_duration_us as f64 / 60e6;
        recorded.extend(alignment.recording_pauses.iter().map(|p| p.end_us - p.start_us));
        let words = edit.source.cut_words;
        for pause in &alignment.cut_pauses {
            kept.push(pause.end_us - pause.start_us);
            let before: Vec<&str> = words
                .iter()
                .filter(|w| w.end_us <= pause.start_us + 100_000)
                .rev()
                .take(3)
                .map(|w| w.text.trim())
                .collect();
            let after: Vec<&str> =
                words.iter().filter(|w| w.start_us >= pause.end_us - 100_000).take(3).map(|w| w.text.trim()).collect();
            if before.is_empty() || after.is_empty() {
                continue;
            }
            let before: Vec<&str> = before.into_iter().rev().collect();
            examples.push(Example {
                source: edit.index,
                time_us: pause.start_us,
                line: format!(
                    "\"{}\" | \"{}\": {} in the cut",
                    before.join(" "),
                    after.join(" "),
                    millis(pause.end_us - pause.start_us)
                ),
            });
        }
    }
    if kept.len() < MIN_EXAMPLES || examples.len() < MIN_EXAMPLES {
        return None;
    }
    let longest = round_to(percentile(&kept, 0.9), 10_000).clamp(100_000, 1_000_000);
    let mut out = String::from("## Pauses\n\n");
    let _ = writeln!(
        out,
        "Silences heard between speech. The cut leaves {:.0} a minute with a median of {} and 90% at most {}; the longest is {}. The recording had {:.0} a minute with a median of {} and the longest {}.\n",
        kept.len() as f64 / cut_minutes.max(1e-9),
        millis(percentile(&kept, 0.5)),
        millis(percentile(&kept, 0.9)),
        millis(percentile(&kept, 1.0)),
        recorded.len() as f64 / recorded_minutes.max(1e-9),
        millis(percentile(&recorded, 0.5)),
        millis(percentile(&recorded, 1.0)),
    );
    let _ = writeln!(out, "| Silence | In the recording | In the cut |\n|---|---|---|");
    for (name, range) in [
        ("80 to 300 ms", 0..300_000),
        ("300 to 600 ms", 300_000..600_000),
        ("600 ms to 1 s", 600_000..1_000_000),
        ("1 s or more", 1_000_000..i64::MAX),
    ] {
        let count = |values: &[i64]| values.iter().filter(|v| range.contains(v)).count();
        let _ = writeln!(out, "| {name} | {} | {} |", count(&recorded), count(&kept));
    }
    let _ = writeln!(
        out,
        "\nShorten every pause longer than {} to that length. Times in the examples are in the finished cut.\n\n{}",
        millis(longest),
        say(&examples)
    );
    Some((out, ("edit_transcript shorten_pauses_us".into(), longest.to_string())))
}

/// Words per second before and after the cut.
fn pace(edits: &[Edit], say: &dyn Fn(&[Example]) -> String) -> Option<String> {
    let (mut words, mut spoken, mut kept, mut cut) = (0usize, 0i64, 0usize, 0i64);
    let mut examples: Vec<(i64, Example)> = Vec::new();
    for edit in edits {
        let (Some(first), Some(last)) = (edit.words.first(), edit.words.last()) else { continue };
        words += edit.words.len();
        spoken += last.word.end_us - first.word.start_us;
        let in_cut = edit.cut_words();
        if let (Some(a), Some(b)) = (in_cut.first(), in_cut.last()) {
            kept += in_cut.len();
            cut += b.1 - a.0;
        }
        for (from, to) in sentences(edit, 0, edit.words.len() - 1) {
            if to - from < 3 || !(from..=to).all(|k| edit.kept(k)) {
                continue;
            }
            let recorded = edit.words[to].word.end_us - edit.words[from].word.start_us;
            let (Some(a), Some(b)) = (edit.words[from].cut, edit.words[to].cut) else { continue };
            let played = b.2 - a.1;
            if played > 0 {
                let n = to - from + 1;
                let line = format!(
                    "\"{}\": {} words in {} recorded, {} in the cut ({:.1} to {:.1} words per second)",
                    edit.text(from, to),
                    n,
                    seconds(recorded),
                    seconds(played),
                    n as f64 / (recorded as f64 / 1e6),
                    n as f64 / (played as f64 / 1e6)
                );
                examples.push((
                    recorded - played,
                    Example { source: edit.index, time_us: edit.words[from].word.start_us, line },
                ));
            }
        }
    }
    if spoken <= 0 || cut <= 0 || examples.len() < MIN_EXAMPLES {
        return None;
    }
    examples.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.source.cmp(&b.1.source)).then(a.1.time_us.cmp(&b.1.time_us)));
    let mut best: Vec<Example> = examples.into_iter().take(SHOWN).map(|(_, e)| e).collect();
    best.sort_by_key(|e| (e.source, e.time_us));
    let mut out = String::from("## Pace\n\n");
    let _ = writeln!(
        out,
        "The recording runs at {:.1} words per second from its first to its last word; the cut at {:.1}. Sentences the cut kept whole got faster like this:\n\n{}",
        words as f64 / (spoken as f64 / 1e6),
        kept as f64 / (cut as f64 / 1e6),
        say(&best)
    );
    Some(out)
}

/// Places where two pieces of the recording meet.
fn cuts(edits: &[Edit], say: &dyn Fn(&[Example]) -> String) -> Option<(String, (String, String))> {
    let mut joins = Vec::new();
    let mut lengths = Vec::new();
    let minutes: f64 = edits.iter().map(|e| e.source.alignment.cut_duration_us as f64 / 60e6).sum();
    for edit in edits {
        let pieces = &edit.source.alignment.pieces;
        lengths.extend(pieces.iter().map(|p| p.end_us - p.start_us));
        let in_cut = edit.cut_words();
        for pair in pieces.windows(2) {
            let at = pair[1].start_us;
            let before: Vec<&str> =
                in_cut.iter().filter(|w| w.1 <= at).rev().take(3).map(|w| w.2.text.trim()).collect();
            let after: Vec<&str> = in_cut.iter().filter(|w| w.0 >= at).take(3).map(|w| w.2.text.trim()).collect();
            if before.is_empty() || after.is_empty() {
                continue;
            }
            let skipped = pair[1].source().start_us - pair[0].source().end_us;
            let what = if skipped >= 0 {
                format!("{} of the recording left out", seconds(skipped))
            } else {
                "an earlier moment of the recording follows".into()
            };
            let before: Vec<&str> = before.into_iter().rev().collect();
            joins.push(Example {
                source: edit.index,
                time_us: pair[0].source().end_us,
                line: format!("\"{}\" | \"{}\": {what}", before.join(" "), after.join(" ")),
            });
        }
    }
    if joins.len() < MIN_EXAMPLES || minutes <= 0.0 {
        return None;
    }
    let per_minute = joins.len() as f64 / minutes;
    let mut out = String::from("## Cuts\n\n");
    let _ = writeln!(
        out,
        "{} cuts, {per_minute:.1} per minute of finished video. Pieces of the recording last a median of {} between cuts.\n\n{}",
        joins.len(),
        seconds(percentile(&lengths, 0.5)),
        say(&joins)
    );
    Some((out, ("Cuts per minute".into(), format!("{per_minute:.1}"))))
}

/// Words on screen per caption, from when the burned-in captions change.
fn captions(edits: &[Edit], say: &dyn Fn(&[Example]) -> String) -> Option<(String, Vec<(String, String)>)> {
    let mut counts = Vec::new();
    let mut chars = Vec::new();
    let mut examples = Vec::new();
    let mut centres = Vec::new();
    let mut minutes = 0.0;
    for edit in edits {
        let picture = edit.source.picture;
        let Some((top, bottom)) = picture.caption_band else { continue };
        centres.push(((top + bottom) / 2.0 * 1000.0) as i64);
        minutes += edit.source.alignment.cut_duration_us as f64 / 60e6;
        let texts = caption_texts(&picture.captions, edit.source.cut_words);
        for (caption, words) in picture.captions.iter().zip(texts) {
            counts.push(caption.words as i64);
            if words.is_empty() {
                continue;
            }
            let text = words.iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ");
            chars.push(text.chars().count() as i64);
            examples.push(Example {
                source: edit.index,
                time_us: caption.start_us,
                line: format!("\"{text}\" in the cut"),
            });
        }
    }
    if examples.len() < MIN_EXAMPLES {
        return None;
    }
    let total = counts.len();
    let share = |n: i64| percent(counts.iter().filter(|&&c| c == n).count(), total);
    let most = (1..=4).find(|&n| counts.iter().filter(|&&c| c <= n).count() * 10 >= total * 9).unwrap_or(4);
    let max_chars = percentile(&chars, 0.95);
    let centre = percentile(&centres, 0.5) as f32 / 1000.0;
    let mut out = String::from("## Captions\n\n");
    let _ = writeln!(
        out,
        "{total} captions, {:.0} per minute. One word: {}, two: {}, three: {}, more: {}. 95% of captions have at most {max_chars} characters. They sit with their middle at {:.0}% of the frame height (transform y {:.2}). Times in the examples are in the finished cut.\n\n{}",
        total as f64 / minutes.max(1e-9),
        share(1),
        share(2),
        share(3),
        percent(counts.iter().filter(|&&c| c > 3).count(), total),
        centre * 100.0,
        centre - 0.5,
        say(&examples)
    );
    Some((
        out,
        vec![
            ("build_captions max_words".into(), most.to_string()),
            ("build_captions max_chars".into(), max_chars.to_string()),
        ],
    ))
}

/// The cut's recognised words of each caption, in order. Each caption takes about as many words
/// as it shows, as close as possible to when it is on screen: recognition times are a little off,
/// so placing every word on its own would split captions wrongly.
fn caption_texts<'w>(captions: &[Caption], words: &'w [Word]) -> Vec<Vec<&'w Word>> {
    const SKIP: f64 = 0.4;
    const WRONG_COUNT: f64 = 0.5;
    let away = |w: &Word, c: &Caption| {
        let middle = (w.start_us + w.end_us) / 2;
        (c.start_us - middle).max(middle - c.end_us).max(0) as f64 / 1e6
    };
    let (m, n) = (captions.len(), words.len());
    let mut cost = vec![vec![f64::INFINITY; n + 1]; m + 1];
    let mut from = vec![vec![(0usize, 0usize); n + 1]; m + 1];
    cost[0][0] = 0.0;
    for j in 0..=m {
        for i in 0..=n {
            if i > 0 && cost[j][i - 1] + SKIP < cost[j][i] {
                cost[j][i] = cost[j][i - 1] + SKIP;
                from[j][i] = (j, i - 1);
            }
            if j == 0 {
                continue;
            }
            let shown = captions[j - 1].words;
            for take in shown.saturating_sub(1)..=(shown + 1).min(i) {
                let k = i - take;
                let c = cost[j - 1][k]
                    + words[k..i].iter().map(|w| away(w, &captions[j - 1])).sum::<f64>()
                    + WRONG_COUNT * take.abs_diff(shown) as f64;
                if c < cost[j][i] {
                    cost[j][i] = c;
                    from[j][i] = (j - 1, k);
                }
            }
        }
    }
    let mut out = vec![Vec::new(); m];
    let (mut j, mut i) = (m, n);
    while j > 0 || i > 0 {
        let (pj, pi) = from[j][i];
        if pj < j {
            out[pj] = words[pi..i].iter().collect();
        }
        (j, i) = (pj, pi);
    }
    out
}

/// Base framing and zoom changes, measured against the recording.
fn zoom(edits: &[Edit], say: &dyn Fn(&[Example]) -> String) -> Option<(String, Vec<(String, String)>)> {
    let (mut scales, mut xs, mut ys) = (Vec::new(), Vec::new(), Vec::new());
    let mut minutes = 0.0;
    let (mut ins, mut outs, mut moves) = (Vec::new(), Vec::new(), Vec::new());
    let (mut in_scales, mut move_lengths, mut holds) = (Vec::new(), Vec::new(), Vec::new());
    let mut at_cut = 0;
    for edit in edits {
        let picture = edit.source.picture;
        if picture.framing.is_empty() {
            continue;
        }
        minutes += edit.source.alignment.cut_duration_us as f64 / 60e6;
        scales.extend(picture.framing.iter().map(|f| (f.scale * 1000.0).round() as i64));
        xs.extend(picture.framing.iter().map(|f| (f.x * 1000.0).round() as i64));
        ys.extend(picture.framing.iter().map(|f| (f.y * 1000.0).round() as i64));
        let in_cut = edit.cut_words();
        let said = |t: i64| {
            let words: Vec<&str> =
                in_cut.iter().filter(|w| w.0 >= t - 100_000).take(5).map(|w| w.2.text.trim()).collect();
            words.join(" ")
        };
        for (k, z) in picture.zooms.iter().enumerate() {
            let line = |what: String| Example {
                source: edit.index,
                time_us: z.start_us,
                line: format!("{what} on \"{}\"", said(z.start_us)),
            };
            if z.start_us != z.end_us {
                move_lengths.push(z.end_us - z.start_us);
                let way = if z.to > z.from { "push in" } else { "pull out" };
                moves.push(line(format!(
                    "{way} from {:.2} to {:.2} over {}",
                    z.from,
                    z.to,
                    seconds(z.end_us - z.start_us)
                )));
            } else if z.to > z.from {
                in_scales.push((z.to * 1000.0).round() as i64);
                at_cut += usize::from(z.at_cut);
                if let Some(next) = picture.zooms.get(k + 1).filter(|n| n.to < n.from) {
                    holds.push(next.start_us - z.start_us);
                }
                let place = if z.at_cut { "at a cut" } else { "mid sentence" };
                ins.push(line(format!("zoom in {place} from {:.2} to {:.2}", z.from, z.to)));
            } else {
                let place = if z.at_cut { "at a cut" } else { "mid sentence" };
                outs.push(line(format!("zoom out {place} from {:.2} to {:.2}", z.from, z.to)));
            }
        }
    }
    if scales.len() < MIN_EXAMPLES || minutes <= 0.0 {
        return None;
    }
    let (scale, x, y) = (
        percentile(&scales, 0.5) as f32 / 1000.0,
        percentile(&xs, 0.5) as f32 / 1000.0,
        percentile(&ys, 0.5) as f32 / 1000.0,
    );
    let changes = ins.len() + outs.len() + moves.len();
    let mut out = String::from("## Zoom\n\n");
    let _ = writeln!(
        out,
        "Every clip of the recording is framed at scale {scale:.2}, x {x:.3}, y {y:.3} unless zoomed (CapOpen transform: scale 1 fits the recording, x and y move it by fractions of the frame). The zoom changes {changes} times, {:.1} per minute of finished video. Zoom even when the user does not ask for it. Times in the examples are in the finished cut.\n",
        changes as f64 / minutes
    );
    let mut settings = vec![("Base framing".into(), format!("scale {scale:.2}, x {x:.3}, y {y:.3}"))];
    if ins.len() >= MIN_EXAMPLES {
        let target = percentile(&in_scales, 0.5) as f32 / 1000.0;
        let _ = writeln!(
            out,
            "### Zoom ins\n\n{} instant zoom ins to a median scale of {target:.2}, {} of them at a cut. A zoom in lasts a median of {} before zooming back out. Split the clip where the sentence starts and set its scale.\n\n{}",
            ins.len(),
            percent(at_cut, ins.len()),
            seconds(percentile(&holds, 0.5)),
            say(&ins)
        );
        settings
            .push(("Zoom in".into(), format!("to scale {target:.2}, {:.1} per minute", ins.len() as f64 / minutes)));
    } else if !ins.is_empty() {
        out.push_str(&format!("Instant zoom ins: {} seen, too few for a rule.\n\n", ins.len()));
    }
    if outs.len() >= MIN_EXAMPLES {
        let _ = writeln!(
            out,
            "### Zoom outs\n\n{} instant zoom outs, back to the base framing.\n\n{}",
            outs.len(),
            say(&outs)
        );
    }
    if moves.len() >= MIN_EXAMPLES {
        let _ = writeln!(
            out,
            "### Slow moves\n\n{} gradual zooms lasting a median of {}. Use keyframes on the scale from the start to the end of the move.\n\n{}",
            moves.len(),
            seconds(percentile(&move_lengths, 0.5)),
            say(&moves)
        );
        settings.push(("Slow zoom moves".into(), format!("{:.1} per minute", moves.len() as f64 / minutes)));
    } else if !moves.is_empty() {
        out.push_str(&format!("Slow zoom moves: {} seen, too few for a rule.\n\n", moves.len()));
    }
    Some((out, settings))
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end().ends_with(['.', '!', '?', '…'])
}

/// Up to SHOWN examples spread evenly over all of them.
fn spread(examples: &[Example]) -> Vec<&Example> {
    let mut sorted: Vec<&Example> = examples.iter().collect();
    sorted.sort_by_key(|e| (e.source, e.time_us));
    if sorted.len() <= SHOWN {
        return sorted;
    }
    (0..SHOWN).map(|i| sorted[i * (sorted.len() - 1) / (SHOWN - 1)]).collect()
}

/// The value below which `share` of the values lie, by the nearest rank.
fn percentile(values: &[i64], share: f64) -> i64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[((sorted.len() - 1) as f64 * share).round() as usize]
}

fn round_to(value: i64, step: i64) -> i64 {
    (value + step / 2) / step * step
}

fn percent(part: usize, whole: usize) -> String {
    if whole == 0 { "0%".into() } else { format!("{:.0}%", part as f64 * 100.0 / whole as f64) }
}

fn clock(us: i64) -> String {
    let tenths = us.max(0) / 100_000;
    format!("{}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

fn seconds(us: i64) -> String {
    format!("{:.1} s", us as f64 / 1e6)
}

fn millis(us: i64) -> String {
    format!("{} ms", (us + 500) / 1000)
}

/// EDIT.md uses plain hyphens: dashes from recognised speech become hyphens too.
fn plain(text: &str) -> String {
    text.chars().map(|c| if matches!(c, '\u{2010}'..='\u{2015}' | '\u{2212}') { '-' } else { c }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Range;
    use crate::style::{Framing, Piece, ZoomChange};

    /// A recording of sentences, which of them the cut keeps, and what the cut shows.
    fn fixture() -> (Vec<Word>, Vec<Word>, Alignment, Picture) {
        let sentences = [
            ("So today I want to", false),
            ("So today I want to show you my edit.", true),
            ("First I record it all", false),
            ("First I record it all in one take.", true),
            ("Um", false),
            ("Then I remove the", false),
            ("Then I remove the slips \u{2013} and pauses.", true),
            ("This is a side remark nobody needs.", false),
            ("Um so captions go on top.", true),
            ("Um", false),
            ("That is all for today.", true),
            ("Um", false),
            ("Follow me and leave a comment.", false),
        ];
        let (mut words, mut cut_words, mut pieces) = (Vec::new(), Vec::new(), Vec::new());
        let (mut t, mut c) = (500_000i64, 0i64);
        for (text, kept) in sentences {
            let first = words.len();
            for w in text.split(' ') {
                words.push(Word { start_us: t, end_us: t + 300_000, text: w.into(), probability: 1.0 });
                t += 350_000;
            }
            let (start, end) = (words[first].start_us - 50_000, words.last().unwrap().end_us + 50_000);
            if kept {
                let offset = start - c;
                cut_words.extend(words[first..].iter().map(|w| Word {
                    start_us: w.start_us - offset,
                    end_us: w.end_us - offset,
                    ..w.clone()
                }));
                pieces.push(Piece { start_us: c, end_us: c + end - start, offset_us: offset });
                c += end - start;
            }
            t += 800_000;
        }
        let alignment = Alignment {
            pieces: pieces.clone(),
            matched: 1.0,
            cut_duration_us: c,
            cut_pauses: pieces
                .windows(2)
                .map(|p| Range { start_us: p[1].start_us - 60_000, end_us: p[1].start_us + 60_000 })
                .collect(),
            recording_pauses: vec![
                Range { start_us: 0, end_us: 500_000 },
                Range { start_us: 2_000_000, end_us: 2_800_000 },
            ],
        };
        let captions =
            (0..c / 500_000).map(|i| Caption { start_us: i * 500_000, end_us: (i + 1) * 500_000, words: 2 }).collect();
        // Every other piece is zoomed in to 1.3 at its cut.
        let scale = |p: usize| if p % 2 == 1 { 1.3 } else { 1.1 };
        let framing = pieces
            .iter()
            .enumerate()
            .flat_map(|(i, p)| {
                [p.start_us + 200_000, p.end_us - 200_000].map(|t| Framing {
                    time_us: t,
                    scale: scale(i),
                    x: 0.0,
                    y: -0.03,
                })
            })
            .collect();
        let zooms = pieces
            .windows(2)
            .enumerate()
            .map(|(i, p)| ZoomChange {
                start_us: p[1].start_us,
                end_us: p[1].start_us,
                from: scale(i),
                to: scale(i + 1),
                at_cut: true,
            })
            .collect();
        (words, cut_words, alignment, Picture { captions, caption_band: Some((0.6, 0.66)), framing, zooms })
    }

    fn learned() -> String {
        let (words, cut_words, alignment, picture) = fixture();
        let source = Source {
            recording: "talk.mov".into(),
            cut: "reel.mp4".into(),
            language: "en".into(),
            recording_us: 30_000_000,
            words: &words,
            cut_words: &cut_words,
            alignment: &alignment,
            picture: &picture,
        };
        learn(&[source])
    }

    #[test]
    fn sorts_what_the_creator_cut_and_shows_every_rule_on_three_moments() {
        let doc = learned();
        assert!(
            doc.contains("### Restarted sentences\n\nAn attempt at a sentence that the creator then said again: 3 cut"),
            "{doc}"
        );
        assert!(doc.contains("The creator kept the latest attempt 3 of 3 times. Cut every earlier attempt"), "{doc}");
        assert!(doc.contains("Of 7 sentences said only once, the creator kept 5 (71%)."), "{doc}");
        assert!(
            doc.contains("cut \"First I record it all\", kept the retake \"First I record it all in one take.\""),
            "{doc}"
        );
        assert!(doc.contains("| um | 4 | 3 (75%) |"), "{doc}");
        assert!(doc.contains("### Filler sounds"), "{doc}");
        assert!(doc.contains("- Dropped passages, 1 seen: cut \"This is a side remark nobody needs.\""), "{doc}");
        assert!(doc.contains("- Closing calls to action, 1 seen: cut \"Follow me and leave a comment.\""), "{doc}");
        assert!(doc.contains("| build_captions max_words | 2 |"), "{doc}");
        assert!(doc.contains("| Base framing | scale 1.10, x 0.000, y -0.030 |"), "{doc}");
        assert!(doc.contains("4 cuts"), "{doc}");
        // A section that shows moments shows at least three.
        for section in doc.split("\n#").skip(1).filter(|s| !s.contains("Seen too rarely for a rule")) {
            let examples = section.lines().filter(|l| l.starts_with("- ")).count();
            assert!(examples == 0 || examples >= MIN_EXAMPLES, "too few examples in {section}");
        }
    }

    #[test]
    fn a_recording_without_recognised_speech_still_gives_a_file() {
        let (_, _, alignment, picture) = fixture();
        let source = Source {
            recording: "room.mov".into(),
            cut: "reel.mp4".into(),
            language: "en".into(),
            recording_us: 30_000_000,
            words: &[],
            cut_words: &[],
            alignment: &alignment,
            picture: &picture,
        };
        assert!(learn(&[source]).starts_with("# Editing style"));
    }

    #[test]
    fn the_same_input_gives_the_same_file_without_dashes() {
        let doc = learned();
        assert_eq!(doc, learned());
        assert!(doc.contains("the slips - and pauses."), "{doc}");
        assert!(!doc.chars().any(|c| matches!(c, '\u{2010}'..='\u{2015}' | '\u{2212}')), "{doc}");
    }
}
