//! What an agent's run changed, in a few plain lines, from the project before and after it.
//! The panel shows them under the run, so the user sees exactly what happened without
//! trusting the agent's own account.

use nuzky_engine::Project;
use nuzky_engine::edit::MAIN_TRACK;
use nuzky_engine::model::{Clip, ClipContent, Crop, TrackKind};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunChange {
    pub text: String,
    /// Clips of the project after the run that the line is about, to select them.
    pub clip_ids: Vec<String>,
    /// Where on the timeline to look.
    pub at_us: Option<i64>,
}

fn line(text: String, clips: &[&Clip]) -> RunChange {
    RunChange {
        text,
        clip_ids: clips.iter().map(|c| c.id.clone()).collect(),
        at_us: clips.iter().map(|c| c.start_us).min(),
    }
}

fn secs(us: i64) -> String {
    format!("{:.1} s", us as f64 / 1e6)
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

fn quote(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = flat.chars().take(40).collect();
    if short.len() < flat.len() { format!("“{short}…”") } else { format!("“{short}”") }
}

/// Source time a media clip plays: [in, in + duration × speed).
fn source(clip: &Clip) -> Option<(&str, i64, i64)> {
    match &clip.content {
        ClipContent::Media { asset_id, source_in_us, speed, .. } => {
            Some((asset_id, *source_in_us, source_in_us + (clip.duration_us as f64 * *speed as f64).round() as i64))
        }
        ClipContent::Text { .. } => None,
    }
}

/// Merged source ranges each asset plays on the main track.
fn coverage(project: &Project) -> HashMap<String, Vec<(i64, i64)>> {
    let mut map: HashMap<String, Vec<(i64, i64)>> = HashMap::new();
    for clip in project.tracks.iter().filter(|t| t.id == MAIN_TRACK).flat_map(|t| &t.clips) {
        if let Some((asset, a, b)) = source(clip) {
            map.entry(asset.to_owned()).or_default().push((a, b));
        }
    }
    for ranges in map.values_mut() {
        ranges.sort();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for &(a, b) in ranges.iter() {
            match merged.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        *ranges = merged;
    }
    map
}

/// Parts of `before` not in `after`, ignoring slivers under a frame that rounding leaves.
fn removed(before: &[(i64, i64)], after: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for &(a, b) in before {
        let mut at = a;
        for &(c, d) in after.iter().filter(|(c, d)| *d > a && *c < b) {
            if c > at {
                out.push((at, c));
            }
            at = at.max(d);
        }
        if at < b {
            out.push((at, b));
        }
    }
    out.retain(|(a, b)| b - a >= 20_000);
    out
}

/// The clip of the earlier project that a clip of the later one comes from: the same clip, or
/// the one playing most of the same source (a split piece gets a new id).
fn origins<'a>(before: &'a Project, after: &'a Project) -> Vec<(&'a Clip, Option<&'a Clip>)> {
    let old: HashMap<&str, &Clip> = before.tracks.iter().flat_map(|t| &t.clips).map(|c| (c.id.as_str(), c)).collect();
    let media: Vec<&Clip> = before.tracks.iter().flat_map(|t| &t.clips).filter(|c| source(c).is_some()).collect();
    after
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .map(|clip| {
            let origin = old.get(clip.id.as_str()).copied().or_else(|| {
                let (asset, a, b) = source(clip)?;
                media
                    .iter()
                    .filter_map(|c| {
                        let (asset2, c0, c1) = source(c)?;
                        let overlap = b.min(c1) - a.max(c0);
                        (asset2 == asset && overlap > 0).then_some((overlap, *c))
                    })
                    .max_by_key(|(overlap, _)| *overlap)
                    .map(|(_, c)| c)
            });
            (clip, origin)
        })
        .collect()
}

fn scale(clip: &Clip) -> f32 {
    match &clip.content {
        ClipContent::Media { transform, .. } | ClipContent::Text { transform, .. } => transform.scale,
    }
}

fn crop(clip: &Clip) -> Option<Crop> {
    match &clip.content {
        ClipContent::Media { transform, .. } | ClipContent::Text { transform, .. } => transform.crop,
    }
}

pub fn summarize(before: &Project, after: &Project) -> Vec<RunChange> {
    let mut out = Vec::new();
    let asset_name =
        |id: &str| after.asset(id).or_else(|| before.asset(id)).map_or("a file".to_owned(), |a| a.name.clone());

    // Length.
    let (was, now) = (before.duration_us(), after.duration_us());
    if (was - now).abs() >= 50_000 {
        let verb = if now < was { "Shortened" } else { "Lengthened" };
        out.push(RunChange {
            text: format!("{verb} the video from {} to {}", secs(was), secs(now)),
            clip_ids: Vec::new(),
            at_us: None,
        });
    }

    // What was cut out of the main track, and takes that left it entirely.
    let (cov_before, cov_after) = (coverage(before), coverage(after));
    let mut gone_takes = Vec::new();
    let mut cut: Vec<(i64, i64)> = Vec::new();
    for (asset, ranges) in &cov_before {
        match cov_after.get(asset) {
            None => gone_takes.push(asset_name(asset)),
            Some(left) => cut.extend(removed(ranges, left)),
        }
    }
    if !cut.is_empty() {
        let total: i64 = cut.iter().map(|(a, b)| b - a).sum();
        out.push(RunChange {
            text: format!("Cut out {}, {} in total", count(cut.len(), "passage", "passages"), secs(total)),
            clip_ids: Vec::new(),
            at_us: None,
        });
    }
    gone_takes.sort();
    for name in gone_takes {
        out.push(RunChange { text: format!("Removed {name} from the main track"), clip_ids: Vec::new(), at_us: None });
    }

    let pairs = origins(before, after);
    let track_of: HashMap<&str, (&str, TrackKind, bool)> = after
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(move |c| (c.id.as_str(), (t.id.as_str(), t.kind, t.is_captions()))))
        .collect();

    // Media placed on the timeline that was not there before (split pieces are not new).
    let added: Vec<&Clip> = pairs.iter().filter(|(c, o)| o.is_none() && source(c).is_some()).map(|(c, _)| *c).collect();
    let mut by_asset: Vec<(String, Vec<&Clip>)> = Vec::new();
    for clip in added {
        let name = asset_name(source(clip).unwrap().0);
        match by_asset.iter_mut().find(|(n, _)| *n == name) {
            Some((_, clips)) => clips.push(clip),
            None => by_asset.push((name, vec![clip])),
        }
    }
    for (name, clips) in &by_asset {
        let place = match track_of.get(clips[0].id.as_str()) {
            Some((_, TrackKind::Audio, _)) => " on an audio track",
            Some((id, _, _)) if *id == MAIN_TRACK => "",
            _ => " as an overlay",
        };
        out.push(line(format!("Added {name}{place}"), clips));
    }

    // Captions and titles.
    let text_clips = |p: &'_ Project, captions: bool| -> Vec<Clip> {
        p.tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Text && t.is_captions() == captions)
            .flat_map(|t| t.clips.clone())
            .collect()
    };
    let (caps_before, caps_after) = (text_clips(before, true), text_clips(after, true));
    let new_caps: Vec<&Clip> = caps_after.iter().filter(|c| !caps_before.iter().any(|b| b.id == c.id)).collect();
    if caps_before.is_empty() && !caps_after.is_empty() {
        out.push(line(
            format!("Added {}", count(caps_after.len(), "caption", "captions")),
            &caps_after.iter().collect::<Vec<_>>(),
        ));
    } else if !caps_before.is_empty() && caps_after.is_empty() {
        out.push(RunChange { text: "Removed the captions".into(), clip_ids: Vec::new(), at_us: None });
    } else if !new_caps.is_empty() && new_caps.len() == caps_after.len() {
        out.push(line(
            format!("Made the captions again: {} instead of {}", caps_after.len(), caps_before.len()),
            &new_caps,
        ));
    } else {
        let style = |c: &Clip| match &c.content {
            ClipContent::Text { style, .. } => Some(style.clone()),
            ClipContent::Media { .. } => None,
        };
        let restyled: Vec<&Clip> =
            caps_after.iter().filter(|c| caps_before.iter().any(|b| b.id == c.id && style(b) != style(c))).collect();
        if !restyled.is_empty() {
            out.push(line(format!("Restyled {}", count(restyled.len(), "caption", "captions")), &restyled));
        }
    }
    let (titles_before, titles_after) = (text_clips(before, false), text_clips(after, false));
    let text_of = |c: &Clip| match &c.content {
        ClipContent::Text { text, .. } => text.clone(),
        ClipContent::Media { .. } => String::new(),
    };
    for t in &titles_after {
        match titles_before.iter().find(|b| b.id == t.id) {
            None => out.push(line(format!("Added the text {}", quote(&text_of(t))), &[t])),
            Some(b) if text_of(b) != text_of(t) => {
                out.push(line(format!("Changed a text to {}", quote(&text_of(t))), &[t]))
            }
            _ => {}
        }
    }
    for b in titles_before.iter().filter(|b| !titles_after.iter().any(|t| t.id == b.id)) {
        out.push(RunChange {
            text: format!("Removed the text {}", quote(&text_of(b))),
            clip_ids: Vec::new(),
            at_us: None,
        });
    }

    // Changes to clips that were already there, grouped by what changed.
    let mut zoomed: Vec<(&Clip, f32)> = Vec::new();
    let mut grouped: Vec<(&'static str, Vec<&Clip>)> = Vec::new();
    for &(clip, origin) in &pairs {
        let Some(o) = origin else { continue };
        let mut push = |what: &'static str| match grouped.iter_mut().find(|(w, _)| *w == what) {
            Some((_, clips)) => clips.push(clip),
            None => grouped.push((what, vec![clip])),
        };
        if let (
            ClipContent::Media {
                volume: v1,
                speed: s1,
                keep_pitch: k1,
                adjust: a1,
                fade_in_us: fi1,
                fade_out_us: fo1,
                clean_voice: cv1,
                shape: sh1,
                duck_db: d1,
                ..
            },
            ClipContent::Media {
                volume: v2,
                speed: s2,
                keep_pitch: k2,
                adjust: a2,
                fade_in_us: fi2,
                fade_out_us: fo2,
                clean_voice: cv2,
                shape: sh2,
                duck_db: d2,
                ..
            },
        ) = (&o.content, &clip.content)
        {
            if (scale(clip) - scale(o)).abs() > 0.005 && clip.keyframes == o.keyframes {
                if scale(clip) > scale(o) {
                    zoomed.push((clip, scale(clip) / scale(o).max(0.01)));
                } else {
                    push("Zoomed out on");
                }
            }
            if v1 != v2 {
                push("Changed the volume of");
            }
            if s1 != s2 {
                push("Changed the speed of");
            } else if k1 != k2 {
                push(if *k2 { "Turned on Keep pitch for" } else { "Turned off Keep pitch for" });
            }
            if a1 != a2 {
                push("Changed the colour of");
            }
            if sh1 != sh2 || (crop(clip) != crop(o) && clip.keyframes == o.keyframes) {
                push("Changed the crop or shape of");
            }
            if (fi1, fo1) != (fi2, fo2) {
                push("Changed the sound fades of");
            }
            if !cv1 && *cv2 {
                push("Turned on Clean voice for");
            } else if *cv1 && !cv2 {
                push("Turned off Clean voice for");
            }
            if d1 != d2 {
                push(match (*d1 > 0.0, *d2 > 0.0) {
                    (false, true) => "Turned on Lower under speech for",
                    (true, false) => "Turned off Lower under speech for",
                    _ => "Changed how much speech lowers",
                });
            }
        }
        if clip.keyframes != o.keyframes {
            push(if clip.keyframes.is_empty() { "Removed the motion of" } else { "Added motion to" });
        }
        if (&clip.anim_in, &clip.anim_out) != (&o.anim_in, &o.anim_out) {
            push("Changed the animations of");
        }
        if clip.transition_in != o.transition_in && clip.id == o.id {
            push(if clip.transition_in.is_some() { "Added transitions to" } else { "Removed transitions from" });
        }
    }
    if !zoomed.is_empty() {
        let lo = zoomed.iter().map(|(_, f)| *f).fold(f32::MAX, f32::min);
        let hi = zoomed.iter().map(|(_, f)| *f).fold(0.0, f32::max);
        let factor = if hi - lo < 0.01 { format!("{lo:.2}×") } else { format!("{lo:.2}–{hi:.2}×") };
        let clips: Vec<&Clip> = zoomed.iter().map(|(c, _)| *c).collect();
        out.push(line(format!("Zoomed in on {} ({factor})", count(clips.len(), "place", "places")), &clips));
    }
    // Clips split in two where nothing was cut or zoomed: a new piece of a clip, playing its own part of it.
    if cut.is_empty() && zoomed.is_empty() {
        let pieces: Vec<&Clip> = pairs
            .iter()
            .filter(|(c, o)| o.is_some_and(|o| o.id != c.id))
            .filter_map(|(c, _)| {
                let (asset, a, b) = source(c)?;
                let alone = pairs
                    .iter()
                    .all(|(d, _)| d.id == c.id || source(d).is_none_or(|(x, c0, c1)| x != asset || c1 <= a || c0 >= b));
                alone.then_some(*c)
            })
            .collect();
        for piece in pieces.iter().take(3) {
            let name = asset_name(source(piece).map_or("", |s| s.0));
            out.push(line(format!("Split {name}"), &[*piece]));
        }
        if pieces.len() > 3 {
            out.push(line(format!("Split clips in {} more places", pieces.len() - 3), &pieces[3..]));
        }
    }
    for (what, clips) in grouped {
        let unique: HashSet<&str> = clips.iter().map(|c| c.id.as_str()).collect();
        let noun = if what.contains("transition") {
            count(unique.len(), "cut", "cuts")
        } else {
            count(unique.len(), "clip", "clips")
        };
        out.push(line(format!("{what} {noun}"), &clips));
    }

    // Tracks, canvas and the rest of the project.
    for t in &after.tracks {
        if let Some(b) = before.tracks.iter().find(|b| b.id == t.id) {
            let name = if t.name.is_empty() { "a track".to_owned() } else { format!("the {} track", t.name) };
            if b.muted != t.muted {
                out.push(RunChange {
                    text: format!("{} {name}", if t.muted { "Muted" } else { "Unmuted" }),
                    clip_ids: Vec::new(),
                    at_us: None,
                });
            }
            if b.hidden != t.hidden {
                out.push(RunChange {
                    text: format!("{} {name}", if t.hidden { "Hid" } else { "Showed" }),
                    clip_ids: Vec::new(),
                    at_us: None,
                });
            }
        }
    }
    let (c1, c2) = (&before.canvas, &after.canvas);
    if (c1.width, c1.height) != (c2.width, c2.height) {
        out.push(RunChange {
            text: format!("Changed the format to {} × {}", c2.width, c2.height),
            clip_ids: Vec::new(),
            at_us: None,
        });
    }
    if c1.fps != c2.fps {
        out.push(RunChange {
            text: format!("Changed the frame rate to {} fps", c2.fps),
            clip_ids: Vec::new(),
            at_us: None,
        });
    }
    if c1.background != c2.background || c1.background_blur != c2.background_blur {
        out.push(RunChange { text: "Changed the background".into(), clip_ids: Vec::new(), at_us: None });
    }
    let corrected = after.word_corrections.len() as i64 - before.word_corrections.len() as i64;
    if corrected > 0 {
        out.push(RunChange {
            text: format!("Corrected {}", count(corrected as usize, "word", "words")),
            clip_ids: Vec::new(),
            at_us: None,
        });
    } else if after.word_corrections != before.word_corrections {
        out.push(RunChange { text: "Changed word corrections".into(), clip_ids: Vec::new(), at_us: None });
    }
    let on_timeline: HashSet<&str> =
        after.tracks.iter().flat_map(|t| &t.clips).filter_map(|c| source(c).map(|s| s.0)).collect();
    for a in after.assets.iter().filter(|a| before.asset(&a.id).is_none() && !on_timeline.contains(a.id.as_str())) {
        out.push(RunChange {
            text: format!("Imported {} into the library", a.name),
            clip_ids: Vec::new(),
            at_us: None,
        });
    }
    if before.name != after.name {
        out.push(RunChange {
            text: format!("Renamed the project to {}", quote(&after.name)),
            clip_ids: Vec::new(),
            at_us: None,
        });
    }

    // Anything else still says that something changed.
    if out.is_empty() && before != after {
        out.push(RunChange { text: "Changed the project".into(), clip_ids: Vec::new(), at_us: None });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuzky_engine::edit::{EditCmd, Editor};
    use serde_json::json;

    fn editor() -> Editor {
        let mut e = Editor::new(Project::new("t"));
        let asset = |id: &str, name: &str, kind: &str, secs: i64| {
            json!({"id": id, "name": name, "path": format!("/m/{name}"), "kind": kind, "durationUs": secs * 1_000_000,
                   "width": 1080, "height": 1920, "fps": 30.0, "hasAudio": true, "rotation": 0, "mirror": false})
        };
        apply(
            &mut e,
            json!({"type": "addAssets", "assets": [asset("t1", "take-1.mp4", "video", 20), asset("m", "lofi.mp3", "audio", 60)]}),
        );
        apply(&mut e, json!({"type": "addClip", "assetId": "t1", "startUs": 0, "trackId": "main"}));
        e
    }

    fn apply(e: &mut Editor, cmd: serde_json::Value) {
        e.apply(serde_json::from_value::<EditCmd>(cmd).unwrap(), None).unwrap();
    }

    fn texts(changes: &[RunChange]) -> Vec<&str> {
        changes.iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn a_rough_cut_says_what_was_cut_captioned_and_zoomed() {
        let mut e = editor();
        let before = e.project.clone();
        apply(
            &mut e,
            json!({"type": "rippleDeleteRanges", "ranges": [{"startUs": 2_000_000, "endUs": 4_000_000}, {"startUs": 10_000_000, "endUs": 12_500_000}]}),
        );
        let style = json!({"fontSize": 95.0, "color": "#ffffff", "strokeWidth": 7.5});
        let seg = |a: i64, b: i64, t: &str| json!({"startUs": a, "endUs": b, "text": t});
        apply(
            &mut e,
            json!({"type": "addCaptions", "style": style, "segments": [seg(0, 1_000_000, "dneska vám"), seg(1_000_000, 2_000_000, "ukážu jak"), seg(2_000_000, 3_000_000, "natočit video")]}),
        );
        apply(
            &mut e,
            json!({"type": "zoomRanges", "ranges": [{"startUs": 5_000_000, "endUs": 8_000_000, "scale": 1.2}]}),
        );

        let changes = summarize(&before, &e.project);
        assert_eq!(
            texts(&changes),
            [
                "Shortened the video from 20.0 s to 15.5 s",
                "Cut out 2 passages, 4.5 s in total",
                "Added 3 captions",
                "Zoomed in on 1 place (1.20×)"
            ]
        );
        // Split pieces of the take are not new media, and each line points at its clips.
        assert_eq!(changes[2].clip_ids.len(), 3);
        assert_eq!(changes[2].at_us, Some(0));
        let zoomed = &e.project.tracks[0].clips.iter().find(|c| c.id == changes[3].clip_ids[0]).unwrap();
        assert_eq!(changes[3].at_us, Some(zoomed.start_us));
        assert!(scale(zoomed) > 1.1);
    }

    #[test]
    fn added_music_and_a_volume_change_name_the_file_and_clip() {
        let mut e = editor();
        let before = e.project.clone();
        apply(&mut e, json!({"type": "addClip", "assetId": "m", "startUs": 0}));
        let music = e.project.tracks.iter().find(|t| t.kind == TrackKind::Audio).unwrap().clips[0].id.clone();
        apply(&mut e, json!({"type": "updateClip", "clipId": music, "volume": 0.3}));
        let take = e.project.tracks[0].clips[0].id.clone();
        apply(&mut e, json!({"type": "updateClip", "clipId": take, "cleanVoice": true}));

        let changes = summarize(&before, &e.project);
        assert_eq!(texts(&changes), ["Added lofi.mp3 on an audio track", "Turned on Clean voice for 1 clip"]);
        assert_eq!(changes[0].clip_ids, [music.as_str()]);
        assert_eq!(changes[1].clip_ids, [take]);
        let before = e.project.clone();
        apply(&mut e, json!({"type": "updateClip", "clipId": music, "duckDb": 12}));
        assert_eq!(texts(&summarize(&before, &e.project)), ["Turned on Lower under speech for 1 clip"]);
    }

    #[test]
    fn a_split_says_where() {
        let mut e = editor();
        let before = e.project.clone();
        let take = e.project.tracks[0].clips[0].id.clone();
        apply(&mut e, json!({"type": "splitClip", "clipId": take, "atUs": 5_000_000}));
        let changes = summarize(&before, &e.project);
        assert_eq!(texts(&changes), ["Split take-1.mp4"]);
        assert_eq!(changes[0].at_us, Some(5_000_000));
    }

    #[test]
    fn an_unchanged_project_has_no_lines() {
        let e = editor();
        assert!(summarize(&e.project, &e.project).is_empty());
    }
}
