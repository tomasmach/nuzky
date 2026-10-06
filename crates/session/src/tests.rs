use super::*;
use capopen_engine::model::{Adjust, Asset, AssetKind, ClipContent, Transform};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("capopen-session-{}", new_id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.capopen");
        storage::save(&path, &Project::new("Original")).unwrap();
        Self(path)
    }
    fn open(&self) -> ProjectSession {
        ProjectSession::open(&self.0, Mode::Write).unwrap()
    }
    fn disk(&self) -> Project {
        serde_json::from_slice(&fs::read(&self.0).unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}
fn rename(name: &str) -> Vec<EditCmd> {
    vec![EditCmd::RenameProject { name: name.into() }]
}
fn apply(session: &ProjectSession, run: &str, name: &str) {
    session
        .apply_edits(run, &new_id(), rename(name), None)
        .unwrap();
}

#[test]
fn lock_excludes_readers_and_writers_and_releases_on_drop() {
    let f = Fixture::new();
    let s = f.open();
    for mode in [Mode::ReadOnly, Mode::Write] {
        assert!(
            format!("{:#}", ProjectSession::open(&f.0, mode).err().unwrap())
                .contains("PROJECT_BUSY")
        );
    }
    drop(s);
    assert!(ProjectSession::open(&f.0, Mode::Write).is_ok());
}

#[test]
fn readonly_refuses_all_mutations() {
    let f = Fixture::new();
    let s = ProjectSession::open(&f.0, Mode::ReadOnly).unwrap();
    assert!(
        s.begin_run("test".into())
            .unwrap_err()
            .to_string()
            .contains("READ_ONLY")
    );
    assert!(s.apply_edits("a", "b", rename("changed"), None).is_err());
    assert!(s.end_run("a", EndAction::Keep).is_err());
    assert!(s.undo_run("a").is_err());
    assert_eq!(f.disk().name, "Original");
}

#[test]
fn run_checkpoints_saves_each_batch_and_keeps_one_undo_entry() {
    let f = Fixture::new();
    let s = f.open();
    let run = s.begin_run("AI edit".into()).unwrap();
    let checkpoint: Value =
        serde_json::from_slice(&fs::read(storage::sidecar(&f.0, ".checkpoint.json")).unwrap())
            .unwrap();
    assert_eq!(checkpoint["project"]["name"], "Original");
    assert!(s.begin_run("other".into()).is_err());
    apply(&s, &run.run_id, "First");
    assert_eq!(f.disk().name, "First");
    apply(&s, &run.run_id, "Second");
    assert_eq!(f.disk().name, "Second");
    assert!(s.undo_run(&run.run_id).is_err());
    s.end_run(&run.run_id, EndAction::Keep).unwrap();
    assert!(!storage::sidecar(&f.0, ".checkpoint.json").exists());
    s.undo_run(&run.run_id).unwrap();
    assert_eq!(f.disk().name, "Original");
    assert!(s.undo_run(&run.run_id).is_err());
    assert!(s.state().unwrap().stamp.revision > run.stamp.revision);
}

#[test]
fn discard_restores_checkpoint_and_preserves_previous_history() {
    let f = Fixture::new();
    let s = f.open();
    let a = s.begin_run("a".into()).unwrap().run_id;
    apply(&s, &a, "First");
    s.end_run(&a, EndAction::Keep).unwrap();
    let b = s.begin_run("b".into()).unwrap().run_id;
    apply(&s, &b, "Second");
    s.end_run(&b, EndAction::Discard).unwrap();
    assert_eq!(f.disk().name, "First");
    assert!(s.undo_run(&b).is_err());
    s.undo_run(&a).unwrap();
    assert_eq!(f.disk().name, "Original");
}

#[test]
fn only_last_history_entry_can_be_undone() {
    let f = Fixture::new();
    let s = f.open();
    let a = s.begin_run("a".into()).unwrap().run_id;
    apply(&s, &a, "First");
    s.end_run(&a, EndAction::Keep).unwrap();
    let b = s.begin_run("b".into()).unwrap().run_id;
    apply(&s, &b, "Second");
    s.end_run(&b, EndAction::Keep).unwrap();
    assert!(s.undo_run(&a).is_err());
    s.undo_run(&b).unwrap();
    s.undo_run(&a).unwrap();
    assert_eq!(f.disk().name, "Original");
}

#[test]
fn retries_are_idempotent_and_conflicting_content_is_rejected() {
    let f = Fixture::new();
    let s = f.open();
    let run = s.begin_run("a".into()).unwrap();
    let rev = Some(run.stamp.revision);
    let first = s
        .apply_edits(&run.run_id, "req", rename("First"), rev)
        .unwrap();
    apply(&s, &run.run_id, "Second");
    let retry = s
        .apply_edits(&run.run_id, "req", rename("First"), rev)
        .unwrap();
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(retry).unwrap()
    );
    assert_eq!(f.disk().name, "Second");
    assert!(
        s.apply_edits(&run.run_id, "req", rename("Other"), rev)
            .unwrap_err()
            .to_string()
            .contains("REQUEST_CONFLICT")
    );
    s.end_run(&run.run_id, EndAction::Keep).unwrap();
    assert!(
        s.apply_edits(&run.run_id, "req", rename("First"), rev)
            .unwrap_err().to_string().contains("INVALID_RUN")
    );
}

