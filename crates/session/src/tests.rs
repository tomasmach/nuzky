use super::*;
use nuzky_engine::model::{Adjust, Asset, AssetKind, Clip, ClipContent, Transform};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("nuzky-session-{}", new_id()));
        fs::create_dir_all(&dir).unwrap();
        // The session resolves symlinks in the project path; macOS's temp dir sits under the /var symlink.
        let path = fs::canonicalize(dir).unwrap().join("project.nuzky");
        storage::save(&path, &Project::new("Original")).unwrap();
        Self(path)
    }
    fn open(&self) -> ProjectSession {
        ProjectSession::open(&self.0, Mode::Write, None).unwrap()
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
    session.apply_edits(run, &new_id(), rename(name), Expect::default()).unwrap();
}

#[test]
fn lock_excludes_writers_and_releases_on_drop() {
    let f = Fixture::new();
    let s = f.open();
    assert!(ProjectSession::open(&f.0, Mode::Write, None).err().unwrap().to_string().contains("PROJECT_BUSY"));
    drop(s);
    assert!(ProjectSession::open(&f.0, Mode::Write, None).is_ok());
}

#[test]
fn readonly_refuses_all_mutations() {
    let f = Fixture::new();
    let s = ProjectSession::open(&f.0, Mode::ReadOnly, None).unwrap();
    assert!(s.begin_run("test".into()).unwrap_err().to_string().contains("READ_ONLY"));
    assert!(s.apply_edits("a", "b", rename("changed"), Expect::default()).is_err());
    assert!(s.end_run("a", EndAction::Keep).is_err());
    assert!(s.undo_run("a").is_err());
    assert!(s.edit(rename("Bad"), None, Expect::default()).unwrap_err().to_string().contains("READ_ONLY"));
    assert!(s.undo().unwrap_err().to_string().contains("READ_ONLY"));
    assert!(s.redo().unwrap_err().to_string().contains("READ_ONLY"));
    assert!(s.stop_run().unwrap_err().to_string().contains("READ_ONLY"));
    assert_eq!(f.disk().name, "Original");
}

#[test]
fn run_checkpoints_saves_each_batch_and_keeps_one_undo_entry() {
    let f = Fixture::new();
    let s = f.open();
    let run = s.begin_run("AI edit".into()).unwrap();
    let checkpoint: Value =
        serde_json::from_slice(&fs::read(storage::sidecar(&f.0, ".checkpoint.json")).unwrap()).unwrap();
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
    let first =
        s.apply_edits(&run.run_id, "req", rename("First"), Expect { revision: rev, speech_layout_key: None }).unwrap();
    apply(&s, &run.run_id, "Second");
    let retry =
        s.apply_edits(&run.run_id, "req", rename("First"), Expect { revision: rev, speech_layout_key: None }).unwrap();
    assert_eq!(serde_json::to_value(first).unwrap(), serde_json::to_value(retry).unwrap());
    assert_eq!(f.disk().name, "Second");
    assert!(
        s.apply_edits(&run.run_id, "req", rename("Other"), Expect { revision: rev, speech_layout_key: None })
            .unwrap_err()
            .to_string()
            .contains("REQUEST_CONFLICT")
    );
    s.end_run(&run.run_id, EndAction::Keep).unwrap();
    assert!(
        s.apply_edits(&run.run_id, "req", rename("First"), Expect { revision: rev, speech_layout_key: None })
            .unwrap_err()
            .to_string()
            .contains("INVALID_RUN")
    );
}

#[test]
fn stale_revision_and_wrong_or_expired_run_do_not_mutate() {
    let f = Fixture::new();
    let s = f.open();
    let r = s.begin_run("a".into()).unwrap();
    assert!(
        s.apply_edits(
            &r.run_id,
            "a",
            rename("Bad"),
            Expect { revision: Some(r.stamp.revision + 1), speech_layout_key: None }
        )
        .unwrap_err()
        .to_string()
        .contains("STALE_REVISION")
    );
    assert!(s.apply_edits("wrong", "a", rename("Bad"), Expect::default()).is_err());
    s.end_run(&r.run_id, EndAction::Keep).unwrap();
    assert!(s.apply_edits(&r.run_id, "a", rename("Bad"), Expect::default()).is_err());
    assert_eq!(f.disk().name, "Original");
}

#[test]
fn batch_and_post_validation_errors_roll_back_memory_disk_and_revision() {
    let f = Fixture::new();
    let s = f.open();
    let r = s.begin_run("a".into()).unwrap();
    let mut edits = rename("Bad");
    edits.push(EditCmd::SplitClip { clip_id: "missing".into(), at_us: 1 });
    assert!(s.apply_edits(&r.run_id, "fail", edits, Expect::default()).is_err());
    let mut a = asset();
    a.duration_us = -1;
    assert!(s.apply_edits(&r.run_id, "fail", vec![EditCmd::AddAssets { assets: vec![a] }], Expect::default()).is_err());
    assert_eq!(s.state().unwrap().project, f.disk());
    assert_eq!(s.state().unwrap().stamp.revision, r.stamp.revision);
    assert_eq!(f.disk().name, "Original");
    s.apply_edits(&r.run_id, "fail", rename("retry"), Expect::default()).unwrap();
}

