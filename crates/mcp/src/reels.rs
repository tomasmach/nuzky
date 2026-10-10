//! Reels from a long video: an agent proposes moments of the transcript, the user picks some, and each
//! picked one becomes a project of its own beside the source, playing the same media files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use nuzky_engine::{
    Project,
    edit::{EditCmd, MAIN_TRACK, TimeRange, new_id},
    model::{AssetKind, ClipContent, MAX_REEL_HOOK_CHARS, ReelCandidate, ReelStatus},
    speech::{TimelineWord, is_heard},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::transcript::{self, Derived};

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

/// The cut that keeps only the words `from` to `to` as edit_transcript keep does. That cut leaves clips without
/// speech alone, so those before and after the clips that play the kept words go too.
fn cut(project: &Project, derived: &Derived, from: usize, to: usize) -> Result<transcript::Cut> {
    let ranges =
        transcript::edit_ranges(project, derived, None, Some(&[[from, to]]), Some(transcript::DEFAULT_PAUSE_US))?;
    let cut = transcript::plan_cut(project, derived, ranges)?;
    let preview = &cut.preview;
    // A clip playing any part of a word, so a word split across two clips keeps both pieces.
    let speech: Vec<_> = preview
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter().filter(move |clip| is_heard(preview, track, clip)))
        .filter(|clip| {
            let ClipContent::Media { asset_id, source_in_us, speed, .. } = &clip.content else { return false };
            let end = *source_in_us as f64 + clip.duration_us as f64 * f64::from(*speed);
            let words = derived.sources.get(asset_id).map_or(&[][..], Vec::as_slice);
            words.iter().any(|w| (w.start_us as f64) < end && w.end_us > *source_in_us)
        })
        .collect();
    let (Some(start), Some(end)) = (speech.iter().map(|c| c.start_us).min(), speech.iter().map(|c| c.end_us()).max())
    else {
        return Ok(cut);
    };
    let edges: Vec<_> = [(0, start), (end, preview.duration_us())]
        .into_iter()
        .filter(|(start_us, end_us)| end_us > start_us)
        .map(|(start_us, end_us)| TimeRange { start_us, end_us })
        .collect();
    if edges.is_empty() {
        return Ok(cut);
    }
    let reel = Derived { sources: derived.sources.clone(), words: cut.words.clone(), ..Derived::default() };
    transcript::plan_cut(preview, &reel, edges)
}

