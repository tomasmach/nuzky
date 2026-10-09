//! Cover frames and subject masks on a real face with the real models. The face is the NASA
//! interview in scripts/fixtures.sh; run as AGENTS.md says:
//! `XDG_DATA_HOME=$PWD/tmp-test/xdg/data cargo test -p nuzky-vision -- --ignored`.
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use nuzky_engine::{
    Project,
    edit::EditCmd,
    media::probe,
    model::{ClipContent, TrackKind},
};
use nuzky_vision::{Candidate, segment_subject, thumbnail_frames};

fn fixture(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp-test").join(name);
    path.canonicalize().unwrap_or_else(|_| panic!("{} is missing: run scripts/fixtures.sh", path.display()))
}

fn models() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME").expect("set XDG_DATA_HOME=$PWD/tmp-test/xdg/data");
    PathBuf::from(data).join("nuzky/models")
}

/// A 1080×1920 main track of `(file, source in, source out)` pieces, back to back.
fn timeline(pieces: &[(&str, i64, i64)]) -> Project {
    let mut project = Project::new("covers");
    for (i, &(name, from, to)) in pieces.iter().enumerate() {
        let id = format!("asset-{i}");
        project.apply(EditCmd::AddAssets { assets: vec![probe(&fixture(name), id.clone()).unwrap()] }).unwrap();
        project.apply(EditCmd::AddClip { asset_id: id, start_us: None, track_id: None }).unwrap();
        let clip = project.tracks.iter_mut().find(|t| t.kind == TrackKind::Video).unwrap().clips.last_mut().unwrap();
        if let ClipContent::Media { source_in_us, .. } = &mut clip.content {
            *source_in_us = from;
        }
        clip.duration_us = to - from;
    }
    let main = project.tracks.iter_mut().find(|t| t.kind == TrackKind::Video).unwrap();
    let mut start = 0;
    for clip in &mut main.clips {
        clip.start_us = start;
        start += clip.duration_us;
    }
    assert_eq!((project.canvas.width, project.canvas.height), (1080, 1920));
    project
}

fn frames(project: &Project) -> Vec<Candidate> {
    thumbnail_frames(project, &models(), None, &AtomicBool::new(false), &mut |_, _| {}).unwrap()
}

fn summary(candidates: &[Candidate]) -> String {
    candidates.iter().map(|c| format!("{} {:.3} {:?}\n", c.time_us, c.score, c.parts)).collect()
}

#[test]
#[ignore = "Requires scripts/fixtures.sh media and models; run with XDG_DATA_HOME=$PWD/tmp-test/xdg/data"]
fn open_eyes_win_over_a_blink_and_a_blur() {
    // [0,3) s open eyes blurred, [3,6) the blink, [6,12) sharp open eyes.
    let candidates = frames(&timeline(&[("face-thumb.mp4", 0, 12_000_000)]));
    let report = summary(&candidates);
    let part = |c: &Candidate, name: &str| c.parts[name];
    let segment = |c: &Candidate| c.time_us / 3_000_000;
    let scores = |keep: fn(i64) -> bool| candidates.iter().filter(move |c| keep(segment(c))).map(|c| c.score);
    assert!(segment(&candidates[0]) >= 2, "a sharp open-eyed frame wins:\n{report}");
    let worst_open = scores(|s| s >= 2).fold(f32::MAX, f32::min);
    let best_blurred = scores(|s| s == 0).fold(f32::MIN, f32::max);
    let best_blink = scores(|s| s == 1).fold(f32::MIN, f32::max);
    assert!(best_blurred > f32::MIN && best_blink > f32::MIN, "every segment is offered:\n{report}");
    assert!(worst_open > best_blurred && worst_open > best_blink, "{report}");
    for c in &candidates {
        match segment(c) {
            0 => assert!(part(c, "sharpness") < 0.3 && part(c, "eyes_open") > 0.9, "blurred: {report}"),
            1 => assert!(part(c, "eyes_open") < 0.5, "blink: {report}"),
            _ => assert!(part(c, "sharpness") > 0.9 && part(c, "eyes_open") > 0.9, "open: {report}"),
        }
        assert_eq!(c.faces.len(), 1, "{report}");
    }
    // Candidates are best first and at least 2 s apart.
    assert!(candidates.windows(2).all(|w| w[0].score >= w[1].score), "{report}");
    for (i, a) in candidates.iter().enumerate() {
        assert!(candidates[i + 1..].iter().all(|b| (a.time_us - b.time_us).abs() >= 2_000_000), "{report}");
    }
}