#[test]
fn save_failure_keeps_live_commit_and_retry_does_not_reapply() {
    let f = Fixture::new();
    let s = f.open();
    let r = s.begin_run("a".into()).unwrap();
    let backup = f.0.with_extension("backup");
    fs::rename(&f.0, &backup).unwrap();
    fs::create_dir(&f.0).unwrap();
    assert!(s.apply_edits(&r.run_id, "retry", rename("Changed"), Expect::default()).is_err());
    assert_eq!(s.state().unwrap().project.name, "Changed");
    assert_eq!(s.state().unwrap().stamp.revision, 1);
    fs::remove_dir(&f.0).unwrap();
    fs::rename(backup, &f.0).unwrap();
    s.apply_edits(&r.run_id, "retry", rename("Changed"), Expect::default()).unwrap();
}

#[test]
fn continuous_editing_still_saves_within_the_maximum_wait() {
    use std::sync::mpsc;
    let f = Fixture::new();
    let (tx, rx) = mpsc::channel();
    let s = ProjectSession::open(&f.0, Mode::Write, Some(tx)).unwrap();
    let began = Instant::now();
    for i in 0.. {
        s.edit(rename(&format!("Edit {i}")), Some("typing".into()), Expect::default()).unwrap();
        if rx.try_iter().any(|event| matches!(event, SessionEvent::Saved { error: None, .. })) {
            break;
        }
        assert!(began.elapsed() < SAVE_MAX_WAIT + Duration::from_secs(1), "edits faster than the debounce never saved");
        std::thread::sleep(SAVE_DEBOUNCE / 4);
    }
    assert!(began.elapsed() >= SAVE_MAX_WAIT.min(SAVE_DEBOUNCE * 2), "saved before the debounce could apply");
    assert_ne!(f.disk().name, "Original");
}

#[test]
fn begin_run_saves_pending_user_edits_before_its_checkpoint() {
    let f = Fixture::new();
    let s = f.open();
    s.edit(rename("User typing"), Some("typing".into()), Expect::default()).unwrap();
    assert_eq!(f.disk().name, "Original");
    s.begin_run("AI".into()).unwrap();
    // A crash now must not lose the user's edit when the user keeps the file.
    assert_eq!(f.disk().name, "User typing");
}

#[test]
fn only_the_run_owner_postpones_idle_auto_keep() {
    let f = Fixture::new();
    let timeout = Duration::from_millis(300);
    let s = ProjectSession::open_with_idle_timeout(&f.0, Mode::Write, timeout, None).unwrap();
    let run = s.begin_run("abandoned".into()).unwrap().run_id;
    apply(&s, &run, "Agent");
    // The UI keeps reading the project and the user keeps trying to edit.
    let began = Instant::now();
    while s.state().unwrap().open_run.is_some() {
        assert!(began.elapsed() < timeout * 5, "UI activity kept the abandoned run open");
        s.view().unwrap();
        s.stamp();
        let _ = s.edit(rename("User"), None, Expect::default());
        let _ = s.apply_edits("someone-else", &new_id(), rename("Other"), Expect::default());
        s.touch_run(|_| false).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    let run = s.begin_run("active".into()).unwrap().run_id;
    let began = Instant::now();
    while began.elapsed() < timeout * 3 {
        s.touch_run(|id| id == run).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(s.state().unwrap().open_run.map(|r| r.run_id), Some(run.clone()));
    apply(&s, &run, "Still open");
}

#[test]
fn idle_timeout_and_disconnect_keep_edits() {
    let f = Fixture::new();
    let s = ProjectSession::open_with_idle_timeout(&f.0, Mode::Write, Duration::from_millis(30), None).unwrap();
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
        mirror: false,
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
            clean_voice: false,
            shape: None,
            duck_db: 0.0,
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
                EditCmd::AddAssets { assets: vec![asset()] },
                EditCmd::AddClip { asset_id: "a".into(), start_us: Some(9_000_000), track_id: None },
            ],
            Expect::default(),
        )
        .unwrap();
    assert_eq!(result.outcome.created.len(), 1);
    let id = result.outcome.created[0].clone();
    assert_eq!(result.clips[0].clip.start_us, 0);
    let result = s
        .apply_edits(&r, "split", vec![EditCmd::SplitClip { clip_id: id.clone(), at_us: 5_000_000 }], Expect::default())
        .unwrap();
    assert_eq!(result.changed, vec![id.clone()]);
    assert_eq!(result.outcome.created.len(), 1);
    let result = s
        .apply_edits(&r, "delete", vec![EditCmd::DeleteClips { clip_ids: vec![id.clone()] }], Expect::default())
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
    assert_eq!(f.disk().name, "Original");
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
                EditCmd::AddAssets { assets: vec![asset()] },
                EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None },
            ],
            Expect::default(),
        )
        .unwrap();
    let id = result.outcome.created[0].clone();
    let result = s
        .apply_edits(
            &r,
            "remove",
            vec![
                EditCmd::MoveClip { clip_id: id.clone(), track_id: Some("main".into()), start_us: 0 },
                EditCmd::DeleteClips { clip_ids: vec![id] },
            ],
            Expect::default(),
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
            EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None },
            EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None },
        ],
        Expect::default(),
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
                EditCmd::AddAssets { assets: vec![asset()] },
                EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None },
            ],
            Expect::default(),
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
                    nuzky_engine::model::Keyframe {
                        t_us: 0,
                        transform: Transform::default(),
                        ease: Default::default(),
                    },
                    nuzky_engine::model::Keyframe {
                        t_us: 10_000_000,
                        transform: Transform { scale: 2.0, ..Transform::default() },
                        ease: Default::default(),
                    },
                ],
            },
            EditCmd::TrimClip { clip_id: id.clone(), start_us: 0, duration_us: 5_000_000, source_in_us: None },
            EditCmd::TrimClip {
                clip_id: id,
                start_us: 1_000_000,
                duration_us: 4_000_000,
                source_in_us: Some(1_000_000),
            },
        ],
        Expect::default(),
    )
    .unwrap();
    let project = s.state().unwrap().project;
    let clip = &project.tracks[0].clips[0];
    assert_eq!(clip.keyframes[0].t_us, -1_000_000);
    assert!(clip.keyframes[1].t_us > clip.duration_us);
    assert_eq!(project, f.disk());
}