#[test]
fn stale_revision_and_wrong_or_expired_run_do_not_mutate() {
    let f = Fixture::new();
    let s = f.open();
    let r = s.begin_run("a".into()).unwrap();
    assert!(
        s.apply_edits(&r.run_id, "a", rename("Bad"), Some(r.stamp.revision + 1))
            .unwrap_err()
            .to_string()
            .contains("STALE_REVISION")
    );
    assert!(s.apply_edits("wrong", "a", rename("Bad"), None).is_err());
    s.end_run(&r.run_id, EndAction::Keep).unwrap();
    assert!(s.apply_edits(&r.run_id, "a", rename("Bad"), None).is_err());
    assert_eq!(f.disk().name, "Original");
}

#[test]
fn batch_and_post_validation_errors_roll_back_memory_disk_and_revision() {
    let f = Fixture::new();
    let s = f.open();
    let r = s.begin_run("a".into()).unwrap();
    let mut edits = rename("Bad");
    edits.push(EditCmd::SplitClip {
        clip_id: "missing".into(),
        at_us: 1,
    });
    assert!(s.apply_edits(&r.run_id, "fail", edits, None).is_err());
    let mut a = asset();
    a.duration_us = -1;
    assert!(
        s.apply_edits(
            &r.run_id,
            "fail",
            vec![EditCmd::AddAssets { assets: vec![a] }],
            None
        )
        .is_err()
    );
    assert_eq!(s.state().unwrap().project, f.disk());
    assert_eq!(s.state().unwrap().stamp.revision, r.stamp.revision);
    assert_eq!(f.disk().name, "Original");
    s.apply_edits(&r.run_id, "fail", rename("retry"), None)
        .unwrap();
}

#[test]
fn save_failure_does_not_commit_or_remember_request() {
    let f = Fixture::new();
    let s = f.open();
    let r = s.begin_run("a".into()).unwrap();
    let backup = f.0.with_extension("backup");
    fs::rename(&f.0, &backup).unwrap();
    fs::create_dir(&f.0).unwrap();
    assert!(
        s.apply_edits(&r.run_id, "retry", rename("Changed"), None)
            .is_err()
    );
    assert_eq!(s.state().unwrap().project.name, "Original");
    fs::remove_dir(&f.0).unwrap();
    fs::rename(backup, &f.0).unwrap();
    s.apply_edits(&r.run_id, "retry", rename("Changed"), None)
        .unwrap();
}

