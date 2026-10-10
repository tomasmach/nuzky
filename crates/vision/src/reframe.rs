//! Reframe: the canvas in a new shape, with the picture of each video and image clip placed to keep the
//! speaker's face in it. The face is looked for twice a second in the clip as recorded; the picture holds still
//! while the face stays near where it was put and catches up on a smooth curve once it leaves. The result is
//! plain keyframes, so a cut jumps to the next clip's framing and everything stays editable.
use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, ensure};
use nuzky_engine::{
    Project, Renderer, Wait,
    edit::{EditCmd, MAIN_TRACK},
    model::{Asset, AssetKind, Background, Canvas, Clip, ClipContent, Crop, Ease, Keyframe, TrackKind, Transform},
};

use crate::frame::Frame;
use crate::models;
use crate::nets::Yunet;
use crate::thumbnails::in_parallel;

/// One look every half second; a longer timeline is looked at more sparsely instead of for longer.
const LOOK_EVERY_US: i64 = 500_000;
const MAX_LOOKS: usize = 1_200;
/// Long side of a look; YuNet finds faces down to about 10 px there.
const LOOK_SIDE: u32 = 512;
/// How far the face may move, as a share of the canvas across and down, before the picture follows it.
const DEAD_ZONE: [f32; 2] = [0.08, 0.06];
/// How long the picture takes to catch up with the face.
const PAN_US: i64 = 600_000;

/// What a reframe changes, as one edit: the canvas, and the transform and keyframes of each clip.
pub struct Reframe {
    pub commands: Vec<EditCmd>,
    /// Clips placed to follow a face.
    pub followed: Vec<String>,
    /// Clips without a face anywhere, centred.
    pub centred: Vec<String>,
}

/// A face in a look, in fractions of the picture.
#[derive(Clone, Copy, Debug)]
struct Face {
    at: [f32; 2],
    area: f32,
}