#[test]
fn readonly_sessions_take_no_lock_and_reload_writer_changes() {
    let f = Fixture::new();
    let reader = ProjectSession::open(&f.0, Mode::ReadOnly, None).unwrap();
    let writer = f.open();
    let second_reader = ProjectSession::open(&f.0, Mode::ReadOnly, None).unwrap();
    let run = writer.begin_run("writer".into()).unwrap();
    apply(&writer, &run.run_id, "Written");
    assert_eq!(reader.state().unwrap().project, writer.state().unwrap().project);
    assert_eq!(second_reader.state().unwrap().project, f.disk());
    assert_eq!(reader.state().unwrap().stamp.revision, 1);
    assert_eq!(reader.state().unwrap().stamp.revision, 1);
}

#[test]
fn requests_are_scoped_to_open_run_and_cleared_on_finish_and_undo() {
    let f = Fixture::new();
    let s = f.open();
    for action in [EndAction::Keep, EndAction::Discard] {
        let a = s.begin_run("first".into()).unwrap().run_id;
        s.apply_edits(&a, "same", rename("First"), Expect::default()).unwrap();
        s.end_run(&a, action).unwrap();
        assert!(s.inner.lock().unwrap().requests.is_empty());
        let b = s.begin_run("second".into()).unwrap().run_id;
        assert!(
            s.apply_edits(&a, "same", rename("First"), Expect::default())
                .unwrap_err()
                .to_string()
                .contains("INVALID_RUN")
        );
        s.apply_edits(&b, "same", rename("Second"), Expect::default()).unwrap();
        s.end_run(&b, EndAction::Keep).unwrap();
        s.undo_run(&b).unwrap();
        assert!(s.inner.lock().unwrap().requests.is_empty());
        assert!(
            s.apply_edits(&b, "same", rename("Second"), Expect::default())
                .unwrap_err()
                .to_string()
                .contains("INVALID_RUN")
        );
    }
}

#[test]
fn revision_tracks_project_changes_only() {
    let f = Fixture::new();
    let s = f.open();
    let a = s.begin_run("empty".into()).unwrap();
    assert_eq!(a.stamp.revision, 0);
    assert_eq!(s.end_run(&a.run_id, EndAction::Keep).unwrap().stamp.revision, 0);
    assert!(s.undo_run(&a.run_id).is_err());
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
    let s = ProjectSession::open(&f.0, Mode::ReadOnly, None).unwrap();
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
    editor
        .apply_batch(
            vec![
                EditCmd::AddAssets { assets: vec![a] },
                EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None },
            ],
            None,
        )
        .unwrap();
    assert_eq!(editor.project.tracks[0].clips[0].duration_us, 33_334);
    validate(&editor.project).unwrap();
    editor.project.tracks[0].clips[0].duration_us = 43_335;
    assert!(validate(&editor.project).is_err());
}

