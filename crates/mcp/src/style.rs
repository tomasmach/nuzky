//! The creator's style on this computer. EDIT.md is the style agents follow; beside it, `style/`
//! holds what Nuzky learned from each video (`evidence/`) and every version of the style
//! (`versions.jsonl`, whose last line is the style as it is now). Learning only adds evidence. The
//! style changes when the creator accepts, rejects, removes, reverts, edits or restores, and every
//! change is a version. Writers hold `style/lock`; the MCP server only reads EDIT.md, which is
//! always replaced whole.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use nuzky_analysis::Range;
use nuzky_analysis::Word;
use nuzky_analysis::style::doc::{self, Doc, OVERVIEW, RARE};
use nuzky_analysis::style::{
    self as learning, Alignment, Caption, Choice, Correction, Framing, Learned, Picture, Piece, Place, Rule, Source,
    ZoomChange,
};
use nuzky_engine::effects::transform_at;
use nuzky_engine::model::{Asset, AssetKind, ClipContent, Project};
use nuzky_engine::speech::is_heard;
use nuzky_session::ProjectSession;
use nuzky_session::transcripts::{Record, TranscriptStore};
use serde::{Deserialize, Serialize};

use crate::transcript::{Stage, best_model, models, recognise};

/// Versions kept.
const MAX_VERSIONS: usize = 100;
/// Videos learned from; older ones are forgotten as new ones come.
const MAX_EVIDENCE: usize = 12;
/// A longer versions file is not ours to read.
const READ_LIMIT: u64 = 64 << 20;
/// Less of the cut's speech found in the recording means the cut was not made from it.
const MIN_MATCH: f32 = 0.6;
const MAX_OWN_RULE: usize = 500;

/// Why learning leaves a rule alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename = "StyleFrozen"))]
#[serde(rename_all = "lowercase")]
pub enum Frozen {
    Rejected,
    Removed,
    Reverted,
    /// The creator changed it by hand.
    Edited,
}

/// A learned rule in the style: what it said when the creator accepted it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Accepted {
    summary: String,
    choices: Vec<Choice>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Version {
    index: u64,
    at_ms: u64,
    label: String,
    /// EDIT.md; none is the default style, without the file.
    text: Option<String>,
    #[serde(default)]
    accepted: BTreeMap<String, Accepted>,
    /// By rule title, or "header", "overview" and "rare" for the text around the rules.
    #[serde(default)]
    frozen: BTreeMap<String, Frozen>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename = "StyleSourceKind"))]
#[serde(rename_all = "lowercase")]
pub enum EvidenceKind {
    /// A recording and the finished video the creator cut from it.
    Pair,
    /// A project the creator exported or cut after the AI.
    Project,
}

/// Everything learning needs from one video, so the style can be learned again at any time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    #[serde(default)]
    pub seq: u64,
    /// Learning the same video again replaces its evidence: the pair's files, or the project.
    pub key: String,
    pub kind: EvidenceKind,
    /// What the creator calls it.
    pub title: String,
    pub at_ms: u64,
    /// The share of the cut's speech found in the recording, for pairs.
    pub matched: Option<f32>,
    pub recording: String,
    pub cut: String,
    pub language: String,
    pub recording_us: i64,
    pub words: Vec<Word>,
    pub cut_words: Vec<Word>,
    pub alignment: Alignment,
    pub picture: Picture,
    /// Where each recording word plays in a project's timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub places: Option<Vec<Place>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corrections: Vec<Correction>,
}

impl Evidence {
    pub fn source(&self) -> Source<'_> {
        Source {
            recording: self.recording.clone(),
            cut: self.cut.clone(),
            language: self.language.clone(),
            recording_us: self.recording_us,
            words: &self.words,
            cut_words: &self.cut_words,
            alignment: &self.alignment,
            picture: &self.picture,
            places: self.places.as_deref(),
            corrections: &self.corrections,
        }
    }

    /// The same lesson as `other`: it would teach the same, whatever its name and time.
    fn same_lesson(&self, other: &Evidence) -> bool {
        let lesson = |e: &Evidence| {
            serde_json::to_string(&(&e.words, &e.cut_words, &e.alignment, &e.picture, &e.places, &e.corrections))
                .unwrap_or_default()
        };
        lesson(self) == lesson(other)
    }
}

/// The style as the app shows it.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StyleView {
    /// The version shown; saving the text checks it is still the newest.
    pub version: u64,
    /// EDIT.md, none without a style.
    pub text: Option<String>,
    /// The creator's own rules.
    pub own: Vec<String>,
    /// Learned rules in the style, in file order.
    pub rules: Vec<ActiveRule>,
    pub suggestions: Vec<Suggestion>,
    pub not_learned: Vec<NotLearned>,
    /// Newest first.
    pub sources: Vec<LearnedFrom>,
    /// Newest first.
    pub versions: Vec<StyleVersion>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename = "StyleRule"))]
#[serde(rename_all = "camelCase")]
pub struct ActiveRule {
    pub title: String,
    pub summary: String,
    /// The creator changed it by hand, so learning leaves it alone.
    pub by_you: bool,
    /// How well the videos learned from now back it; none when they no longer show it.
    pub confidence: Option<Confidence>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub title: String,
    pub summary: String,
    /// The style already has this rule and learning would change it.
    pub update: bool,
    pub changes: Vec<Change>,
    pub confidence: Confidence,
    pub moments: Vec<StyleMoment>,
    /// The rule as EDIT.md would hold it.
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename = "StyleChange"))]
pub struct Change {
    pub name: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename = "StyleConfidence"))]
#[serde(rename_all = "camelCase")]
pub struct Confidence {
    /// High in 3 or more videos, medium in 2, low in 1.
    pub level: Level,
    pub videos: usize,
    /// Videos learned from in all.
    pub of: usize,
    pub moments: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename = "StyleLevel"))]
#[serde(rename_all = "lowercase")]
pub enum Level {
    High,
    Medium,
    Low,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StyleMoment {
    pub video: String,
    /// In the recording, or in the finished cut where the rule says so.
    pub time_us: i64,
    pub line: String,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename = "StyleNotLearned"))]
pub struct NotLearned {
    pub title: String,
    pub reason: Frozen,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename = "StyleSource"))]
#[serde(rename_all = "camelCase")]
pub struct LearnedFrom {
    pub title: String,
    pub kind: EvidenceKind,
    pub at_ms: u64,
    pub matched: Option<f32>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StyleVersion {
    pub index: u64,
    pub label: String,
    pub at_ms: u64,
}

/// What the creator, or an agent they agreed with, does to the style.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StyleAction {
    /// Puts suggested rules into the style. `seen` is each one's text as the caller showed it, in
    /// the same order, so a suggestion learned anew meanwhile is not accepted unseen.
    Accept {
        titles: Vec<String>,
        #[serde(default)]
        seen: Vec<String>,
    },
    /// Keeps a suggestion out of the style until the creator asks to learn it again.
    Reject {
        title: String,
    },
    Remove {
        title: String,
    },
    /// Puts back how the style had the rule before its last change.
    Revert {
        title: String,
    },
    LearnAgain {
        title: String,
    },
    /// Adds the creator's own rule, or with `index` changes it; empty text removes it. `was` is
    /// the rule at `index` as the caller saw it, so a rule moved meanwhile is never changed instead.
    #[serde(rename_all = "camelCase")]
    SetOwn {
        index: Option<usize>,
        text: String,
        #[serde(default)]
        was: Option<String>,
    },
    /// The whole EDIT.md as the creator wrote it, over the version they started from.
    #[serde(rename_all = "camelCase")]
    SetText {
        text: String,
        base_version: u64,
    },
    Restore {
        index: u64,
    },
    /// No style: the guide's defaults, and everything learned is forgotten.
    Reset,
}

pub struct Store {
    dir: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Self::at(learning::style_path().parent().expect("EDIT.md lives in a folder"))
    }
}

impl Store {
    /// The style in `dir`, which holds EDIT.md.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// What changes whenever the style or what was learned changes.
    pub fn files(&self) -> [PathBuf; 3] {
        [self.edit_md(), self.versions_path(), self.evidence_dir()]
    }

    fn edit_md(&self) -> PathBuf {
        self.dir.join("EDIT.md")
    }

