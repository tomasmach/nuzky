//! Credits for the library sounds a video uses, as its description should carry them. CC BY asks for
//! the title, the author, the source, the licence and what was changed; syncing a sound to a video
//! is always a change. CC0 sounds need no credit and are listed after them as a courtesy; the sounds
//! built into Nuzky are left out.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::export::heard_clips;
use crate::model::{Asset, ClipContent, Credit, License, Project};

/// Under this a trim is the file's own rounding, not a cut.
const TRIM_TOLERANCE_US: i64 = 50_000;

/// What the video does to one sound.
#[derive(Default)]
struct Changes {
    trimmed: bool,
    faded: bool,
    volume: bool,
    speed: bool,
}

impl Changes {
    fn describe(&self) -> String {
        let done: Vec<&str> = [
            (self.trimmed, "trimmed"),
            (self.faded, "faded"),
            (self.volume, "volume changed"),
            (self.speed, "speed changed"),
        ]
        .into_iter()
        .filter_map(|(on, what)| on.then_some(what))
        .collect();
        if done.is_empty() { "Synced to video.".into() } else { format!("Synced to video; {}.", done.join(", ")) }
    }
}

fn line(credit: &Credit, changes: &Changes) -> String {
    let mut line = format!("\"{}\"", credit.title);
    if !credit.author.is_empty() {
        line += &format!(" by {}", credit.author);
    }
    if !credit.url.is_empty() {
        line += &format!(" ({})", credit.url);
    }
    line += &format!(", licensed under {} ({}).", credit.license_name(), credit.license_url);
    if credit.license == License::CcBy {
        line += &format!(" {}", changes.describe());
    }
    line
}

/// The credits of every library sound heard in the exported video, in the order they first play.
/// None when none of them needs credit.
pub fn credits(project: &Project) -> Option<String> {
    let mut order: Vec<&Asset> = Vec::new();
    let mut changes: HashMap<&str, Changes> = HashMap::new();
    let mut clips: Vec<_> = heard_clips(project).collect();
    clips.sort_by_key(|clip| clip.start_us);
    for clip in clips {
        let ClipContent::Media { asset_id, source_in_us, speed, volume, fade_in_us, fade_out_us, duck_db, .. } =
            &clip.content
        else {
            continue;
        };
        let Some(asset) = project.asset(asset_id) else { continue };
        if asset.credit.as_ref().is_none_or(|credit| credit.source == "nuzky") {
            continue;
        }
        if !changes.contains_key(asset_id.as_str()) {
            order.push(asset);
        }
        let entry = changes.entry(asset_id).or_default();
        let source_end = *source_in_us as f64 + clip.duration_us as f64 * f64::from(*speed);
        entry.trimmed |=
            *source_in_us > TRIM_TOLERANCE_US || source_end < (asset.duration_us - TRIM_TOLERANCE_US) as f64;
        entry.faded |= *fade_in_us > 0 || *fade_out_us > 0;
        entry.volume |= *volume != 1.0 || *duck_db != 0.0;
        entry.speed |= *speed != 1.0;
    }
    let lines = |license: License| -> Vec<String> {
        order
            .iter()
            .filter_map(|asset| Some((asset.credit.as_ref()?, &changes[asset.id.as_str()])))
            .filter(|(credit, _)| credit.license == license)
            .map(|(credit, changes)| line(credit, changes) + "\n")
            .collect()
    };
    let needed = lines(License::CcBy);
    if needed.is_empty() {
        return None;
    }
    let mut text = format!("Music and sound effects:\n{}", needed.concat());
    let courtesy = lines(License::Cc0);
    if !courtesy.is_empty() {
        text += &format!("\nAlso used, no credit required:\n{}", courtesy.concat());
    }
    Some(text)
}

/// The credits file written beside an export: `reel.mp4` gets `reel.credits.txt`.
pub fn credits_path(video: &Path) -> PathBuf {
    video.with_extension("credits.txt")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::EditCmd;
    use crate::model::AssetKind;

    fn sound(id: &str, license: License, source: &str) -> Asset {
        Asset {
            id: id.into(),
            name: format!("{id}.mp3"),
            path: format!("/sounds/{id}.mp3"),
            kind: AssetKind::Audio,
            duration_us: 10_000_000,
            width: 0,
            height: 0,
            fps: 0.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
            credit: Some(Credit {
                source: source.into(),
                id: id.into(),
                title: format!("Song {id}"),
                author: "Ann Author".into(),
                license,
                license_version: if license == License::Cc0 { "1.0".into() } else { "4.0".into() },
                license_url: if license == License::Cc0 {
                    "https://creativecommons.org/publicdomain/zero/1.0/".into()
                } else {
                    "https://creativecommons.org/licenses/by/4.0/".into()
                },
                url: format!("https://example.org/{id}"),
            }),
        }
    }

    fn project(assets: Vec<Asset>) -> Project {
        let mut project = Project::new("Credits");
        let ids: Vec<String> = assets.iter().map(|a| a.id.clone()).collect();
        project.apply(EditCmd::AddAssets { assets }).unwrap();
        for (i, id) in ids.iter().enumerate() {
            project
                .apply(EditCmd::AddClip { asset_id: id.clone(), start_us: Some(i as i64 * 20_000_000), track_id: None })
                .unwrap();
        }
        project
    }

    #[test]
    fn cc_by_sounds_are_credited_with_their_changes_and_cc0_ones_as_a_courtesy() {
        let mut project = project(vec![
            sound("by", License::CcBy, "openverse"),
            sound("zero", License::Cc0, "openverse"),
            sound("builtin", License::Cc0, "nuzky"),
        ]);
        let text = credits(&project).unwrap();
        assert_eq!(
            text,
            "Music and sound effects:\n\
             \"Song by\" by Ann Author (https://example.org/by), licensed under CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/). Synced to video.\n\
             \nAlso used, no credit required:\n\
             \"Song zero\" by Ann Author (https://example.org/zero), licensed under CC0 1.0 (https://creativecommons.org/publicdomain/zero/1.0/).\n"
        );
        // Trimmed and faded in the video, so the credit says so.
        for track in &mut project.tracks {
            for clip in &mut track.clips {
                if let ClipContent::Media { asset_id, fade_in_us, .. } = &mut clip.content
                    && asset_id == "by"
                {
                    clip.duration_us = 4_000_000;
                    *fade_in_us = 500_000;
                }
            }
        }
        assert!(credits(&project).unwrap().contains("Synced to video; trimmed, faded."));
    }

    #[test]
    fn only_heard_sounds_need_credit() {
        let mut project =
            project(vec![sound("by", License::CcBy, "openverse"), sound("zero", License::Cc0, "openverse")]);
        // CC0 alone needs nothing.
        project.tracks.iter_mut().for_each(|t| {
            t.clips.retain(|c| !matches!(&c.content, ClipContent::Media { asset_id, .. } if asset_id == "by"))
        });
        assert_eq!(credits(&project), None);
        // A CC BY sound in the project but muted is not in the video.
        let mut muted = self::project(vec![sound("by", License::CcBy, "openverse")]);
        muted.tracks.iter_mut().filter(|t| !t.clips.is_empty()).for_each(|t| t.muted = true);
        assert_eq!(credits(&muted), None);
    }

    #[test]
    fn credits_file_sits_beside_the_video() {
        assert_eq!(credits_path(Path::new("/v/reel.mp4")), Path::new("/v/reel.credits.txt"));
        assert_eq!(credits_path(Path::new("/v/my.trip.mp4")), Path::new("/v/my.trip.credits.txt"));
    }
}