#[test]
fn user_run_user_share_history_and_redo_restores_whole_run() {
    let f = Fixture::new();
    let s = f.open();
    s.edit(rename("User before"), Some("typing".into()), Expect::default()).unwrap();
    let run = s.begin_run("AI".into()).unwrap().run_id;
    apply(&s, &run, "AI first");
    apply(&s, &run, "AI last");
    s.end_run(&run, EndAction::Keep).unwrap();
    s.edit(rename("User after"), Some("typing".into()), Expect::default()).unwrap();
    assert!(s.undo_run(&run).is_err());
    for expected in ["AI last", "User before", "Original"] {
        s.undo().unwrap();
        assert_eq!(s.state().unwrap().project.name, expected);
    }
    s.redo().unwrap();
    s.redo().unwrap();
    assert_eq!(s.state().unwrap().project.name, "AI last");
    assert_eq!(s.state().unwrap().undo_run_id, Some(run.clone()));
    s.undo_run(&run).unwrap();
    assert_eq!(f.disk().name, "User before");
    s.redo().unwrap();
    assert_eq!(s.state().unwrap().project.name, "AI last");
    s.redo().unwrap();
    assert_eq!(s.state().unwrap().project.name, "User after");
}

#[test]
fn discard_preserves_user_history_and_has_no_redo() {
    let f = Fixture::new();
    let s = f.open();
    s.edit(rename("User"), None, Expect::default()).unwrap();
    let run = s.begin_run("AI".into()).unwrap().run_id;
    apply(&s, &run, "AI");
    s.end_run(&run, EndAction::Discard).unwrap();
    assert_eq!(f.disk(), s.state().unwrap().project);
    let revision = s.state().unwrap().stamp.revision;
    assert_eq!(s.redo().unwrap().revision, revision);
    assert_eq!(s.state().unwrap().project.name, "User");
    s.undo().unwrap();
    assert_eq!(s.state().unwrap().project.name, "Original");
}

#[test]
fn active_run_blocks_user_edits_undo_redo_and_stop_revokes_late_calls() {
    let f = Fixture::new();
    let s = f.open();
    let run = s.begin_run("AI".into()).unwrap().run_id;
    assert!(s.edit(rename("Bad"), None, Expect::default()).unwrap_err().to_string().contains("RUN_ACTIVE"));
    assert!(s.undo().unwrap_err().to_string().contains("RUN_ACTIVE"));
    assert!(s.redo().unwrap_err().to_string().contains("RUN_ACTIVE"));
    apply(&s, &run, "AI");
    s.stop_run().unwrap();
    assert_eq!(f.disk(), s.state().unwrap().project);
    let next = s.begin_run("Next".into()).unwrap().run_id;
    assert!(
        s.apply_edits(&run, "late", rename("Bad"), Expect::default()).unwrap_err().to_string().contains("RUN_STOPPED")
    );
    s.end_run(&next, EndAction::Keep).unwrap();
    s.undo().unwrap();
    assert_eq!(s.state().unwrap().project.name, "Original");
}

#[test]
fn speech_expectation_allows_caption_restyle_but_rejects_moved_speech() {
    let f = Fixture::new();
    let mut project = f.disk();
    project.assets.push(asset());
    project.tracks[0].clips.push(media_clip());
    storage::save(&f.0, &project).unwrap();
    let s = f.open();
    let key = s.state().unwrap().speech_layout_key;
    let caption: EditCmd = serde_json::from_value(serde_json::json!({
        "type": "addText", "startUs": 0, "text": "Caption",
        "style": {"fontSize": 64, "color": "#ffffff", "bold": false, "strokeWidth": 0, "strokeColor": "#000000"}
    }))
    .unwrap();
    let id = s.edit(vec![caption], None, Expect::default()).unwrap().outcome.created[0].clone();
    let restyle = serde_json::from_value(serde_json::json!({"type": "updateClip", "clipId": id,
        "style": {"fontSize": 80, "color": "#ffffff", "bold": true, "strokeWidth": 0, "strokeColor": "#000000"}
    }))
    .unwrap();
    s.edit(vec![restyle], None, Expect { revision: None, speech_layout_key: Some(key.clone()) }).unwrap();
    let run = s.begin_run("speech".into()).unwrap().run_id;
    s.apply_edits(
        &run,
        "caption",
        rename("Unrelated"),
        Expect { revision: None, speech_layout_key: Some(key.clone()) },
    )
    .unwrap();
    let speed = serde_json::from_value(serde_json::json!({"type": "updateClip", "clipId": "c", "speed": 2})).unwrap();
    s.apply_edits(&run, "speed", vec![speed], Expect { revision: None, speech_layout_key: Some(key.clone()) }).unwrap();
    assert!(
        s.apply_edits(&run, "stale", rename("Bad"), Expect { revision: None, speech_layout_key: Some(key.clone()) })
            .unwrap_err()
            .to_string()
            .contains("SPEECH_CHANGED")
    );
    s.end_run(&run, EndAction::Keep).unwrap();
    assert!(
        s.edit(rename("Bad"), None, Expect { revision: None, speech_layout_key: Some(key) })
            .unwrap_err()
            .to_string()
            .contains("SPEECH_CHANGED")
    );
}