    fn own_dir(&self) -> PathBuf {
        self.dir.join("style")
    }

    fn evidence_dir(&self) -> PathBuf {
        self.own_dir().join("evidence")
    }

    fn versions_path(&self) -> PathBuf {
        self.own_dir().join("versions.jsonl")
    }

    /// Names the version whose EDIT.md is being written, until it is.
    fn writing_path(&self) -> PathBuf {
        self.own_dir().join("writing")
    }

    /// Holds the style's lock, so one writer changes it at a time, across windows and processes.
    fn lock(&self) -> Result<File> {
        fs::create_dir_all(self.own_dir()).with_context(|| format!("Cannot create {}", self.own_dir().display()))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.own_dir().join("lock"))
            .context("Opening the style's lock")?;
        lock.lock().context("Locking the style")?;
        Ok(lock)
    }

    pub fn view(&self) -> Result<StyleView> {
        let _lock = self.lock()?;
        let versions = self.synced()?;
        self.show(&versions)
    }

    /// Keeps what was learned from one video. Suggestions are worked out only when the style is
    /// shown, so learning in the background stays cheap.
    pub fn keep_evidence(&self, mut evidence: Evidence) -> Result<()> {
        let _lock = self.lock()?;
        let all = self.evidence()?;
        // Learned again, a video is the newest one, so the newest 12 are those learned last.
        evidence.seq = all.iter().map(|e| e.seq).max().unwrap_or(0) + 1;
        let dir = self.evidence_dir();
        fs::create_dir_all(&dir).with_context(|| format!("Cannot create {}", dir.display()))?;
        write_whole(&dir.join(format!("{:06}.json", evidence.seq)), &serde_json::to_vec(&evidence)?)?;
        let replaced = all.iter().filter(|e| e.key == evidence.key).map(|e| e.seq);
        let mut others: Vec<u64> = all.iter().filter(|e| e.key != evidence.key).map(|e| e.seq).collect();
        others.sort_unstable();
        let oldest = others.iter().rev().skip(MAX_EVIDENCE - 1).copied();
        for old in replaced.chain(oldest) {
            fs::remove_file(dir.join(format!("{old:06}.json"))).context("Forgetting what was learned before")?;
        }
        Ok(())
    }

    /// Writes the style `nuzky style learn` learned, keeping the creator's own rules, as a version.
    pub fn replace(&self, learned: &Learned) -> Result<()> {
        let _lock = self.lock()?;
        let versions = self.synced()?;
        let current = versions.last().cloned().unwrap_or_default();
        let mut doc = Doc::parse(&learned.document());
        let old = Doc::parse(current.text.as_deref().unwrap_or(""));
        if let Some(own) = old.get(doc::OWN) {
            doc.set(doc::OWN, own.to_owned());
        }
        let accepted = learned.rules.iter().map(|r| (r.title.to_owned(), accept(r))).collect();
        self.commit(&versions, "Learned with nuzky style learn", Some(doc.text()), accepted, BTreeMap::new())
    }

    pub fn act(&self, action: StyleAction) -> Result<StyleView> {
        self.act_as(action, "")
    }

    /// A change an agent makes for the creator; its version says so.
    pub fn act_for_agent(&self, action: StyleAction) -> Result<StyleView> {
        ensure!(
            !matches!(action, StyleAction::Reset),
            "INVALID_ARGUMENTS: only the creator goes back to the default style, in the app (Your style, Versions)"
        );
        self.act_as(action, "AI: ")
    }

    fn act_as(&self, action: StyleAction, by: &str) -> Result<StyleView> {
        let _lock = self.lock()?;
        let mut versions = self.synced()?;
        let current = versions.last().cloned().unwrap_or_default();
        let mut doc = Doc::parse(current.text.as_deref().unwrap_or(""));
        let (mut accepted, mut frozen) = (current.accepted.clone(), current.frozen.clone());
        let evidence = self.evidence()?;
        let learned = learned_from(&evidence);
        let suggested = |title: &str| {
            learned.as_ref().and_then(|l| l.rules.iter().find(|r| r.title == title)).filter(|r| {
                !current.frozen.contains_key(r.title)
                    && current.accepted.get(r.title).is_none_or(|a| !Rule::same(&a.choices, &r.choices))
            })
        };
        let label = match action {
            StyleAction::Accept { titles, seen } => {
                ensure!(!titles.is_empty(), "INVALID_ARGUMENTS: name the rules to accept");
                ensure!(
                    seen.is_empty() || seen.len() == titles.len(),
                    "INVALID_ARGUMENTS: give the text of every rule or none"
                );
                let learned = learned.as_ref().context("NOT_SUGGESTED: nothing was learned yet")?;
                for (i, title) in titles.iter().enumerate() {
                    let rule =
                        suggested(title).with_context(|| format!("NOT_SUGGESTED: {title} is not a suggestion now"))?;
                    ensure!(
                        seen.get(i).is_none_or(|text| *text == rule.text),
                        "STYLE_CHANGED: the suggestion {title} changed meanwhile; look at it again"
                    );
                    doc.set(doc::rule_heading(rule.title).expect("learned rules have headings"), rule.text.clone());
                    doc.rebuild_settings(Some((rule.title, &rule.settings)));
                    accepted.insert(rule.title.to_owned(), accept(rule));
                }
                if !frozen.contains_key("header") && doc.preamble_is_ours() {
                    doc.set_preamble(format!("{}\n", learned.header));
                }
                if !frozen.contains_key("overview") {
                    doc.set(OVERVIEW, learned.overview.clone());
                }
                if !frozen.contains_key("rare") {
                    match &learned.rare {
                        Some(rare) => doc.set(RARE, rare.clone()),
                        None => doc.remove(RARE),
                    }
                }
                match &titles[..] {
                    [one] => format!("Accepted {one}"),
                    many => format!("Accepted {} rules", many.len()),
                }
            }
            StyleAction::Reject { title } => {
                let rule =
                    suggested(&title).with_context(|| format!("NOT_SUGGESTED: {title} is not a suggestion now"))?;
                frozen.insert(rule.title.to_owned(), Frozen::Rejected);
                format!("Rejected {title}")
            }
            StyleAction::Remove { title } => {
                let heading = rule_in(&doc, &title)?;
                doc.remove(heading);
                doc.rebuild_settings(None);
                accepted.remove(&title);
                frozen.insert(title.clone(), Frozen::Removed);
                format!("Removed {title}")
            }
            StyleAction::Revert { title } => {
                let heading = doc::rule_heading(&title).with_context(|| format!("UNKNOWN_RULE: {title}"))?;
                let now = doc.get(heading).map(str::to_owned);
                // Before the first version there was no style.
                let default = Version::default();
                let older = versions
                    .iter()
                    .rev()
                    .skip(1)
                    .chain([&default])
                    .find(|v| Doc::parse(v.text.as_deref().unwrap_or("")).get(heading).map(str::to_owned) != now)
                    .with_context(|| format!("NOTHING_TO_REVERT: {title} has not changed"))?;
                let then = Doc::parse(older.text.as_deref().unwrap_or(""));
                match then.get(heading) {
                    Some(text) => doc.set(heading, text.to_owned()),
                    None => doc.remove(heading),
                }
                let rows: Vec<(String, String)> = then
                    .settings()
                    .into_iter()
                    .filter(|(name, _)| learning::SETTINGS.contains(&(name.as_str(), title.as_str())))
                    .collect();
                doc.rebuild_settings(Some((&title, &rows)));
                match older.accepted.get(&title) {
                    Some(a) => accepted.insert(title.clone(), a.clone()),
                    None => accepted.remove(&title),
                };
                frozen.insert(title.clone(), Frozen::Reverted);
                format!("Reverted {title}")
            }
            StyleAction::LearnAgain { title } => {
                ensure!(frozen.remove(&title).is_some(), "UNKNOWN_RULE: learning does not leave {title} alone");
                format!("Learn {title} again")
            }
            StyleAction::SetOwn { index, text, was } => {
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                ensure!(text.chars().count() <= MAX_OWN_RULE, "TOO_LONG: keep a rule under {MAX_OWN_RULE} characters");
                let own = doc.own();
                ensure!(index.is_none_or(|i| i < own.len()), "UNKNOWN_RULE: that rule of yours is gone");
                ensure!(
                    index.zip(was.as_ref()).is_none_or(|(i, was)| &own[i] == was),
                    "STYLE_CHANGED: that rule of yours changed meanwhile; read the style again"
                );
                ensure!(index.is_some() || !text.is_empty(), "INVALID_ARGUMENTS: write the rule first");
                doc.set_own(index, (!text.is_empty()).then_some(text.as_str()));
                match (index, text.is_empty()) {
                    (None, _) => "Added your rule",
                    (Some(_), false) => "Changed your rule",
                    (Some(_), true) => "Removed your rule",
                }
                .to_owned()
            }
            StyleAction::SetText { text, base_version } => {
                ensure!(
                    base_version == current.index,
                    "STYLE_CHANGED: your style changed meanwhile. Copy your text, open the style again and redo your change."
                );
                let new = Doc::parse(&text);
                hand_edits(&doc, &new, &mut accepted, &mut frozen);
                doc = new;
                "Edited by hand".to_owned()
            }
            StyleAction::Restore { index } => {
                // Version 0 is the style before any change: none, while what was learned stays.
                let none = Version { label: "No style yet".into(), ..Version::default() };
                let version = match index {
                    0 => &none,
                    _ => versions
                        .iter()
                        .find(|v| v.index == index)
                        .with_context(|| format!("UNKNOWN_VERSION: version {index} is not kept"))?,
                };
                doc = Doc::parse(version.text.as_deref().unwrap_or(""));
                (accepted, frozen) = (version.accepted.clone(), version.frozen.clone());
                format!("Restored: {}", version.label.strip_prefix("Restored: ").unwrap_or(&version.label))
            }
            StyleAction::Reset => {
                doc = Doc::parse("");
                (accepted, frozen) = (BTreeMap::new(), BTreeMap::new());
                match fs::remove_dir_all(self.evidence_dir()) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        return Err(e).context("Forgetting what was learned");
                    }
                    _ => {}
                }
                "Back to default".to_owned()
            }
        };
        tidy(&mut doc, &frozen);
        let text = (!doc.is_empty()).then(|| doc.text());
        self.commit(&versions, &format!("{by}{label}"), text, accepted, frozen)?;
        versions = self.read_versions()?;
        self.show(&versions)
    }

    /// The versions, after recording a change made to EDIT.md outside Nuzky.
    fn synced(&self) -> Result<Vec<Version>> {
        let versions = self.read_versions()?;
        let file = match fs::read_to_string(self.edit_md()) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).context("Reading EDIT.md"),
        };
        let current = versions.last().cloned().unwrap_or_default();
        // A write that stopped after its version was saved: finish it.
        if let Ok(index) = fs::read_to_string(self.writing_path()) {
            if index.trim() == current.index.to_string() {
                write_style(&self.edit_md(), current.text.as_deref())?;
                fs::remove_file(self.writing_path()).context("Finishing a change of the style")?;
                return Ok(versions);
            }
            fs::remove_file(self.writing_path()).context("Finishing a change of the style")?;
        }
        if file == current.text {
            return Ok(versions);
        }
        let (mut accepted, mut frozen) = (current.accepted, current.frozen);
        let label = if versions.is_empty() {
            // Whoever wrote it, learning leaves the text around the rules as it is.
            for name in ["header", "overview", "rare"] {
                frozen.insert(name.to_owned(), Frozen::Edited);
            }
            "Found EDIT.md"
        } else {
            let old = Doc::parse(current.text.as_deref().unwrap_or(""));
            hand_edits(&old, &Doc::parse(file.as_deref().unwrap_or("")), &mut accepted, &mut frozen);
            "Changed outside Nuzky"
        };
        self.append(&versions, label, file, accepted, frozen)?;
        self.read_versions()
    }

    fn commit(
        &self,
        versions: &[Version],
        label: &str,
        text: Option<String>,
        accepted: BTreeMap<String, Accepted>,
        frozen: BTreeMap<String, Frozen>,
    ) -> Result<()> {
        // The version is the commit point; a file not written yet is finished on the next change.
        let index = versions.last().map_or(1, |v| v.index + 1);
        write_whole(&self.writing_path(), index.to_string().as_bytes())?;
        self.append(versions, label, text.clone(), accepted, frozen)?;
        write_style(&self.edit_md(), text.as_deref())?;
        fs::remove_file(self.writing_path()).context("Finishing a change of the style")
    }

    fn append(
        &self,
        versions: &[Version],
        label: &str,
        text: Option<String>,
        accepted: BTreeMap<String, Accepted>,
        frozen: BTreeMap<String, Frozen>,
    ) -> Result<()> {
        let version = Version {
            index: versions.last().map_or(1, |v| v.index + 1),
            at_ms: now_ms(),
            label: label.to_owned(),
            text,
            accepted,
            frozen,
        };
        let mut line = serde_json::to_string(&version)?;
        line.push('\n');
        let path = self.versions_path();
        // A line cut short, by a full disk or a crash, would swallow the next one.
        let torn = !ends_with_newline(&path);
        if torn || versions.len() >= 2 * MAX_VERSIONS {
            let mut all = String::new();
            for v in &versions[versions.len().saturating_sub(MAX_VERSIONS - 1)..] {
                all.push_str(&serde_json::to_string(v)?);
                all.push('\n');
            }
            all.push_str(&line);
            return write_whole(&path, all.as_bytes());
        }
        let mut file =
            OpenOptions::new().create(true).append(true).open(&path).context("Opening the style's versions")?;
        file.write_all(line.as_bytes()).and_then(|()| file.sync_data()).context("Saving a version of the style")
    }

    fn read_versions(&self) -> Result<Vec<Version>> {
        let path = self.versions_path();
        let text = match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Ok(meta) if !meta.is_file() || meta.len() > READ_LIMIT => {
                log::error!("Not reading the style's versions in {}", path.display());
                return Ok(Vec::new());
            }
            _ => fs::read_to_string(&path).context("Reading the style's versions")?,
        };
        Ok(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
    }

    /// Evidence in the order it was learned.
    fn evidence(&self) -> Result<Vec<Evidence>> {
        let dir = self.evidence_dir();
        let entries = match fs::read_dir(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            entries => entries.with_context(|| format!("Reading {}", dir.display()))?,
        };
        let mut all: Vec<Evidence> = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            // A file that cannot be read is left out; the video can be learned again.
            match fs::read(&path).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice(&b)?)) {
                Ok(evidence) => all.push(evidence),
                Err(error) => log::error!("Leaving out {}: {error:#}", path.display()),
            }
        }
        all.sort_by_key(|e| e.seq);
        Ok(all)
    }

    fn show(&self, versions: &[Version]) -> Result<StyleView> {
        let current = versions.last().cloned().unwrap_or_default();
        let doc = Doc::parse(current.text.as_deref().unwrap_or(""));
        let evidence = self.evidence()?;
        let learned = learned_from(&evidence);
        let rules_now = |title: &str| learned.as_ref().and_then(|l| l.rules.iter().find(|r| r.title == title));
        let confidence = |rule: &Rule| {
            let mut videos: Vec<usize> = rule.moments.iter().map(|m| m.source).collect();
            videos.sort_unstable();
            videos.dedup();
            Confidence {
                level: match videos.len() {
                    3.. => Level::High,
                    2 => Level::Medium,
                    _ => Level::Low,
                },
                videos: videos.len(),
                of: evidence.len(),
                moments: rule.moments.len(),
            }
        };
        let rules = doc
            .rules()
            .into_iter()
            .map(|title| {
                // A rule changed by hand says what the creator wrote, which neither the summary
                // nor what learning finds now describes.
                let by_you = current.frozen.get(title) == Some(&Frozen::Edited);
                ActiveRule {
                    title: title.to_owned(),
                    summary: current
                        .accepted
                        .get(title)
                        .filter(|_| !by_you)
                        .map(|a| a.summary.clone())
                        .unwrap_or_default(),
                    by_you,
                    confidence: rules_now(title).filter(|_| !by_you).map(confidence),
                }
            })
            .collect();
        let suggestions = learned
            .iter()
            .flat_map(|l| &l.rules)
            .filter(|r| !current.frozen.contains_key(r.title))
            .filter_map(|rule| {
                let before = current.accepted.get(rule.title);
                if before.is_some_and(|a| Rule::same(&a.choices, &rule.choices)) {
                    return None;
                }
                Some(Suggestion {
                    title: rule.title.to_owned(),
                    summary: rule.summary.clone(),
                    update: doc.rules().contains(&rule.title),
                    changes: changes(before.map_or(&[][..], |a| &a.choices), &rule.choices),
                    confidence: confidence(rule),
                    moments: rule
                        .moments
                        .iter()
                        .map(|m| StyleMoment {
                            video: evidence[m.source].recording.clone(),
                            time_us: m.time_us,
                            line: m.line.clone(),
                        })
                        .collect(),
                    text: rule.text.clone(),
                })
            })
            .collect();
        Ok(StyleView {
            version: current.index,
            own: doc.own(),
            rules,
            suggestions,
            not_learned: current
                .frozen
                .iter()
                .filter(|(title, _)| doc::rule_heading(title).is_some())
                .map(|(title, reason)| NotLearned { title: title.clone(), reason: *reason })
                .collect(),
            sources: evidence
                .iter()
                .rev()
                .map(|e| LearnedFrom { title: e.title.clone(), kind: e.kind, at_ms: e.at_ms, matched: e.matched })
                .collect(),
            versions: versions
                .iter()
                .rev()
                .map(|v| StyleVersion { index: v.index, label: v.label.clone(), at_ms: v.at_ms })
                .collect(),
            text: current.text,
        })
    }
}