#[test]
fn idle_timeout_and_disconnect_keep_edits() {
    let f = Fixture::new();
    let s = ProjectSession::open_with_idle_timeout(&f.0, Mode::Write, Duration::from_millis(30))
        .unwrap();
    let r = s.begin_run("a".into()).unwrap();
    apply(&s, &r.run_id, "Changed");
    // Observe disk without touching the session: the background timer must do the work.
    let deadline = Instant::now() + Duration::from_secs(2);
    while storage::sidecar(&f.0, ".checkpoint.json").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(s.state().unwrap().open_run.is_none());
    s.undo_run(&r.run_id).unwrap();
    let r = s.begin_run("b".into()).unwrap();
    apply(&s, &r.run_id, "Disconnected");
    s.disconnect().unwrap();
    s.undo_run(&r.run_id).unwrap();
    assert_eq!(f.disk().name, "Original");
}

#[test]
fn leftover_checkpoint_is_reported_and_not_overwritten_and_epoch_changes() {
    let f = Fixture::new();
    let s = f.open();
    let epoch = s.state().unwrap().stamp.session_epoch;
    drop(s);
    let path = storage::sidecar(&f.0, ".checkpoint.json");
    fs::write(&path, b"crash checkpoint").unwrap();
    let s = f.open();
    assert_ne!(epoch, s.state().unwrap().stamp.session_epoch);
    assert_eq!(s.state().unwrap().recovery_checkpoint, Some(path.clone()));
    assert!(s.begin_run("a".into()).is_err());
    assert_eq!(fs::read(path).unwrap(), b"crash checkpoint");
}

fn asset() -> Asset {
    Asset {
        id: "a".into(),
        name: "a".into(),
        path: "/missing.mov".into(),
        kind: AssetKind::Video,
        duration_us: 10_000_000,
        width: 1080,
        height: 1920,
        fps: 30.0,
        has_audio: true,
        rotation: 0,
    }
}
fn media_clip() -> Clip {
    Clip::new(
        "c".into(),
        0,
        5_000_000,
        ClipContent::Media {
            asset_id: "a".into(),
            source_in_us: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            adjust: Adjust::default(),
            fade_in_us: 0,
            fade_out_us: 0,
        },
    )
}
#[test]
fn validates_references_overlaps_source_ranges_and_unique_ids() {
    let mut p = Project::new("test");
    p.assets.push(asset());
    p.tracks[0].clips.push(media_clip());
    validate(&p).unwrap();
    let mut bad = p.clone();
    bad.assets.clear();
    assert!(validate(&bad).is_err());
    let mut bad = p.clone();
    let mut clip = media_clip();
    clip.id = "other".into();
    bad.tracks[0].clips.push(clip);
    assert!(validate(&bad).is_err());
    let mut bad = p.clone();
    bad.tracks[0].clips[0].duration_us = 11_000_000;
    assert!(validate(&bad).is_err());
    let mut bad = p.clone();
    bad.assets.push(asset());
    assert!(validate(&bad).is_err());
    let mut bad = p.clone();
    bad.tracks[0].clips[0].start_us = 10;
    assert!(validate(&bad).is_err());
    let mut bad = p.clone();
    if let ClipContent::Media { speed, .. } = &mut bad.tracks[0].clips[0].content {
        *speed = f32::NAN;
    }
    assert!(validate(&bad).is_err());
}

#[test]
fn result_reports_created_changed_removed_and_actual_magnetic_positions() {
    let f = Fixture::new();
    let s = f.open();
    let r = s.begin_run("a".into()).unwrap().run_id;
    let result = s
        .apply_edits(
            &r,
            "add",
            vec![
                EditCmd::AddAssets {
                    assets: vec![asset()],
                },
                EditCmd::AddClip {
                    asset_id: "a".into(),
                    start_us: Some(9_000_000),
                    track_id: None,
                },
            ],
            None,
        )
        .unwrap();
    assert_eq!(result.outcome.created.len(), 1);
    let id = result.outcome.created[0].clone();
    assert_eq!(result.clips[0].clip.start_us, 0);
    let result = s
        .apply_edits(
            &r,
            "split",
            vec![EditCmd::SplitClip {
                clip_id: id.clone(),
                at_us: 5_000_000,
            }],
            None,
        )
        .unwrap();
    assert_eq!(result.changed, vec![id.clone()]);
    assert_eq!(result.outcome.created.len(), 1);
    let result = s
        .apply_edits(
            &r,
            "delete",
            vec![EditCmd::DeleteClips {
                clip_ids: vec![id.clone()],
            }],
            None,
        )
        .unwrap();
    assert_eq!(result.outcome.removed, vec![id]);
    assert_eq!(result.clips[0].clip.start_us, 0);
}