#[test]
fn user_validation_failure_preserves_history_redo_revision_and_coalescing() {
    let f = Fixture::new();
    let s = f.open();
    s.edit(rename("First"), Some("typing".into()), Expect::default()).unwrap();
    let before = s.state().unwrap();
    let mut bad = asset();
    bad.duration_us = -1;
    let invalid = vec![EditCmd::AddAssets { assets: vec![bad] }];
    assert!(s.edit(invalid.clone(), None, Expect::default()).is_err());
    assert_eq!(s.state().unwrap().project, before.project);
    assert_eq!(s.state().unwrap().stamp.revision, before.stamp.revision);
    s.edit(rename("Second"), Some("typing".into()), Expect::default()).unwrap();
    s.undo().unwrap();
    assert_eq!(s.state().unwrap().project.name, "Original");
    assert!(s.edit(invalid, None, Expect::default()).is_err());
    s.redo().unwrap();
    assert_eq!(s.state().unwrap().project.name, "Second");
}

#[test]
fn user_autosave_debounces_and_events_follow_commits_with_origins() {
    use std::sync::mpsc;
    let f = Fixture::new();
    let (tx, rx) = mpsc::channel();
    let s = ProjectSession::open(&f.0, Mode::Write, Some(tx)).unwrap();
    s.edit(rename("User first"), Some("typing".into()), Expect::default()).unwrap();
    s.edit(rename("User last"), Some("typing".into()), Expect::default()).unwrap();
    assert_eq!(f.disk().name, "Original");
    for revision in [1, 2] {
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), SessionEvent::Changed { revision: r, origin: Origin::User } if r == revision)
        );
    }
    assert!(matches!(
        rx.recv_timeout(SAVE_DEBOUNCE + Duration::from_secs(2)).unwrap(),
        SessionEvent::Saved { revision: 2, error: None }
    ));
    assert_eq!(f.disk(), s.state().unwrap().project);
    let run = s.begin_run("AI".into()).unwrap().run_id;
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Saved { revision: 2, error: None }));
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Run(Some(info)) if info.run_id == run && info.label == "AI"));
    apply(&s, &run, "AI");
    assert!(
        matches!(rx.recv().unwrap(), SessionEvent::Changed { revision: 3, origin: Origin::Run { run_id, label } } if run_id == run && label == "AI")
    );
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Saved { revision: 3, error: None }));
    assert_eq!(f.disk(), s.state().unwrap().project);
    s.end_run(&run, EndAction::Keep).unwrap();
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Saved { revision: 3, error: None }));
    // What the run changed comes just before the run ends, so the UI can show it with the run.
    assert!(matches!(rx.recv().unwrap(), SessionEvent::RunSummary(changes) if !changes.is_empty()));
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Run(None)));
    s.undo_run(&run).unwrap();
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Changed { revision: 4, origin: Origin::Undo }));
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Saved { revision: 4, error: None }));
    s.redo().unwrap();
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Changed { revision: 5, origin: Origin::Redo }));
}

#[test]
fn agent_outcomes_never_replace_ui_context() {
    let f = Fixture::new();
    let s = f.open();
    s.set_ui_context(vec!["user-selection".into()], 123_000);
    let run = s.begin_run("AI".into()).unwrap().run_id;
    s.apply_edits(
        &run,
        "clip",
        vec![
            EditCmd::AddAssets { assets: vec![asset()] },
            EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None },
        ],
        Expect::default(),
    )
    .unwrap();
    s.end_run(&run, EndAction::Discard).unwrap();
    let state = s.state().unwrap();
    assert_eq!(state.selection, vec!["user-selection"]);
    assert_eq!(state.playhead_us, 123_000);
}

#[test]
fn autosave_failure_emits_error_and_next_change_retries() {
    let f = Fixture::new();
    let (tx, rx) = std::sync::mpsc::channel();
    let s = ProjectSession::open(&f.0, Mode::Write, Some(tx)).unwrap();
    fs::remove_file(&f.0).unwrap();
    fs::create_dir(&f.0).unwrap();
    s.edit(rename("First"), None, Expect::default()).unwrap();
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Changed { .. }));
    assert!(matches!(
        rx.recv_timeout(SAVE_DEBOUNCE + Duration::from_secs(2)).unwrap(),
        SessionEvent::Saved { revision: 1, error: Some(_) }
    ));
    fs::remove_dir(&f.0).unwrap();
    s.edit(rename("Second"), None, Expect::default()).unwrap();
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Changed { .. }));
    assert!(matches!(
        rx.recv_timeout(SAVE_DEBOUNCE + Duration::from_secs(2)).unwrap(),
        SessionEvent::Saved { revision: 2, error: None }
    ));
    assert_eq!(f.disk(), s.state().unwrap().project);
}