/// The style as an agent reads it: what the app shows, with the first moments of each
/// suggestion and the newest versions.
pub fn for_agent(view: &StyleView) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(view)?;
    for suggestion in value["suggestions"].as_array_mut().into_iter().flatten() {
        if let Some(moments) = suggestion["moments"].as_array_mut() {
            moments.truncate(4);
        }
    }
    if let Some(versions) = value["versions"].as_array_mut() {
        versions.truncate(10);
    }
    Ok(value)
}

fn accept(rule: &Rule) -> Accepted {
    Accepted { summary: rule.summary.clone(), choices: rule.choices.clone() }
}

fn learned_from(evidence: &[Evidence]) -> Option<Learned> {
    let sources: Vec<Source> = evidence.iter().map(Evidence::source).collect();
    (!sources.is_empty()).then(|| learning::learned(&sources))
}

fn rule_in(doc: &Doc, title: &str) -> Result<&'static str> {
    doc::rule_heading(title)
        .filter(|h| doc.get(h).is_some())
        .with_context(|| format!("UNKNOWN_RULE: your style has no rule {title}"))
}

/// Learning leaves alone what the creator changed by hand, and forgets rules they deleted.
fn hand_edits(old: &Doc, new: &Doc, accepted: &mut BTreeMap<String, Accepted>, frozen: &mut BTreeMap<String, Frozen>) {
    for name in doc::edited(old, new) {
        let gone = doc::rule_heading(&name).is_some_and(|h| new.get(h).is_none());
        if gone {
            accepted.remove(&name);
        }
        frozen.insert(name, if gone { Frozen::Removed } else { Frozen::Edited });
    }
}