#[test]
fn checkpoint_cleanup_failure_leaves_discard_consistent_and_retryable() {
    let f = Fixture::new();
    let s = f.open();
    let run = s.begin_run("test".into()).unwrap().run_id;
    apply(&s, &run, "Changed");
    let checkpoint = storage::sidecar(&f.0, ".checkpoint.json");
    let saved = fs::read(&checkpoint).unwrap();
    fs::remove_file(&checkpoint).unwrap();
    fs::create_dir(&checkpoint).unwrap();
    assert!(s.end_run(&run, EndAction::Discard).is_err());
    assert_eq!(s.state().unwrap().project, f.disk());
    assert_eq!(f.disk().name, "Changed");
    fs::remove_dir(&checkpoint).unwrap();
    fs::write(&checkpoint, saved).unwrap();
    s.end_run(&run, EndAction::Discard).unwrap();
    assert_eq!(f.disk().name, "Original");
}

#[test]
fn selection_does_not_retain_clips_removed_later_in_the_batch() {
    let f = Fixture::new();
    let s = f.open();
    let r = s.begin_run("selection".into()).unwrap().run_id;
    let result = s
        .apply_edits(
            &r,
            "add",
            vec![
                EditCmd::AddAssets {
                    assets: vec![asset()],
                },
                EditCmd::AddClip {
                    asset_id: "a".into(),
                    start_us: None,
                    track_id: None,
                },
            ],
            None,
        )
        .unwrap();
    let id = result.outcome.created[0].clone();
    let result = s
        .apply_edits(
            &r,
            "remove",
            vec![
                EditCmd::MoveClip {
                    clip_id: id.clone(),
                    track_id: Some("main".into()),
                    start_us: 0,
                },
                EditCmd::DeleteClips { clip_ids: vec![id] },
            ],
            None,
        )
        .unwrap();
    assert!(result.outcome.select.is_empty());
    assert!(s.state().unwrap().selection.is_empty());
}

#[test]
fn extreme_times_cannot_poison_session_or_partially_commit() {
    let f = Fixture::new();
    let s = f.open();
    let run = s.begin_run("overflow".into()).unwrap();
    let mut huge = asset();
    huge.duration_us = i64::MAX;
    let result = s.apply_edits(
        &run.run_id,
        "overflow",
        vec![
            EditCmd::AddAssets { assets: vec![huge] },
            EditCmd::AddClip {
                asset_id: "a".into(),
                start_us: None,
                track_id: None,
            },
            EditCmd::AddClip {
                asset_id: "a".into(),
                start_us: None,
                track_id: None,
            },
        ],
        None,
    );
    assert!(result.is_err());
    assert_eq!(s.state().unwrap().project, f.disk());
    assert_eq!(s.state().unwrap().stamp.revision, run.stamp.revision);
    apply(&s, &run.run_id, "Still usable");
    assert_eq!(f.disk().name, "Still usable");
}