#[test]
fn failed_run_flush_retries_created_clip_without_duplicates() {
    let f = Fixture::new();
    let (tx, rx) = std::sync::mpsc::channel();
    let s = ProjectSession::open(&f.0, Mode::Write, Some(tx)).unwrap();
    let run = s.begin_run("retry".into()).unwrap().run_id;
    let edits = vec![
        EditCmd::AddAssets { assets: vec![asset()] },
        EditCmd::AddClip { asset_id: "a".into(), start_us: None, track_id: None },
    ];
    fs::remove_file(&f.0).unwrap();
    fs::create_dir(&f.0).unwrap();
    assert!(s.apply_edits(&run, "clip", edits.clone(), Expect::default()).is_err());
    let committed = s.state().unwrap();
    assert_eq!(committed.project.tracks[0].clips.len(), 1);
    assert_eq!(committed.stamp.revision, 1);
    assert!(rx.try_iter().any(|event| matches!(event, SessionEvent::Saved { revision: 1, error: Some(_) })));
    fs::remove_dir(&f.0).unwrap();
    let result = s.apply_edits(&run, "clip", edits, Expect::default()).unwrap();
    assert_eq!(result.outcome.created, vec![committed.project.tracks[0].clips[0].id.clone()]);
    assert_eq!(result.stamp.revision, 1);
    assert_eq!(f.disk(), committed.project);
    assert_eq!(s.state().unwrap().project, committed.project);
}

#[test]
fn failed_discard_blocks_more_batches_and_retry_cannot_change_its_action() {
    let f = Fixture::new();
    let s = f.open();
    s.edit(rename("User"), None, Expect::default()).unwrap();
    let run = s.begin_run("discard".into()).unwrap().run_id;
    apply(&s, &run, "AI");
    fs::remove_file(&f.0).unwrap();
    fs::create_dir(&f.0).unwrap();
    assert!(s.end_run(&run, EndAction::Discard).is_err());
    assert_eq!(s.state().unwrap().project.name, "User");
    assert!(
        s.apply_edits(&run, "late", rename("Bad"), Expect::default()).unwrap_err().to_string().contains("RUN_ENDING")
    );
    fs::remove_dir(&f.0).unwrap();
    s.end_run(&run, EndAction::Keep).unwrap();
    assert_eq!(f.disk().name, "User");
    s.redo().unwrap();
    assert_eq!(s.state().unwrap().project.name, "User");
    s.undo().unwrap();
    assert_eq!(s.state().unwrap().project.name, "Original");
}

#[test]
fn stale_user_revision_and_reserved_run_key_do_not_commit() {
    let f = Fixture::new();
    let s = f.open();
    assert!(
        s.edit(rename("Bad"), None, Expect { revision: Some(1), speech_layout_key: None })
            .unwrap_err()
            .to_string()
            .contains("STALE_REVISION")
    );
    assert!(
        s.edit(rename("Bad"), Some("run:spoof".into()), Expect::default())
            .unwrap_err()
            .to_string()
            .contains("INVALID_REQUEST")
    );
    assert_eq!(s.state().unwrap().stamp.revision, 0);
    assert_eq!(s.state().unwrap().project, f.disk());
}

#[test]
fn recovery_restore_is_undoable_and_emits_recovery_before_saved() {
    let f = Fixture::new();
    checkpoint(&f, &Project::new("Before run"));
    let original = f.disk();
    let (tx, rx) = std::sync::mpsc::channel();
    let s = ProjectSession::open(&f.0, Mode::Write, Some(tx)).unwrap();
    assert!(s.edit(rename("Bad"), None, Expect::default()).unwrap_err().to_string().contains("RECOVERY_PENDING"));
    s.resolve_recovery(RecoveryAction::Restore).unwrap();
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Changed { revision: 1, origin: Origin::Recovery }));
    assert!(matches!(rx.recv().unwrap(), SessionEvent::Saved { revision: 1, error: None }));
    assert_eq!(f.disk(), s.state().unwrap().project);
    assert_eq!(f.disk().name, "Before run");
    s.undo().unwrap();
    assert_eq!(s.state().unwrap().project, original);
    s.redo().unwrap();
    assert_eq!(s.state().unwrap().project.name, "Before run");
}

