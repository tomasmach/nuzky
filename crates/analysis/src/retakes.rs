//! Restarted sentences and fillers that start a sentence, found in the words of a whole timeline.
//!
//! A creator restarts sentences ("Dneska vám ukážu, jak natočit." "Dneska vám ukážu, jak natočit
//! video za deset minut.") and starts them with fillers ("Jakoby, potom řešíte světlo."). This
//! groups the attempts at one sentence, recommends keeping the last complete one and proposes
//! deleting the others and the leading fillers. It only reads words: the same words always give
//! the same result, with fixed thresholds and no ordering taken from maps or sets.

use capopen_engine::speech::TimelineWord;
use serde::Serialize;

/// A pause this long ends a sentence, as in get_transcript.
const SENTENCE_GAP_US: i64 = 600_000;
/// A pause this long after "jakoby" or "prostě" sets it apart from the rest of the sentence.
const FILLER_PAUSE_US: i64 = 150_000;
/// A sentence of at most this many compared words may stand between two attempts.
const ASIDE_WORDS: usize = 3;
/// A trailing-off attempt is at most this many words.
const UNFINISHED_WORDS: usize = 12;
/// A restart inside a sentence repeats at least this many words of its opening, within this many
/// words of where it began.
const RESTART_WORDS: usize = 3;
const RESTART_SPAN: usize = 12;
/// A word this long is a content word: said differently instead of misheard, it changes what the
/// sentence says ("mikrofonu" and "kamery"); shorter ones ("z" and "s") are often misheard.
const CONTENT_LETTERS: usize = 4;
/// Two attempts share at least two words with this many letters: "Tak jo." is too little.
const MIN_SHARED_LETTERS: usize = 6;

/// Hesitation sounds, Czech and English, without diacritics.
const SOUNDS: &[&str] = &["ehm", "hm", "hmm", "hmmm", "eh", "ee", "eee", "em", "mmm", "um", "umm", "uh", "uhm", "erm"];
/// Fillers only when a comma or a pause sets them apart: "Jakoby, potom…" but not "Jakoby to
/// nešlo". "No" is left out because it often answers or negates; "so" and "well" carry meaning.
const SET_APART: &[&str] = &["jakoby", "proste", "vlastne", "like"];
/// Words that negate, without diacritics. Czech also negates verbs with "ne-", see `negated`.
const NEGATIONS: &[&str] = &[
    "ne", "neni", "nikdy", "nic", "nikdo", "zadny", "zadna", "zadne", "not", "no", "never", "nothing", "nobody", "none",
];
/// Number words recognition writes either way: "deset" and "10" are the same.
const NUMBERS: &[(&str, &str)] = &[
    ("nula", "0"),
    ("jeden", "1"),
    ("jedna", "1"),
    ("jedno", "1"),
    ("jednu", "1"),
    ("dva", "2"),
    ("dve", "2"),
    ("tri", "3"),
    ("ctyri", "4"),
    ("pet", "5"),
    ("sest", "6"),
    ("sedm", "7"),
    ("osm", "8"),
    ("devet", "9"),
    ("deset", "10"),
    ("jedenact", "11"),
    ("dvanact", "12"),
    ("trinact", "13"),
    ("ctrnact", "14"),
    ("patnact", "15"),
    ("sestnact", "16"),
    ("sedmnact", "17"),
    ("osmnact", "18"),
    ("devatenact", "19"),
    ("dvacet", "20"),
    ("tricet", "30"),
    ("ctyricet", "40"),
    ("padesat", "50"),
    ("sedesat", "60"),
    ("sedmdesat", "70"),
    ("osmdesat", "80"),
    ("devadesat", "90"),
    ("sto", "100"),
    ("tisic", "1000"),
    ("zero", "0"),
    ("one", "1"),
    ("two", "2"),
    ("three", "3"),
    ("four", "4"),
    ("five", "5"),
    ("six", "6"),
    ("seven", "7"),
    ("eight", "8"),
    ("nine", "9"),
    ("ten", "10"),
    ("eleven", "11"),
    ("twelve", "12"),
    ("thirteen", "13"),
    ("fourteen", "14"),
    ("fifteen", "15"),
    ("sixteen", "16"),
    ("seventeen", "17"),
    ("eighteen", "18"),
    ("nineteen", "19"),
    ("twenty", "20"),
    ("thirty", "30"),
    ("forty", "40"),
    ("fifty", "50"),
    ("sixty", "60"),
    ("seventy", "70"),
    ("eighty", "80"),
    ("ninety", "90"),
    ("hundred", "100"),
    ("thousand", "1000"),
];

/// One attempt at a sentence. Word indices are inclusive timeline word numbers.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Attempt {
    pub from: usize,
    pub to: usize,
    pub start_us: i64,
    pub end_us: i64,
    pub text: String,
    /// No later attempt goes on past where this one stops, and no earlier one is clearly longer.
    pub complete: bool,
}

/// Attempts at one sentence, in timeline order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RetakeGroup {
    pub sentences: Vec<Attempt>,
    /// Index into `sentences` of the attempt to keep: the last complete one.
    pub keep: usize,
    /// Inclusive word ranges of every other attempt.
    pub delete: Vec<[usize; 2]>,
}

/// Filler words that start a sentence, never the meaningful rest of it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Filler {
    pub from: usize,
    pub to: usize,
    pub text: String,
}