#[test]
fn trim_preserves_valid_keyframes_outside_clip_bounds() {
    let f = Fixture::new();
    let s = f.open();
    let run = s.begin_run("trim animation".into()).unwrap().run_id;
    let result = s
        .apply_edits(
            &run,
            "add",
            vec![
                EditCmd::AddAssets {
                    assets: vec![asset()],
                },
                EditCmd::AddClip {
                    asset_id: "a".into(),
                    start_us: None,
                    track_id: None,
                },
            ],
            None,
        )
        .unwrap();
    let id = result.outcome.created[0].clone();
    s.apply_edits(
        &run,
        "trim",
        vec![
            EditCmd::SetKeyframes {
                clip_id: id.clone(),
                keyframes: vec![
                    capopen_engine::model::Keyframe {
                        t_us: 0,
                        transform: Transform::default(),
                    },
                    capopen_engine::model::Keyframe {
                        t_us: 10_000_000,
                        transform: Transform {
                            scale: 2.0,
                            ..Transform::default()
                        },
                    },
                ],
            },
            EditCmd::TrimClip {
                clip_id: id.clone(),
                start_us: 0,
                duration_us: 5_000_000,
                source_in_us: None,
            },
            EditCmd::TrimClip {
                clip_id: id,
                start_us: 1_000_000,
                duration_us: 4_000_000,
                source_in_us: Some(1_000_000),
            },
        ],
        None,
    )
    .unwrap();
    let project = s.state().unwrap().project;
    let clip = &project.tracks[0].clips[0];
    assert_eq!(clip.keyframes[0].t_us, -1_000_000);
    assert!(clip.keyframes[1].t_us > clip.duration_us);
    assert_eq!(project, f.disk());
}

#[test]
fn readers_share_lock_and_exclude_writers() {
    let f = Fixture::new();
    let a = ProjectSession::open(&f.0, Mode::ReadOnly).unwrap();
    let b = ProjectSession::open(&f.0, Mode::ReadOnly).unwrap();
    assert!(ProjectSession::open(&f.0, Mode::Write).err().unwrap().to_string().contains("PROJECT_BUSY"));
    drop(a);
    assert!(ProjectSession::open(&f.0, Mode::Write).is_err());
    drop(b);
    assert!(ProjectSession::open(&f.0, Mode::Write).is_ok());
}

#[test]
fn requests_are_scoped_to_open_run_and_cleared_on_finish_and_undo() {
    let f = Fixture::new();
    let s = f.open();
    for action in [EndAction::Keep, EndAction::Discard] {
        let a = s.begin_run("first".into()).unwrap().run_id;
        s.apply_edits(&a, "same", rename("First"), None).unwrap();
        s.end_run(&a, action).unwrap();
        assert!(s.inner.lock().unwrap().requests.is_empty());
        let b = s.begin_run("second".into()).unwrap().run_id;
        assert!(s.apply_edits(&a, "same", rename("First"), None).unwrap_err().to_string().contains("INVALID_RUN"));
        s.apply_edits(&b, "same", rename("Second"), None).unwrap();
        s.end_run(&b, EndAction::Keep).unwrap();
        s.undo_run(&b).unwrap();
        assert!(s.inner.lock().unwrap().requests.is_empty());
        assert!(s.apply_edits(&b, "same", rename("Second"), None).unwrap_err().to_string().contains("INVALID_RUN"));
    }
}

#[test]
fn revision_tracks_project_changes_only() {
    let f = Fixture::new();
    let s = f.open();
    let a = s.begin_run("empty".into()).unwrap();
    assert_eq!(a.stamp.revision, 0);
    assert_eq!(s.end_run(&a.run_id, EndAction::Keep).unwrap().stamp.revision, 0);
    assert_eq!(s.undo_run(&a.run_id).unwrap().stamp.revision, 0);
    let a = s.begin_run("discard empty".into()).unwrap();
    assert_eq!(s.end_run(&a.run_id, EndAction::Discard).unwrap().stamp.revision, 0);
    let a = s.begin_run("edit".into()).unwrap();
    apply(&s, &a.run_id, "Changed");
    assert_eq!(s.state().unwrap().stamp.revision, 1);
    apply(&s, &a.run_id, "Changed");
    assert_eq!(s.end_run(&a.run_id, EndAction::Keep).unwrap().stamp.revision, 1);
    assert_eq!(s.undo_run(&a.run_id).unwrap().stamp.revision, 2);
    let a = s.begin_run("discard".into()).unwrap();
    assert_eq!(a.stamp.revision, 2);
    apply(&s, &a.run_id, "Other");
    assert_eq!(s.end_run(&a.run_id, EndAction::Discard).unwrap().stamp.revision, 4);
}

