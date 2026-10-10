//! Person mattes behind a clip's background, with a stand-in for the person model: a white square moving fast
//! over black is the person. A phone-style file (HEVC stored sideways with a rotation, uneven frame times)
//! shows whether each frame gets its own matte the right way round, and a cut-up timeline whether exactly
//! the chunks clips show are made.
mod qa_support;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use nuzky_engine::export::{ExportOptions, export};
use nuzky_engine::matte;
use nuzky_engine::media::probe;
use nuzky_engine::model::{Asset, Background, ClipContent, Project, Transition, TransitionKind};
use nuzky_engine::{Pending, Renderer, Wait};
use qa_support::*;

const W: u32 = 360;
const H: u32 = 640;
const SQUARE: i64 = 120;
const SQUARE_Y: i64 = 260;
/// How far the square jumps each frame: a matte one frame off would miss it by this much.
const STEP: i64 = 30;
/// The outline is a few pixels soft and the matte grid coarser than the picture.
const EDGE: i64 = 8;

/// Where the square starts in each frame as FFmpeg decodes and turns the file.
fn square_xs(path: &Path) -> Vec<i64> {
    let frames = raw(path, &[]);
    let row = (SQUARE_Y + SQUARE / 2) as usize * W as usize;
    frames
        .as_chunks::<{ (W * H * 4) as usize }>()
        .0
        .iter()
        .map(|f| (0..W as usize).find(|&x| f[(row + x) * 4] > 128).expect("a square in every frame") as i64)
        .collect()
}

/// `seconds` of the square over black, upright 360×640 at 30 fps with every fourth and fifth frame left out, stored sideways
/// in HEVC with a display rotation, as phones record portrait video.
fn phone_video(dir: &Path, name: &str, seconds: u32) -> std::path::PathBuf {
    let sideways = dir.join(format!("sideways-{name}"));
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("color=black:s={W}x{H}:r=30:d={seconds}"),
            "-f",
            "lavfi",
            "-i",
            &format!("color=white:s={SQUARE}x{SQUARE}:r=30:d={seconds}"),
            "-filter_complex",
            &format!("[0:v][1:v]overlay=x='20+{STEP}*mod(n\\,8)':y={SQUARE_Y},transpose=2,select='lt(mod(n\\,5)\\,3)'"),
            "-fps_mode",
            "vfr",
            "-c:v",
            "libx265",
            "-preset",
            "veryfast",
            "-x265-params",
            "log-level=error:crf=10",
            "-tag:v",
            "hvc1",
            "-pix_fmt",
            "yuv420p",
        ],
        &sideways,
    );
    let out = dir.join(name);
    ff(&["-display_rotation:v:0", "-90", "-i", sideways.to_str().unwrap(), "-c", "copy"], &out);
    out
}

/// Bright cells in the left half of the upright picture are the person, so a picture the model gets the wrong
/// way round, or a matte turned back wrong, cuts out another part of the square.
fn stand_in(rgba: &[u8]) -> anyhow::Result<Vec<f32>> {
    let n = matte::SIDE as usize;
    Ok(rgba
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .map(|(i, p)| if i % n < n / 2 && p[0] as u32 + p[1] as u32 + p[2] as u32 > 384 { 1.0 } else { 0.0 })
        .collect())
}

fn project_with(asset: &Asset, background: Background) -> Project {
    let mut project = Project::new("matte");
    (project.canvas.width, project.canvas.height) = (W, H);
    project.assets.push(asset.clone());
    let mut c = clip("c", &asset.id, 0, asset.duration_us);
    if let ClipContent::Media { background: b, .. } = &mut c.content {
        *b = background;
    }
    project.tracks[0].clips.push(c);
    project
}

fn prepare(cache: &Path, project: &Project) {
    for (asset, chunks) in matte::missing(cache, project) {
        matte::prepare(cache, &asset, &chunks, &mut stand_in, &mut |_| Ok(())).unwrap();
    }
}

fn green() -> Background {
    Background::Color { color: "#00ff00".into() }
}

/// Pixels that are not white inside the left half of the picture of the square starting at `x0`, or not green
/// away from it, shrunk and grown by `EDGE`.
fn wrong_pixels(rgba: &[u8], x0: i64) -> usize {
    let x1 = (x0 + SQUARE).min(W as i64 / 2);
    rgba.as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(i, p)| {
            let (x, y) = ((*i as u32 % W) as i64, (*i as u32 / W) as i64);
            let inside = |m: i64| x >= x0 + m && x < x1 - m && y >= SQUARE_Y + m && y < SQUARE_Y + SQUARE - m;
            if inside(EDGE) {
                p[0] < 200 || p[1] < 200 || p[2] < 200
            } else if !inside(-EDGE) {
                p[0] > 60 || p[1] < 200 || p[2] > 60
            } else {
                false
            }
        })
        .count()
}