/// Changes the canvas to `width`×`height` and places the clips `clip_ids`, by default every video and image clip of
/// the main track and any other that filled the canvas, to follow the face. A clip's zoom beyond filling the canvas
/// stays, and so do its rotation, opacity and crop; its keyframes are replaced.
pub fn reframe(
    project: &Project,
    models_dir: &Path,
    (width, height): (u32, u32),
    clip_ids: Option<&[String]>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Reframe> {
    models::require(models::REFRAME, models_dir)?;
    let clips = targets(project, clip_ids)?;
    let mut canvas = project.canvas.clone();
    (canvas.width, canvas.height) = (width & !1, height & !1);
    let total: i64 = clips.iter().map(|(clip, _)| clip.duration_us).sum();
    let every = LOOK_EVERY_US.max(total / MAX_LOOKS as i64);
    let solos: Vec<_> = clips.iter().map(|(clip, asset)| solo(project, clip, asset)).collect();
    let looks: Vec<(usize, i64)> = clips
        .iter()
        .enumerate()
        .flat_map(|(i, (clip, asset))| times(clip, asset, every).into_iter().map(move |u| (i, u)))
        .collect();
    let found = in_parallel(
        &looks,
        cancel,
        progress,
        || Ok((Renderer::new().context("Starting frame renderer")?, Yunet::load(models_dir)?)),
        |(renderer, yunet), &(i, u), cancel| {
            let (solo, (w, h)) = &solos[i];
            let rgba = renderer.render(solo, u, *w, *h, Wait::Exact, false).context("Rendering a clip")?;
            let (fw, fh) = (*w as f32, *h as f32);
            Ok(yunet
                .detect(&Frame { width: *w, height: *h, rgba }, cancel)?
                .iter()
                .map(|d| Face {
                    at: [(d.rect[0] + d.rect[2] / 2.0) / fw, (d.rect[1] + d.rect[3] / 2.0) / fh],
                    area: d.rect[2] * d.rect[3] / (fw * fh),
                })
                .collect::<Vec<_>>())
        },
    )?;
    let mut out = Reframe {
        commands: vec![EditCmd::SetCanvas { width, height, background: None, background_blur: None }],
        followed: Vec::new(),
        centred: Vec::new(),
    };
    for (i, (clip, asset)) in clips.iter().enumerate() {
        let (times, faces): (Vec<i64>, Vec<Vec<Face>>) =
            looks.iter().zip(&found).filter(|((c, _), _)| *c == i).map(|((_, u), f)| (*u, f.clone())).unzip();
        let base = base(clip);
        let placement = Placement::new(&project.canvas, &canvas, asset, &base);
        let track = smooth(&follow(&faces));
        let positions: Vec<[f32; 2]> = match &track {
            Some(track) => track.iter().map(|&face| placement.position(face, placement.aim)).collect(),
            None => vec![placement.centred()],
        };
        let at = |[x, y]: [f32; 2]| Transform { x, y, scale: placement.scale, ..base };
        out.commands.push(update(&clip.id, at(positions[0])));
        out.commands.push(EditCmd::SetKeyframes {
            clip_id: clip.id.clone(),
            keyframes: keys(&times, &positions)
                .into_iter()
                .map(|(t_us, p)| Keyframe { t_us, transform: at(p), ease: Ease::Smooth })
                .collect(),
        });
        match track {
            Some(_) => out.followed.push(clip.id.clone()),
            None => out.centred.push(clip.id.clone()),
        }
    }
    Ok(out)
}

/// The clips to reframe with their pictures.
fn targets<'a>(project: &'a Project, clip_ids: Option<&[String]>) -> Result<Vec<(&'a Clip, &'a Asset)>> {
    let pictured = |clip: &'a Clip| match &clip.content {
        ClipContent::Media { asset_id, .. } => project.asset(asset_id).filter(|a| a.kind != AssetKind::Audio),
        ClipContent::Text { .. } => None,
    };
    let video = project.tracks.iter().filter(|t| t.kind == TrackKind::Video);
    let found: Vec<_> = match clip_ids {
        Some(ids) => {
            for id in ids {
                ensure!(
                    video.clone().flat_map(|t| &t.clips).any(|c| &c.id == id && pictured(c).is_some()),
                    "INVALID_ARGUMENTS: {id} is not a video or image clip"
                );
            }
            video
                .flat_map(|t| &t.clips)
                .filter(|c| ids.contains(&c.id))
                .filter_map(|c| Some((c, pictured(c)?)))
                .collect()
        }
        None => video
            .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
            .filter_map(|(t, c)| Some((t, c, pictured(c)?)))
            .filter(|(t, c, asset)| t.id == MAIN_TRACK || (!t.hidden && fills(&project.canvas, asset, &base(c))))
            .map(|(_, c, asset)| (c, asset))
            .collect(),
    };
    ensure!(!found.is_empty(), "INVALID_ARGUMENTS: there is no video or image clip to reframe");
    Ok(found)
}

/// The clip's own framing: its first keyframe when it has keyframes, which replace its transform.
fn base(clip: &Clip) -> Transform {
    match (&clip.content, clip.keyframes.first()) {
        (_, Some(key)) => key.transform,
        (ClipContent::Media { transform, .. } | ClipContent::Text { transform, .. }, None) => *transform,
    }
}

/// The clip alone on a canvas of its picture's shape, as recorded: no framing, animation or background.
fn solo(project: &Project, clip: &Clip, asset: &Asset) -> (Project, (u32, u32)) {
    let k = LOOK_SIDE as f32 / asset.width.max(asset.height).max(1) as f32;
    let even = |v: u32| ((v as f32 * k).round() as u32).max(16) & !1;
    let size = (even(asset.width), even(asset.height));
    let mut solo = Project::new("reframe");
    solo.canvas = Canvas { width: size.0, height: size.1, background_blur: 0.0, ..project.canvas.clone() };
    solo.assets = vec![asset.clone()];
    let mut clip =
        Clip { start_us: 0, anim_in: None, anim_out: None, keyframes: Vec::new(), transition_in: None, ..clip.clone() };
    if let ClipContent::Media { transform, shape, background, .. } = &mut clip.content {
        *transform = Transform::default();
        *shape = None;
        *background = Background::None;
    }
    solo.tracks[0].clips = vec![clip];
    (solo, size)
}