/// Two consecutive sentences that would be one restarted sentence, but differ in a negation or a
/// number. They are not a group and nothing of them is in `suggested_delete`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Review {
    pub sentences: Vec<Attempt>,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Retakes {
    pub groups: Vec<RetakeGroup>,
    pub fillers: Vec<Filler>,
    /// Attempts the speaker gave up on: words that trail off ("Měl jsem několik…") before a new
    /// sentence starts.
    pub unfinished: Vec<Filler>,
    pub review: Vec<Review>,
    /// Every group's `delete`, every filler and every unfinished attempt, sorted and merged.
    pub suggested_delete: Vec<[usize; 2]>,
}

/// A sentence: inclusive words, its leading fillers and the words compared with other sentences.
struct Sentence {
    from: usize,
    to: usize,
    /// Last word of the fillers that start it.
    filler_to: Option<usize>,
    /// Compared words: word index and normalised token.
    words: Vec<(usize, String)>,
}

impl Sentence {
    fn tokens(&self) -> Vec<&str> {
        self.words.iter().map(|(_, t)| t.as_str()).collect()
    }
}

/// `words` are timeline words in timeline order, numbered as get_transcript numbers them.
pub fn retakes(words: &[TimelineWord]) -> Retakes {
    let sentences = sentences(words);
    let mut group_of: Vec<Option<usize>> = vec![None; sentences.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut pairs: Vec<(usize, usize, String)> = Vec::new();
    for s in 0..sentences.len() {
        // The previous sentence, or the one before it when only a short aside stands between.
        let aside = s >= 2 && sentences[s - 1].words.len() <= ASIDE_WORDS;
        for p in [s.checked_sub(1), s.checked_sub(2).filter(|_| aside)].into_iter().flatten() {
            if group_of[p].is_some_and(|g| groups[g].last() != Some(&p)) {
                continue;
            }
            // Every attempt already in the group, `p` last. A short unfinished attempt must not
            // bridge two different sentences, so the new one has to match all of them.
            let members = group_of[p].map_or_else(|| vec![p], |g| groups[g].clone());
            let relations: Vec<(usize, Relation)> =
                members.iter().map(|&m| (m, relate(&sentences[m], &sentences[s], words))).collect();
            if matches!(relations.last(), Some((_, Relation::Unrelated))) {
                continue;
            }
            if relations.iter().all(|(_, r)| matches!(r, Relation::Same)) {
                let g = *group_of[p].get_or_insert_with(|| {
                    groups.push(vec![p]);
                    groups.len() - 1
                });
                groups[g].push(s);
                group_of[s] = Some(g);
            } else {
                pairs.extend(relations.into_iter().filter_map(|(m, r)| match r {
                    Relation::Differs(reason) => Some((m, s, reason)),
                    _ => None,
                }));
            }
            break;
        }
    }
    let mut groups: Vec<RetakeGroup> = groups
        .iter()
        .map(|members| {
            let sentences = attempts(members, &sentences, words);
            let keep = sentences.iter().rposition(|a| a.complete).unwrap_or(sentences.len() - 1);
            let delete =
                sentences.iter().enumerate().filter(|(k, _)| *k != keep).map(|(_, a)| [a.from, a.to]).collect();
            RetakeGroup { sentences, keep, delete }
        })
        .collect();
    // Restarts inside a sentence, where recognition put no full stop between the attempts. A
    // sentence a group already deletes needs no second look.
    let deleted: Vec<[usize; 2]> = groups.iter().flat_map(|g| g.delete.iter().copied()).collect();
    let mut reviews_within = Vec::new();
    for sentence in sentences.iter().filter(|s| !deleted.contains(&[s.from, s.to])) {
        let (found, review) = restarts_within(sentence, words);
        groups.extend(found);
        reviews_within.extend(review);
    }
    groups.sort_by_key(|g| g.sentences[0].from);
    let mut review: Vec<Review> = pairs
        .into_iter()
        .map(|(p, s, reason)| Review { sentences: attempts(&[p, s], &sentences, words), reason })
        .collect();
    review.extend(reviews_within);
    review.sort_by_key(|r| r.sentences[0].from);
    let fillers: Vec<Filler> = sentences
        .iter()
        .filter_map(|s| s.filler_to.map(|to| Filler { from: s.from, to, text: text(words, s.from, to) }))
        .collect();
    let grouped: Vec<[usize; 2]> = groups.iter().flat_map(|g| g.delete.iter().copied()).collect();
    let unfinished: Vec<Filler> = trailing_off(words)
        .into_iter()
        .filter(|&[from, to]| !grouped.iter().any(|&[a, b]| a <= from && to <= b))
        .map(|[from, to]| Filler { from, to, text: text(words, from, to) })
        .collect();
    let mut ranges: Vec<[usize; 2]> = grouped
        .into_iter()
        .chain(fillers.iter().map(|f| [f.from, f.to]))
        .chain(unfinished.iter().map(|u| [u.from, u.to]))
        .collect();
    ranges.sort_unstable();
    let mut suggested_delete: Vec<[usize; 2]> = Vec::new();
    for [from, to] in ranges {
        match suggested_delete.last_mut() {
            Some(last) if from <= last[1] + 1 => last[1] = last[1].max(to),
            _ => suggested_delete.push([from, to]),
        }
    }
    Retakes { groups, fillers, unfinished, review, suggested_delete }
}

/// Words that trail off, ending in an ellipsis, followed in the same file by a new sentence that
/// recognition starts with a capital letter: "Měl jsem několik… Pak ale přišel…". At most
/// UNFINISHED_WORDS long, so a long passage that only ends hesitantly stays.
fn trailing_off(words: &[TimelineWord]) -> Vec<[usize; 2]> {
    let mut found = Vec::new();
    // Where the clause that may trail off begins: after a sentence end, a pause, or a comma, so
    // what the speaker finished saying before it stays.
    let mut from = 0;
    for (i, word) in words.iter().enumerate() {
        let text = word.text.trim();
        let next = words.get(i + 1).filter(|n| n.asset_id == word.asset_id);
        let trails = text.ends_with("...") || text.ends_with('…');
        if trails {
            let restarts =
                next.and_then(|n| n.text.trim().chars().find(|c| c.is_alphanumeric())).is_some_and(char::is_uppercase);
            if restarts && i + 1 - from <= UNFINISHED_WORDS {
                found.push([from, i]);
            }
        }
        let paused = next.is_none_or(|n| n.start_us - word.end_us >= SENTENCE_GAP_US);
        if trails || paused || text.ends_with(['.', '!', '?', ',', ';', ':']) {
            from = i + 1;
        }
    }
    found
}

/// Restarts inside one sentence: the same opening of at least RESTART_WORDS words said again
/// within RESTART_SPAN words, as in "Co mi třeba přijde je, co mi třeba přijde je, že…" Each such
/// chain of attempts is a group that keeps the last one, which runs to the end of the sentence.
fn restarts_within(sentence: &Sentence, words: &[TimelineWord]) -> (Vec<RetakeGroup>, Vec<Review>) {
    let tokens = sentence.tokens();
    let n = tokens.len();
    let opens_alike = |i: usize, j: usize| {
        (0..RESTART_WORDS).all(|k| alike(tokens[i + k], tokens[j + k]))
            && (0..RESTART_WORDS).map(|k| tokens[i + k].chars().count()).sum::<usize>() >= MIN_SHARED_LETTERS + 2
    };
    let restart = |i: usize| {
        (i + RESTART_WORDS..=(i + RESTART_SPAN).min(n.saturating_sub(RESTART_WORDS))).find(|&j| opens_alike(i, j))
    };
    let word = |k: usize| sentence.words[k].0;
    let attempt = |from: usize, to: usize, complete: bool| Attempt {
        from,
        to,
        start_us: words[from].start_us,
        end_us: words[to].end_us,
        text: text(words, from, to),
        complete,
    };
    let (mut found, mut review) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i + 2 * RESTART_WORDS <= n {
        let Some(next) = restart(i) else {
            i += 1;
            continue;
        };
        let mut starts = vec![i, next];
        while let Some(again) = restart(*starts.last().unwrap()) {
            starts.push(again);
        }
        starts.push(n);
        // An attempt is given up only when the next one says all of it again, give or take
        // recognition errors; "kávu a mám rád černou čokoládu" says something else, for review.
        let mut kept = 0;
        for m in 0..starts.len() - 2 {
            let (earlier, later) = (&tokens[starts[m]..starts[m + 1]], &tokens[starts[m + 1]..starts[m + 2]]);
            let found = prefix_match(earlier, later);
            // Nothing it said may be missing from the next one: "klopový" said once is not a restart.
            if found.accepted() && found.meaning.is_empty() && found.swapped.is_empty() && found.dropped.is_empty() {
                kept = m + 1;
                continue;
            }
            let span =
                |m: usize| (word(starts[m]), if starts[m + 1] == n { sentence.to } else { word(starts[m + 1]) - 1 });
            let ((a, b), (c, d)) = (span(m), span(m + 1));
            review.push(Review {
                sentences: vec![attempt(a, b, true), attempt(c, d, true)],
                reason: "a restart inside one sentence goes on differently".into(),
            });
            break;
        }
        if kept > 0 {
            let end = if starts[kept + 1] == n { sentence.to } else { word(starts[kept + 1]) - 1 };
            let mut attempts: Vec<Attempt> =
                (0..kept).map(|m| attempt(word(starts[m]), word(starts[m + 1]) - 1, false)).collect();
            attempts.push(attempt(word(starts[kept]), end, true));
            let delete = attempts[..kept].iter().map(|a| [a.from, a.to]).collect();
            found.push(RetakeGroup { keep: kept, sentences: attempts, delete });
        }
        i = starts[starts.len() - 2] + RESTART_WORDS;
    }
    (found, review)
}

/// Sentences end at . ! ? …, at a pause of SENTENCE_GAP_US and where the timeline moves to
/// another file: words of two recordings are never one sentence.
fn sentences(words: &[TimelineWord]) -> Vec<Sentence> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, word) in words.iter().enumerate() {
        let next = words.get(i + 1);
        let ends = word.text.trim_end().ends_with(['.', '!', '?', '…'])
            || next.is_none_or(|n| n.start_us - word.end_us >= SENTENCE_GAP_US || n.asset_id != word.asset_id);
        if !ends {
            continue;
        }
        let mut k = from;
        while k <= i && is_filler(words, k, i) {
            k += 1;
        }
        let compared = (k..=i).map(|j| (j, token(&words[j].text))).filter(|(_, t)| !t.is_empty()).collect();
        out.push(Sentence { from, to: i, filler_to: (k > from).then(|| k - 1), words: compared });
        from = i + 1;
    }
    out
}