#[test]
fn every_frame_of_a_turned_phone_video_gets_its_own_matte() {
    if !available() {
        return;
    }
    let dir = dir("matte-phone");
    let cache = dir.join("cache");
    let _ = std::fs::remove_dir_all(&cache);
    let path = phone_video(&dir, "phone.mov", 3);
    let asset = probe(&path, "a".into()).unwrap();
    assert_eq!((asset.width, asset.height, asset.rotation), (W, H, 90), "{asset:?}");
    let project = project_with(&asset, green());

    // Nothing is made yet: export and a strict frame refuse, the preview shows the clip as recorded.
    let out = dir.join("out.mp4");
    let _ = std::fs::remove_file(&out);
    let error = export(&project, &cache, &out, &ExportOptions::default(), &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(format!("{error:#}").starts_with("MATTE_MISSING"), "{error:#}");
    assert!(
        !out.exists()
            && std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().starts_with(".nuzky-part"))
    );
    let mut strict = Renderer::new().unwrap();
    strict.use_mattes(cache.clone(), Pending::Fail);
    let error = strict.render(&project, 0, W, H, Wait::Exact, false).unwrap_err();
    assert!(format!("{error:#}").starts_with("MATTE_MISSING"), "{error:#}");
    let mut preview = Renderer::new().unwrap();
    preview.use_mattes(cache.clone(), Pending::Original);
    let xs = square_xs(&path);
    let plain = preview.render(&project, 0, W, H, Wait::Exact, false).unwrap();
    assert!(wrong_pixels(&plain, xs[0]) > 1000, "the clip shows as recorded until its matte is made");

    prepare(&cache, &project);
    assert!(matte::missing(&cache, &project).is_empty());
    // Every frame, at its own uneven time: the square is white and everything around it green.
    let times = pts(&path);
    assert!(times.len() >= 50 && times.len() == xs.len(), "{} frames, {} squares", times.len(), xs.len());
    assert!(times.windows(2).any(|p| p[1] - p[0] > 40_000), "uneven frame times: {:?}", &times[..6]);
    for (i, &t) in times.iter().enumerate() {
        let frame = strict.render(&project, t, W, H, Wait::Exact, false).unwrap();
        assert_eq!(wrong_pixels(&frame, xs[i]), 0, "frame {i} at {t} us");
    }
    // The preview switches to the matte once it is there, and at twice the size it lines up the same.
    let shown = preview.render(&project, times[10], W, H, Wait::Exact, false).unwrap();
    assert_eq!(wrong_pixels(&shown, xs[10]), 0);
    let big = strict.render(&project, times[11], W * 2, H * 2, Wait::Exact, false).unwrap();
    let small: Vec<u8> = (0..H)
        .flat_map(|y| (0..W).map(move |x| ((y * 2) * W * 2 + x * 2) as usize))
        .flat_map(|i| big[i * 4..i * 4 + 4].to_vec())
        .collect();
    assert_eq!(wrong_pixels(&small, xs[11]), 0);

    // The export has the background on the frame it shows.
    export(&project, &cache, &out, &ExportOptions::default(), &AtomicBool::new(false), |_| {}).unwrap();
    let frames = raw(&out, &["-vf", &format!("scale={W}:{H}")]);
    let size = (W * H * 4) as usize;
    let exported: Vec<i64> = pts(&out);
    for k in [5, 40, 77] {
        // The export frame at time t shows the source frame at or before it.
        let source = times.partition_point(|&s| s <= exported[k]) - 1;
        let wrong = wrong_pixels(&frames[k * size..(k + 1) * size], xs[source]);
        // H.264 rings a little around the square's sharp corners.
        assert!(wrong < 40, "export frame {k} (source frame {source}): {wrong} wrong pixels");
    }
}

#[test]
fn only_the_chunks_clips_show_are_made_and_every_frame_finds_its_matte() {
    if !available() {
        return;
    }
    let dir = dir("matte-cuts");
    let cache = dir.join("cache");
    let _ = std::fs::remove_dir_all(&cache);
    let path = phone_video(&dir, "long.mov", 14);
    let asset = probe(&path, "a".into()).unwrap();
    let mut project = project_with(&asset, green());
    // 0-2 s at 1.5x, then 9-11.8 s dissolving in, then the last second of the file blurring in: the clip
    // before it plays on past the file's end and is held there.
    let main = &mut project.tracks[0].clips;
    let template = main.remove(0);
    let piece = |id: &str, start: i64, duration: i64, source_in: i64, speed: f32| {
        let mut c = template.clone();
        (c.id, c.start_us, c.duration_us) = (id.into(), start, duration);
        if let ClipContent::Media { source_in_us, speed: s, .. } = &mut c.content {
            (*source_in_us, *s) = (source_in, speed);
        }
        c
    };
    let end = asset.duration_us;
    main.push(piece("fast", 0, 1_333_333, 0, 1.5));
    main.push(piece("middle", 1_333_333, 2_800_000, 9_000_000, 1.0));
    main.push(piece("last", 4_133_333, 1_000_000, end - 1_000_000, 1.0));
    main[1].transition_in = Some(Transition { kind: TransitionKind::Dissolve, duration_us: 600_000 });
    main[2].transition_in = Some(Transition { kind: TransitionKind::Blur, duration_us: 500_000 });
    back_to_back(&project);

    let needed: BTreeSet<i64> = matte::needed(&project).into_iter().flat_map(|(_, chunks)| chunks).collect();
    assert!(needed.contains(&0) && needed.contains(&4) && needed.contains(&((end - 1) / 2_000_000)), "{needed:?}");
    assert!(!needed.contains(&2) && !needed.contains(&3), "unused source is not made: {needed:?}");
    prepare(&cache, &project);
    let made = std::fs::read_dir(cache.join("matte"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".bin"))
        .count();
    assert_eq!(made, needed.len());

    let mut strict = Renderer::new().unwrap();
    strict.use_mattes(cache.clone(), Pending::Fail);
    let mut t = 0;
    while t < project.duration_us() {
        strict.render(&project, t, W / 2, H / 2, Wait::Exact, false).unwrap_or_else(|e| panic!("at {t} us: {e:#}"));
        t += 33_333;
    }
    // Made once: preparing again reads nothing.
    let mut decoded = 0;
    for (asset, chunks) in matte::needed(&project) {
        matte::prepare(
            &cache,
            &asset,
            &chunks,
            &mut |rgba| {
                decoded += 1;
                stand_in(rgba)
            },
            &mut |_| Ok(()),
        )
        .unwrap();
    }
    assert_eq!(decoded, 0);
}

