mod qa_support;
use capopen_engine::{
    edit::{CaptionSegment, EditCmd, Editor},
    model::*,
};
use qa_support::*;
fn asset(id: &str, duration: i64) -> Asset {
    Asset {
        id: id.into(),
        name: id.into(),
        path: String::new(),
        kind: AssetKind::Video,
        duration_us: duration,
        width: 64,
        height: 64,
        fps: 30.0,
        has_audio: true,
        rotation: 0,
        mirror: false,
    }
}
fn editor() -> Editor {
    let mut p = Project::new("properties");
    p.assets = vec![asset("a", 5_000_000), asset("b", 3_000_000)];
    Editor::new(p)
}
fn update_speed(id: &str, speed: f32) -> EditCmd {
    EditCmd::UpdateClip {
        clip_id: id.into(),
        transform: None,
        volume: None,
        text: None,
        style: None,
        speed: Some(speed),
        adjust: None,
        fade_in_us: None,
        fade_out_us: None,
        clean_voice: None,
    }
}
fn invariants(p: &Project) -> Result<(), String> {
    for track in &p.tracks {
        let mut end = 0;
        for c in &track.clips {
            if c.start_us < end || track.id == "main" && c.start_us != end {
                return Err(format!("track {} overlap/gap: expected >= {end}, clip starts {}", track.id, c.start_us));
            }
            if c.duration_us <= 0 {
                return Err("nonpositive duration".into());
            }
            end = c.end_us();
            if let ClipContent::Media { asset_id, source_in_us, speed, .. } = &c.content {
                let a = p.asset(asset_id).unwrap();
                let source_end = *source_in_us as f64 + c.duration_us as f64 * *speed as f64;
                if *source_in_us < 0 || a.kind != AssetKind::Image && source_end > a.duration_us as f64 + 2.0 {
                    return Err(format!(
                        "source range exceeds {}: in={source_in_us}, duration={}, speed={speed}, end={source_end}, limit={}",
                        a.id, c.duration_us, a.duration_us
                    ));
                }
            }
        }
    }
    let json = serde_json::to_string(p).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(p, &back);
    Ok(())
}
#[test]
fn seeded_edits_without_speed_preserve_invariants_and_history() {
    seeded_sequences(false);
}
#[test]
fn seeded_edit_sequences_preserve_invariants_and_history() {
    seeded_sequences(true);
}
fn seeded_sequences(with_speed: bool) {
    let mut failures = Vec::new();
    for seed in [0x12345678, 0xCAFEBABE, 0xF00DFACE, 0xBADC0DE] {
        let mut rng = Rng(seed);
        let mut e = editor();
        let mut history = Vec::new();
        for step in 0..120 {
            let clips: Vec<_> =
                e.project.tracks.iter().flat_map(|t| t.clips.iter().map(|c| (t.id.clone(), c.clone()))).collect();
            let cmd = if clips.is_empty() || rng.next().is_multiple_of(7) {
                EditCmd::AddClip {
                    asset_id: if rng.next().is_multiple_of(2) { "a" } else { "b" }.into(),
                    start_us: None,
                    track_id: None,
                }
            } else {
                let (track, c) = &clips[rng.next() as usize % clips.len()];
                match rng.next() % if with_speed { 6 } else { 5 } {
                    0 => EditCmd::MoveClip {
                        clip_id: c.id.clone(),
                        track_id: if rng.next().is_multiple_of(2) { Some("main".into()) } else { None },
                        start_us: (rng.next() % 10_000_000) as i64,
                    },
                    1 => EditCmd::TrimClip {
                        clip_id: c.id.clone(),
                        start_us: c.start_us,
                        duration_us: 100_000 + (rng.next() % 6_000_000) as i64,
                        source_in_us: Some((rng.next() % 5_000_000) as i64),
                    },
                    2 => EditCmd::SplitClip { clip_id: c.id.clone(), at_us: c.start_us + c.duration_us / 2 },
                    3 => EditCmd::DuplicateClip { clip_id: c.id.clone() },
                    4 => EditCmd::DeleteClips { clip_ids: vec![c.id.clone()] },
                    _ => {
                        let _ = track;
                        update_speed(&c.id, [0.5, 1.0, 2.0, 4.0][rng.next() as usize % 4])
                    }
                }
            };
            let before = e.project.clone();
            let desc = format!("{cmd:?}");
            let result = e.apply(cmd, None);
            history.push(desc);
            if result.is_err() {
                assert_eq!(e.project, before, "rejected command mutated project");
                continue;
            }
            let after = e.project.clone();
            if after != before {
                assert!(e.undo());
                assert_eq!(e.project, before);
                assert!(e.redo());
                assert_eq!(e.project, after);
            }
            if let Err(error) = invariants(&e.project) {
                std::fs::write(dir("edit-traces").join(format!("seed-{seed:x}-{with_speed}.txt")), history.join("\n"))
                    .unwrap();
                failures.push(format!("seed={seed:#x} step={step}: {error}"));
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn slowing_overlay_must_not_overlap_next_clip() {
    let mut e = editor();
    e.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }, None).unwrap();
    let id = e.project.tracks[0].clips[0].id.clone();
    e.apply(EditCmd::MoveClip { clip_id: id.clone(), track_id: None, start_us: 0 }, None).unwrap();
    let track = e.project.tracks[1].id.clone();
    e.apply(EditCmd::AddClip { asset_id: "b".into(), start_us: Some(5_000_000), track_id: Some(track) }, None).unwrap();
    let _ = e.apply(update_speed(&id, 0.5), None);
    invariants(&e.project).unwrap();
}
#[test]
fn fast_trim_stays_inside_source() {
    let mut e = editor();
    e.apply(EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None }, None).unwrap();
    let id = e.project.tracks[0].clips[0].id.clone();
    e.apply(update_speed(&id, 10.0), None).unwrap();
    let _ = e.apply(
        EditCmd::TrimClip { clip_id: id, start_us: 0, duration_us: 33_334, source_in_us: Some(4_966_666) },
        None,
    );
    invariants(&e.project).unwrap();
}
#[test]
fn equal_start_captions_do_not_overlap() {
    let mut e = editor();
    let style = TextStyle {
        font_family: None,
        font_size: 40.0,
        color: "#fff".into(),
        bold: false,
        stroke_width: 0.0,
        stroke_color: "#000".into(),
        background: None,
        max_width: None,
    };
    let _ = e.apply(
        EditCmd::AddCaptions {
            segments: vec![
                CaptionSegment { start_us: 0, end_us: 1_000_000, text: "first".into() },
                CaptionSegment { start_us: 0, end_us: 2_000_000, text: "second".into() },
            ],
            style,
        },
        None,
    );
    invariants(&e.project).unwrap();
}