/// Word `k` of a sentence ending at `last` is a filler when it is a hesitation sound, or a word
/// such as "jakoby" set apart by a comma or a pause, or it ends the sentence.
fn is_filler(words: &[TimelineWord], k: usize, last: usize) -> bool {
    let word = &words[k];
    let token = token(&word.text);
    if SOUNDS.contains(&token.as_str()) {
        return true;
    }
    SET_APART.contains(&token.as_str())
        && (k == last
            || word.text.trim_end().ends_with([',', ';', ':', '…', '-', '–', '—'])
            || words[k + 1].start_us - word.end_us >= FILLER_PAUSE_US)
}

enum Relation {
    Unrelated,
    /// One is the start of the other, or both say the same, give or take recognition errors.
    Same,
    /// As Same, except for a negation or a number.
    Differs(String),
}

fn relate(earlier: &Sentence, later: &Sentence, words: &[TimelineWord]) -> Relation {
    let flipped = earlier.words.len() > later.words.len();
    let (short, long) = if flipped { (later, earlier) } else { (earlier, later) };
    let found = prefix_match(&short.tokens(), &long.tokens());
    if !found.accepted() {
        return Relation::Unrelated;
    }
    // Content words of the earlier attempt the later does not say: "klopový" said once is not a retake.
    // A later attempt that stops two words or more before the earlier one ends is cut short, and the
    // earlier one is kept.
    let lost: &[usize] = match flipped {
        false => &found.dropped,
        true if long.words.len() - found.end >= 2 => &[],
        true => &found.unsaid,
    };
    if found.meaning.is_empty() && found.swapped.is_empty() && lost.is_empty() {
        return Relation::Same;
    }
    let said = |sentence: &Sentence, k: Option<usize>| match k {
        Some(k) => format!("\"{}\"", bare(&words[sentence.words[k].0].text)),
        None => "nothing".to_owned(),
    };
    let reasons: Vec<String> = found
        .meaning
        .iter()
        .map(|&(s, l, what)| {
            let (e, l) = if flipped { (said(long, l), said(short, s)) } else { (said(short, s), said(long, l)) };
            format!("a {what} differs: the earlier says {e}, the later {l}")
        })
        .chain(found.swapped.iter().map(|&(s, l)| {
            let (e, l) = if flipped {
                (said(long, Some(l)), said(short, Some(s)))
            } else {
                (said(short, Some(s)), said(long, Some(l)))
            };
            format!("a word differs: the earlier says {e}, the later {l}")
        }))
        .chain(lost.iter().map(|&k| format!("only the earlier says {}", said(earlier, Some(k)))))
        .collect();
    Relation::Differs(reasons.join("; "))
}