/// Candidates for `proposals`, each inside the transcript, from the start of a sentence to the end of one, apart
/// from each other, and at most `max_duration_us` long once made. Made reels do not count: their words may have
/// moved since.
pub fn plan(
    project: &Project,
    derived: &Derived,
    proposals: &[ReelProposal],
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
    let mut all: Vec<&ReelCandidate> = candidates.iter().collect();
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
        ensure!(
            reel.duration_us() == candidate.duration_us,
            "SPEECH_CHANGED: the video changed since reel {id} was proposed; propose it again"
        );
        reel.name = candidate.title.clone();
        reel.reel_candidates.clear();
        reel.thumbnails.clear();
        frame(&mut reel, framing)?;
        nuzky_session::validate(&reel)?;
        planned.push((candidate, reel));
    }
    let dir = source.parent().context("INVALID_PROJECT: the project has no directory")?;
    let stem = source.file_stem().context("INVALID_PROJECT: the project has no name")?.to_string_lossy();
    // A made reel's file may have moved away, but its name still belongs to it.
    let named: HashSet<&str> = project.reel_candidates.iter().filter_map(|c| c.project_path.as_deref()).collect();
    let mut made = Vec::new();
    let mut n = 1;
    for (candidate, reel) in &planned {
        let path = loop {
            let path = dir.join(format!("{stem}-reel-{n}.nuzky"));
            n += 1;
            if nuzky_session::vacant(&path) && !named.contains(&*path.to_string_lossy()) {
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

/// A 9:16 canvas. Cropping scales every main-track picture, keyframes too, until it covers the frame, keeping its
/// own zoom; a wider picture then loses its sides evenly.
fn frame(reel: &mut Project, framing: Framing) -> Result<()> {
    let blur = if framing == Framing::Blur { BLUR } else { 0.0 };
    reel.apply(EditCmd::SetCanvas { width: WIDTH, height: HEIGHT, background: None, background_blur: Some(blur) })?;
    if framing != Framing::Crop {
        return Ok(());
    }
    let assets = reel.assets.clone();
    let main = reel.tracks.iter_mut().find(|t| t.id == MAIN_TRACK).context("INVALID_PROJECT: no main track")?;
    for clip in &mut main.clips {
        let ClipContent::Media { asset_id, transform, .. } = &mut clip.content else { continue };
        let Some(asset) = assets.iter().find(|a| &a.id == asset_id && a.kind != AssetKind::Audio) else { continue };
        if asset.width == 0 || asset.height == 0 {
            continue;
        }
        let (x, y) = (WIDTH as f32 / asset.width as f32, HEIGHT as f32 / asset.height as f32);
        let fill = x.max(y) / x.min(y);
        transform.scale *= fill;
        for key in &mut clip.keyframes {
            key.transform.scale *= fill;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuzky_engine::model::{Asset, Keyframe, Transform};

    /// B-roll before and after the take stays out of a reel, and cropping scales a moving picture too.
    #[test]
    fn a_reel_drops_the_clips_around_its_words_and_crops_keyframes() {
        let (mut project, mut sources) = transcript::tests::fixture();
        let broll = Asset {
            id: "broll".into(),
            name: "broll".into(),
            path: std::env::temp_dir().join("broll.mov").to_string_lossy().into(),
            duration_us: 6_000_000,
            width: 1920,
            height: 1080,
            ..project.assets[0].clone()
        };
        project.apply(EditCmd::AddAssets { assets: vec![broll] }).unwrap();
        project.apply(EditCmd::AddClip { asset_id: "broll".into(), start_us: Some(0), track_id: None }).unwrap();
        project.apply(EditCmd::AddClip { asset_id: "broll".into(), start_us: None, track_id: None }).unwrap();
        sources.insert("broll".into(), vec![]);
        // The take plays from 6 s to 16 s between the two pieces of B-roll, split inside word3 (9.5 to 9.9 s).
        let take = project.tracks[0].clips[1].id.clone();
        project.apply(EditCmd::SplitClip { clip_id: take, at_us: 9_600_000 }).unwrap();
        let derived = Derived::new(&project, sources, vec![]);
        let reel = cut(&project, &derived, 3, 5).unwrap().preview;
        let assets: Vec<_> = reel.tracks[0]
            .clips
            .iter()
            .map(|c| match &c.content {
                ClipContent::Media { asset_id, .. } => asset_id.as_str(),
                ClipContent::Text { .. } => "",
            })
            .collect();
        assert!(assets.iter().all(|a| *a == "talk"), "{assets:?}");
        // The words, 80 ms before and 120 ms after them, and two 600 ms pauses down to 300 ms.
        assert_eq!(reel.duration_us(), 2_400_000 + 80_000 + 120_000 - 2 * 300_000);

        let mut wide = project.clone();
        wide.tracks[0].clips[0].keyframes =
            vec![Keyframe { t_us: 0, transform: Transform::default(), ease: Default::default() }];
        frame(&mut wide, Framing::Crop).unwrap();
        let fill = (1920.0 / 1080.0) / (1080.0 / 1920.0);
        let clip = &wide.tracks[0].clips[0];
        let ClipContent::Media { transform, .. } = &clip.content else { panic!() };
        assert!((transform.scale - fill).abs() < 1e-4 && (clip.keyframes[0].transform.scale - fill).abs() < 1e-4);
    }
}
