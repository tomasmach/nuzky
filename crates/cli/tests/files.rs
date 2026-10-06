use std::path::PathBuf;
use std::process::Command;
use capopen_engine::{Project, edit::new_id};

fn fixture() -> (PathBuf, PathBuf, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("cli-files-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("project.capopen");
    let bytes = serde_json::to_vec(&Project::new("preserve me")).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    (dir, path, bytes)
}

#[test]
fn render_refuses_project_and_sidecars_including_symlinks() {
    let (dir, project, bytes) = fixture();
    for suffix in ["", ".lock", ".checkpoint.json", ".tmp"] {
        let out = dir.join(format!("project.capopen{suffix}"));
        let result = Command::new(env!("CARGO_BIN_EXE_capopen")).arg("render").arg(&project).arg(&out).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Output would overwrite"));
        #[cfg(unix)] {
            let alias = dir.join("alias.mp4");
            std::os::unix::fs::symlink(&out, &alias).unwrap();
            let result = Command::new(env!("CARGO_BIN_EXE_capopen")).arg("render").arg(&project).arg(&alias).output().unwrap();
            assert!(!result.status.success());
            assert!(String::from_utf8_lossy(&result.stderr).contains("Output would overwrite"));
            std::fs::remove_file(alias).unwrap();
        }
    }
    assert_eq!(std::fs::read(&project).unwrap(), bytes);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn new_refuses_existing_or_locked_project_and_publishes_complete_json() {
    let (dir, project, bytes) = fixture();
    let image = dir.join("image.ppm");
    std::fs::write(&image, b"P6\n2 2\n255\nabcdefghijkl").unwrap();
    let create = |path: &std::path::Path| Command::new(env!("CARGO_BIN_EXE_capopen")).arg("new").arg(path).arg(&image).output().unwrap();
    let lock = capopen_session::lock_project(&project, true).unwrap();
    let busy = create(&project);
    assert!(!busy.status.success());
    assert!(String::from_utf8_lossy(&busy.stderr).contains("PROJECT_BUSY"));
    drop(lock);
    let exists = create(&project);
    assert!(!exists.status.success());
    assert!(String::from_utf8_lossy(&exists.stderr).contains("already exists"));
    assert_eq!(std::fs::read(&project).unwrap(), bytes);
    let fresh = dir.join("fresh.capopen");
    let created = create(&fresh);
    assert!(created.status.success(), "{}", String::from_utf8_lossy(&created.stderr));
    let saved: Project = serde_json::from_slice(&std::fs::read(&fresh).unwrap()).unwrap();
    assert_eq!(saved.assets.len(), 1);
    assert_eq!(saved.tracks[0].clips.len(), 1);
    assert!(!std::fs::read_dir(&dir).unwrap().any(|p| p.unwrap().path().extension().is_some_and(|x| x == "tmp")));
    std::fs::remove_dir_all(dir).unwrap();
}