#[test]
fn recovery_save_retry_preserves_one_undo_step_and_checkpoint() {
    let f = Fixture::new();
    let original = f.disk();
    let mut restored = Project::new("Before run");
    restored.assets.push(asset());
    restored.tracks[0].clips.push(media_clip());
    let checkpoint_path = checkpoint(&f, &restored);
    let s = f.open();
    fs::remove_file(&f.0).unwrap();
    fs::create_dir(&f.0).unwrap();
    assert!(s.resolve_recovery(RecoveryAction::Restore).unwrap_err().to_string().contains("SAVE_FAILED"));
    assert_eq!(s.state().unwrap().project, restored);
    assert_eq!(s.state().unwrap().stamp.revision, 1);
    assert!(checkpoint_path.exists());
    assert!(s.undo().unwrap_err().to_string().contains("RECOVERY_PENDING"));
    fs::remove_dir(&f.0).unwrap();
    assert_eq!(s.resolve_recovery(RecoveryAction::Restore).unwrap().revision, 1);
    assert!(!checkpoint_path.exists());
    assert_eq!(f.disk(), restored);
    assert_eq!(s.undo().unwrap().revision, 2);
    assert_eq!(s.state().unwrap().project, original);
    assert_eq!(s.undo().unwrap().revision, 2);
    assert_eq!(s.redo().unwrap().revision, 3);
    assert_eq!(s.state().unwrap().project, restored);
}

#[test]
fn view_tracks_history_and_project_together() {
    let f = Fixture::new();
    let s = f.open();
    let (_, undo, redo) = s.view().unwrap();
    assert!(!undo && !redo);
    s.edit(rename("Changed"), None, Expect::default()).unwrap();
    let (state, undo, redo) = s.view().unwrap();
    assert_eq!(state.project.name, "Changed");
    assert!(undo && !redo);
    s.undo().unwrap();
    let (state, undo, redo) = s.view().unwrap();
    assert_eq!(state.project.name, "Original");
    assert!(!undo && redo);
}

#[test]
fn oversized_agent_text_is_rejected_without_changing_state_or_history() {
    let f = Fixture::new();
    let session = f.open();
    let run = session.begin_run("unsafe text".into()).unwrap();
    let before = session.state().unwrap();
    let style = serde_json::from_value(serde_json::json!({"fontSize":1e20,"color":"#ffffff"})).unwrap();
    let result = session.apply_edits(
        &run.run_id,
        "oversized",
        vec![EditCmd::AddText { start_us: 0, text: "Too large".into(), style }],
        Expect::default(),
    );
    assert!(result.unwrap_err().to_string().contains("INVALID_PROJECT: text style"));
    let after = session.state().unwrap();
    assert_eq!(before.project, after.project);
    assert_eq!(before.stamp.revision, after.stamp.revision);
    assert_eq!(f.disk(), before.project);
}

fn labels(s: &ProjectSession) -> Vec<String> {
    s.list_history(MAX_VERSIONS).unwrap().versions.into_iter().map(|v| v.label).collect()
}

#[test]
fn versions_follow_steps_and_runs_and_survive_a_torn_file() {
    let f = Fixture::new();
    let s = f.open();
    s.edit(rename("Typed"), None, Expect::default()).unwrap();
    // One typing burst is one undo step, so one version.
    for name in ["B", "Bu", "Burst"] {
        s.edit(rename(name), Some("typing".into()), Expect::default()).unwrap();
    }
    // The next gesture ends it, so it stays a version of its own.
    for name in ["Dragged", "Dragged on"] {
        s.edit(rename(name), Some("drag".into()), Expect::default()).unwrap();
        // Reading in the middle of the drag shows it first, without splitting it into versions.
        let list = s.list_history(10).unwrap();
        assert_eq!((list.versions[0].label.as_str(), list.versions.len()), ("Rename project", 4));
        assert_eq!(list.versions[0].hash, list.current_hash);
    }
    let run = s.begin_run("Tighten".into()).unwrap().run_id;
    apply(&s, &run, "Run 1");
    apply(&s, &run, "Run 2");
    assert_eq!(s.list_history(10).unwrap().versions.len(), 4, "an open run is a version only once it ends");
    s.end_run(&run, EndAction::Keep).unwrap();
    let discarded = s.begin_run("Thrown away".into()).unwrap().run_id;
    apply(&s, &discarded, "Never kept");
    s.end_run(&discarded, EndAction::Discard).unwrap();
    s.undo().unwrap();
    assert_eq!(labels(&s), ["Undo", "Tighten", "Rename project", "Rename project", "Rename project", "Opened"]);
    let list = s.list_history(10).unwrap();
    assert_eq!(list.versions[1].run_id.as_deref(), Some(run.as_str()));
    assert_eq!(list.current_hash, list.versions[0].hash);
    assert_eq!(list.versions[0].hash, list.versions[2].hash, "the undo is back at the drag");
    drop(s);

    let file = storage::sidecar(&f.0, HISTORY_SUFFIX);
    let mut torn = fs::read_to_string(&file).unwrap();
    torn.push_str("{\"index\":9,\"hash\":\"ab");
    fs::write(&file, torn).unwrap();
    let s = f.open();
    assert_eq!(labels(&s), ["Undo", "Tighten", "Rename project", "Rename project", "Rename project", "Opened"]);
    s.edit(rename("After"), None, Expect::default()).unwrap();
    drop(s);
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.lines().all(|line| serde_json::from_str::<Value>(line).is_ok()), "the torn line was replaced");
    assert_eq!(labels(&f.open())[..2], ["Rename project", "Undo"]);
}