/// Without learned rules the text around them goes too, so a style of the creator's own rules
/// reads as only that.
fn tidy(doc: &mut Doc, frozen: &BTreeMap<String, Frozen>) {
    if !doc.rules().is_empty() {
        return;
    }
    if !frozen.contains_key("overview") {
        doc.remove(OVERVIEW);
    }
    if !frozen.contains_key("rare") {
        doc.remove(RARE);
    }
    if !frozen.contains_key("header") && doc.preamble_is_ours() {
        doc.set_preamble(if doc.blocks.len() > 1 { doc::PREAMBLE.to_owned() } else { String::new() });
    }
}

fn changes(before: &[Choice], after: &[Choice]) -> Vec<Change> {
    let mut out: Vec<Change> = after
        .iter()
        .filter_map(|new| {
            let old = before.iter().find(|c| c.name == new.name);
            (!old.is_some_and(|old| old.same(new))).then(|| Change {
                name: new.name.clone(),
                from: old.map(|c| c.value.clone()),
                to: Some(new.value.clone()),
            })
        })
        .collect();
    out.extend(before.iter().filter(|old| !after.iter().any(|c| c.name == old.name)).map(|old| Change {
        name: old.name.clone(),
        from: Some(old.value.clone()),
        to: None,
    }));
    out
}

/// An empty or missing file counts as ending well.
fn ends_with_newline(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = File::open(path) else { return true };
    if file.seek(SeekFrom::End(-1)).is_err() {
        return true;
    }
    let mut last = [0u8];
    file.read_exact(&mut last).is_ok_and(|()| last[0] == b'\n')
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Writes beside the target first, so a reader never sees half a file.
fn write_whole(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path.file_name().context("A style file needs a name")?.to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.{}.part", std::process::id()));
    fs::write(&tmp, bytes)
        .and_then(|()| fs::rename(&tmp, path))
        .inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
        .with_context(|| format!("Cannot write {}", path.display()))
}

fn write_style(path: &Path, text: Option<&str>) -> Result<()> {
    match text {
        Some(text) => write_whole(path, text.as_bytes()),
        None => match fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).context("Removing EDIT.md"),
            _ => Ok(()),
        },
    }
}

/// A video file with its words, recognised now if none are stored.
pub struct Video {
    pub asset: Asset,
    pub record: Record,
}

/// Reads a video and its words the way `nuzky style` always has: stored words when they are in
/// `language` (any for "auto"), else recognised with the best installed model.
pub fn video(
    path: &Path,
    language: &str,
    store: &TranscriptStore,
    cache: &Path,
    cancel: &AtomicBool,
    stage: impl FnMut(Stage),
) -> Result<Video> {
    let asset = nuzky_engine::media::probe(path, stable_id(path)?)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    ensure!(nuzky_engine::audio::has_audio(&asset), "{} has no sound", path.display());
    // Nuzky hears speech in videos only; a sound file on the timeline is music.
    ensure!(
        asset.kind == AssetKind::Video,
        "{} has no picture: style learns from and scores video recordings",
        path.display()
    );
    // A language asked for explicitly corrects a recognition stored in another one.
    if let Some(record) = store.get(&asset)?
        && (language == "auto" || record.language == language)
    {
        return Ok(Video { asset, record });
    }
    let model = best_model();
    let record = recognise(store, &asset, cache, model, &models(model)?, language, cancel, stage)?;
    Ok(Video { asset, record })
}

/// The same file gets the same id, so its sound is extracted once into the shared cache.
fn stable_id(path: &Path) -> Result<String> {
    use std::hash::{Hash, Hasher};
    let path = fs::canonicalize(path).with_context(|| format!("Cannot find {}", path.display()))?;
    let metadata = path.metadata()?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (&path, metadata.len(), metadata.modified()?).hash(&mut hash);
    Ok(format!("style-{:016x}", hash.finish()))
}

/// Where the cut came from in the recording, refused when too little of it is there: by sound,
/// and by the words the cut says, which a cut of another recording only seems to share.
pub fn aligned(recording: &Video, cut: &Video, cache: &Path, cancelled: &dyn Fn() -> bool) -> Result<Alignment> {
    let mut alignment = learning::align(&recording.asset, &cut.asset, cache, cancelled)?;
    if let Some(words) = alignment.words_found(&recording.record.words, &cut.record.words) {
        alignment.matched = alignment.matched.min(words);
    }
    ensure!(
        alignment.matched >= MIN_MATCH,
        "NO_MATCH: only {:.0}% of the speech of {} is in {}, so it was not cut from it",
        alignment.matched * 100.0,
        cut.asset.name,
        recording.asset.name
    );
    Ok(alignment)
}