#[test]
#[ignore = "Requires scripts/fixtures.sh media and models; run with XDG_DATA_HOME=$PWD/tmp-test/xdg/data"]
fn frames_beside_a_cut_are_not_offered() {
    // The only open eyes are 0.8 s between two cuts; every frame of them is within 0.4 s of one.
    let project = timeline(&[
        ("face-thumb.mp4", 3_000_000, 6_000_000),
        ("face-thumb.mp4", 6_000_000, 6_800_000),
        ("face-thumb.mp4", 3_000_000, 6_000_000),
    ]);
    let candidates = frames(&project);
    let report = summary(&candidates);
    assert!(!candidates.is_empty(), "the blinks away from the cuts are offered");
    for c in &candidates {
        for cut in [0, 3_000_000, 3_800_000, 6_800_000] {
            assert!((c.time_us - cut).abs() >= 400_000, "{} is beside the cut at {cut}:\n{report}", c.time_us);
        }
        assert!(c.parts["eyes_open"] < 0.5, "only blinks are settled:\n{report}");
    }
}

/// The hand-drawn outline of the person in face-open.png, filled.
fn reference(width: u32, height: u32) -> Vec<bool> {
    let data: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/face-open-person.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!((data["width"].as_u64(), data["height"].as_u64()), (Some(width as u64), Some(height as u64)));
    let polygon: Vec<(f32, f32)> = data["polygon"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p[0].as_f64().unwrap() as f32, p[1].as_f64().unwrap() as f32))
        .collect();
    let mut inside = vec![false; (width * height) as usize];
    for y in 0..height {
        let cy = y as f32 + 0.5;
        // Even-odd crossings of this row, sorted, fill between pairs.
        let mut xs: Vec<f32> = polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .filter(|((_, y0), (_, y1))| (*y0 <= cy) != (*y1 <= cy))
            .map(|((x0, y0), (x1, y1))| x0 + (cy - y0) / (y1 - y0) * (x1 - x0))
            .collect();
        xs.sort_by(f32::total_cmp);
        for pair in xs.as_chunks::<2>().0 {
            for x in 0..width {
                let cx = x as f32 + 0.5;
                if cx >= pair[0] && cx < pair[1] {
                    inside[(y * width + x) as usize] = true;
                }
            }
        }
    }
    inside
}

fn read_mask(path: &Path) -> (u32, u32, Vec<u8>) {
    let mut reader =
        png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap())).read_info().unwrap();
    let mut alpha = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut alpha).unwrap();
    assert_eq!((info.color_type, info.bit_depth), (png::ColorType::Grayscale, png::BitDepth::Eight));
    (info.width, info.height, alpha)
}

#[test]
#[ignore = "Requires scripts/fixtures.sh media and models; run with XDG_DATA_HOME=$PWD/tmp-test/xdg/data"]
fn the_mask_follows_the_person() {
    let cache =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tmp-test/vision-cache-{}", std::process::id()));
    let cancel = AtomicBool::new(false);
    let still = timeline(&[("face-open.png", 0, 3_000_000)]);
    let mask = segment_subject(&still, 1_000_000, &models(), &cache, &cancel, &mut |_| {}).unwrap();
    assert!(!mask.cached && mask.person, "{mask:?}");
    let (width, height, alpha) = read_mask(&mask.path);
    assert_eq!((width, height), (1080, 1920));
    let person = reference(width, height);
    let (both, either) = alpha.iter().zip(&person).fold((0u32, 0u32), |(both, either), (&a, &p)| {
        let m = a >= 128;
        (both + (m && p) as u32, either + (m || p) as u32)
    });
    let iou = both as f32 / either as f32;
    assert!(iou > 0.95, "IoU {iou} against the hand-drawn outline");
    let share = person.iter().filter(|&&p| p).count() as f32 / person.len() as f32;
    assert!((mask.subject_share - share).abs() < 0.03, "{mask:?} against {share}");

    // The same frame comes from the cache; another frame is masked anew.
    let again = segment_subject(&still, 2_000_000, &models(), &cache, &cancel, &mut |_| {}).unwrap();
    assert!(again.cached && again.path == mask.path, "{again:?}");
    let blink = timeline(&[("face-blink.png", 0, 3_000_000)]);
    let other = segment_subject(&blink, 1_000_000, &models(), &cache, &cancel, &mut |_| {}).unwrap();
    assert!(!other.cached && other.path != mask.path, "{other:?}");
    std::fs::remove_dir_all(cache).unwrap();
}

#[test]
fn missing_models_fail_with_a_code_before_any_work() {
    let empty = std::env::temp_dir().join(format!("nuzky-no-models-{}", std::process::id()));
    let project = Project::new("empty");
    let error = thumbnail_frames(&project, &empty, None, &AtomicBool::new(false), &mut |_, _| {}).unwrap_err();
    assert!(format!("{error:#}").starts_with("MODEL_MISSING: face detector"), "{error:#}");
    let error = segment_subject(&project, 0, &empty, &empty, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
    assert!(format!("{error:#}").contains("subject mask (birefnet-lite.onnx)"), "{error:#}");
}