/// When the clip is looked at, from its start: every `every`, or once for a still image.
fn times(clip: &Clip, asset: &Asset, every: i64) -> Vec<i64> {
    let times: Vec<i64> = match asset.kind {
        AssetKind::Image => Vec::new(),
        _ => (0..).map(|i| every / 2 + i * every).take_while(|&u| u < clip.duration_us).collect(),
    };
    if times.is_empty() { vec![clip.duration_us / 2] } else { times }
}

/// Where a clip's picture can sit on a canvas.
struct Placement {
    canvas: [f32; 2],
    /// The picture's size on the canvas at `scale`, in pixels.
    size: [f32; 2],
    /// Left, top, right and bottom edge of the part the crop leaves, in fractions of the picture.
    visible: [f32; 4],
    scale: f32,
    /// Where the face goes, in fractions of the canvas.
    aim: [f32; 2],
}

impl Placement {
    /// On `canvas`, scaled so the visible part of the picture fills it, times the zoom it had beyond filling
    /// `before`.
    fn new(before: &Canvas, canvas: &Canvas, asset: &Asset, base: &Transform) -> Placement {
        let visible = Crop::visible(base.crop);
        let zoom = (base.scale / filling(before, asset, visible)).max(1.0);
        let scale = filling(canvas, asset, visible) * zoom;
        Placement {
            canvas: [canvas.width as f32, canvas.height as f32],
            size: fitted(canvas, asset).map(|v| v * scale),
            visible,
            scale,
            aim: aim(canvas),
        }
    }

    /// The transform position that puts the point `at` of the picture on the point `on` of the canvas, both in
    /// fractions, or as near as it can while the picture covers the canvas.
    fn position(&self, at: [f32; 2], on: [f32; 2]) -> [f32; 2] {
        [0, 1].map(|i| {
            let (low, high) = covering(self.canvas[i], self.size[i], self.visible[i], self.visible[i + 2]);
            // When the picture just fills the canvas, rounding can put `low` a hair above `high`.
            (on[i] - 0.5 - (at[i] - 0.5) * self.size[i] / self.canvas[i]).max(low).min(high)
        })
    }

    /// The visible part of the picture in the middle of the canvas.
    fn centred(&self) -> [f32; 2] {
        let [left, top, right, bottom] = self.visible;
        self.position([(left + right) / 2.0, (top + bottom) / 2.0], [0.5, 0.5])
    }
}

/// The picture's size on the canvas at scale 1, which fits it inside.
fn fitted(canvas: &Canvas, asset: &Asset) -> [f32; 2] {
    let (w, h) = (asset.width.max(1) as f32, asset.height.max(1) as f32);
    let fit = (canvas.width as f32 / w).min(canvas.height as f32 / h);
    [w * fit, h * fit]
}

/// The scale at which the `visible` part of the picture just covers the canvas.
fn filling(canvas: &Canvas, asset: &Asset, visible: [f32; 4]) -> f32 {
    let [w, h] = fitted(canvas, asset);
    let [left, top, right, bottom] = visible;
    (canvas.width as f32 / (w * (right - left)).max(1e-3)).max(canvas.height as f32 / (h * (bottom - top)).max(1e-3))
}

/// The positions, as transform x or y, at which a picture `size` pixels long, visible from `from` to `to` of its
/// length, covers a canvas side `side` pixels long.
fn covering(side: f32, size: f32, from: f32, to: f32) -> (f32, f32) {
    (0.5 - (to - 0.5) * size / side, -0.5 - (from - 0.5) * size / side)
}