/// What a recording and the finished video cut from it teach.
pub fn compare(
    recording: &Video,
    cut: &Video,
    store: &TranscriptStore,
    cache: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<Evidence> {
    let alignment = aligned(recording, cut, cache, cancelled)?;
    let picture = learning::picture(&recording.asset, &cut.asset, &alignment, cancelled)?;
    if cancelled() {
        bail!("CANCELLED: learning was stopped");
    }
    Ok(Evidence {
        seq: 0,
        key: format!("pair:{}:{}", store.fingerprint(&recording.asset)?, store.fingerprint(&cut.asset)?),
        kind: EvidenceKind::Pair,
        title: format!("{} and {}", recording.asset.name, cut.asset.name),
        at_ms: now_ms(),
        matched: Some(alignment.matched),
        recording: recording.asset.name.clone(),
        cut: cut.asset.name.clone(),
        language: recording.record.language.clone(),
        recording_us: recording.asset.duration_us,
        words: recording.record.words.clone(),
        cut_words: cut.record.words.clone(),
        alignment,
        picture,
        places: None,
        corrections: Vec::new(),
    })
}

/// Pauses shorter than this are gaps inside speech, as when sound is compared.
const MIN_PAUSE_US: i64 = 80_000;
/// Framing is read this often within a clip, and this far from its ends, as in a finished video.
const SAMPLE_US: i64 = 500_000;
const EDGE_US: i64 = 200_000;
/// A smaller change of scale is not a zoom.
const MIN_ZOOM: f32 = 0.03;

/// What a project teaches, read from its timeline: the words its heard clips play, where each
/// clip comes from, the pauses between words, its captions and how its clips are framed and
/// zoomed. The recording is every transcribed video it hears, one after another. None when it
/// hears no transcribed speech or plays everything that was recorded.
pub fn project_evidence(project: &Project, path: &Path, transcripts: &TranscriptStore) -> Result<Option<Evidence>> {
    let heard = crate::transcript::heard_assets(project);
    let mut records = Vec::new();
    for asset in project.assets.iter().filter(|a| heard.contains(&a.id)) {
        if let Some(record) = transcripts.get(asset)? {
            records.push((asset, record));
        }
    }
    let Some((_, first)) = records.first() else { return Ok(None) };
    let language = first.language.clone();
    let sources = records.iter().map(|(a, r)| (a.id.clone(), r.words.clone())).collect();
    let derived = crate::transcript::Derived::new(project, sources, Vec::new());
    // Each file's words after the files before it.
    let mut offsets = std::collections::HashMap::new();
    let (mut words, mut owners) = (Vec::new(), Vec::new());
    let mut recorded = 0;
    for (asset, _) in &records {
        offsets.insert(asset.id.as_str(), recorded);
        for word in &derived.sources[&asset.id] {
            words.push(Word { start_us: word.start_us + recorded, end_us: word.end_us + recorded, ..word.clone() });
            owners.push((asset.id.as_str(), word.start_us));
        }
        recorded += asset.duration_us;
    }
    // Every clip that is heard playing a transcribed file is a piece of the recording.
    let mut clips: Vec<_> = project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().filter(move |c| is_heard(project, t, c)))
        .filter_map(|c| match &c.content {
            ClipContent::Media { asset_id, source_in_us, .. } => {
                offsets.get(asset_id.as_str()).map(|offset| (c, offset + source_in_us))
            }
            ClipContent::Text { .. } => None,
        })
        .collect();
    clips.sort_by_key(|(c, _)| (c.start_us, c.id.clone()));
    let pieces: Vec<Piece> = clips
        .iter()
        .map(|(c, from)| Piece { start_us: c.start_us, end_us: c.end_us(), offset_us: from - c.start_us })
        .collect();
    let mut places: Vec<Place> = vec![None; words.len()];
    for word in &derived.words {
        let Some(i) = owners.iter().position(|&(a, s)| a == word.asset_id && s == word.source_start_us) else {
            continue;
        };
        let piece = clips.iter().position(|(c, _)| c.id == word.clip_id);
        if places[i].is_none()
            && let Some(piece) = piece
        {
            places[i] = Some((piece, word.start_us, word.end_us));
        }
    }
    // Every word, each file in one piece: the recording as it was, nothing cut.
    if places.iter().all(Option::is_some) && clips.len() <= records.len() {
        return Ok(None);
    }
    let cut_words: Vec<Word> = derived
        .words
        .iter()
        .map(|w| Word { start_us: w.start_us, end_us: w.end_us, text: w.text.clone(), probability: w.probability })
        .collect();
    let gaps = |words: &[Word]| -> Vec<Range> {
        words
            .windows(2)
            .filter(|p| p[1].start_us - p[0].end_us >= MIN_PAUSE_US)
            .map(|p| Range { start_us: p[0].end_us, end_us: p[1].start_us })
            .collect()
    };
    let alignment = Alignment {
        recording_pauses: gaps(&words),
        cut_pauses: gaps(&cut_words),
        matched: 1.0,
        cut_duration_us: project.duration_us(),
        pieces,
    };
    let corrections = project
        .word_corrections
        .iter()
        .filter_map(|c| {
            let word =
                derived.words.iter().find(|w| w.asset_id == c.asset_id && w.source_start_us == c.source_start_us)?;
            Some(Correction { time_us: word.start_us, heard: c.original.clone(), text: c.text.clone() })
        })
        .collect();
    let key = format!("project:{}", fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()).display());
    Ok(Some(Evidence {
        seq: 0,
        key,
        kind: EvidenceKind::Project,
        title: project.name.clone(),
        at_ms: now_ms(),
        matched: None,
        recording: project.name.clone(),
        cut: format!("{}, as edited", project.name),
        language,
        recording_us: recorded,
        words,
        cut_words,
        picture: project_picture(project, &clips.iter().map(|(c, _)| *c).collect::<Vec<_>>()),
        alignment,
        places: Some(places),
        corrections,
    }))
}

/// Captions from the Captions track; framing and zoom from the clips of the recording.
fn project_picture(project: &Project, clips: &[&nuzky_engine::model::Clip]) -> Picture {
    let mut captions: Vec<Caption> = Vec::new();
    let mut heights: Vec<f32> = Vec::new();
    for clip in project.tracks.iter().filter(|t| t.is_captions() && !t.hidden).flat_map(|t| &t.clips) {
        let ClipContent::Text { text, transform, .. } = &clip.content else { continue };
        captions.push(Caption {
            start_us: clip.start_us,
            end_us: clip.end_us(),
            words: text.split_whitespace().count(),
        });
        heights.push(0.5 + transform.y);
    }
    captions.sort_by_key(|c| c.start_us);
    heights.sort_by(f32::total_cmp);
    let caption_band = heights.get(heights.len() / 2).map(|&middle| (middle, middle));
    // The picture of the recording: video clips, which a detached sound clip is not.
    let pictured: Vec<&nuzky_engine::model::Clip> = project
        .tracks
        .first()
        .into_iter()
        .flat_map(|t| &t.clips)
        .filter(|c| match &c.content {
            ClipContent::Media { asset_id, .. } => {
                project.asset(asset_id).is_some_and(|a| a.kind == AssetKind::Video)
                    && clips
                        .iter()
                        .any(|h| matches!(&h.content, ClipContent::Media { asset_id: a, .. } if a == asset_id))
            }
            ClipContent::Text { .. } => false,
        })
        .collect();
    let mut framing = Vec::new();
    let mut zooms = Vec::new();
    for (i, clip) in pictured.iter().enumerate() {
        let mut t = clip.start_us + EDGE_US;
        while t < clip.end_us() - EDGE_US {
            let (transform, _) = transform_at(clip, t);
            framing.push(Framing { time_us: t, scale: transform.scale, x: transform.x, y: transform.y });
            t += SAMPLE_US;
        }
        if let Some(next) = pictured.get(i + 1).filter(|n| n.start_us == clip.end_us()) {
            let (from, to) = (transform_at(clip, clip.end_us() - 1).0.scale, transform_at(next, next.start_us).0.scale);
            if (to - from).abs() >= MIN_ZOOM {
                zooms.push(ZoomChange { start_us: next.start_us, end_us: next.start_us, from, to, at_cut: true });
            }
        }
        for pair in clip.keyframes.windows(2) {
            let (from, to) = (pair[0].transform.scale, pair[1].transform.scale);
            if (to - from).abs() >= MIN_ZOOM {
                let at = |t_us: i64| clip.start_us + t_us;
                let (start_us, end_us) = (at(pair[0].t_us), at(pair[1].t_us));
                zooms.push(ZoomChange { start_us, end_us, from, to, at_cut: false });
            }
        }
    }
    zooms.sort_by_key(|z| z.start_us);
    Picture { skipped: None, captions, caption_band, framing, zooms }
}

