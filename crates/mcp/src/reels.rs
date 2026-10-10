//! Reels from a long video: an agent proposes moments of the transcript, the user picks some, and each
//! picked one becomes a project of its own beside the source, playing the same media files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use nuzky_engine::{
    Project,
    edit::{EditCmd, MAIN_TRACK, new_id},
    model::{AssetKind, ClipContent, MAX_REEL_HOOK_CHARS, ReelCandidate, ReelStatus},
    speech::TimelineWord,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::transcript::{self, Derived};

/// Reels, TikTok and Shorts all take a minute.
pub const DEFAULT_MAX_DURATION_US: i64 = 60_000_000;
const WIDTH: u32 = 1080;
const HEIGHT: u32 = 1920;
/// The blur the inspector's Canvas: Blur starts with.
const BLUR: f32 = 0.5;

/// How a reel fills its 9:16 frame from a wider video.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename = "ReelFraming"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Framing {
    /// The middle of the picture fills the frame and its sides are cut off.
    #[default]
    Crop,
    /// The whole picture fits the frame, with a blurred copy of it behind.
    Blur,
}

/// A moment an agent proposes; the transcript gives the rest of its candidate.
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReelProposal {
    /// INCLUSIVE word range: from is the first word of a sentence, to the last word of one.
    pub from: usize,
    pub to: usize,
    /// Short, in the video's language, up to 100 characters.
    pub title: String,
    /// Why it works on its own, up to 300 characters.
    pub why: String,
    /// 0 to 1, higher is better.
    pub score: f32,
}

/// A reel project written beside its source.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MadeReel {
    pub id: String,
    pub path: PathBuf,
    pub duration_us: i64,
}

/// A full stop, question or exclamation mark, an ellipsis or a pause of 600 ms ends a sentence; a comma does
/// not, so a reel never stops halfway through one.
fn ends_sentence(words: &[TimelineWord], i: usize) -> bool {
    words[i].text.trim_end().ends_with(['.', '!', '?', '…'])
        || words.get(i + 1).is_none_or(|next| next.start_us - words[i].end_us >= transcript::SENTENCE_GAP_US)
}

/// The cut that keeps only the words `from` to `to`, exactly as edit_transcript keep does.
fn cut(project: &Project, derived: &Derived, from: usize, to: usize) -> Result<transcript::Cut> {
    let ranges =
        transcript::edit_ranges(project, derived, None, Some(&[[from, to]]), Some(transcript::DEFAULT_PAUSE_US))?;
    transcript::plan_cut(project, derived, ranges)
}

/// Candidates for `proposals`, each inside the transcript, from the start of a sentence to the end of one,
/// apart from the others and from `kept`, and at most `max_duration_us` long once made.
pub fn plan(
    project: &Project,
    derived: &Derived,
    proposals: &[ReelProposal],
    kept: &[ReelCandidate],
    max_duration_us: i64,
) -> Result<Vec<ReelCandidate>> {
    ensure!(derived.untranscribed.is_empty(), "TRANSCRIPT_MISSING: transcribe all heard assets before proposing reels");
    ensure!(!proposals.is_empty(), "INVALID_ARGUMENTS: propose at least one reel");
    ensure!(max_duration_us > 0, "INVALID_ARGUMENTS: max_duration_us must be positive");
    let words = &derived.words;
    let mut candidates = Vec::new();
    for p in proposals {
        let (from, to) = (p.from, p.to);
        ensure!(
            from <= to && to < words.len(),
            "REEL_OUTSIDE_TRANSCRIPT: words {from}-{to} are not in the transcript of {} words",
            words.len()
        );
        ensure!(from == 0 || ends_sentence(words, from - 1), "REEL_BOUNDARY: word {from} does not start a sentence");
        ensure!(ends_sentence(words, to), "REEL_BOUNDARY: word {to} {:?} does not end a sentence", words[to].text);
        let duration_us = cut(project, derived, from, to)?.preview.duration_us();
        ensure!(
            duration_us <= max_duration_us,
            "REEL_TOO_LONG: words {from}-{to} make a {:.1} s reel, longer than {:.1} s",
            duration_us as f64 / 1e6,
            max_duration_us as f64 / 1e6
        );
        let end = (from..=to).find(|&i| ends_sentence(words, i)).unwrap_or(to);
        let hook = words[from..=end].iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ");
        candidates.push(ReelCandidate {
            id: new_id(),
            from,
            to,
            start_us: words[from].start_us,
            end_us: words[to].end_us,
            title: p.title.trim().to_owned(),
            hook: hook.chars().take(MAX_REEL_HOOK_CHARS).collect(),
            why: p.why.trim().to_owned(),
            duration_us,
            score: p.score,
            status: ReelStatus::Proposed,
            project_path: None,
            thumbnail: None,
        });
    }
    let mut all: Vec<&ReelCandidate> = candidates.iter().chain(kept).collect();
    all.sort_by_key(|c| c.from);
    for pair in all.windows(2) {
        ensure!(
            pair[0].to < pair[1].from,
            "REEL_OVERLAP: words {}-{} and {}-{} overlap",
            pair[0].from,
            pair[0].to,
            pair[1].from,
            pair[1].to
        );
    }
    Ok(candidates)
}