#[test]
fn versions_stay_bounded_and_a_tampered_one_is_refused() {
    let f = Fixture::new();
    let s = f.open();
    for i in 0..MAX_VERSIONS + 30 {
        s.edit(rename(&format!("Name {i}")), None, Expect::default()).unwrap();
    }
    let list = s.list_history(MAX_VERSIONS).unwrap();
    assert_eq!((list.versions.len(), list.older), (MAX_VERSIONS, 0));
    assert_eq!(list.versions[0].index, MAX_VERSIONS as u64 + 30, "indices stay as older versions go");
    let oldest = list.versions.last().unwrap().index;
    assert!(s.undo_to(Target::Index(oldest - 1)).unwrap_err().to_string().starts_with("UNKNOWN_VERSION"));
    drop(s);
    let file = storage::sidecar(&f.0, HISTORY_SUFFIX);
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.lines().count() <= 2 * MAX_VERSIONS, "the file is written again once it holds twice what is kept");
    let s = f.open();
    assert_eq!(s.list_history(MAX_VERSIONS).unwrap().versions.len(), MAX_VERSIONS);
    drop(s);

    // A version whose project was changed in the file no longer matches its hash, so it is dropped.
    fs::write(&file, text.replace("\"name\":\"Name 100\"", "\"name\":\"Forged\"")).unwrap();
    let s = f.open();
    let error = s.undo_to(Target::Index(101)).unwrap_err().to_string();
    assert!(error.starts_with("UNKNOWN_VERSION"), "{error}");
    assert_eq!(s.state().unwrap().project.name, format!("Name {}", MAX_VERSIONS + 29));
    // The same project made again is a sound version of its own.
    s.edit(rename("Name 100"), None, Expect::default()).unwrap();
    s.undo_to(Target::Index(102)).unwrap();
    assert_eq!(s.state().unwrap().project.name, "Name 101");
    s.undo_to(Target::Index(MAX_VERSIONS as u64 + 31)).unwrap();
    assert_eq!(s.state().unwrap().project.name, "Name 100");
}

/// The first save event with `revision` (or any, with None), and its error.
fn saved(rx: &std::sync::mpsc::Receiver<SessionEvent>, revision: Option<u64>) -> Option<String> {
    loop {
        if let SessionEvent::Saved { revision: at, error } =
            rx.recv_timeout(SAVE_DEBOUNCE + Duration::from_secs(2)).unwrap()
            && revision.is_none_or(|r| r == at)
        {
            return error;
        }
    }
}

#[test]
fn versions_that_cannot_be_written_show_as_not_saved_until_they_are() {
    let f = Fixture::new();
    let file = storage::sidecar(&f.0, HISTORY_SUFFIX);
    fs::create_dir(&file).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let s = ProjectSession::open(&f.0, Mode::Write, Some(tx)).unwrap();
    let error = saved(&rx, None).unwrap();
    assert!(error.contains("saving versions"), "{error}");
    s.edit(rename("First"), None, Expect::default()).unwrap();
    assert!(saved(&rx, Some(1)).is_some(), "the project is on disk, but its versions are not");
    fs::remove_dir(&file).unwrap();
    s.edit(rename("Second"), None, Expect::default()).unwrap();
    assert_eq!(saved(&rx, Some(2)), None);
    drop(s);
    assert_eq!(labels(&f.open()), ["Rename project", "Rename project", "Opened"], "nothing kept in memory was lost");
}

#[cfg(unix)]
#[test]
fn a_linked_versions_file_is_replaced_not_followed() {
    let f = Fixture::new();
    let file = storage::sidecar(&f.0, HISTORY_SUFFIX);
    drop(f.open());
    // Someone else's project folder: its versions file links to the versions of another project.
    let other = f.0.with_file_name("other.nuzky.history.jsonl");
    fs::rename(&file, &other).unwrap();
    std::os::unix::fs::symlink(&other, &file).unwrap();
    let before = fs::read(&other).unwrap();
    let s = f.open();
    s.edit(rename("Mine"), None, Expect::default()).unwrap();
    drop(s);
    assert_eq!(fs::read(&other).unwrap(), before);
    assert!(fs::symlink_metadata(&file).unwrap().is_file());
    assert_eq!(labels(&f.open()), ["Rename project", "Opened"]);
}