/// Whether the clip's visible picture covers the whole canvas, as a clip that fills the frame does and a picture
/// in picture does not.
fn fills(canvas: &Canvas, asset: &Asset, t: &Transform) -> bool {
    let visible = Crop::visible(t.crop);
    let size = fitted(canvas, asset).map(|v| v * t.scale);
    let side = [canvas.width as f32, canvas.height as f32];
    t.rotation.rem_euclid(360.0) == 0.0
        && [t.x, t.y].into_iter().enumerate().all(|(i, at)| {
            let (low, high) = covering(side[i], size[i], visible[i], visible[i + 2]);
            let slack = 1.0 / side[i];
            low - slack <= at && at <= high + slack
        })
}

/// Where the face goes on the canvas, in fractions of it: across the middle of what Reels and TikTok leave free on
/// a vertical canvas, else of the canvas, and 40 % of the way down that, so the head keeps room above and stays
/// clear of captions, which sit below the middle.
fn aim(canvas: &Canvas) -> [f32; 2] {
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    let (left, top, right, bottom) =
        canvas.safe_area().map_or((0.0, 0.0, w, h), |area| (area.left, area.top, area.right, area.bottom));
    [(left + right) / 2.0 / w, (top + 0.4 * (bottom - top)) / h]
}

/// The face to follow in each look: the largest, or one nearly as large near the one followed last, so another
/// face does not take over for a moment. None where no face was found.
fn follow(looks: &[Vec<Face>]) -> Vec<Option<[f32; 2]>> {
    let mut last: Option<[f32; 2]> = None;
    looks
        .iter()
        .map(|faces| {
            let weight = |f: &Face| {
                let near = last.is_some_and(|l| (l[0] - f.at[0]).hypot(l[1] - f.at[1]) < 0.1);
                f.area * if near { 1.5 } else { 1.0 }
            };
            let face = faces.iter().max_by(|a, b| weight(a).total_cmp(&weight(b)))?.at;
            last = Some(face);
            Some(face)
        })
        .collect()
}

/// The followed face in every look: where it was not found, where it was last (or first) seen, and each value the
/// median of five looks around it, so one wrong detection never moves the picture. None without any face.
fn smooth(track: &[Option<[f32; 2]>]) -> Option<Vec<[f32; 2]>> {
    let mut seen = *track.iter().flatten().next()?;
    let held: Vec<[f32; 2]> = track
        .iter()
        .map(|face| {
            seen = face.unwrap_or(seen);
            seen
        })
        .collect();
    Some(
        (0..held.len())
            .map(|i| {
                let window = &held[i.saturating_sub(2)..(i + 3).min(held.len())];
                [0, 1].map(|axis| {
                    let mut values: Vec<f32> = window.iter().map(|p| p[axis]).collect();
                    values.sort_by(f32::total_cmp);
                    values[values.len() / 2]
                })
            })
            .collect(),
    )
}

/// Keyframes, in clip time, that hold the picture while the face stays within the dead zone around where it was
/// put and move it on one smooth curve once it leaves: over `PAN_US` up to the look that found it gone, and on to
/// the last look of those that find it still going. None when it never moves.
fn keys(times: &[i64], positions: &[[f32; 2]]) -> Vec<(i64, [f32; 2])> {
    let mut held = positions[0];
    let mut keys: Vec<(i64, [f32; 2])> = Vec::new();
    let mut previous = None;
    for (&t, &at) in times.iter().zip(positions) {
        if (at[0] - held[0]).abs() > DEAD_ZONE[0] || (at[1] - held[1]).abs() > DEAD_ZONE[1] {
            if keys.last().is_some_and(|k| Some(k.0) == previous) {
                // It went on since the last look: the same move goes on to here, instead of stopping on the way.
                keys.pop();
            } else {
                let from = (t - PAN_US).max(keys.last().map_or(0, |k| k.0));
                if keys.last().is_none_or(|k| k.0 < from) {
                    keys.push((from, held));
                }
            }
            keys.push((t, at));
            held = at;
        }
        previous = Some(t);
    }
    keys
}

