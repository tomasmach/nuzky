use nuzky_engine::{Project, edit::new_id};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> (PathBuf, PathBuf, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("cli-files-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("project.nuzky");
    let bytes = serde_json::to_vec(&Project::new("preserve me")).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    (dir, path, bytes)
}

/// `render` and `frame` take the same output checks; `frame` also needs a time.
fn write_output(command: &str, project: &Path, out: &Path) -> std::process::Output {
    let mut cli = Command::new(env!("CARGO_BIN_EXE_nuzky"));
    cli.arg(command).arg(project);
    if command == "frame" {
        cli.arg("0");
    }
    cli.arg(out).output().unwrap()
}

#[test]
fn render_and_frame_refuse_project_and_sidecars_including_symlinks() {
    let (dir, project, bytes) = fixture();
    for command in ["render", "frame"] {
        for suffix in ["", ".lock", ".checkpoint.json", ".tmp"] {
            let out = dir.join(format!("project.nuzky{suffix}"));
            let result = write_output(command, &project, &out);
            assert!(!result.status.success(), "{command} {suffix}");
            assert!(String::from_utf8_lossy(&result.stderr).contains("Output would overwrite"));
            #[cfg(unix)]
            {
                let alias = dir.join("alias.mp4");
                std::os::unix::fs::symlink(&out, &alias).unwrap();
                let result = write_output(command, &project, &alias);
                assert!(!result.status.success(), "{command} alias {suffix}");
                assert!(String::from_utf8_lossy(&result.stderr).contains("Output would overwrite"));
                std::fs::remove_file(alias).unwrap();
            }
        }
    }
    assert_eq!(std::fs::read(&project).unwrap(), bytes);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn frame_refuses_project_media_and_keeps_hard_links_intact() {
    let (dir, project, _) = fixture();
    let source = dir.join("source.mp4");
    std::fs::write(&source, b"source media").unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&std::fs::read(&project).unwrap()).unwrap();
    json["assets"] = serde_json::json!([{
        "id": "a", "name": "source.mp4", "path": source, "kind": "video", "durationUs": 1_000_000,
        "width": 64, "height": 64, "fps": 25.0, "hasAudio": false
    }]);
    std::fs::write(&project, serde_json::to_vec(&json).unwrap()).unwrap();
    let result = write_output("frame", &project, &source);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("is used in this project"));
    // A hard link is another name for the media; the frame replaces the name, not the file.
    let link = dir.join("link.png");
    std::fs::hard_link(&source, &link).unwrap();
    let empty = dir.join("empty.nuzky");
    std::fs::write(&empty, serde_json::to_vec(&Project::new("empty")).unwrap()).unwrap();
    let result = write_output("frame", &empty, &link);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(std::fs::read(&source).unwrap(), b"source media");
    assert!(std::fs::read(&link).unwrap().starts_with(b"\x89PNG"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn render_refuses_atomic_save_temporaries_and_legacy_names_through_aliases() {
    let (dir, project, bytes) = fixture();
    let outputs = [
        nuzky_session::json_temp_path(&project),
        nuzky_session::json_temp_path(&dir.join("project.nuzky.checkpoint.json")),
        dir.join("project.nuzky.future-sidecar"),
        dir.join("project.tmp"),
    ];
    let refused = |input: &std::path::Path, output: &std::path::Path| {
        let result = Command::new(env!("CARGO_BIN_EXE_nuzky")).arg("render").arg(input).arg(output).output().unwrap();
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("Output would overwrite"),
            "{}: {}",
            output.display(),
            String::from_utf8_lossy(&result.stderr)
        );
    };
    for output in outputs {
        refused(&project, &output);
        assert!(!output.exists());
        // An in-flight save already owns its temporary file; rendering must preserve its bytes.
        std::fs::write(&output, b"pending JSON save").unwrap();
        refused(&project, &output);
        assert_eq!(std::fs::read(&output).unwrap(), b"pending JSON save");
        #[cfg(unix)]
        {
            let input_alias = dir.join("input-alias.nuzky");
            let output_alias = dir.join("output-alias.mp4");
            std::os::unix::fs::symlink(&project, &input_alias).unwrap();
            std::os::unix::fs::symlink(output.file_name().unwrap(), &output_alias).unwrap();
            refused(&input_alias, &output_alias);
            std::fs::remove_file(&output).unwrap();
            // Also resolve a dangling output link to a not-yet-created save temporary.
            refused(&input_alias, &output_alias);
            std::fs::remove_file(input_alias).unwrap();
            std::fs::remove_file(output_alias).unwrap();
        }
    }
    #[cfg(unix)]
    {
        let alias = dir.join("directory-alias");
        std::os::unix::fs::symlink(&dir, &alias).unwrap();
        refused(&alias.join("project.nuzky"), &alias.join("project.nuzky.some-id.tmp"));
        let input_alias = dir.join("alternate.nuzky");
        std::os::unix::fs::symlink(&project, &input_alias).unwrap();
        refused(&input_alias, &dir.join("alternate.nuzky.some-id.tmp"));
        refused(&input_alias, &dir.join("alternate.tmp"));
        // A reserved destination remains reserved even when it is a symlink pointing outwards.
        let external = dir.join("unrelated.mp4");
        std::fs::write(&external, b"unrelated output").unwrap();
        let sidecar = dir.join("project.nuzky.outward.tmp");
        std::os::unix::fs::symlink(&external, &sidecar).unwrap();
        refused(&project, &sidecar);
        assert_eq!(std::fs::read(external).unwrap(), b"unrelated output");
    }
    let other = dir.join("other");
    std::fs::create_dir(&other).unwrap();
    for output in [dir.join("project.nuzky-movie.mp4"), other.join("project.nuzky.export.mp4")] {
        let result =
            Command::new(env!("CARGO_BIN_EXE_nuzky")).arg("render").arg(&project).arg(output).output().unwrap();
        // The empty fixture stops at export validation, after the output guard has accepted it.
        assert!(String::from_utf8_lossy(&result.stderr).contains("The timeline is empty"));
    }
    assert_eq!(std::fs::read(&project).unwrap(), bytes);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn new_refuses_existing_or_locked_project_and_publishes_complete_json() {
    let (dir, project, bytes) = fixture();
    let image = dir.join("image.ppm");
    std::fs::write(&image, b"P6\n2 2\n255\nabcdefghijkl").unwrap();
    let create = |path: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_nuzky")).arg("new").arg(path).arg(&image).output().unwrap()
    };
    let lock = nuzky_session::lock_project(&project, true).unwrap();
    let busy = create(&project);
    assert!(!busy.status.success());
    assert!(String::from_utf8_lossy(&busy.stderr).contains("PROJECT_BUSY"));
    drop(lock);
    let exists = create(&project);
    assert!(!exists.status.success());
    assert!(String::from_utf8_lossy(&exists.stderr).contains("already exists"));
    assert_eq!(std::fs::read(&project).unwrap(), bytes);
    let fresh = dir.join("fresh.nuzky");
    let created = create(&fresh);
    assert!(created.status.success(), "{}", String::from_utf8_lossy(&created.stderr));
    let saved: Project = serde_json::from_slice(&std::fs::read(&fresh).unwrap()).unwrap();
    assert_eq!(saved.assets.len(), 1);
    assert_eq!(saved.tracks[0].clips.len(), 1);
    assert!(!std::fs::read_dir(&dir).unwrap().any(|p| p.unwrap().path().extension().is_some_and(|x| x == "tmp")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn frame_and_render_reject_bad_sizes_and_invalid_projects_without_panicking() {
    let (dir, project, _) = fixture();
    let out = dir.join("frame.png");
    for (width, message) in
        [("0", "Frame width"), ("8", "Frame width"), ("100000", "Frame width"), ("7000", "Frame height")]
    {
        let result = Command::new(env!("CARGO_BIN_EXE_nuzky"))
            .arg("frame")
            .arg(&project)
            .arg("0")
            .arg(&out)
            .arg(width)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1), "width {width}");
        assert!(String::from_utf8_lossy(&result.stderr).contains(message), "width {width}");
    }
    let mut json: serde_json::Value = serde_json::from_slice(&std::fs::read(&project).unwrap()).unwrap();
    json["canvas"]["background"] = serde_json::json!("#€");
    std::fs::write(&project, serde_json::to_vec(&json).unwrap()).unwrap();
    for command in ["frame", "render"] {
        let result = write_output(command, &project, &dir.join(format!("out-{command}")));
        assert_eq!(result.status.code(), Some(1), "{command}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("INVALID_COLOR"), "{command}");
    }
    assert!(!out.exists());
    std::fs::remove_dir_all(dir).unwrap();
}