/// How a sentence lines up with the start of a longer one.
struct Match {
    short: usize,
    /// Words of the longer sentence the shorter one covers.
    end: usize,
    edits: usize,
    matched: usize,
    matched_letters: usize,
    /// Edits that change meaning: positions in the short and long sentence, and what differs.
    meaning: Vec<(Option<usize>, Option<usize>, &'static str)>,
    /// Content words said differently, not misheard: positions in the short and long sentence.
    /// They count as edits, and they make the pair one to review rather than a restart.
    swapped: Vec<(usize, usize)>,
    /// Content words of the short sentence the long one does not say.
    dropped: Vec<usize>,
    /// Content words of the long sentence the short one does not say, past its end too.
    unsaid: Vec<usize>,
}

impl Match {
    /// Mostly the same words: at most one edit per four words, besides those changing meaning.
    fn accepted(&self) -> bool {
        self.matched >= 2
            && self.matched_letters >= MIN_SHARED_LETTERS
            && self.edits - self.meaning.len() <= self.short / 4
    }
}

/// Aligns all of `short` with the best start of `long` by edits of whole words.
fn prefix_match(short: &[&str], long: &[&str]) -> Match {
    let (n, m) = (short.len(), long.len());
    // An unfinished attempt may stop inside its last word: "natoč" is the start of "natočit".
    let same = |i: usize, j: usize| {
        alike(short[i], long[j]) || (i + 1 == n && short[i].chars().count() >= 2 && long[j].starts_with(short[i]))
    };
    let mut cost = vec![vec![0usize; m + 1]; n + 1];
    cost[0] = (0..=m).collect();
    for (i, row) in cost.iter_mut().enumerate() {
        row[0] = i;
    }
    for i in 1..=n {
        for j in 1..=m {
            let step = usize::from(!same(i - 1, j - 1));
            cost[i][j] = (cost[i - 1][j - 1] + step).min(cost[i - 1][j] + 1).min(cost[i][j - 1] + 1);
        }
    }
    // The cheapest end, then the one closest to the short sentence's length, then the earlier.
    let end = (0..=m).min_by_key(|&j| (cost[n][j], j.abs_diff(n), j)).unwrap_or(0);
    // A content word said once, not stuttered as "ukážu ukážu", which repeats it right away.
    let missing = |words: &[&str], k: usize| {
        let stuttered = |next: Option<&&str>| next.is_some_and(|w| alike(w, words[k]));
        content(words[k]) && !stuttered(k.checked_sub(1).and_then(|p| words.get(p))) && !stuttered(words.get(k + 1))
    };
    let mut found = Match {
        short: n,
        end,
        edits: cost[n][end],
        matched: 0,
        matched_letters: 0,
        meaning: Vec::new(),
        swapped: Vec::new(),
        dropped: Vec::new(),
        unsaid: (end..m).filter(|&j| missing(long, j)).collect(),
    };
    let (mut i, mut j) = (n, end);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && same(i - 1, j - 1) && cost[i][j] == cost[i - 1][j - 1] {
            found.matched += 1;
            found.matched_letters += short[i - 1].chars().count();
            (i, j) = (i - 1, j - 1);
        } else if i > 0 && j > 0 && cost[i][j] == cost[i - 1][j - 1] + 1 {
            if let Some(what) = meaning((short[i - 1], i - 1), (long[j - 1], j - 1)) {
                found.meaning.push((Some(i - 1), Some(j - 1), what));
            } else if content(short[i - 1]) && content(long[j - 1]) {
                found.swapped.push((i - 1, j - 1));
            }
            (i, j) = (i - 1, j - 1);
        } else if i > 0 && cost[i][j] == cost[i - 1][j] + 1 {
            if let Some(what) = lone_meaning(short[i - 1], i - 1) {
                found.meaning.push((Some(i - 1), None, what));
            } else if missing(short, i - 1) {
                found.dropped.push(i - 1);
            }
            i -= 1;
        } else {
            if let Some(what) = lone_meaning(long[j - 1], j - 1) {
                found.meaning.push((None, Some(j - 1), what));
            } else if missing(long, j - 1) {
                found.unsaid.push(j - 1);
            }
            j -= 1;
        }
    }
    found.meaning.reverse();
    found.swapped.reverse();
    found.dropped.reverse();
    found.unsaid.sort_unstable();
    found
}