/// The main track is laid out as the session keeps it, back to back.
fn back_to_back(project: &Project) {
    let clips = &project.tracks[0].clips;
    for pair in clips.windows(2) {
        assert_eq!(pair[0].end_us(), pair[1].start_us);
    }
}

#[test]
fn a_frame_held_across_a_chunk_boundary_finds_its_matte() {
    if !available() {
        return;
    }
    let dir = dir("matte-gap");
    let cache = dir.join("cache");
    let _ = std::fs::remove_dir_all(&cache);
    // No frames from 3.7 s to 7.6 s, as in a screen recording of a still screen: the one at 3.667 s shows until then,
    // across chunk 2, which no clip shows, into chunk 3.
    let path = dir.join("gap.mp4");
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("color=black:s={W}x{H}:r=30:d=9"),
            "-f",
            "lavfi",
            "-i",
            &format!("color=white:s={SQUARE}x{SQUARE}:r=30:d=9"),
            "-filter_complex",
            &format!("[0:v][1:v]overlay=x='20+{STEP}*mod(n\\,8)':y={SQUARE_Y},select='not(between(t\\,3.7\\,7.6))'"),
            "-fps_mode",
            "vfr",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "10",
            "-pix_fmt",
            "yuv420p",
        ],
        &path,
    );
    let asset = probe(&path, "a".into()).unwrap();
    let mut project = project_with(&asset, green());
    let clip = &mut project.tracks[0].clips[0];
    clip.duration_us = 300_000;
    if let ClipContent::Media { source_in_us, .. } = &mut clip.content {
        *source_in_us = 6_200_000;
    }
    assert_eq!(matte::needed(&project)[0].1, BTreeSet::from([3]), "the clip shows source from chunk 3 only");
    prepare(&cache, &project);
    let (times, xs) = (pts(&path), square_xs(&path));
    let held = times.partition_point(|&t| t <= 6_200_000) - 1;
    assert!(times[held] < 4_000_000, "the frame shown is from chunk 1: {}", times[held]);
    let mut strict = Renderer::new().unwrap();
    strict.use_mattes(cache.clone(), Pending::Fail);
    let frame = strict.render(&project, 0, W, H, Wait::Exact, false).unwrap();
    assert_eq!(wrong_pixels(&frame, xs[held]), 0);
}

#[test]
fn a_blur_transition_leaves_a_picked_colour_as_picked() {
    if !available() {
        return;
    }
    let dir = dir("matte-transition");
    let cache = dir.join("cache");
    let _ = std::fs::remove_dir_all(&cache);
    let path = phone_video(&dir, "phone.mov", 3);
    let asset = probe(&path, "a".into()).unwrap();
    let grey = Background::Color { color: "#808080".into() };
    let mut project = project_with(&asset, grey.clone());
    let mut second = project.tracks[0].clips[0].clone();
    (second.id, second.start_us) = ("b".into(), asset.duration_us);
    project.tracks[0].clips.push(second);
    for clip in &mut project.tracks[0].clips {
        if let ClipContent::Media { adjust, .. } = &mut clip.content {
            adjust.brightness = 1.0;
        }
    }
    project.tracks[0].clips[1].transition_in = Some(Transition { kind: TransitionKind::Blur, duration_us: 600_000 });
    prepare(&cache, &project);
    let mut strict = Renderer::new().unwrap();
    strict.use_mattes(cache.clone(), Pending::Fail);
    // The top corner is far from the square: only the grey shows there, before and in the middle of the transition.
    for t in [1_000_000, asset.duration_us] {
        let frame = strict.render(&project, t, W, H, Wait::Exact, false).unwrap();
        let corner = &frame[(20 * W as usize + 20) * 4..][..3];
        assert!(corner.iter().all(|&v| v.abs_diff(128) <= 2), "at {t} us the grey is {corner:?}");
    }
}