fn checkpoint(f: &Fixture, project: &Project) -> PathBuf {
    let path = storage::sidecar(&f.0, ".checkpoint.json");
    storage::save(&path, &serde_json::json!({"project": project})).unwrap();
    path
}

#[test]
fn recovery_keep_preserves_file_and_restore_validates_and_saves() {
    for action in [RecoveryAction::Keep, RecoveryAction::Restore] {
        let f = Fixture::new();
        let path = checkpoint(&f, &Project::new("Before"));
        let bytes = fs::read(&f.0).unwrap();
        let s = f.open();
        assert!(s.begin_run("blocked".into()).is_err());
        let stamp = s.resolve_recovery(action).unwrap();
        match action {
            RecoveryAction::Keep => {
                assert_eq!(fs::read(&f.0).unwrap(), bytes);
                assert_eq!(stamp.revision, 0);
            }
            RecoveryAction::Restore => {
                assert_eq!(f.disk().name, "Before");
                assert_eq!(stamp.revision, 1);
            }
        }
        assert_eq!(s.state().unwrap().project, f.disk());
        assert!(s.state().unwrap().recovery_checkpoint.is_none());
        assert!(!path.exists());
        assert!(s.begin_run("unblocked".into()).is_ok());
    }
    let f = Fixture::new();
    checkpoint(&f, &f.disk());
    assert_eq!(f.open().resolve_recovery(RecoveryAction::Restore).unwrap().revision, 0);
}

#[test]
fn recovery_refuses_readonly_open_run_missing_and_invalid_checkpoint() {
    let f = Fixture::new();
    let mut invalid = Project::new("Invalid");
    invalid.canvas.fps = 0;
    let path = checkpoint(&f, &invalid);
    let s = ProjectSession::open(&f.0, Mode::ReadOnly).unwrap();
    for action in [RecoveryAction::Keep, RecoveryAction::Restore] {
        assert!(s.resolve_recovery(action).unwrap_err().to_string().contains("READ_ONLY"));
    }
    drop(s);
    let s = f.open();
    assert!(s.resolve_recovery(RecoveryAction::Restore).unwrap_err().to_string().contains("INVALID_PROJECT"));
    fs::write(&path, b"broken json").unwrap();
    assert!(s.resolve_recovery(RecoveryAction::Restore).is_err());
    assert_eq!(f.disk().name, "Original");
    assert_eq!(s.state().unwrap().stamp.revision, 0);
    assert!(path.exists());
    s.resolve_recovery(RecoveryAction::Keep).unwrap();
    for action in [RecoveryAction::Keep, RecoveryAction::Restore] {
        assert!(s.resolve_recovery(action).unwrap_err().to_string().contains("NO_RECOVERY"));
    }
    s.begin_run("open".into()).unwrap();
    for action in [RecoveryAction::Keep, RecoveryAction::Restore] {
        assert!(s.resolve_recovery(action).unwrap_err().to_string().contains("RUN_BUSY"));
    }
}

#[test]
fn add_short_video_validates_one_frame_source_overrun() {
    let mut a = asset();
    a.duration_us = 10_000;
    let mut project = Project::new("Short video");
    project.canvas.fps = 30;
    let mut editor = Editor::new(project);
    editor.apply_batch(vec![
        EditCmd::AddAssets { assets: vec![a] },
        EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None },
    ], None).unwrap();
    assert_eq!(editor.project.tracks[0].clips[0].duration_us, 33_334);
    validate(&editor.project).unwrap();
    editor.project.tracks[0].clips[0].duration_us = 43_335;
    assert!(validate(&editor.project).is_err());
}