/// A project to learn from as it was at one moment, with the AI runs before it, so what it
/// teaches is checked against that same state however long learning takes.
pub struct Moment {
    project: Project,
    path: PathBuf,
    edited: bool,
    runs: Option<(Project, Option<Project>)>,
}

/// What learning from a project the creator exported, or `edited` after an AI cut it, needs, taken
/// now. None while an AI run is open, or for edits when no AI run was kept. It costs a look at the
/// project's newest versions, so the slow part is left to `Moment::lesson`.
pub fn moment(session: &ProjectSession, project: Project, run_open: bool, edited: bool) -> Result<Option<Moment>> {
    if run_open {
        return Ok(None);
    }
    let runs = session.run_versions()?;
    if edited && runs.is_none() {
        return Ok(None);
    }
    Ok(Some(Moment { project, path: session.path(), edited, runs }))
}

impl Moment {
    /// What it teaches. Nothing from a project exactly as the AI's last run left it: the AI would
    /// only learn its own cut. Edits that undo the run teach nothing either, while an export of
    /// the creator's own cut from before the run does.
    pub fn lesson(&self, transcripts: &TranscriptStore) -> Result<Option<Evidence>> {
        let Some(evidence) = project_evidence(&self.project, &self.path, transcripts)? else { return Ok(None) };
        if let Some((after, before)) = &self.runs {
            for other in std::iter::once(after).chain(before.as_ref().filter(|_| self.edited)) {
                if project_evidence(other, &self.path, transcripts)?.is_some_and(|o| o.same_lesson(&evidence)) {
                    return Ok(None);
                }
            }
        }
        Ok(Some(evidence))
    }
}

#[cfg(test)]
mod tests {
    use nuzky_analysis::Range;
    use nuzky_analysis::style::{Caption, Piece};
    use nuzky_session::host::Host;

    use super::*;

    /// A recording of retakes, fillers and a side remark, cut the way a creator would, with
    /// pauses of `pause_us` left between the kept sentences.
    fn evidence(key: &str, pause_us: i64) -> Evidence {
        let sentences = [
            ("So today I want to", false),
            ("So today I want to show you my edit.", true),
            ("First I record it all", false),
            ("First I record it all in one take.", true),
            ("Um", false),
            ("Then I remove the", false),
            ("Then I remove the slips and pauses.", true),
            ("This is a side remark nobody needs.", false),
            ("Captions go on top.", true),
            ("That is all for today.", true),
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
            cut_pauses: pieces
                .windows(2)
                .map(|p| Range { start_us: p[1].start_us - pause_us / 2, end_us: p[1].start_us + pause_us / 2 })
                .collect(),
            recording_pauses: vec![Range { start_us: 0, end_us: 500_000 }],
            matched: 1.0,
            cut_duration_us: c,
            pieces,
        };
        let captions =
            (0..c / 500_000).map(|i| Caption { start_us: i * 500_000, end_us: (i + 1) * 500_000, words: 2 }).collect();
        Evidence {
            seq: 0,
            key: key.into(),
            kind: EvidenceKind::Pair,
            title: format!("{key}.mov and reel.mp4"),
            at_ms: 1,
            matched: Some(1.0),
            recording: format!("{key}.mov"),
            cut: "reel.mp4".into(),
            language: "en".into(),
            recording_us: t,
            words,
            cut_words,
            alignment,
            picture: Picture {
                skipped: None,
                captions,
                caption_band: Some((0.6, 0.66)),
                framing: Vec::new(),
                zooms: Vec::new(),
            },
            places: None,
            corrections: Vec::new(),
        }
    }

    fn add(store: &Store, evidence: Evidence) -> StyleView {
        store.keep_evidence(evidence).unwrap();
        store.view().unwrap()
    }