fn content(token: &str) -> bool {
    token.chars().count() >= CONTENT_LETTERS
}

/// Words at these positions that differ in what they say rather than in how they were recognised.
fn meaning((a, at): (&str, usize), (b, bt): (&str, usize)) -> Option<&'static str> {
    if negated(a, b) { Some("negation") } else { lone_meaning(a, at).or(lone_meaning(b, bt)) }
}

/// A number or a negation said in only one of the sentences. A sentence that opens with "no" is
/// usually Czech "well", not a negation.
fn lone_meaning(token: &str, position: usize) -> Option<&'static str> {
    if numeric(token) {
        Some("number")
    } else if NEGATIONS.contains(&token) && !(token == "no" && position == 0) {
        Some("negation")
    } else {
        None
    }
}

fn numeric(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| c.is_ascii_digit())
}

/// "musí" and "nemusí", "je" and "není", "do" and "don't".
fn negated(a: &str, b: &str) -> bool {
    let one_way = |x: &str, y: &str| {
        x.strip_prefix("ne") == Some(y)
            || x.strip_suffix("nt") == Some(y)
            || matches!((x, y), ("neni", "je") | ("cant", "can") | ("wont", "will"))
    };
    one_way(a, b) || one_way(b, a)
}

/// The same word, allowing recognition errors: a letter in words of four to six letters, two in
/// longer ones ("stříhám" heard as "stvíchám"). Short words, numbers and negations must match.
fn alike(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // "nikdy" and "někdy" are a letter apart and mean the opposite.
    if numeric(a) || numeric(b) || negated(a, b) || NEGATIONS.contains(&a) || NEGATIONS.contains(&b) {
        return false;
    }
    let allowed = match a.chars().count().min(b.chars().count()) {
        0..=3 => return false,
        4..=6 => 1,
        _ => 2,
    };
    distance(a, b) <= allowed
}

/// Levenshtein distance in letters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, &y) in b.iter().enumerate() {
            let next = (diagonal + usize::from(x != y)).min(row[j] + 1).min(row[j + 1] + 1);
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

fn attempts(members: &[usize], sentences: &[Sentence], words: &[TimelineWord]) -> Vec<Attempt> {
    members
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            let s = &sentences[i];
            // Cut short: a later attempt goes on past where it stops, or an earlier one by two words or more.
            let short = members.iter().enumerate().any(|(l, &j)| {
                let found = prefix_match(&s.tokens(), &sentences[j].tokens());
                l != k && found.accepted() && sentences[j].words.len() - found.end >= if l > k { 1 } else { 2 }
            });
            Attempt {
                from: s.from,
                to: s.to,
                start_us: words[s.from].start_us,
                end_us: words[s.to].end_us,
                text: text(words, s.from, s.to),
                complete: !short,
            }
        })
        .collect()
}

fn text(words: &[TimelineWord], from: usize, to: usize) -> String {
    words[from..=to].iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ")
}

/// The word without surrounding punctuation, as said.
fn bare(text: &str) -> &str {
    text.trim_matches(|c: char| !c.is_alphanumeric())
}

/// Lowercase letters and digits without diacritics, number words as digits.
fn token(text: &str) -> String {
    let token: String = text.chars().flat_map(char::to_lowercase).map(fold).filter(|c| c.is_alphanumeric()).collect();
    match NUMBERS.iter().find(|(word, _)| *word == token) {
        Some((_, digits)) => (*digits).to_owned(),
        None => token,
    }
}