fn update(clip_id: &str, transform: Transform) -> EditCmd {
    EditCmd::UpdateClip {
        clip_id: clip_id.into(),
        transform: Some(transform),
        volume: None,
        text: None,
        style: None,
        speed: None,
        keep_pitch: None,
        adjust: None,
        fade_in_us: None,
        fade_out_us: None,
        clean_voice: None,
        shape: None,
        duck_db: None,
        background: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(width: u32, height: u32) -> Canvas {
        Canvas { width, height, ..Project::new("reframe").canvas }
    }

    fn asset(width: u32, height: u32) -> Asset {
        Asset {
            id: "a".into(),
            name: "a.mp4".into(),
            path: "a.mp4".into(),
            kind: AssetKind::Video,
            duration_us: 10_000_000,
            width,
            height,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
            mirror: false,
            credit: None,
        }
    }

    /// Where the point `at` of the picture lands on the canvas, in pixels, as the renderer places it.
    fn landing(p: &Placement, [x, y]: [f32; 2], at: [f32; 2]) -> [f32; 2] {
        let centre = [p.canvas[0] * (0.5 + x), p.canvas[1] * (0.5 + y)];
        [0, 1].map(|i| centre[i] + (at[i] - 0.5) * p.size[i])
    }

    #[test]
    fn a_wide_picture_fills_a_vertical_canvas_with_the_face_in_the_safe_area_and_never_shows_an_edge() {
        let (wide, tall) = (canvas(1920, 1080), canvas(1080, 1920));
        let p = Placement::new(&wide, &tall, &asset(1920, 1080), &Transform::default());
        assert!((p.scale - 1920.0 / 607.5).abs() < 1e-4, "{}", p.scale);
        // A face a third of the way across lands in the middle of what Reels leaves free, 480 px across.
        let at = p.position([1.0 / 3.0, 0.4], p.aim);
        assert!((landing(&p, at, [1.0 / 3.0, 0.4])[0] - 480.0).abs() < 0.5, "{at:?}");
        assert!(at[1].abs() < 1e-5, "the picture is exactly as tall as the canvas: {at:?}");
        // A face at the very edge: the picture stops at its own edge.
        let edge = p.position([0.02, 0.4], p.aim);
        assert!(landing(&p, edge, [0.0, 0.0])[0].abs() < 0.01, "{edge:?}");
        // A punch-in on the wide canvas stays a punch-in; doing it again changes nothing.
        let zoomed = Transform { scale: 1.2, ..Transform::default() };
        let p = Placement::new(&wide, &tall, &asset(1920, 1080), &zoomed);
        assert!((p.scale - 1.2 * 1920.0 / 607.5).abs() < 1e-3, "{}", p.scale);
        let again = Transform { scale: p.scale, ..Transform::default() };
        assert!((Placement::new(&tall, &tall, &asset(1920, 1080), &again).scale - p.scale).abs() < 1e-4);
        // A picture in picture does not fill the canvas, a full frame does.
        assert!(!fills(&wide, &asset(1920, 1080), &Transform { scale: 0.45, x: 0.25, ..Transform::default() }));
        assert!(fills(&wide, &asset(1920, 1080), &Transform::default()));
        assert!(!fills(&tall, &asset(1920, 1080), &Transform::default()));
    }

    #[test]
    fn the_picture_holds_inside_the_dead_zone_and_one_wrong_face_never_moves_it() {
        let times: Vec<i64> = (0..12).map(|i| 250_000 + i * 500_000).collect();
        // A face wobbling a little, one false detection far off, then a move across.
        let mut track: Vec<Option<[f32; 2]>> = [0.33, 0.35, 0.31, 0.34, 0.9, 0.33, 0.32, 0.66, 0.67, 0.65, 0.66, 0.66]
            .into_iter()
            .map(|x| Some([x, 0.4]))
            .collect();
        track[2] = None;
        let smoothed = smooth(&track).unwrap();
        assert!(smoothed[..6].iter().all(|p| (p[0] - 0.33).abs() < 0.03), "{smoothed:?}");
        let positions: Vec<[f32; 2]> = smoothed.iter().map(|p| [0.5 - p[0], 0.0]).collect();
        let keys = keys(&times, &positions);
        // One move, from where it held to the new place, arriving with the look that saw the face gone.
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert_eq!(keys[1].0 - keys[0].0, PAN_US);
        assert!((keys[0].1[0] - positions[0][0]).abs() < 1e-6 && (keys[1].1[0] - (0.5 - 0.66)).abs() < 0.02);
        // Without any face nothing is followed; a still face never moves.
        assert!(smooth(&[None, None]).is_none());
        assert!(super::keys(&times[..3], &[[0.1, 0.0]; 3]).is_empty());
        // A face that keeps going is followed in one move, not one that stops at every look.
        let walking = [[0.0, 0.0], [0.0, 0.0], [0.1, 0.0], [0.2, 0.0], [0.3, 0.0], [0.3, 0.0]];
        assert_eq!(super::keys(&times[..6], &walking), [(times[2] - PAN_US, [0.0, 0.0]), (times[4], [0.3, 0.0])]);
    }

    /// The interview filmed wide, her face crossing from a third to two thirds of the way at 5 s, cut at 7 s
    /// (scripts/fixtures.sh). Reframed to 9:16, the detector finds the face whole and centred inside what Reels leaves
    /// free in every quarter second of the result as the renderer draws it, and each clip moves at most once.
    #[test]
    #[ignore = "Requires scripts/fixtures.sh media and models; run with XDG_DATA_HOME=$PWD/tmp-test/xdg/data"]
    fn a_wide_talking_head_reframed_to_vertical_keeps_the_face_in_the_frame() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp-test/wide-head.mp4");
        let file = file.canonicalize().expect("tmp-test/wide-head.mp4 is missing: run scripts/fixtures.sh");
        let data = std::env::var_os("XDG_DATA_HOME").expect("set XDG_DATA_HOME=$PWD/tmp-test/xdg/data");
        let models = std::path::PathBuf::from(data).join("nuzky/models");
        let mut project = Project::new("reframe");
        let asset = nuzky_engine::media::probe(&file, "wide".into()).unwrap();
        for cmd in [
            EditCmd::SetCanvas { width: 1920, height: 1080, background: None, background_blur: None },
            EditCmd::AddAssets { assets: vec![asset] },
            EditCmd::AddClip { asset_id: "wide".into(), start_us: None, track_id: None },
        ] {
            project.apply(cmd).unwrap();
        }
        let first = project.tracks[0].clips[0].id.clone();
        project.apply(EditCmd::SplitClip { clip_id: first, at_us: 7_000_000 }).unwrap();
        let reframed = reframe(&project, &models, (1080, 1920), None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!((reframed.followed.len(), reframed.centred.len()), (2, 0));
        for cmd in reframed.commands {
            project.apply(cmd).unwrap();
        }
        let moves: Vec<usize> = project.tracks[0].clips.iter().map(|c| c.keyframes.len() / 2).collect();
        assert!(moves.iter().all(|&m| m <= 1), "{moves:?}");

        let (w, h) = (540, 960);
        let area = Canvas { width: w, height: h, ..project.canvas.clone() }.safe_area().unwrap();
        let mut renderer = Renderer::new().unwrap();
        let mut yunet = Yunet::load(&models).unwrap();
        for t in (0..40).map(|i| 125_000 + i * 250_000) {
            let rgba = renderer.render(&project, t, w, h, Wait::Exact, false).unwrap();
            let mut faces = yunet.detect(&Frame { width: w, height: h, rgba }, &AtomicBool::new(false)).unwrap();
            faces.sort_by(|a, b| (b.rect[2] * b.rect[3]).total_cmp(&(a.rect[2] * a.rect[3])));
            let [x, y, fw, fh] = faces.first().unwrap_or_else(|| panic!("no face at {t} µs")).rect;
            let centre = x + fw / 2.0;
            assert!(
                x >= 0.0 && x + fw <= w as f32 && y >= 0.0 && y + fh <= h as f32,
                "face cut at {t} µs: {:?}",
                [x, y, fw, fh]
            );
            assert!(area.left < centre && centre < area.right, "face outside the safe area at {t} µs: {centre}");
        }
    }
}