/// Writes a project beside `source` for each candidate in `ids`: only its words, cut as edit_transcript keep
/// cuts, on a 9:16 canvas filled as `framing` says. Returns them with the edits that mark them made in the
/// source, which the caller applies; see [`forget`] when that fails.
pub fn make(
    source: &Path,
    project: &Project,
    derived: &Derived,
    ids: &[String],
    framing: Framing,
) -> Result<(Vec<MadeReel>, Vec<EditCmd>)> {
    ensure!(!ids.is_empty(), "INVALID_ARGUMENTS: give the ids of the reels to make");
    ensure!(derived.untranscribed.is_empty(), "TRANSCRIPT_MISSING: transcribe all heard assets before making reels");
    let mut seen = HashSet::new();
    let mut planned = Vec::new();
    for id in ids {
        ensure!(seen.insert(id), "INVALID_ARGUMENTS: reel {id} is listed twice");
        let candidate =
            project.reel_candidates.iter().find(|c| &c.id == id).with_context(|| format!("UNKNOWN_REEL: {id}"))?;
        if let Some(path) = &candidate.project_path {
            bail!("REEL_MADE: reel {id} is already made at {path}");
        }
        let words = &derived.words;
        ensure!(
            words.get(candidate.from).is_some_and(|w| w.start_us == candidate.start_us)
                && words.get(candidate.to).is_some_and(|w| w.end_us == candidate.end_us),
            "SPEECH_CHANGED: the video changed since reel {id} was proposed; propose it again"
        );
        let mut reel = cut(project, derived, candidate.from, candidate.to)?.preview;
        reel.name = candidate.title.clone();
        reel.reel_candidates.clear();
        reel.thumbnails.clear();
        frame(&mut reel, framing)?;
        nuzky_session::validate(&reel)?;
        planned.push((candidate, reel));
    }
    let dir = source.parent().context("INVALID_PROJECT: the project has no directory")?;
    let stem = source.file_stem().context("INVALID_PROJECT: the project has no name")?.to_string_lossy();
    let mut made = Vec::new();
    let mut n = 1;
    for (candidate, reel) in &planned {
        let path = loop {
            let path = dir.join(format!("{stem}-reel-{n}.nuzky"));
            n += 1;
            if nuzky_session::vacant(&path) {
                break path;
            }
        };
        if let Err(error) = nuzky_session::publish_new(&path, reel) {
            forget(&made, project);
            return Err(error);
        }
        made.push(MadeReel { id: candidate.id.clone(), path, duration_us: reel.duration_us() });
    }
    let edits = planned
        .iter()
        .zip(&made)
        .map(|((candidate, _), made)| EditCmd::UpdateReelCandidate {
            candidate: ReelCandidate {
                status: ReelStatus::Made,
                project_path: Some(made.path.to_string_lossy().into_owned()),
                ..(*candidate).clone()
            },
        })
        .collect();
    Ok((made, edits))
}

/// Removes the reels `make` wrote unless `source` names them: a failed save keeps the live edit, so its
/// projects stay, while any other failure leaves the source without them.
pub fn forget(made: &[MadeReel], source: &Project) -> bool {
    let named = |reel: &MadeReel| {
        source.reel_candidates.iter().any(|c| c.project_path.as_deref() == Some(&*reel.path.to_string_lossy()))
    };
    if made.iter().any(named) {
        return false;
    }
    for reel in made {
        let _ = std::fs::remove_file(&reel.path);
    }
    true
}

/// A 9:16 canvas. Cropping scales every main-track picture, except one that moves by keyframes, until it covers
/// the frame, keeping its own zoom; a wider picture then loses its sides evenly.
fn frame(reel: &mut Project, framing: Framing) -> Result<()> {
    let blur = if framing == Framing::Blur { BLUR } else { 0.0 };
    reel.apply(EditCmd::SetCanvas { width: WIDTH, height: HEIGHT, background: None, background_blur: Some(blur) })?;
    if framing != Framing::Crop {
        return Ok(());
    }
    let assets = reel.assets.clone();
    let main = reel.tracks.iter_mut().find(|t| t.id == MAIN_TRACK).context("INVALID_PROJECT: no main track")?;
    for clip in main.clips.iter_mut().filter(|c| c.keyframes.is_empty()) {
        let ClipContent::Media { asset_id, transform, .. } = &mut clip.content else { continue };
        let Some(asset) = assets.iter().find(|a| &a.id == asset_id && a.kind != AssetKind::Audio) else { continue };
        if asset.width == 0 || asset.height == 0 {
            continue;
        }
        let (x, y) = (WIDTH as f32 / asset.width as f32, HEIGHT as f32 / asset.height as f32);
        transform.scale *= x.max(y) / x.min(y);
    }
    Ok(())
}