fn fold(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ą' => 'a',
        'č' | 'ć' | 'ç' => 'c',
        'ď' => 'd',
        'é' | 'è' | 'ê' | 'ë' | 'ě' | 'ę' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ľ' | 'ĺ' | 'ł' => 'l',
        'ň' | 'ń' | 'ñ' => 'n',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ő' => 'o',
        'ř' | 'ŕ' => 'r',
        'š' | 'ś' => 's',
        'ť' => 't',
        'ú' | 'ù' | 'û' | 'ü' | 'ů' | 'ű' => 'u',
        'ý' | 'ÿ' => 'y',
        'ž' | 'ź' | 'ż' => 'z',
        c => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Takes of sentences, each followed by its pause in microseconds, laid out back to back on
    /// the timeline as one clip per take. Words inside a sentence are 100 ms apart.
    fn timeline(takes: &[&[(&str, i64)]]) -> Vec<TimelineWord> {
        let mut words = Vec::new();
        let mut t = 300_000;
        for (take, sentences) in takes.iter().enumerate() {
            let source = t;
            for (sentence, pause) in sentences.iter() {
                for text in sentence.split(' ') {
                    words.push(TimelineWord {
                        start_us: t,
                        end_us: t + 280_000,
                        text: format!(" {text}"),
                        probability: 0.9,
                        clip_id: format!("clip-{take}"),
                        asset_id: format!("reel-{take}"),
                        source_start_us: t - source,
                    });
                    t += 380_000;
                }
                t += pause - 100_000;
            }
        }
        words
    }

    /// What Whisper large-v3-turbo-q5_0 recognised in tmp-test/reel-1..3.mp4, misheard words
    /// and full stops after unfinished attempts included.
    fn reel() -> Vec<TimelineWord> {
        timeline(&[
            &[
                ("Dneska vám ukážu, jak natočit.", 900_000),
                ("Dneska vám ukážu, jak natočit video.", 1_100_000),
                ("Dneska vám ukážu, jak natočit video za 10 minut.", 2_400_000),
                ("Ehm, nejdžív potřebujete dobrý mikrofon.", 800_000),
            ],
            &[
                ("Mikrofon vejte co nejblíž k puse.", 2_200_000),
                ("Jakoby, potom rešíte světlo z oka.", 900_000),
                ("Kamera musí stát.", 1_000_000),
                ("Kamera musí stát pevně na stativu.", 800_000),
            ],
            &[
                ("Prostě, hotové video nachrajte na Instagram a TikTok.", 2_600_000),
                ("Celé to zabere 10 minut.", 1_200_000),
                ("Napište mi do komentáře, co chcete vidět díšně.", 600_000),
            ],
        ])
    }

    fn texts(group: &RetakeGroup) -> Vec<(&str, bool)> {
        group.sentences.iter().map(|a| (a.text.as_str(), a.complete)).collect()
    }

    fn one(sentences: &[(&str, i64)]) -> Retakes {
        retakes(&timeline(&[sentences]))
    }

    #[test]
    fn the_reel_takes_give_two_restarted_sentences_and_three_leading_fillers() {
        let words = reel();
        let found = retakes(&words);
        assert_eq!(found.groups.len(), 2, "{found:#?}");
        assert_eq!(
            texts(&found.groups[0]),
            [
                ("Dneska vám ukážu, jak natočit.", false),
                ("Dneska vám ukážu, jak natočit video.", false),
                ("Dneska vám ukážu, jak natočit video za 10 minut.", true)
            ]
        );
        assert_eq!((found.groups[0].keep, &found.groups[0].delete), (2, &vec![[0, 4], [5, 10]]));
        assert_eq!(
            texts(&found.groups[1]),
            [("Kamera musí stát.", false), ("Kamera musí stát pevně na stativu.", true)]
        );
        assert_eq!((found.groups[1].keep, &found.groups[1].delete), (1, &vec![[37, 39]]));
        let fillers: Vec<_> = found.fillers.iter().map(|f| (f.from, f.to, f.text.as_str())).collect();
        assert_eq!(fillers, [(20, 20, "Ehm,"), (31, 31, "Jakoby,"), (46, 46, "Prostě,")]);
        assert!(found.review.is_empty(), "{:#?}", found.review);
        assert_eq!(found.suggested_delete, [[0, 10], [20, 20], [31, 31], [37, 39], [46, 46]]);
        let kept = &found.groups[0].sentences[2];
        assert_eq!((kept.from, kept.to, kept.start_us, kept.end_us), (11, 19, words[11].start_us, words[19].end_us));
        // The same words give byte-identical output.
        let json = serde_json::to_string(&found).unwrap();
        assert_eq!(json, serde_json::to_string(&retakes(&reel())).unwrap());
    }

    #[test]
    fn a_whole_sentence_said_twice_keeps_the_second() {
        let found =
            one(&[("Kamera musí stát pevně na stativu.", 900_000), ("Kamera musí stát pevně na stativu.", 900_000)]);
        assert_eq!(found.groups.len(), 1);
        assert_eq!(texts(&found.groups[0]).iter().map(|t| t.1).collect::<Vec<_>>(), [true, true]);
        assert_eq!((found.groups[0].keep, &found.groups[0].delete), (1, &vec![[0, 5]]));
    }

    #[test]
    fn an_unfinished_last_attempt_keeps_the_complete_one_before_it() {
        let found = one(&[
            ("Kamera musí stát pevně na stativu.", 900_000),
            ("Kamera musí stát.", 900_000),
            ("Celé to zabere deset minut.", 900_000),
        ]);
        assert_eq!(found.groups.len(), 1);
        assert_eq!(
            texts(&found.groups[0]),
            [("Kamera musí stát pevně na stativu.", true), ("Kamera musí stát.", false)]
        );
        assert_eq!((found.groups[0].keep, &found.groups[0].delete), (0, &vec![[6, 8]]));
        // A last attempt only a word shorter is still complete: the creator's latest take stays.
        let found = one(&[
            ("Kamera musí stát pevně na stativu, jo.", 900_000),
            ("Kamera musí stát pevně na stativu.", 900_000),
        ]);
        assert_eq!(found.groups[0].keep, 1);
    }

    #[test]
    fn an_attempt_cut_off_at_the_end_of_one_clip_groups_with_the_next_clip() {
        // No full stop and only 50 ms on the timeline between the clips, but two files.
        let mut words = timeline(&[&[("Kamera musí stát", 0)], &[("Kamera musí stát pevně na stativu.", 900_000)]]);
        let gap = words[3].start_us - words[2].end_us - 50_000;
        for w in &mut words[3..] {
            w.start_us -= gap;
            w.end_us -= gap;
        }
        let found = retakes(&words);
        assert_eq!(found.groups.len(), 1, "{found:#?}");
        assert_eq!((found.groups[0].keep, &found.groups[0].delete), (1, &vec![[0, 2]]));
    }

    #[test]
    fn sentences_that_only_open_alike_are_not_retakes() {
        // The same opening several sentences later, and different sentences in a row.
        let found = one(&[
            ("Dneska vám ukážu, jak natočit video.", 900_000),
            ("Mikrofon dejte co nejblíž k puse.", 900_000),
            ("Potom řešíte světlo z okna.", 900_000),
            ("Dneska vám ukážu, jak natočit video.", 900_000),
            ("Dneska vám ukážu, jak ho sestříhat.", 900_000),
            ("Kamera musí stát pevně.", 900_000),
            ("Kamera musí mít dobré světlo.", 900_000),
            ("Tak jo.", 900_000),
            ("Tak jo, jdeme na to.", 900_000),
        ]);
        assert!(found.groups.is_empty(), "{:#?}", found.groups);
        assert!(found.review.is_empty() && found.suggested_delete.is_empty(), "{found:#?}");
    }

    #[test]
    fn a_short_unfinished_attempt_does_not_join_two_different_sentences() {
        let found = one(&[
            ("Kamera musí stát pevně na stativu.", 900_000),
            ("Kamera musí.", 900_000),
            ("Kamera musí být čistá.", 900_000),
        ]);
        assert_eq!(found.groups.len(), 1, "{found:#?}");
        assert_eq!(texts(&found.groups[0]), [("Kamera musí stát pevně na stativu.", true), ("Kamera musí.", false)]);
        assert_eq!(found.suggested_delete, [[6, 7]]);
    }

    #[test]
    fn a_negation_or_a_number_difference_is_named_and_not_deleted() {
        let found = one(&[
            ("Kamera nemusí stát na stativu.", 900_000),
            ("Kamera musí stát na stativu.", 900_000),
            ("Celé to zabere 10 minut.", 900_000),
            ("Celé to zabere 15 minut.", 900_000),
        ]);
        assert!(found.groups.is_empty() && found.suggested_delete.is_empty(), "{found:#?}");
        let reasons: Vec<&str> = found.review.iter().map(|r| r.reason.as_str()).collect();
        assert_eq!(
            reasons,
            [
                "a negation differs: the earlier says \"nemusí\", the later \"musí\"",
                "a number differs: the earlier says \"10\", the later \"15\""
            ]
        );
        assert_eq!(found.review[1].sentences.iter().map(|a| a.from).collect::<Vec<_>>(), [10, 15]);
        // A negation the later attempt adds, and the same number written as a word.
        let found = one(&[("I do want to show it.", 900_000), ("I do not want to show it.", 900_000)]);
        assert_eq!(found.review.len(), 1, "{found:#?}");
        assert!(found.review[0].reason.contains("the earlier says nothing, the later \"not\""), "{found:#?}");
        let found = one(&[("Celé to zabere deset minut.", 900_000), ("Celé to zabere 10 minut.", 900_000)]);
        assert_eq!((found.groups.len(), found.review.len()), (1, 0), "{found:#?}");
    }

    /// Nearly the same sentences that say different things are for review, never deleted.
    #[test]
    fn a_different_word_or_a_sometimes_for_a_never_is_reviewed_not_deleted() {
        let found = one(&[
            ("Tohle se nikdy nepovede.", 900_000),
            ("Tohle se někdy nepovede.", 900_000),
            ("Teď ukážu nastavení mikrofonu.", 900_000),
            ("Teď ukážu nastavení kamery.", 900_000),
        ]);
        assert!(found.groups.is_empty() && found.suggested_delete.is_empty(), "{found:#?}");
        let reasons: Vec<&str> = found.review.iter().map(|r| r.reason.as_str()).collect();
        assert_eq!(
            reasons,
            [
                "a negation differs: the earlier says \"nikdy\", the later \"někdy\"",
                "a word differs: the earlier says \"mikrofonu\", the later \"kamery\""
            ]
        );
        // An attempt that stops inside its last word is still the start of the next one.
        let found = one(&[("Dneska vám ukážu, jak natoč", 900_000), ("Dneska vám ukážu, jak natočit video.", 900_000)]);
        assert_eq!((found.groups.len(), found.groups[0].keep), (1, 1), "{found:#?}");
        // A word only the earlier complete attempt says, in its middle or as its last word.
        for (earlier, later, word) in [
            ("Použijte ten malý klopový mikrofon venku.", "Použijte ten malý mikrofon venku.", "klopový"),
            ("Použijte ten malý klopový mikrofon.", "Použijte ten malý mikrofon na rozhovory.", "klopový"),
            ("Použijte ten malý mikrofon venku.", "Použijte ten malý mikrofon.", "venku"),
        ] {
            let found = one(&[(earlier, 900_000), (later, 900_000)]);
            assert!(found.groups.is_empty() && found.suggested_delete.is_empty(), "{found:#?}");
            let reasons: Vec<&str> = found.review.iter().map(|r| r.reason.as_str()).collect();
            assert_eq!(reasons, [format!("only the earlier says \"{word}\"")], "{found:#?}");
        }
        // A word stuttered twice is said by the later attempt too: still a retake.
        let found = one(&[
            ("Dneska vám ukážu ukážu jak natočit video.", 900_000),
            ("Dneska vám ukážu jak natočit video.", 900_000),
        ]);
        assert!(found.review.is_empty(), "{found:#?}");
        assert_eq!((found.groups.len(), found.groups[0].keep), (1, 1), "{found:#?}");
        // The same word said again elsewhere is no stutter: "malý pokoj" is lost.
        let found = one(&[
            ("Použijte malý mikrofon jen pro malý pokoj bez ozvěny.", 900_000),
            ("Použijte malý mikrofon jen pro pokoj bez ozvěny.", 900_000),
        ]);
        let reasons: Vec<&str> = found.review.iter().map(|r| r.reason.as_str()).collect();
        assert_eq!(reasons, ["only the earlier says \"malý\""], "{found:#?}");
    }

    /// Recognition often puts no full stop between attempts said in one breath. Only an attempt
    /// the next one says all of again is given up; one that goes on differently is for review.
    #[test]
    fn restarts_inside_one_sentence_keep_the_last_attempt() {
        let found = one(&[
            ("Co mi třeba přijde je, co mi třeba přijde je, že má dobrý zvuk.", 900_000),
            (
                "Protože jsem si uvědomil, že jsem měl Protože jsem si uvědomil, že jsem měl malý Protože jsem si uvědomil, že mi zmizela úzkost.",
                900_000,
            ),
            ("Ptám se, jestli s ním pracuješ, jestli tě baví, jestli za něj platíš.", 900_000),
            ("Mám rád černou kávu a mám rád černou čokoládu.", 900_000),
        ]);
        let groups: Vec<_> = found.groups.iter().map(|g| (texts(g), g.keep, g.delete.clone())).collect();
        assert_eq!(groups.len(), 2, "{groups:#?}");
        assert_eq!(
            groups[0].0,
            [("Co mi třeba přijde je,", false), ("co mi třeba přijde je, že má dobrý zvuk.", true)]
        );
        assert_eq!((groups[0].1, &groups[0].2), (1, &vec![[0, 4]]));
        assert_eq!(
            groups[1].0,
            [("Protože jsem si uvědomil, že jsem měl", false), ("Protože jsem si uvědomil, že jsem měl malý", true)]
        );
        assert_eq!(found.suggested_delete, [[0, 4], [14, 20]], "{found:#?}");
        let reviewed: Vec<Vec<&str>> =
            found.review.iter().map(|r| r.sentences.iter().map(|a| a.text.as_str()).collect()).collect();
        // A word said only in the earlier attempt keeps it for review too.
        let dropped = one(&[("Použijte ten malý klopový mikrofon, použijte ten malý mikrofon.", 900_000)]);
        assert!(dropped.groups.is_empty() && dropped.suggested_delete.is_empty(), "{dropped:#?}");
        assert_eq!(dropped.review.len(), 1, "{dropped:#?}");
        assert_eq!(
            reviewed,
            [
                vec!["Protože jsem si uvědomil, že jsem měl malý", "Protože jsem si uvědomil, že mi zmizela úzkost."],
                vec!["Mám rád černou kávu a", "mám rád černou čokoládu."]
            ]
        );
    }

    /// Recognition marks words the speaker trailed off with an ellipsis; a new sentence follows.
    #[test]
    fn attempts_that_trail_off_before_a_new_sentence_are_proposed() {
        let found = one(&[
            ("Měl jsem několik...", 900_000),
            ("Pak ale přišel nový model.", 900_000),
            ("A když se mi podařilo... I když jsme... A když se mi to podařilo, bylo to fajn.", 900_000),
            ("A pak... jsem to zkusil znovu.", 900_000),
            ("Na konci jsem prostě...", 900_000),
        ]);
        let unfinished: Vec<_> = found.unfinished.iter().map(|u| u.text.as_str()).collect();
        // "A pak…" goes on in lower case, so it is one sentence; the take's last words have nothing
        // after them; "A když se mi podařilo…" is already an earlier attempt of a restarted sentence.
        assert_eq!(unfinished, ["Měl jsem několik...", "I když jsme..."], "{found:#?}");
        assert_eq!(found.suggested_delete, [[0, 2], [8, 15]], "{found:#?}");
        // What was finished before a pause or a comma stays; only the clause that trails off goes.
        let found = one(&[
            ("Mikrofon má velmi dobrý zvuk", 900_000),
            ("Ale když...", 900_000),
            ("Teď ukážu nastavení, ale nejdřív...", 900_000),
            ("Kamera musí stát.", 900_000),
        ]);
        let unfinished: Vec<_> = found.unfinished.iter().map(|u| u.text.as_str()).collect();
        assert_eq!(unfinished, ["Ale když...", "ale nejdřív..."], "{found:#?}");
    }

    #[test]
    fn recognition_errors_do_not_hide_a_retake() {
        let found = one(&[
            ("Ehm, dneska stříhám video z telefonu.", 900_000),
            ("Dneska stvíchám vydeo s telefonu na notebooku.", 900_000),
            ("Potom řešíte světlo z okna.", 900_000),
            ("Potom rešíte světlo z oka a stín.", 900_000),
            // Whisper heard the reel's first "Kamera musí stát." like this in one run.
            ("Kamera musí start.", 900_000),
            ("Kamera musí stát pevně na stativu.", 900_000),
        ]);
        assert_eq!(found.groups.len(), 3, "{found:#?}");
        assert_eq!(found.groups.iter().map(|g| g.keep).collect::<Vec<_>>(), [1, 1, 1]);
    }

    #[test]
    fn only_fillers_that_start_a_sentence_are_proposed() {
        let found = one(&[
            ("Jakoby to nešlo.", 900_000),
            ("To je prostě, jednoduché.", 900_000),
            ("Ehm.", 900_000),
            ("Ehm, prostě, kamera stojí.", 900_000),
            ("Um, like, I think it works.", 900_000),
            ("No, to je jasné.", 900_000),
            ("Vlastně jsem to nevěděl.", 900_000),
        ]);
        let fillers: Vec<&str> = found.fillers.iter().map(|f| f.text.as_str()).collect();
        assert_eq!(fillers, ["Ehm.", "Ehm, prostě,", "Um, like,"]);
        // "Ehm." and the next sentence's "Ehm, prostě," are one range: the deletion touches.
        assert_eq!(found.suggested_delete, [[7, 9], [12, 13]]);
    }
}
