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