    fn store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("nuzky-style-{}", nuzky_engine::edit::new_id()));
        (Store::at(&dir), dir)
    }

    fn file(dir: &Path) -> Option<String> {
        fs::read_to_string(dir.join("EDIT.md")).ok()
    }

    fn titles(view: &StyleView) -> Vec<String> {
        view.suggestions.iter().map(|s| s.title.clone()).collect()
    }

    #[test]
    fn learning_suggests_and_accepting_all_writes_what_the_cli_writes() {
        let (store, dir) = store();
        let view = add(&store, evidence("talk", 120_000));
        assert_eq!(file(&dir), None, "learning alone never writes the style");
        assert!(view.suggestions.len() >= 4 && view.versions.is_empty(), "{:?}", titles(&view));
        assert!(view.suggestions.iter().all(|s| !s.update && s.confidence.level == Level::Low && s.confidence.of == 1));
        let restart = view.suggestions.iter().find(|s| s.title == "Restarted sentences").unwrap();
        assert!(restart.moments.len() == 3 && restart.moments[0].video == "talk.mov", "{:?}", restart.moments);

        let seen: Vec<String> = view.suggestions.iter().map(|s| s.text.clone()).collect();
        let changed = StyleAction::Accept {
            titles: titles(&view),
            seen: seen.iter().map(|t| t.replace("120 ms", "90 ms")).collect(),
        };
        assert!(
            store.act(changed).unwrap_err().to_string().starts_with("STYLE_CHANGED"),
            "only what was shown is accepted"
        );
        let view = store.act(StyleAction::Accept { titles: titles(&view), seen }).unwrap();
        let cli = learning::learn(&[evidence("talk", 120_000).source()]);
        assert_eq!(file(&dir).unwrap(), cli, "accepting every suggestion gives the CLI's EDIT.md byte for byte");
        assert!(view.suggestions.is_empty() && view.versions[0].label.starts_with("Accepted "), "{:?}", view.versions);

        // The same video learned again with a few more milliseconds of pause is no news; a much
        // longer pause is, and only for Pauses.
        let view = add(&store, evidence("talk", 130_000));
        assert!(view.suggestions.is_empty(), "{:?}", titles(&view));
        assert_eq!(view.sources.len(), 1, "learning a video again replaces it");
        let view = add(&store, evidence("talk", 400_000));
        assert_eq!(titles(&view), ["Pauses"]);
        let pauses = &view.suggestions[0];
        assert!(
            pauses.update
                && pauses.changes[0].from.as_deref() == Some("120 ms")
                && pauses.changes[0].to.as_deref() == Some("400 ms"),
            "{:?}",
            pauses.changes
        );
        assert_eq!(file(&dir).unwrap(), cli, "a suggestion changes nothing until accepted");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejected_and_removed_rules_stay_out_until_learned_again() {
        let (store, dir) = store();
        add(&store, evidence("talk", 120_000));
        let view = store.act(StyleAction::Reject { title: "Pauses".into() }).unwrap();
        assert!(!titles(&view).contains(&"Pauses".to_owned()));
        assert_eq!(file(&dir), None, "rejecting writes no style");
        let view = store.act(StyleAction::Accept { titles: titles(&view), seen: Vec::new() }).unwrap();
        assert!(!file(&dir).unwrap().contains("## Pauses") && view.suggestions.is_empty());
        let view = add(&store, evidence("talk", 400_000));
        assert!(view.suggestions.is_empty(), "a rejected rule is not suggested again: {:?}", titles(&view));

        let view = store.act(StyleAction::Remove { title: "Captions".into() }).unwrap();
        let text = file(&dir).unwrap();
        assert!(!text.contains("## Captions") && !text.contains("build_captions"), "{text}");
        assert!(view.not_learned.iter().any(|n| n.title == "Captions" && n.reason == Frozen::Removed));
        let view = store.act(StyleAction::LearnAgain { title: "Pauses".into() }).unwrap();
        assert_eq!(titles(&view), ["Pauses"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn own_rules_and_hand_edits_outlive_learning() {
        let (store, dir) = store();
        store.act(StyleAction::SetOwn { index: None, text: "Never cut  the product name.".into(), was: None }).unwrap();
        assert!(file(&dir).unwrap().contains("## Your rules\n\nThe creator's own instructions."), "{:?}", file(&dir));
        let view = add(&store, evidence("talk", 120_000));
        let view = store.act(StyleAction::Accept { titles: titles(&view), seen: Vec::new() }).unwrap();
        assert_eq!(view.own, ["Never cut the product name."]);
        let text = file(&dir).unwrap();
        assert!(text.find("## Your rules").unwrap() < text.find("## Settings").unwrap(), "{text}");

        // The creator sets a pause by hand: learning leaves that rule alone from then on.
        let edited = text.replace(
            "| edit_transcript shorten_pauses_us | 120000 |",
            "| edit_transcript shorten_pauses_us | 250000 |",
        );
        assert_ne!(edited, text);
        let stale = store.act(StyleAction::SetText { text: edited.clone(), base_version: view.version - 1 });
        assert!(stale.unwrap_err().to_string().starts_with("STYLE_CHANGED"));
        let view = store.act(StyleAction::SetText { text: edited.clone(), base_version: view.version }).unwrap();
        let pauses = view.rules.iter().find(|r| r.title == "Pauses").unwrap();
        assert!(pauses.by_you && pauses.summary.is_empty() && pauses.confidence.is_none(), "{pauses:?}");
        let view = add(&store, evidence("talk", 400_000));
        assert!(view.suggestions.is_empty(), "{:?}", titles(&view));
        assert_eq!(file(&dir).unwrap(), edited);

        // nuzky style learn replaces the learned rules and keeps the creator's own.
        store.replace(&learning::learned(&[evidence("talk", 120_000).source()])).unwrap();
        let text = file(&dir).unwrap();
        assert!(text.contains("- Never cut the product name.") && text.contains("| 120000 |"), "{text}");
        let view = store.view().unwrap();
        assert_eq!(view.versions[0].label, "Learned with nuzky style learn");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_change_outside_nuzky_is_a_version_and_any_version_comes_back() {
        let (store, dir) = store();
        let view = add(&store, evidence("talk", 120_000));
        let accepted = store.act(StyleAction::Accept { titles: titles(&view), seen: Vec::new() }).unwrap();
        let learned = file(&dir).unwrap();
        let outside = learned.replace("Cut every earlier attempt", "Cut each earlier attempt");
        assert_ne!(outside, learned);
        fs::write(dir.join("EDIT.md"), outside).unwrap();
        let view = store.view().unwrap();
        assert_eq!(view.versions[0].label, "Changed outside Nuzky");
        assert!(view.rules.iter().any(|r| r.title == "Restarted sentences" && r.by_you), "{:?}", view.rules);

        let view = store.act(StyleAction::Restore { index: accepted.version }).unwrap();
        assert_eq!(file(&dir).unwrap(), learned);
        assert!(view.versions[0].label.starts_with("Restored: Accepted"), "{:?}", view.versions[0]);

        let view = store.act(StyleAction::Revert { title: "Pauses".into() }).unwrap();
        assert!(!file(&dir).unwrap().contains("## Pauses"), "before it was accepted, the style had no Pauses");
        assert!(view.not_learned.iter().any(|n| n.title == "Pauses" && n.reason == Frozen::Reverted));

        // A write that stopped after its version is finished by the next change...
        let after = file(&dir).unwrap();
        fs::write(dir.join("EDIT.md"), &learned).unwrap();
        fs::write(dir.join("style/writing"), view.version.to_string()).unwrap();
        let view = store.view().unwrap();
        assert_eq!(file(&dir).unwrap(), after);
        assert!(!dir.join("style/writing").exists() && view.versions[0].label == "Reverted Pauses");
        // ...while the same text put back by hand is the creator's change.
        fs::write(dir.join("EDIT.md"), &learned).unwrap();
        let view = store.view().unwrap();
        assert_eq!((view.versions[0].label.as_str(), file(&dir).unwrap()), ("Changed outside Nuzky", learned.clone()));
        store.act(StyleAction::Restore { index: view.versions[1].index }).unwrap();
        assert_eq!(file(&dir).unwrap(), after);

        // The style before any change comes back too, and what was learned stays.
        let view = store.act(StyleAction::Restore { index: 0 }).unwrap();
        assert_eq!(file(&dir), None);
        assert!(
            !view.sources.is_empty()
                && !view.suggestions.is_empty()
                && view.versions[0].label == "Restored: No style yet"
        );
        store.act(StyleAction::Restore { index: view.versions[1].index }).unwrap();
        assert_eq!(file(&dir).unwrap(), after);

        // A version cut short by a full disk does not swallow the next one.
        let mut versions = fs::OpenOptions::new().append(true).open(dir.join("style/versions.jsonl")).unwrap();
        versions.write_all(b"{\"index\":99,\"lab").unwrap();
        drop(versions);
        store.act(StyleAction::SetOwn { index: None, text: "Keep it short.".into(), was: None }).unwrap();
        let view = store.view().unwrap();
        assert_eq!(view.versions[0].label, "Added your rule");
        assert_eq!(view.own, ["Keep it short."]);
        let moved = store.act(StyleAction::SetOwn { index: Some(0), text: String::new(), was: Some("Gone.".into()) });
        assert!(
            moved.unwrap_err().to_string().starts_with("STYLE_CHANGED"),
            "a rule that moved is never removed instead"
        );
        store
            .act(StyleAction::SetOwn { index: Some(0), text: String::new(), was: Some("Keep it short.".into()) })
            .unwrap();

        let view = store.act(StyleAction::Reset).unwrap();
        assert_eq!(file(&dir), None);
        assert!(view.sources.is_empty() && view.suggestions.is_empty() && view.own.is_empty());
        let view = store.act(StyleAction::Restore { index: view.versions[1].index }).unwrap();
        assert_eq!(file(&dir).unwrap(), after, "back to default can be undone");
        assert!(view.versions.len() >= 6);
        fs::remove_dir_all(dir).unwrap();
    }

    /// A 30 s talk of six four-word sentences on one clip, its words stored.
    fn talk() -> (PathBuf, Host) {
        use nuzky_engine::edit::EditCmd;
        use nuzky_engine::model::Asset;
        use nuzky_session::transcripts::VERSION;
        let dir = std::env::temp_dir().join(format!("nuzky-style-talk-{}", nuzky_engine::edit::new_id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("talk.mp4");
        fs::write(&path, "talk").unwrap();
        let asset = Asset {
            id: "talk".into(),
            name: "talk.mp4".into(),
            path: path.to_string_lossy().into(),
            kind: AssetKind::Video,
            duration_us: 30_000_000,
            width: 1080,
            height: 1920,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
        };
        let words = (0..6)
            .flat_map(|s: i64| {
                (0..4).map(move |w: i64| Word {
                    start_us: 1_000_000 + s * 5_000_000 + w * 500_000,
                    end_us: 1_400_000 + s * 5_000_000 + w * 500_000,
                    text: format!("w{s}{w}{}", if w == 3 { "." } else { "" }),
                    probability: 0.9,
                })
            })
            .collect();
        let mut project = Project::new("Talk");
        project.apply(EditCmd::AddAssets { assets: vec![asset.clone()] }).unwrap();
        project.apply(EditCmd::AddClip { asset_id: "talk".into(), start_us: None, track_id: None }).unwrap();
        let file = dir.join("talk.nuzky");
        fs::write(&file, serde_json::to_vec(&project).unwrap()).unwrap();
        let transcripts = TranscriptStore::at(dir.join("transcripts")).unwrap();
        let record = Record {
            version: VERSION,
            fingerprint: transcripts.fingerprint(&asset).unwrap(),
            duration_us: asset.duration_us,
            model: "fixture".into(),
            language: "en".into(),
            words,
            segments: vec![],
            alignment: None,
        };
        transcripts.put(&asset, &record).unwrap();
        let session = nuzky_session::ProjectSession::open(&file, nuzky_session::Mode::Write, None).unwrap();
        (dir.clone(), Host { session, jobs: Default::default(), transcripts, cache_dir: dir.join("cache") })
    }

    /// Cuts a sentence, with `before` sentences before it already cut; each cut takes 2 s.
    fn cut(sentence: i64, before: i64) -> Vec<nuzky_engine::edit::EditCmd> {
        let start = 900_000 + sentence * 5_000_000 - before * 2_000_000;
        vec![serde_json::from_value(serde_json::json!({"type": "rippleDeleteRanges", "ranges": [{"startUs": start, "endUs": start + 2_000_000}]})).unwrap()]
    }

    /// Cuts the silence between the first two sentences, 3 s to 5.9 s.
    fn cut_silence() -> Vec<nuzky_engine::edit::EditCmd> {
        vec![
            serde_json::from_value(serde_json::json!({"type": "rippleDeleteRanges", "ranges": [{"startUs": 3_000_000, "endUs": 5_900_000}]}))
                .unwrap(),
        ]
    }

    /// Which of the six sentences the evidence says were kept.
    fn kept(evidence: &Evidence) -> Vec<bool> {
        let places = evidence.places.as_ref().unwrap();
        (0..6).map(|s| places[s * 4..s * 4 + 4].iter().all(Option::is_some)).collect()
    }

    #[test]
    fn the_ai_never_learns_its_own_cut_and_the_creators_edits_teach() {
        let (dir, host) = talk();
        let learn = |edited| {
            let state = host.session.state().unwrap();
            let moment = moment(&host.session, state.project, state.open_run.is_some(), edited).unwrap();
            moment.and_then(|m| m.lesson(&host.transcripts).unwrap())
        };
        assert!(learn(false).is_none(), "a project that plays everything recorded teaches nothing");
        // Silence cut between two sentences: every word stays, and that is a cut too.
        host.session.edit(cut_silence(), None, Default::default()).unwrap();
        let taught = learn(false).unwrap();
        assert!(taught.places.as_ref().unwrap().iter().all(Option::is_some) && taught.alignment.pieces.len() == 2);
        host.session.undo().unwrap();
        host.session.edit(cut(5, 0), None, Default::default()).unwrap();
        assert!(learn(true).is_none(), "edits teach only after an AI cut");
        assert_eq!(kept(&learn(false).unwrap()), [true, true, true, true, true, false], "an export does");

        let run = host.session.begin_run("Rough cut".into()).unwrap().run_id;
        host.session.apply_edits(&run, "cut", cut(1, 0), Default::default()).unwrap();
        assert!(learn(true).is_none() && learn(false).is_none(), "nothing while the AI edits");
        host.session.end_run(&run, nuzky_session::EndAction::Keep).unwrap();
        assert!(learn(true).is_none() && learn(false).is_none(), "nor from the AI's cut as it left it");

        host.session.edit(cut(3, 1), None, Default::default()).unwrap();
        let taught = learn(true).unwrap();
        assert_eq!(kept(&taught), [true, false, true, false, true, false]);
        assert_eq!(taught.alignment.pieces.len(), 4, "three cuts and the silence after the last");
        assert_eq!(taught.kind, EvidenceKind::Project);
        assert!(taught.alignment.cut_pauses.iter().all(|p| p.end_us - p.start_us >= MIN_PAUSE_US));
        host.session.undo().unwrap();
        assert!(learn(true).is_none(), "undoing the edit is the AI's cut again");
        host.session.undo_run(&run).unwrap();
        assert!(learn(true).is_none(), "undoing the AI's run teaches nothing");
        assert_eq!(
            kept(&learn(false).unwrap()),
            [true, true, true, true, true, false],
            "exporting the creator's own cut does"
        );
        drop(host);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_project_shows_its_captions_zoom_and_corrected_words() {
        let (dir, host) = talk();
        host.session.edit(cut(2, 0), None, Default::default()).unwrap();
        let mut project = host.session.state().unwrap().project;
        let main = &mut project.tracks[0].clips;
        if let ClipContent::Media { transform, .. } = &mut main[1].content {
            transform.scale = 1.25;
        }
        project.word_corrections = (0..3)
            .map(|w| nuzky_engine::model::WordCorrection {
                asset_id: "talk".into(),
                source_start_us: 1_000_000 + w * 500_000,
                original: format!("w0{w}"),
                text: "Nuzky".into(),
            })
            .collect();
        let mut captions = nuzky_engine::model::Track {
            id: "captions".into(),
            kind: nuzky_engine::model::TrackKind::Text,
            name: nuzky_engine::model::CAPTIONS_TRACK.into(),
            muted: false,
            hidden: false,
            keep_in_place: false,
            clips: Vec::new(),
        };
        for i in 0..4 {
            let mut clip = project.tracks[0].clips[0].clone();
            clip.id = format!("caption{i}");
            (clip.start_us, clip.duration_us) = (i * 1_000_000, 1_000_000);
            let transform = nuzky_engine::model::Transform { y: 0.15, ..Default::default() };
            clip.content = serde_json::from_value(serde_json::json!({"type": "text", "text": "two words", "style": crate::params::reel_style(), "transform": transform})).unwrap();
            captions.clips.push(clip);
        }
        project.tracks.push(captions);
        let path = dir.join("talk.nuzky");
        project.tracks.last_mut().unwrap().hidden = true;
        let hidden = project_evidence(&project, &path, &host.transcripts).unwrap().unwrap();
        assert!(
            hidden.picture.captions.is_empty() && hidden.picture.caption_band.is_none(),
            "hidden captions are not exported"
        );
        project.tracks.last_mut().unwrap().hidden = false;
        let evidence = project_evidence(&project, &path, &host.transcripts).unwrap().unwrap();
        let picture = &evidence.picture;
        assert_eq!(
            (picture.captions.len(), picture.captions[0].words, picture.caption_band),
            (4, 2, Some((0.65, 0.65)))
        );
        let zoom = picture.zooms[0];
        assert!(zoom.at_cut && zoom.from == 1.0 && zoom.to == 1.25, "{zoom:?}");
        assert!(picture.framing.iter().any(|f| f.scale == 1.25) && picture.framing.iter().any(|f| f.scale == 1.0));
        let times: Vec<i64> = evidence.corrections.iter().map(|c| c.time_us).collect();
        assert_eq!(times, [1_000_000, 1_500_000, 2_000_000]);
        let style = learning::learn(&[evidence.source()]);
        assert!(style.contains("## Spelling") && style.contains("| w00 | Nuzky | 1 |"), "{style}");
        let rule = learning::learned(&[evidence.source()]).rules.into_iter().find(|r| r.title == "Spelling").unwrap();
        assert_eq!(rule.summary, r#"Write "w00" as "Nuzky", "w01" as "Nuzky", "w02" as "Nuzky""#);
        drop(host);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_edit_md_from_before_keeps_the_creators_own_text() {
        let (store, dir) = store();
        let cli = learning::learn(&[evidence("talk", 120_000).source()]);
        let mine = cli
            .replacen("| Recording | Language", "Always keep my intro.\n\n| Recording | Language", 1)
            .replace("## Pauses\n", "## My brand\n\nSay Nuzky, never Nůžky.\n\n## Pauses\n");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("EDIT.md"), &mine).unwrap();
        let view = add(&store, evidence("talk", 400_000));
        assert_eq!(view.versions[0].label, "Found EDIT.md");
        assert!(view.suggestions.iter().all(|s| s.update), "{:?}", titles(&view));
        store.act(StyleAction::Accept { titles: titles(&view), seen: Vec::new() }).unwrap();
        let text = file(&dir).unwrap();
        assert!(
            text.contains("Always keep my intro.") && text.contains("## My brand\n\nSay Nuzky, never Nůžky.\n\n"),
            "{text}"
        );
        assert!(text.contains("| edit_transcript shorten_pauses_us | 400000 |"), "{text}");
        store.act(StyleAction::Remove { title: "Cuts".into() }).unwrap();
        assert!(file(&dir).unwrap().contains("## My brand\n\nSay Nuzky"), "{}", file(&dir).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_video_learned_again_is_the_newest_and_the_newest_12_stay() {
        let (store, dir) = store();
        for i in 0..12 {
            store.keep_evidence(evidence(&format!("take{i}"), 120_000)).unwrap();
        }
        store.keep_evidence(evidence("take0", 130_000)).unwrap();
        store.keep_evidence(evidence("take12", 120_000)).unwrap();
        let titles: Vec<String> = store.view().unwrap().sources.iter().map(|s| s.title.clone()).collect();
        assert_eq!(titles.len(), 12);
        assert_eq!(titles[..2], ["take12.mov and reel.mp4", "take0.mov and reel.mp4"]);
        assert!(!titles.contains(&"take1.mov and reel.mp4".to_owned()), "the one learned longest ago goes: {titles:?}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn evidence_read_back_learns_the_same_style() {
        let e = evidence("talk", 137_000);
        let back: Evidence = serde_json::from_slice(&serde_json::to_vec(&e).unwrap()).unwrap();
        assert_eq!(learning::learn(&[back.source()]), learning::learn(&[e.source()]));
    }
}
