//! Builds and exercises the actual CLI executable over stdio.
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use nuzky_engine::{Project, edit::new_id};
use serde_json::{Value, json};

struct Client {
    init: Value,
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
    dir: PathBuf,
    seq: u64,
}
impl Client {
    fn new(write: bool) -> Self {
        Self::with_checkpoint(write, None)
    }
    fn with_checkpoint(write: bool, checkpoint: Option<Project>) -> Self {
        Self::start(write, checkpoint, None)
    }
    /// `style` is this client's own data directory with that EDIT.md, or `Some(None)` for none at all.
    fn with_style(style: Option<&str>) -> Self {
        Self::start(false, None, Some(style))
    }
    fn start(write: bool, checkpoint: Option<Project>, style: Option<Option<&str>>) -> Self {
        let dir = std::env::temp_dir().join(format!("nuzky-mcp-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.nuzky");
        std::fs::write(&path, serde_json::to_vec(&Project::new("Original")).unwrap()).unwrap();
        if let Some(project) = checkpoint {
            std::fs::write(
                dir.join("project.nuzky.checkpoint.json"),
                serde_json::to_vec(&json!({"project": project})).unwrap(),
            )
            .unwrap();
        }
        Self::spawn(dir, write, style)
    }
    /// An agent started again on the same project once this one quit.
    fn restart(mut self, write: bool) -> Self {
        self.finish();
        let dir = std::mem::take(&mut self.dir);
        drop(self);
        Self::spawn(dir, write, None)
    }
    fn spawn(dir: PathBuf, write: bool, style: Option<Option<&str>>) -> Self {
        let project = dir.join("project.nuzky");
        Self::spawn_at(dir, &project, write, style)
    }
    /// An agent on another project of the same folder, sharing the cache and data directory.
    fn spawn_at(dir: PathBuf, project: &std::path::Path, write: bool, style: Option<Option<&str>>) -> Self {
        Self::spawn_at_env(dir, project, write, style, &[])
    }
    fn spawn_at_env(
        dir: PathBuf,
        project: &std::path::Path,
        write: bool,
        style: Option<Option<&str>>,
        env: &[(&str, &std::path::Path)],
    ) -> Self {
        Self::spawn_binary(env!("CARGO_BIN_EXE_nuzky").as_ref(), dir, project, write, style, env)
    }
    fn spawn_binary(
        binary: &std::path::Path,
        dir: PathBuf,
        project: &std::path::Path,
        write: bool,
        style: Option<Option<&str>>,
        env: &[(&str, &std::path::Path)],
    ) -> Self {
        let mut command = Command::new(binary);
        command.envs(env.iter().copied());
        command.arg("mcp").arg("--project").arg(project).arg("--cache").arg(dir.join("cache"));
        if write {
            command.arg("--allow-write");
        }
        if let Some(style) = style {
            let data = dir.join("data/nuzky");
            std::fs::create_dir_all(&data).unwrap();
            if let Some(text) = style {
                std::fs::write(data.join("EDIT.md"), text).unwrap();
            }
            command.env("NUZKY_DATA_DIR", data);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Build nuzky-cli first");
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                let value = serde_json::from_str(&line).unwrap_or_else(|_| panic!("Non-JSON on MCP stdout: {line}"));
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let mut client = Self { init: Value::Null, child, input, output, dir, seq: 0 };
        let init = client.rpc("initialize", json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"integration-test","version":"1"}}));
        assert_eq!(init["result"]["serverInfo"]["name"], "nuzky");
        client.init = init;
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }
    fn send(&mut self, value: Value) {
        writeln!(self.input.as_mut().unwrap(), "{value}").unwrap();
        self.input.as_mut().unwrap().flush().unwrap();
    }
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.seq += 1;
        self.send(json!({"jsonrpc":"2.0","id":self.seq,"method":method,"params":params}));
        loop {
            let value = self.output.recv_timeout(Duration::from_secs(20)).expect("MCP response timed out");
            if value["id"] == self.seq {
                return value;
            }
        }
    }
    fn call(&mut self, tool: &str, args: Value) -> Value {
        let result = self.rpc("tools/call", json!({"name":tool,"arguments":args}));
        assert!(result.get("error").is_none(), "{result}");
        assert_ne!(result["result"]["isError"], true, "{result}");
        result["result"]["structuredContent"].clone()
    }
    /// The message of a call that must fail.
    fn error(&mut self, tool: &str, args: Value) -> String {
        let result = self.rpc("tools/call", json!({"name":tool,"arguments":args}));
        assert_eq!(result["result"]["isError"], true, "{result}");
        result["result"]["content"][0]["text"].as_str().unwrap().to_owned()
    }
    fn finish(&mut self) {
        self.input.take();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(std::time::Instant::now() < deadline, "MCP did not exit on EOF");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn an_opened_capcut_draft_keeps_its_import_report_over_read_only_stdio() {
    let root = dunce::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    let dir = root.join("tmp-test").join(format!("capcut-open-mcp-{}", std::process::id()));
    let draft = dir.join("draft");
    std::fs::create_dir_all(&draft).unwrap();
    let talk = draft.join("talk.mp4");
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=540x960:r=30:d=1",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&talk)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let clip = |id: &str, at: i64| {
        json!({"material_id": id,
        "target_timerange": {"start": at, "duration": 1_000_000},
        "source_timerange": {"start": 0, "duration": 1_000_000}, "speed": 1.0})
    };
    let content = json!({"fps": 30, "canvas_config": {"width": 540, "height": 960},
        "materials": {"videos": [
            {"id": "talk", "type": "video", "path": talk, "material_name": "talk.mp4"},
            {"id": "gone", "type": "video", "path": "/nonexistent/gone.mov", "material_name": "gone.mov", "width": 540, "height": 960}]},
        "tracks": [{"type": "video", "attribute": 0, "segments": [clip("talk", 0), clip("gone", 1_000_000)]}]});
    std::fs::write(draft.join("draft_info.json"), content.to_string()).unwrap();
    let project = dir.join("project.nuzky");
    let out = Command::new(env!("CARGO_BIN_EXE_nuzky"))
        .arg("new")
        .arg(&project)
        .arg("--from")
        .arg(&draft)
        .env("HOME", dir.join("home"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_RUNTIME_DIR", dir.join("runtime"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let saved: Value = serde_json::from_slice(&std::fs::read(&project).unwrap()).unwrap();
    let mut c = Client::spawn_at(dir, &project, false, Some(None));
    let state = c.call("get_state", json!({}));
    assert_eq!(state["imported"]["editor"], "CapCut");
    assert_eq!(state["imported"]["path"], dunce::canonicalize(&draft).unwrap().to_str().unwrap());
    assert!(
        state["imported"]["leftOut"].as_array().unwrap().iter().any(|line| line.as_str().unwrap().contains("gone.mov")),
        "{state}"
    );
    assert_eq!(state["imported"], saved["imported"]);
    c.finish();
}

/// The built-in sound effects and songs need no network: listing finds each under its kind, and add_sound places
/// one at the time asked as an edit of the run, with its credit saved in the project and its file in Nuzky's library.
#[test]
fn built_in_sounds_are_listed_and_added_with_their_credit() {
    let mut c = Client::start(true, None, Some(None));
    let listed = c.call("search_sounds", json!({"query": "", "kind": "effect"}));
    let sounds = listed["sounds"].as_array().unwrap();
    assert!(sounds.len() >= 40, "{listed}");
    assert!(sounds.iter().all(|s| s["license"] == "cc0" && s["provider"] == "Built in" && s["kind"] == "effect"));
    assert!(sounds.iter().any(|s| s["id"] == "nuzky:whoosh" && s["category"] == "Whoosh"));
    // The built-in song is music only, and an empty query sends nothing anywhere.
    let music = c.call("search_sounds", json!({"query": " ", "kind": "music"}));
    let songs: Vec<_> =
        music["sounds"].as_array().unwrap().iter().map(|s| (&s["id"], &s["kind"], &s["license"])).collect();
    assert_eq!(songs, [(&json!("nuzky:dancing"), &json!("music"), &json!("cc0"))], "{music}");

    let run = c.call("begin_run", json!({"label": "Add a whoosh"}))["run_id"].clone();
    let added = c.call("add_sound", json!({"run_id": run, "id": "nuzky:whoosh", "at_us": 1_500_000}));
    assert_eq!(added["start_us"], 1_500_000, "{added}");
    assert_eq!(added["needs_credit"], false);
    assert_eq!(added["credit"]["source"], "nuzky");
    // The same sound again is another clip of the same asset.
    let again = c.call("add_sound", json!({"run_id": run, "id": "nuzky:whoosh", "at_us": 4_000_000}));
    assert_eq!(again["asset_id"], added["asset_id"]);
    assert_ne!(again["clip_id"], added["clip_id"]);
    assert!(
        c.error("add_sound", json!({"run_id": run, "id": "https://example.org/x.mp3", "at_us": 0}))
            .contains("SOUND_UNKNOWN")
    );
    let song = c.call("add_sound", json!({"run_id": run, "id": "nuzky:dancing", "at_us": 0}));
    assert_eq!((&song["needs_credit"], &song["credit"]["license"]), (&json!(false), &json!("cc0")), "{song}");
    c.call("end_run", json!({"run_id": run, "action": "keep"}));

    let disk: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.nuzky")).unwrap()).unwrap();
    let assets = disk["assets"].as_array().unwrap();
    let titles: Vec<_> = assets.iter().map(|a| (&a["credit"]["title"], &a["credit"]["license"])).collect();
    assert_eq!(titles, [(&json!("Whoosh"), &json!("cc0")), (&json!("Dancing"), &json!("cc0"))]);
    for asset in assets {
        let file = PathBuf::from(asset["path"].as_str().unwrap());
        // The temporary folder is a link on macOS (/var to /private/var); the sound is in its real path.
        assert!(
            file.starts_with(dunce::canonicalize(&c.dir).unwrap().join("data/nuzky/sounds")) && file.is_file(),
            "{file:?}"
        );
    }
    let mut starts: Vec<_> = disk["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["clips"].as_array().unwrap())
        .map(|c| c["startUs"].as_i64().unwrap())
        .collect();
    starts.sort();
    assert_eq!(starts, [0, 1_500_000, 4_000_000]);
}

#[test]
fn stickers_are_added_by_emoji_and_name_as_one_asset() {
    let mut c = Client::start(true, None, Some(None));
    let listed = c.rpc("tools/list", json!({}));
    let tool = listed["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "add_sticker").unwrap();
    let description = tool["description"].as_str().unwrap();
    for item in &nuzky_mcp::stickers::catalog().groups[0].items {
        assert!(description.contains(&item.key), "{description}");
    }
    assert!(description.contains("single emoji"));
    let run = c.call("begin_run", json!({"label": "Add stickers"}))["run_id"].clone();
    let args = json!({"run_id": run, "sticker": "🔥", "at_us": 1_500_000, "request_id": "fire"});
    let added = c.call("add_sticker", args.clone());
    assert_eq!(added["start_us"], 1_500_000);
    assert_eq!(added["duration_us"], 3_000_000);
    assert_eq!(added["sticker"], json!({"key":"1f525", "emoji":"🔥", "name":"fire"}));
    let transform = &added["transform"];
    assert!((transform["scale"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    for (field, expected) in [("x", 0.0), ("y", 0.0), ("rotation", 0.0), ("opacity", 1.0)] {
        assert_eq!(transform[field], expected);
    }
    let disk =
        |c: &Client| -> Value { serde_json::from_slice(&std::fs::read(c.dir.join("project.nuzky")).unwrap()).unwrap() };
    let first = disk(&c);
    let asset = &first["assets"][0];
    let file = PathBuf::from(asset["path"].as_str().unwrap());
    assert!(file.starts_with(dunce::canonicalize(&c.dir).unwrap().join("data/nuzky/stickers")), "{file:?}");
    assert_eq!(asset["kind"], "image");
    assert_eq!(asset["width"], 1024);
    assert_eq!(asset["height"], 1024);
    assert!(asset["credit"].is_null());
    let png = std::fs::read(&file).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 1024);
    assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 1024);
    assert_eq!(first["tracks"][0]["id"], "main");
    assert_eq!(first["tracks"][0]["clips"], json!([]));
    assert_eq!(first["tracks"][1]["kind"], "video");
    assert_eq!(first["tracks"][1]["id"], added["track_id"]);
    assert_eq!(first["tracks"][1]["clips"][0]["content"]["assetId"], added["asset_id"]);
    assert_eq!(
        serde_json::from_value::<nuzky_engine::model::Transform>(
            first["tracks"][1]["clips"][0]["content"]["transform"].clone()
        )
        .unwrap(),
        serde_json::from_value::<nuzky_engine::model::Transform>(transform.clone()).unwrap()
    );
    assert_eq!(c.call("add_sticker", args.clone())["clip_id"], added["clip_id"]);
    assert_eq!(disk(&c), first);
    let conflict =
        c.error("add_sticker", json!({"run_id": run,"sticker":"arrow","at_us":1_500_000,"request_id":"fire"}));
    assert!(conflict.contains("REQUEST_CONFLICT"), "{conflict}");
    let conflict =
        c.error("add_sticker", json!({"run_id": run,"sticker":"fire","at_us":2_000_000,"request_id":"fire"}));
    assert!(conflict.contains("REQUEST_CONFLICT"), "{conflict}");
    for name in ["fire", ":fire:"] {
        let again = c.call("add_sticker", json!({"run_id":run,"sticker":name,"at_us":1_500_000}));
        assert_eq!(again["asset_id"], added["asset_id"]);
        assert_ne!(again["clip_id"], added["clip_id"]);
        assert_ne!(again["track_id"], added["track_id"]);
    }
    for text in ["❤", "❤️"] {
        c.call("add_sticker", json!({"run_id":run,"sticker":text,"at_us":0}));
    }
    let arrow = c.call("add_sticker", json!({"run_id":run,"sticker":"arrow","at_us":0}));
    assert_eq!(arrow["sticker"]["key"], "arrow");
    let error = c.error("add_sticker", json!({"run_id":run,"sticker":"firee","at_us":0}));
    assert!(error.contains("STICKER_UNKNOWN") && error.contains("fire 🔥"), "{error}");
    let before = disk(&c);
    for bad in [
        json!({"at_us":-1}),
        json!({"at_us":0,"duration_us":0}),
        json!({"at_us":0,"transform":{"x":0,"y":0,"scale":0,"rotation":0,"opacity":1}}),
    ] {
        let mut bad = bad.as_object().unwrap().clone();
        bad.insert("run_id".into(), run.clone());
        bad.insert("sticker".into(), json!("👍🏽"));
        let error = c.error("add_sticker", Value::Object(bad));
        assert!(error.contains("INVALID_ARGUMENTS") || error.contains("INVALID_PROJECT"), "{error}");
        assert_eq!(disk(&c), before);
    }
    assert_eq!(before["assets"].as_array().unwrap().len(), 3);
    assert_eq!(
        before["tracks"].as_array().unwrap().iter().map(|t| t["clips"].as_array().unwrap().len()).sum::<usize>(),
        6
    );
    c.call("end_run", json!({"run_id":run,"action":"keep"}));
    c.call("undo_run", json!({"run_id":run}));
    let undone = disk(&c);
    assert_eq!(undone["assets"], json!([]));
    assert!(undone["tracks"].as_array().unwrap().iter().all(|t| t["clips"] == json!([])));
}

#[cfg(unix)]
#[test]
fn youtube_video_parts_download_with_mocked_ytdlp_and_require_rights() {
    use std::os::unix::fs::PermissionsExt;
    let root = dunce::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    let dir = root.join("tmp-test").join(format!("youtube-video-mcp-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.join("project.nuzky");
    std::fs::write(&project, serde_json::to_vec(&Project::new("YouTube video test")).unwrap()).unwrap();
    let picture = dir.join("picture.mp4");
    let audio = dir.join("sound.m4a");
    for (source, codec, path) in
        [("testsrc2=s=64x64:r=25:d=10", "libx264", &picture), ("sine=frequency=1000:duration=10", "aac", &audio)]
    {
        let out = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", source, "-c", codec])
            .arg(path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    let fake = dir.join("yt-dlp");
    std::fs::write(
        &fake,
        r#"#!/usr/bin/env python3
import json, os, shutil, sys
from pathlib import Path
args = sys.argv[1:]
with open(os.environ['NUZKY_FAKE_YTDLP_LOG'], 'a') as out:
    out.write(json.dumps(args) + '\n')
assert args[args.index('--') + 1:] == ['https://www.youtube.com/watch?v=ytVideo0001']
fmt = args[args.index('-f') + 1]
assert fmt in ('bestvideo[height<=1080][vcodec^=avc1]/bestvideo[height<=1080]', 'bestaudio[ext=m4a]/bestaudio'), fmt
source = os.environ['NUZKY_FAKE_YTDLP_PICTURE' if fmt.startswith('bestvideo') else 'NUZKY_FAKE_YTDLP_AUDIO']
if '--skip-download' in args:
    print(json.dumps({'id': 'ytVideo0001', 'title': 'Test video', 'duration': 10,
                      'filesize': Path(source).stat().st_size, 'live_status': 'not_live'}))
else:
    shutil.copyfile(source, args[args.index('-o') + 1])
"#,
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (log, home, data, cache, runtime) =
        (dir.join("ytdlp.jsonl"), dir.join("home"), dir.join("data"), dir.join("cache"), dir.join("runtime"));
    let env = [
        ("NUZKY_YTDLP", fake.as_path()),
        ("NUZKY_FAKE_YTDLP_PICTURE", &picture),
        ("NUZKY_FAKE_YTDLP_AUDIO", &audio),
        ("NUZKY_FAKE_YTDLP_LOG", &log),
        ("HOME", &home),
        ("XDG_DATA_HOME", &data),
        ("XDG_CACHE_HOME", &cache),
        ("XDG_RUNTIME_DIR", &runtime),
    ];
    let mut c = Client::spawn_at_env(dir, &project, true, Some(None), &env);
    let run = c.call("begin_run", json!({"label": "Add YouTube video"}))["run_id"].clone();
    let args = json!({"run_id": run, "link": "https://youtu.be/ytVideo0001?si=test",
        "start_us": 2_000_000, "end_us": 4_000_000, "at_us": 0, "request_id": "video-part"});
    let unchanged = c.call("get_state", json!({}));
    assert!(c.error("add_youtube_video", args.clone()).contains("RIGHTS_UNCONFIRMED"));
    assert_eq!(c.call("get_state", json!({})), unchanged);
    let mut confirmed = args.clone();
    confirmed["rights_confirmed"] = json!(true);
    let added = c.call("add_youtube_video", confirmed);
    assert_eq!(c.call("add_youtube_video", args.clone()), added);
    assert_eq!(added["start_us"], 0);
    assert_eq!(added["duration_us"], 2_000_000);
    let mut conflict = args.clone();
    conflict["at_us"] = json!(5_000_000);
    assert!(c.error("add_youtube_video", conflict).contains("REQUEST_CONFLICT"));
    assert!(
        c.error("add_youtube_video", json!({"run_id": run, "link": args["link"], "start_us": 2_000_000}))
            .contains("INVALID_ARGUMENTS")
    );
    let whole = c.call("add_youtube_video", json!({"run_id": run, "link": args["link"]}));
    assert!(whole["part"].is_null() && whole["clip_id"].is_null());
    assert_eq!(whole["duration_us"], whole["asset_duration_us"]);
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    let disk: Value = serde_json::from_slice(&std::fs::read(&project).unwrap()).unwrap();
    let assets = disk["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 2);
    let asset = assets.iter().find(|a| a["id"] == added["asset_id"]).unwrap();
    let part = &asset["part"];
    let (start, end) = (part["startUs"].as_i64().unwrap(), part["endUs"].as_i64().unwrap());
    assert!(0 <= start && start < end && end <= asset["durationUs"].as_i64().unwrap());
    assert_eq!(added["part"], *part);
    assert_eq!(added["asset_duration_us"], asset["durationUs"]);
    for asset in assets {
        let path = PathBuf::from(asset["path"].as_str().unwrap());
        assert!(path.is_file() && path.starts_with(c.dir.join("data/nuzky/videos")), "{path:?}");
        assert_eq!(asset["hasAudio"], true);
    }
    assert!(assets.iter().find(|a| a["id"] == whole["asset_id"]).unwrap()["part"].is_null());
    let track = disk["tracks"].as_array().unwrap().iter().find(|t| t["id"] == added["track_id"]).unwrap();
    let clip = &track["clips"][0];
    assert_eq!(clip["id"], added["clip_id"]);
    assert_eq!(clip["content"]["assetId"], added["asset_id"]);
    assert_eq!(clip["content"]["sourceInUs"], start);
    assert_eq!(clip["durationUs"], end - start);
    assert_eq!(clip["startUs"], added["start_us"]);
    for line in std::fs::read_to_string(&log).unwrap().lines() {
        let args: Vec<String> = serde_json::from_str(line).unwrap();
        assert_eq!(
            &args[args.iter().position(|a| a == "--").unwrap() + 1..],
            &["https://www.youtube.com/watch?v=ytVideo0001"]
        );
    }
    c.call("undo_run", json!({"run_id": run}));
    assert_eq!(c.call("get_state", json!({}))["assets"], json!([]));
    c.finish();
}

#[cfg(unix)]
#[test]
fn youtube_links_download_with_mocked_ytdlp_and_require_rights() {
    let root = dunce::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    let dir = root.join("tmp-test").join(format!("youtube-mcp-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.join("project.nuzky");
    std::fs::write(&project, serde_json::to_vec(&Project::new("YouTube test")).unwrap()).unwrap();
    let audio = dir.join("tone.m4a");
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "sine=frequency=5000:duration=20", "-c:a", "aac"])
        .arg(&audio)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let fake = root.join("tests/e2e/fake_ytdlp");
    let log = dir.join("ytdlp.jsonl");
    let cache = dir.join("cache");
    let runtime = dir.join("runtime");
    let home = dir.join("home");
    let data = dir.join("data/nuzky");
    let offline = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let unavailable = PathBuf::from(format!("http://{}/", offline.local_addr().unwrap()));
    drop(offline);
    let mut env: [(&str, &std::path::Path); 8] = [
        ("NUZKY_YTDLP", &fake),
        ("NUZKY_FAKE_YTDLP_AUDIO", &audio),
        ("NUZKY_FAKE_YTDLP_LOG", &log),
        ("NUZKY_OPENVERSE_URL", &unavailable),
        ("HOME", &home),
        ("NUZKY_DATA_DIR", &data),
        ("XDG_CACHE_HOME", &cache),
        ("XDG_RUNTIME_DIR", &runtime),
    ];
    let mut c = Client::spawn_at_env(dir, &project, true, Some(None), &env);
    // Before any song from a link, music lists only the built-in one.
    let ids = |found: &Value| found["sounds"].as_array().unwrap().iter().map(|s| s["id"].clone()).collect::<Vec<_>>();
    assert_eq!(ids(&c.call("search_sounds", json!({"query": "", "kind": "music"}))), ["nuzky:dancing"]);
    for query in ["https://music.youtube.com/watch?v=yt_Song0001&list=RDtest", "https://youtu.be/yt_Song0001?si=test"] {
        let found = c.call("search_sounds", json!({"query": query, "kind": "music"}));
        assert_eq!(found["sounds"].as_array().unwrap().len(), 1, "{found}");
        assert_eq!(found["sounds"][0]["id"], "youtube:yt_Song0001");
        assert_eq!(found["sounds"][0]["title"], "Steady Song");
        assert_eq!(found["sounds"][0]["author"], "Test Artist");
        assert_eq!(found["sounds"][0]["durationUs"], 20_000_000);
        assert_eq!(found["more"], false);
        assert_eq!(found["online"], "YouTube");
        assert_eq!(found["rights_confirmed"], false);
    }
    let before = std::fs::read(&log).unwrap();
    assert!(
        c.error("search_sounds", json!({"query": "https://youtube.com/playlist?list=PLtest", "kind": "music"}))
            .contains("PLAYLIST")
    );
    let run = c.call("begin_run", json!({"label": "Add YouTube music"}))["run_id"].clone();
    let unchanged = c.call("get_state", json!({}));
    assert!(
        c.error("add_sound", json!({"run_id": run, "id": "youtube:yt_Song0001", "at_us": 1_500_000}))
            .contains("RIGHTS_UNCONFIRMED")
    );
    assert_eq!(c.call("get_state", json!({})), unchanged);
    assert_eq!(std::fs::read(&log).unwrap(), before);
    let added = c.call(
        "add_sound",
        json!({"run_id": run, "id": "youtube:yt_Song0001", "at_us": 1_500_000, "rights_confirmed": true}),
    );
    assert_eq!(added["start_us"], 1_500_000, "{added}");
    assert_eq!(added["needs_credit"], false);
    assert_eq!(added["credit"], Value::Null);
    // An id with an underscore names its file in the library, so the song is found again.
    let again = c.call("add_sound", json!({"run_id": run, "id": "youtube:yt_Song0001", "at_us": 4_000_000}));
    assert_eq!(again["asset_id"], added["asset_id"], "{again}");
    let state = c.call("get_state", json!({}));
    assert!(
        c.error("add_sound", json!({"run_id": run, "id": "youtube:ytPrivate01", "at_us": 0}))
            .contains("SOUND_UNAVAILABLE")
    );
    assert_eq!(c.call("get_state", json!({})), state);
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    let disk: Value = serde_json::from_slice(&std::fs::read(&project).unwrap()).unwrap();
    assert_eq!(disk["assets"].as_array().unwrap().len(), 1);
    assert!(disk["assets"][0]["credit"].is_null());
    let file = dunce::canonicalize(&c.dir).unwrap().join("data/nuzky/sounds/youtube-yt_Song0001.m4a");
    assert_eq!(PathBuf::from(disk["assets"][0]["path"].as_str().unwrap()), file);
    assert_eq!(std::fs::read(&file).unwrap(), std::fs::read(&audio).unwrap());
    let track = disk["tracks"].as_array().unwrap().iter().find(|t| t["id"] == added["track_id"]).unwrap();
    assert_eq!(track["kind"], "audio");
    assert_eq!(track["clips"][0]["startUs"], 1_500_000);
    assert_eq!(track["clips"][0]["content"]["assetId"], added["asset_id"]);
    let settings: Value =
        serde_json::from_slice(&std::fs::read(c.dir.join("data/nuzky/sound-library.json")).unwrap()).unwrap();
    assert_eq!(settings["youtubeRights"], true);
    assert_eq!(
        c.call("search_sounds", json!({"query": "https://youtu.be/yt_Song0001", "kind": "music"}))["rights_confirmed"],
        true
    );
    for line in std::fs::read_to_string(&log).unwrap().lines() {
        let args: Vec<String> = serde_json::from_str(line).unwrap();
        let url = args.iter().position(|arg| arg == "--").unwrap() + 1;
        assert_eq!(args.len(), url + 1);
        assert!(
            args[url] == "https://www.youtube.com/watch?v=yt_Song0001"
                || args[url] == "https://www.youtube.com/watch?v=ytPrivate01"
        );
    }
    c.finish();
    let dir = std::mem::take(&mut c.dir);
    drop(c);
    let missing = dir.join("not-installed");
    env[0] = ("NUZKY_YTDLP", &missing);
    let second = dir.join("second.nuzky");
    std::fs::write(&second, serde_json::to_vec(&Project::new("Another project")).unwrap()).unwrap();
    let before = std::fs::read(&log).unwrap();
    let mut c = Client::spawn_at_env(dir, &second, true, None, &env);
    let run = c.call("begin_run", json!({"label": "Reuse downloaded music"}))["run_id"].clone();
    c.call("add_sound", json!({"run_id": run, "id": "youtube:yt_Song0001", "at_us": 0}));
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    assert_eq!(c.call("get_state", json!({}))["assets"][0]["path"], file.to_str().unwrap());
    for query in ["", "sTEAdY", "test ARTIST"] {
        let mut found = c.call("search_sounds", json!({"query": query, "kind": "music"}));
        if query.is_empty() {
            assert_eq!(ids(&found), ["nuzky:dancing", "youtube:yt_Song0001"]);
            found["sounds"].as_array_mut().unwrap().remove(0);
        }
        assert_eq!(found["sounds"].as_array().unwrap().len(), 1, "{found}");
        assert_eq!(found["sounds"][0]["id"], "youtube:yt_Song0001");
        assert_eq!(found["sounds"][0]["title"], "Steady Song");
        assert_eq!(found["sounds"][0]["author"], "Test Artist");
        assert_eq!(found["sounds"][0]["durationUs"], 20_000_000);
        if !query.is_empty() {
            assert!(found["online_error"].as_str().unwrap().contains("OFFLINE"), "{found}");
        }
    }
    assert!(c.error("search_sounds", json!({"query": "unmatched", "kind": "music"})).contains("OFFLINE"));
    assert_eq!(std::fs::read(&log).unwrap(), before);
    assert!(
        c.error("search_sounds", json!({"query": "https://youtu.be/ytSlow00001", "kind": "music"}))
            .contains("YTDLP_MISSING")
    );
    c.finish();
}

fn png_of(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&rgba.repeat((width * height) as usize)).unwrap();
    bytes
}

/// What the web server for import_web_image sends for a path.
fn answer(
    path: &str,
    out: &mut std::net::TcpStream,
    files: &[(&str, &str, Vec<u8>)],
    redirect: u16,
) -> std::io::Result<()> {
    let head = |status: &str, kind: &str, length: Option<usize>| {
        let length = length.map(|n| format!("Content-Length: {n}\r\n")).unwrap_or_default();
        format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\n{length}Connection: close\r\n")
    };
    match (path, files.iter().find(|(name, _, _)| *name == path)) {
        ("/redirect", _) => {
            write!(
                out,
                "{}Location: http://127.0.0.1:{redirect}/logo.png\r\n\r\n",
                head("302 Found", "text/plain", Some(0))
            )
        }
        ("/huge.png", _) => write!(out, "{}\r\n", head("200 OK", "image/png", Some(30 << 20))),
        ("/endless.png", _) => {
            write!(out, "{}\r\n", head("200 OK", "image/png", None))?;
            loop {
                out.write_all(&[0; 1 << 16])?;
            }
        }
        (_, Some((_, kind, body))) => {
            write!(out, "{}\r\n", head("200 OK", kind, Some(body.len())))?;
            out.write_all(body)
        }
        _ => write!(out, "{}\r\n", head("404 Not Found", "text/plain", Some(0))),
    }
}

/// An agent brings a logo from the web. Only the public internet is reached, never this computer or a private
/// network, not by a redirect, another spelling of the address or a proxy; what arrives is judged by its bytes and by
/// the frames FFmpeg decodes from it within the size limit, whatever its header says, an SVG is drawn without the local
/// files it names and never without its text, a save that failed is retried, and the picture undoes with the run.
#[test]
fn a_picture_from_the_web_comes_only_from_the_public_internet_over_stdio() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    let root = dunce::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    let dir = root.join("tmp-test").join(format!("web-image-mcp-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.join("project.nuzky");
    std::fs::write(&project, serde_json::to_vec(&Project::new("Brand deal")).unwrap()).unwrap();
    // A file on this computer that an SVG from the web names; it must never be drawn.
    let secret = dir.join("secret.svg");
    std::fs::write(&secret, r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#ff0000"/></svg>"##).unwrap();
    let logo = png_of(40, 20, [255, 0, 0, 255]);
    let brand = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="100" height="100" fill="#0000ff"/><image x="100" width="100" height="100" href="{}"/></svg>"##,
        secret.display()
    );
    let mut giant = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    giant.extend(60_000u32.to_be_bytes().repeat(2));
    giant.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    // 6000×5000 px, with its pHYs before its IHDR: read where an IHDR belongs, the size is 1×1.
    let lying = {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 6000, 5000);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::One);
        encoder.set_pixel_dims(Some(png::PixelDimensions { xppu: 1, yppu: 1, unit: png::Unit::Unspecified }));
        encoder.write_header().unwrap().write_image_data(&vec![0; 750 * 5000]).unwrap();
        let mut chunks = Vec::new();
        let mut at = 8;
        while at < bytes.len() {
            let end = at + 12 + u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            chunks.push(&bytes[at..end]);
            at = end;
        }
        let phys = chunks.iter().position(|chunk| &chunk[4..8] == b"pHYs").unwrap();
        let phys = chunks.remove(phys);
        [&bytes[..8], phys, &chunks.concat()[..]].concat()
    };
    // An animated 1×1 WebP, which the FFmpeg Nuzky links does not decode.
    let chunk =
        |name: &[u8], data: &[u8]| [name, &(data.len() as u32).to_le_bytes(), data, &[0][..data.len() % 2]].concat();
    let frame = chunk(b"VP8L", &[0x2f, 0, 0, 0, 0x10, 0x07, 0x10, 0x11, 0x11, 0x88, 0x88, 0xfe, 0x07]);
    let webp = [
        chunk(b"VP8X", &[0x12, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        chunk(b"ANIM", &[255, 255, 255, 255, 0, 0]),
        chunk(b"ANMF", &[&[0; 12][..], &[100, 0, 0, 0], &frame[..]].concat()),
    ]
    .concat();
    let animated = [&b"RIFF"[..], &(4 + webp.len() as u32).to_le_bytes(), b"WEBP", &webp[..]].concat();
    let mark = png_of(30, 30, [0, 255, 0, 255]);
    let page = "<!DOCTYPE html><html><head><meta property='og:image' content='https://cdn.example.com/share.jpg'>\
        <link rel=\"apple-touch-icon\" href=\"https://cdn.example.com/touch.png\"><link rel=icon href=/favicon.ico></head>\
        <body><img src=\"https://cdn.example.com/hero.jpg\"><img class=site-logo src=\"https://cdn.example.com/brand.svg?v=1&amp;s=2\">\
        <img src=\"data:image/png;base64,AAAA\"></body></html>";
    let files = vec![
        ("/logo.png", "image/png", logo.clone()),
        ("/brand.svg", "image/svg+xml", brand.into_bytes()),
        (
            "/text.svg",
            "image/svg+xml",
            br#"<svg xmlns="http://www.w3.org/2000/svg"><text y="20">Spotify</text></svg>"#.to_vec(),
        ),
        ("/page.html", "text/html; charset=utf-8", page.as_bytes().to_vec()),
        (
            "/prefixed-text.svg",
            "image/svg+xml",
            br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:s="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#0000ff"/><s:text y="8">Spotify</s:text></svg>"##.to_vec(),
        ),
        ("/giant.png", "image/png", giant),
        ("/lying.png", "image/png", lying),
        ("/animated.webp", "image/webp", animated),
        ("/logo.gif", "image/png", b"GIF89a\x01\0\x01\0\0\0\0;".to_vec()),
        ("/mark.png", "image/png", mark.clone()),
    ];
    // Where a redirect points, on this computer.
    let inside = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let inside_port = inside.local_addr().unwrap().port();
    let reached = Arc::new(AtomicUsize::new(0));
    let count = reached.clone();
    std::thread::spawn(move || {
        for _ in inside.incoming() {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    let web = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = web.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let (log, files) = (requests.clone(), Arc::new(files));
    std::thread::spawn(move || {
        for mut stream in web.incoming().flatten() {
            let (log, files) = (log.clone(), files.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                while reader.read_line(&mut head).unwrap_or(0) > 2 {}
                let first = head.lines().next().unwrap_or_default().to_owned();
                let path = first.split(' ').nth(1).unwrap_or_default().to_owned();
                log.lock().unwrap().push(first);
                answer(&path, &mut stream, &files, inside_port).ok();
            });
        }
    });
    let base = format!("http://127.0.0.1:{port}");
    let (test_url, proxy) = (PathBuf::from(&base), PathBuf::from(format!("{base}/")));
    let (home, cache, runtime, data) = (dir.join("home"), dir.join("cache"), dir.join("runtime"), dir.join("data"));
    let none = PathBuf::new();
    // A proxy, here the test server itself, would take an address to look up itself; Nuzky never uses one.
    let env: [(&str, &std::path::Path); 11] = [
        ("NUZKY_WEB_TEST_URL", &test_url),
        ("ALL_PROXY", &proxy),
        ("HTTPS_PROXY", &proxy),
        ("HTTP_PROXY", &proxy),
        ("NO_PROXY", &none),
        ("no_proxy", &none),
        ("HOME", &home),
        ("XDG_DATA_HOME", &data),
        ("XDG_CACHE_HOME", &cache),
        ("XDG_RUNTIME_DIR", &runtime),
        ("NUZKY_DATA_DIR", &data.join("nuzky")),
    ];
    let mut c = Client::spawn_at_env(dir, &project, true, None, &env);
    let run = c.call("begin_run", json!({"label": "Add the brand's logo"}))["run_id"].clone();
    let import = |c: &mut Client, url: String| {
        let answer: Value =
            serde_json::from_str(&c.error("import_web_image", json!({"run_id": run, "url": url}))).unwrap();
        answer["error"].as_str().unwrap().to_owned()
    };
    for url in [
        "https://10.0.0.1/x.png".to_owned(),
        format!("https://[::ffff:127.0.0.1]:{port}/logo.png"),
        format!("https://localhost:{port}/logo.png"),
        format!("{base}/redirect"),
    ] {
        let error = import(&mut c, url.clone());
        assert!(error.starts_with("BLOCKED_ADDRESS"), "{url}: {error}");
    }
    assert_eq!(reached.load(Ordering::SeqCst), 0, "the redirect reached this computer");
    for url in ["http://example.com/x.png", "file:///etc/hosts", "https://user:secret@example.com/x.png", "logo.png"] {
        assert!(import(&mut c, url.into()).starts_with("INVALID_URL"), "{url}");
    }
    for (path, code) in [
        ("/huge.png", "TOO_LARGE"),
        ("/endless.png", "TOO_LARGE"),
        ("/giant.png", "TOO_LARGE"),
        ("/lying.png", "NOT_A_PICTURE"),
        ("/animated.webp", "NOT_A_PICTURE"),
        ("/logo.gif", "NOT_A_PICTURE"),
        ("/text.svg", "NOT_A_PICTURE"),
        ("/prefixed-text.svg", "NOT_A_PICTURE"),
        ("/missing.png", "NOT_FOUND"),
    ] {
        let error = import(&mut c, format!("{base}{path}"));
        assert!(error.starts_with(code), "{path}: {error}");
    }
    let unchanged = c.call("get_state", json!({}));
    assert_eq!(unchanged["assets"], json!([]));

    let page = c.call("import_web_image", json!({"run_id": run, "url": format!("{base}/page.html")}));
    assert_eq!(page["asset_id"], Value::Null, "{page}");
    assert_eq!(
        page["pictures"],
        json!([
            "https://cdn.example.com/brand.svg?v=1&s=2",
            "https://cdn.example.com/touch.png",
            format!("{base}/favicon.ico"),
            "https://cdn.example.com/share.jpg",
            "https://cdn.example.com/hero.jpg"
        ])
    );
    assert_eq!(page["revision"], unchanged["revision"]);

    let added = c.call("import_web_image", json!({"run_id": run, "url": format!("{base}/logo.png")}));
    assert_eq!((&added["name"], &added["width"], &added["height"]), (&json!("logo.png"), &json!(40), &json!(20)));
    assert_eq!(added["url"], format!("{base}/logo.png"));
    assert_ne!(added["revision"], unchanged["revision"]);
    let again = c.call("import_web_image", json!({"run_id": run, "url": format!("{base}/logo.png")}));
    assert_eq!((&again["asset_id"], &again["revision"]), (&added["asset_id"], &added["revision"]));
    let svg = c.call("import_web_image", json!({"run_id": run, "url": format!("{base}/brand.svg")}));
    assert_eq!((&svg["name"], &svg["width"], &svg["height"]), (&json!("brand.svg"), &json!(2048), &json!(1024)));

    // The picture stays in the run when the project cannot be saved; asking again saves it.
    std::fs::remove_file(&project).unwrap();
    std::fs::create_dir(&project).unwrap();
    assert!(import(&mut c, format!("{base}/mark.png")).starts_with("SAVE_FAILED"));
    std::fs::remove_dir(&project).unwrap();
    let saved = c.call("import_web_image", json!({"run_id": run, "url": format!("{base}/mark.png")}));
    let disk: Project = serde_json::from_slice(&std::fs::read(&project).unwrap()).unwrap();
    assert!(disk.assets.iter().any(|asset| json!(asset.id) == saved["asset_id"]), "{saved}");

    let state = c.call("get_state", json!({}));
    let assets = state["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 3, "{state}");
    let folder = dunce::canonicalize(&c.dir).unwrap().join("data/nuzky/web-images");
    for asset in assets {
        assert_eq!(asset["kind"], "image");
        assert_eq!(PathBuf::from(asset["path"].as_str().unwrap()).parent(), Some(folder.as_path()));
    }
    assert_eq!(std::fs::read(assets[0]["path"].as_str().unwrap()).unwrap(), logo);
    let (info, rgba) = decode_png(&std::fs::read(assets[1]["path"].as_str().unwrap()).unwrap());
    let at = |x: u32, y: u32| &rgba[((y * info.width + x) * 4) as usize..][..4];
    assert_eq!(at(512, 512), [0, 0, 255, 255]);
    assert_eq!(at(1536, 512)[3], 0, "the SVG drew a file from this computer");
    assert_eq!(std::fs::read(assets[2]["path"].as_str().unwrap()).unwrap(), mark);
    let kept: Vec<_> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(kept.len(), 3, "{kept:?}");
    assert!(kept.iter().all(|name| !name.to_string_lossy().starts_with(".nuzky-part-")), "{kept:?}");
    let requests = requests.lock().unwrap().clone();
    assert!(requests.iter().all(|line| line.starts_with("GET /")), "{requests:?}");

    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    c.call("undo_run", json!({"run_id": run}));
    assert_eq!(c.call("get_state", json!({}))["assets"], json!([]));
    c.finish();
}

#[test]
fn clip_sound_settings_apply_and_reject_silent_media_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    place_still(&mut c);
    let image = c.call("get_state", json!({}))["tracks"][0]["clips"][0]["id"].clone();
    let run = c.call("begin_run", json!({"label": "Sound settings"}))["run_id"].clone();
    let sound = c.call("add_sound", json!({"run_id": run, "id": "nuzky:whoosh", "at_us": 0}))["clip_id"].clone();
    c.call(
        "apply_edits",
        json!({"run_id": run, "request_id": "sound", "edits": [
            {"type": "updateClip", "clipId": sound, "dereverb": 0.8, "normalize": true, "leveler": 0.5}
        ]}),
    );
    let before = c.call("get_state", json!({}));
    for (i, settings) in [
        json!({"dereverb": 0.8, "normalize": true, "leveler": 0.5}),
        json!({"cleanVoice": true}),
        json!({"dereverb": 0.8}),
        json!({"normalize": true}),
        json!({"leveler": 0.5}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut edit = json!({"type": "updateClip", "clipId": image, "volume": 0.3});
        edit.as_object_mut().unwrap().extend(settings.as_object().unwrap().clone());
        let error =
            c.error("apply_edits", json!({"run_id": run, "request_id": format!("silent-{i}"), "edits": [edit]}));
        assert!(error.contains("no sound"), "{error}");
        assert_eq!(c.call("get_state", json!({})), before);
    }
    c.call("apply_edits", json!({"run_id": run, "request_id": "off", "edits": [
        {"type": "updateClip", "clipId": image, "cleanVoice": false, "dereverb": 0.0, "normalize": false, "leveler": 0.0}
    ]}));
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    let disk: Project = serde_json::from_slice(&std::fs::read(c.dir.join("project.nuzky")).unwrap()).unwrap();
    let clip = disk.tracks.iter().flat_map(|t| &t.clips).find(|clip| json!(clip.id) == sound).unwrap();
    assert!(matches!(
        clip.content,
        nuzky_engine::model::ClipContent::Media { dereverb: 0.8, normalize: true, leveler: 0.5, .. }
    ));
    let run = c.call("begin_run", json!({"label": "Clamp strengths"}))["run_id"].clone();
    c.call(
        "apply_edits",
        json!({"run_id": run, "request_id": "clamp", "edits": [
            {"type": "updateClip", "clipId": sound, "dereverb": 2.0, "leveler": -1.0}
        ]}),
    );
    let state = c.call("get_state", json!({}));
    let clip = state["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["clips"].as_array().unwrap())
        .find(|clip| clip["id"] == sound)
        .unwrap();
    assert_eq!(clip["content"]["dereverb"], 1.0);
    assert!(clip["content"].get("leveler").is_none());
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    c.finish();
}

#[test]
fn initialize_list_state_edit_end_undo_over_stdio() {
    let mut c = Client::new(true);
    let list = c.rpc("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 37);
    let search = tools.iter().find(|t| t["name"] == "search_sounds").unwrap();
    assert_eq!(search["annotations"]["openWorldHint"], true);
    assert_eq!(search["annotations"]["readOnlyHint"], true);
    let video = tools.iter().find(|t| t["name"] == "add_youtube_video").unwrap();
    assert_eq!(video["annotations"]["openWorldHint"], true);
    assert_eq!(video["annotations"]["readOnlyHint"], false);
    assert_eq!(video["inputSchema"]["required"], json!(["run_id", "link"]));
    let picture = tools.iter().find(|t| t["name"] == "import_web_image").unwrap();
    assert_eq!(picture["annotations"]["openWorldHint"], true);
    assert_eq!(picture["inputSchema"]["required"], json!(["run_id", "url"]));
    let apply = tools.iter().find(|t| t["name"] == "apply_edits").unwrap();
    assert!(apply["inputSchema"]["$defs"]["EditCmd"].is_object());
    assert!(apply["inputSchema"]["properties"]["expected_speech_layout_key"].is_object());
    let captions = tools.iter().find(|t| t["name"] == "build_captions").unwrap();
    assert!(!captions["inputSchema"]["required"].as_array().unwrap().contains(&json!("style")));
    assert!(captions["inputSchema"]["properties"]["style_preset"].is_object());
    assert!(captions["description"].as_str().unwrap().contains("karaoke"));
    let recovery = tools.iter().find(|t| t["name"] == "resolve_recovery").unwrap();
    assert!(recovery["description"].as_str().unwrap().contains("Ask the user"));
    let before = c.call("get_state", json!({}));
    assert_eq!(before["name"], "Original");
    assert!(before["speech_layout_key"].is_string());
    assert_eq!(before["selection"], json!([]));
    assert_eq!(before["playhead_us"], 0);
    let run = c.call("begin_run", json!({"label":"integration"}));
    let args = json!({"run_id":run["run_id"],"request_id":"rename","expected_revision":run["revision"],"expected_speech_layout_key":before["speech_layout_key"],"edits":[{"type":"renameProject","name":"Edited via MCP"}]});
    let rejected = c.rpc(
        "tools/call",
        json!({"name":"apply_edits", "arguments":{
            "run_id":run["run_id"], "request_id":"wrong-speech", "expected_speech_layout_key":"stale",
            "edits":[{"type":"renameProject","name":"Must not apply"}]
        }}),
    );
    assert_eq!(rejected["result"]["isError"], true);
    assert!(rejected["result"]["content"][0]["text"].as_str().unwrap().contains("SPEECH_CHANGED"));
    let applied = c.call("apply_edits", args.clone());
    assert_eq!(applied, c.call("apply_edits", args));
    assert_eq!(c.call("get_state", json!({}))["name"], "Edited via MCP");
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    c.call("undo_run", json!({"run_id":run["run_id"]}));
    assert_eq!(c.call("get_state", json!({}))["name"], before["name"]);
    c.finish();
    let disk: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.nuzky")).unwrap()).unwrap();
    assert_eq!(disk["name"], "Original");
}

/// An agent restyled 117 karaoke captions one updateClip each and got back 64 thousand characters it could not
/// read. A batch that big now answers with the ids and how many clips were left out, the same on a retry, while a
/// small batch still shows its clips in full.
#[test]
fn a_restyle_of_every_caption_answers_short_enough_to_read_over_stdio() {
    let mut c = Client::new(true);
    let run = c.call("begin_run", json!({"label":"restyle captions"}))["run_id"].clone();
    let segments: Vec<Value> = (0..117i64)
        .map(|i| {
            let words: Vec<Value> = ["so", "this", "is", "it"]
                .iter()
                .zip(0i64..)
                .map(|(w, j)| json!({"text":w,"startUs":i * 1_000_000 + j * 250_000,"endUs":i * 1_000_000 + (j + 1) * 250_000}))
                .collect();
            json!({"startUs":i * 1_000_000,"endUs":(i + 1) * 1_000_000,"text":"so this is it","words":words})
        })
        .collect();
    let style = json!({"fontSize":64.0,"color":"#ffffff","bold":true});
    c.call(
        "apply_edits",
        json!({"run_id":run,"request_id":"captions","edits":[{"type":"addCaptions","segments":segments,"style":style}]}),
    );
    let tracks = c.call("get_state", json!({}))["tracks"].clone();
    let captions = tracks.as_array().unwrap().iter().find(|t| t["name"] == "Captions").unwrap();
    let ids: Vec<Value> = captions["clips"].as_array().unwrap().iter().map(|clip| clip["id"].clone()).collect();
    assert_eq!(ids.len(), 117);
    let restyle = |request: &str, ids: &[Value], size: f64| {
        let edits: Vec<Value> = ids
            .iter()
            .map(|id| json!({"type":"updateClip","clipId":id,"style":{"fontSize":size,"color":"#ffcc00","bold":true}}))
            .collect();
        json!({"name":"apply_edits","arguments":{"run_id":run,"request_id":request,"edits":edits}})
    };
    let big = c.rpc("tools/call", restyle("restyle", &ids, 72.0));
    let text = big["result"]["content"][0]["text"].as_str().unwrap();
    eprintln!("apply_edits of 117 updateClip answered {} characters", text.len());
    assert!(text.len() < 4_000, "{} characters", text.len());
    let result = &big["result"]["structuredContent"];
    assert_eq!(result["changed"].as_array().unwrap(), &ids);
    assert!(result.get("clips").is_none(), "{result}");
    assert_eq!(result["clips_omitted"], 117);
    assert_eq!(
        c.rpc("tools/call", restyle("restyle", &ids, 72.0))["result"],
        big["result"],
        "a retry answers the same"
    );

    let small = c.call("apply_edits", restyle("small", &ids[..2], 80.0)["arguments"].clone());
    let clips = small["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0]["content"]["style"]["fontSize"], 80.0);
    assert!(clips[0]["content"]["words"].is_array(), "{}", clips[0]);
    assert!(small.get("clips_omitted").is_none());
    c.call("end_run", json!({"run_id":run,"action":"keep"}));
    c.finish();
}

/// The guide's B-roll over a phrase: a screenshot in a corner window from the first word to the last,
/// then made a round bubble with a border.
#[test]
fn picture_in_picture_over_a_phrase_with_crop_and_shape_over_stdio() {
    let mut c = Client::new(true);
    let (talk, screen) = (c.dir.join("talk.ppm"), c.dir.join("screen.ppm"));
    std::fs::write(&talk, b"P6\n2 2\n255\nabcdefghijkl").unwrap();
    std::fs::write(&screen, [b"P6\n4 2\n255\n".as_slice(), &[200; 24]].concat()).unwrap();
    let run = c.call("begin_run", json!({"label":"b-roll"}));
    let ids = c.call("import_media", json!({"run_id":run["run_id"],"paths":[talk, screen]}))["asset_ids"].clone();
    let edits = json!([{"type":"addClip","assetId":ids[0]},
        {"type":"addPictureInPicture","assetId":ids[1],"startUs":1_000_000,"durationUs":1_500_000}]);
    c.call("apply_edits", json!({"run_id":run["run_id"],"request_id":"place","edits":edits}));
    let pip = c.call("get_state", json!({}))["tracks"][1]["clips"][0].clone();
    assert_eq!((&pip["startUs"], &pip["durationUs"]), (&json!(1_000_000), &json!(1_500_000)));
    assert!((pip["content"]["shape"]["radius"].as_f64().unwrap() - 0.15).abs() < 1e-6, "{pip}");
    let mut transform = pip["content"]["transform"].clone();
    transform["crop"] = json!({"left":0.25,"right":0.25});
    let round = json!({"radius":1.0,"borderWidth":8.0,"borderColor":"#ffffff","shadow":0.5});
    let update = |request: &str, transform: &Value, shape: &Value| {
        json!({"run_id":run["run_id"],"request_id":request,
            "edits":[{"type":"updateClip","clipId":pip["id"],"transform":transform,"shape":shape}]})
    };
    c.call("apply_edits", update("round", &transform, &round));
    let mut wide_open = transform.clone();
    wide_open["crop"] = json!({"left":0.6,"right":0.5});
    assert!(c.error("apply_edits", update("too-much", &wide_open, &round)).contains("crop must leave"));
    let broken = json!({"radius":2});
    assert!(c.error("apply_edits", update("broken", &transform, &broken)).contains("shape of"));
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    c.finish();
    let disk: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.nuzky")).unwrap()).unwrap();
    let content = &disk["tracks"][1]["clips"][0]["content"];
    assert_eq!((&content["shape"], &content["transform"]["crop"]["left"]), (&round, &json!(0.25)), "{content}");
}

/// The guide's repeat: a hook title read from get_state is pasted 20 s later on its track and another title takes
/// its look but keeps its words; the retry pastes nothing twice and undoing the run takes all of it back.
#[test]
fn an_agent_pastes_a_hook_later_and_its_look_onto_another_title_over_stdio() {
    let mut c = Client::new(true);
    let run = c.call("begin_run", json!({"label":"repeat the hook"}));
    let titles = json!([
        {"type":"addText","startUs":0,"text":"Hook","style":{"fontSize":120.0,"color":"#ffcc00","bold":true}},
        {"type":"addText","startUs":5_000_000,"text":"Later","style":{"fontSize":40.0,"color":"#ffffff"}}]);
    c.call("apply_edits", json!({"run_id":run["run_id"],"request_id":"titles","edits":titles}));
    let track = c.call("get_state", json!({}))["tracks"][1].clone();
    let (hook, later) = (&track["clips"][0], &track["clips"][1]);
    let copied =
        json!({"clip":hook,"trackId":track["id"],"trackKind":track["kind"],"keepInPlace":track["keepInPlace"]});
    let paste = json!({"run_id":run["run_id"],"request_id":"repeat","edits":[
        {"type":"pasteClips","clips":[copied],"atUs":20_000_000},
        {"type":"pasteAttributes","source":copied,"clipIds":[later["id"]]}]});
    assert_eq!(c.call("apply_edits", paste.clone()), c.call("apply_edits", paste));
    let tracks = c.call("get_state", json!({}))["tracks"].clone();
    let clips = tracks[1]["clips"].as_array().unwrap();
    assert_eq!(tracks.as_array().unwrap().len(), 2);
    assert_eq!(clips.len(), 3);
    assert_eq!((&clips[2]["startUs"], &clips[2]["content"]["text"]), (&json!(20_000_000), &json!("Hook")));
    assert_ne!(clips[2]["id"], hook["id"]);
    assert_eq!(
        (&clips[1]["content"]["text"], &clips[1]["content"]["style"]),
        (&json!("Later"), &hook["content"]["style"])
    );
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    c.call("undo_run", json!({"run_id":run["run_id"]}));
    assert_eq!(c.call("get_state", json!({}))["tracks"].as_array().unwrap().len(), 1);
    c.finish();
}

/// The Reel agent's restyle: captions built where it wants them, one rewritten by hand, then made bigger and moved
/// with one command that keeps every caption, its id and its text; undoing that run brings the look back.
#[test]
fn restyle_track_changes_every_caption_in_place_over_stdio() {
    use nuzky_session::transcripts::{Record, TranscriptStore, VERSION};

    let mut c = Client::start(true, None, Some(None));
    let video = c.dir.join("talk.mp4");
    assert!(
        Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=s=16x16:r=30:d=4"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=4", "-c:v", "libx264", "-preset", "ultrafast"])
            .args(["-c:a", "aac"])
            .arg(&video)
            .status()
            .unwrap()
            .success()
    );
    let asset = nuzky_engine::media::probe(&video, "take".into()).unwrap();
    let store = TranscriptStore::at(c.dir.join("data/nuzky/transcripts")).unwrap();
    let words = ["Save", "450", "every", "month."]
        .iter()
        .enumerate()
        .map(|(i, text)| nuzky_engine::speech::Word {
            start_us: 200_000 + i as i64 * 800_000,
            end_us: 700_000 + i as i64 * 800_000,
            text: (*text).into(),
            probability: 0.9,
        })
        .collect();
    let record = Record {
        version: VERSION,
        fingerprint: store.fingerprint(&asset).unwrap(),
        duration_us: asset.duration_us,
        model: "fixture".into(),
        language: "en".into(),
        words,
        segments: vec![],
        alignment: None,
    };
    store.put(&asset, &record).unwrap();
    let captions = |c: &mut Client| {
        let state = c.call("get_state", json!({}));
        state["tracks"].as_array().unwrap().iter().find(|t| t["name"] == "Captions").unwrap().clone()
    };

    let run = c.call("begin_run", json!({"label": "captions"}));
    let place = json!([{"type": "addAssets", "assets": [asset]}, {"type": "addClip", "assetId": "take"}]);
    c.call("apply_edits", json!({"run_id": run["run_id"], "request_id": "place", "edits": place}));
    c.call("build_captions", json!({"run_id": run["run_id"], "transform": {"y": 0.25, "scale": 1.25}}));
    let built = captions(&mut c);
    assert!(built["clips"].as_array().unwrap().len() > 1, "{built}");
    for clip in built["clips"].as_array().unwrap() {
        let transform = &clip["content"]["transform"];
        assert_eq!((&transform["x"], &transform["y"], &transform["scale"]), (&json!(0.0), &json!(0.25), &json!(1.25)));
    }
    let first = built["clips"][0]["id"].clone();
    let last = built["clips"].as_array().unwrap().last().unwrap()["id"].clone();
    let rise = json!([{"tUs": 0, "transform": {"x": 0.0, "y": 0.25, "scale": 1.0, "rotation": 0.0, "opacity": 1.0}},
        {"tUs": 300_000, "transform": {"x": 0.0, "y": 0.2, "scale": 1.0, "rotation": 0.0, "opacity": 1.0}}]);
    let by_hand = json!([{"type": "updateClip", "clipId": first, "text": "Keep 450"},
        {"type": "setKeyframes", "clipId": last, "keyframes": rise}]);
    c.call("apply_edits", json!({"run_id": run["run_id"], "request_id": "by-hand", "edits": by_hand}));
    c.call("end_run", json!({"run_id": run["run_id"], "action": "keep"}));
    let before = captions(&mut c);

    let run = c.call("begin_run", json!({"label": "bigger and higher"}));
    let restyle = |request: &str, track: &Value, style: Value, transform: Value| {
        json!({"run_id": run["run_id"], "request_id": request,
            "edits": [{"type": "restyleTrack", "trackId": track, "style": style, "transform": transform}]})
    };
    let main = json!("main");
    let wrong = c.error("apply_edits", restyle("main", &main, json!({"fontSize": 140.0}), Value::Null));
    assert!(wrong.contains("text track"), "{wrong}");
    let tiny = c.error("apply_edits", restyle("tiny", &before["id"], json!({"fontSize": -1.0}), Value::Null));
    assert!(tiny.contains("text style"), "{tiny}");
    let typo = c.error("apply_edits", restyle("typo", &before["id"], json!({"fontsize": 140.0}), Value::Null));
    assert!(typo.contains("fontsize"), "{typo}");
    let style = json!({"fontSize": 140.0, "background": "#000000aa"});
    let applied = c.call("apply_edits", restyle("restyle", &before["id"], style, json!({"y": -0.25})));
    assert_eq!((&applied["created"], &applied["removed"]), (&json!([]), &json!([])), "{applied}");
    assert_eq!(applied["skipped"], json!([last]), "the keyed caption keeps its place");
    let after = captions(&mut c);
    assert_eq!(after["id"], before["id"]);
    let (old, new) = (before["clips"].as_array().unwrap(), after["clips"].as_array().unwrap());
    assert_eq!(old.len(), new.len());
    for (old, new) in old.iter().zip(new) {
        let (was, now) = (&old["content"], &new["content"]);
        assert_eq!(
            (&new["id"], &new["startUs"], &new["durationUs"]),
            (&old["id"], &old["startUs"], &old["durationUs"])
        );
        assert_eq!((&now["text"], &now["words"], &new["keyframes"]), (&was["text"], &was["words"], &old["keyframes"]));
        assert_eq!((&now["style"]["fontSize"], &now["style"]["background"]), (&json!(140.0), &json!("#000000aa")));
        assert_eq!(now["style"]["color"], was["style"]["color"]);
        let y = if new["id"] == last { 0.25 } else { -0.25 };
        assert_eq!((&now["transform"]["y"], &now["transform"]["scale"]), (&json!(y), &json!(1.25)));
    }
    assert_eq!(new[0]["content"]["text"], "Keep 450");
    // A CapCut look: a shadow, capitals and spacing on every caption, the words as typed.
    let shadow = json!({"color": "#000000", "opacity": 0.5, "blur": 7.0, "distance": 3.0, "angle": -45.0});
    let look = json!({"shadow": shadow, "letterCase": "upper", "letterSpacing": 0.25, "lineSpacing": 0.5, "align": "left", "boxRadius": 1.0});
    c.call("apply_edits", restyle("capcut", &before["id"], look, Value::Null));
    for clip in captions(&mut c)["clips"].as_array().unwrap() {
        let style = &clip["content"]["style"];
        assert_eq!((&style["shadow"], &style["letterCase"]), (&shadow, &json!("upper")), "{style}");
        assert_eq!((&style["letterSpacing"], &style["lineSpacing"]), (&json!(0.25), &json!(0.5)), "{style}");
        assert_eq!((&style["align"], &style["boxRadius"]), (&json!("left"), &json!(1.0)), "{style}");
    }
    let rounder = c.error("apply_edits", restyle("rounder", &before["id"], json!({"boxRadius": 2.0}), Value::Null));
    assert!(rounder.contains("corner radius"), "{rounder}");
    assert_eq!(captions(&mut c)["clips"][0]["content"]["text"], "Keep 450");
    let glaring = json!({"shadow": {"color": "#000000", "opacity": 2.0, "blur": 0.0, "distance": 0.0, "angle": 0.0}});
    let glaring = c.error("apply_edits", restyle("glaring", &before["id"], glaring, Value::Null));
    assert!(glaring.contains("shadow"), "{glaring}");
    c.call("apply_edits", restyle("no-box", &before["id"], json!({"background": null}), Value::Null));
    let boxed = captions(&mut c)["clips"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| !c["content"]["style"]["background"].is_null())
        .count();
    assert_eq!(boxed, 0, "null clears the box");
    c.call("end_run", json!({"run_id": run["run_id"], "action": "keep"}));
    c.call("undo_run", json!({"run_id": run["run_id"]}));
    assert_eq!(captions(&mut c), before);
    c.finish();
}

/// Headless, without the app: runs are versions that outlive the agent, a read-only agent lists them,
/// and undo_to brings one back as one step, which undo_to the version before takes back.
#[test]
fn versions_outlive_a_headless_agent_and_restore_over_stdio() {
    let mut c = Client::new(true);
    for name in ["One", "Two"] {
        let run = c.call("begin_run", json!({"label": name}));
        let edit = json!({"run_id":run["run_id"],"request_id":"r","edits":[{"type":"renameProject","name":name}]});
        c.call("apply_edits", edit);
        c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    }
    let mut c = c.restart(false);
    let list = c.call("list_history", json!({"limit": 2}));
    let labels: Vec<_> = list["versions"].as_array().unwrap().iter().map(|v| v["label"].clone()).collect();
    assert_eq!((labels, list["older"].clone()), (vec![json!("Two"), json!("One")], json!(1)));
    assert!(c.error("undo_to", json!({"index": 1})).contains("READ_ONLY"));
    assert!(c.error("list_history", json!({"limit": 0})).contains("INVALID_ARGUMENTS"));
    let mut c = c.restart(true);
    let one = list["versions"][1].clone();
    assert!(c.error("undo_to", json!({"index": 1, "hash": one["hash"]})).contains("INVALID_ARGUMENTS"));
    assert_eq!(c.call("undo_to", json!({"index": one["index"]}))["changed"], true);
    assert_eq!(c.call("get_state", json!({}))["name"], "One");
    let again = c.call("undo_to", json!({"index": one["index"]}));
    assert_eq!(again["changed"], false, "restoring the current version changes nothing");
    let back = c.call("undo_to", json!({"hash": list["versions"][0]["hash"]}));
    assert_eq!(back["changed"], true);
    assert_eq!(c.call("get_state", json!({}))["name"], "Two");
    assert_eq!(c.call("list_history", json!({}))["current_hash"], list["current_hash"]);
    c.finish();
}

#[test]
fn readonly_resources_prompts_and_clear_errors() {
    let mut c = Client::with_style(None);
    assert_eq!(c.call("get_state", json!({}))["read_only"], true);
    let error = c.rpc("tools/call", json!({"name":"begin_run","arguments":{"label":"denied"}}));
    assert_eq!(error["result"]["isError"], true);
    assert!(error["result"]["content"][0]["text"].as_str().unwrap().contains("READ_ONLY"));
    let resources = c.rpc("resources/list", json!({}));
    assert_eq!(resources["result"]["resources"].as_array().unwrap().len(), 2);
    let guide = c.rpc("resources/read", json!({"uri":"nuzky://guide"}));
    let guide = guide["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(
        guide.contains("rippleDeleteRanges")
            && guide.contains(r#""duckDb":12"#)
            && guide.contains(r#""keepPitch":false"#)
    );
    assert!(guide.contains("positive y DOWN") && guide.contains("−(draft y) / 2 is `y`"));
    let schema = c.rpc("resources/read", json!({"uri":"nuzky://schema"}));
    let schema = schema["result"]["contents"][0]["text"].as_str().unwrap();
    for field in ["duckDb", "keepPitch", "dereverb", "normalize", "leveler"] {
        assert!(schema.contains(&format!("\"{field}\"")), "{field}");
    }
    assert!(schema.contains("Positive moves down"), "{schema}");
    let prompt = c.rpc("prompts/get", json!({"name":"edit_selected","arguments":{"goal":"Make a reel"}}));
    assert!(prompt["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("Make a reel"));
    // One prompt starts the whole rough cut; the user's wishes come with it.
    let names: Vec<_> = c.rpc("prompts/list", json!({}))["result"]["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].clone())
        .collect();
    assert_eq!(names, [json!("edit_selected"), json!("thumbnail"), json!("rough_cut")]);
    let rough = c.rpc("prompts/get", json!({"name":"rough_cut","arguments":{"wishes":"keep the call to action"}}));
    let text = rough["result"]["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(
        text.contains("analyze(kind: \"retakes\")")
            && text.contains("preset \"reels\"")
            && text.contains("keep the call to action")
    );
    assert!(
        c.rpc("prompts/get", json!({"name":"rough_cut"}))["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("wishes, which win over the steps: none")
    );
    // One prompt makes both covers, with the text presets and the user's wishes.
    let covers = c.rpc("prompts/get", json!({"name":"thumbnail","arguments":{"wishes":"a red hook"}}));
    let text = covers["result"]["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(
        ["thumbnail_frames", "segment_subject", "setThumbnail", "inspect_thumbnail", "export_thumbnail", "\"Big\""]
            .iter()
            .all(|step| text.contains(step))
            && text.contains("wishes, which win over the steps: a red hook"),
        "{text}"
    );
    let error = c.rpc("tools/call", json!({"name":"inspect_frames","arguments":{"times_us":[0]}}));
    assert_eq!(error["result"]["isError"], true);
    // Offering choices touches nothing, so a read-only client may do it; nonsense is refused with the reason.
    let offer = json!({"question":"Which take?","options":[{"label":"The first one"},{"label":"The last one","detail":"Clearer"}]});
    assert_eq!(c.call("suggest_options", offer)["shown"], 2);
    assert!(c.error("suggest_options", json!({"options":[{"label":"Only one"}]})).contains("2 to 6 options"));
    assert!(
        c.error("suggest_options", json!({"options":[{"label":"Same"},{"label":"same"}]}))
            .contains("labels must differ")
    );
    c.finish();
}

#[test]
fn an_agent_reads_the_style_and_keeps_what_the_creator_agreed_to() {
    let mut c = Client::start(true, None, Some(None));
    let style = c.call("get_style", json!({}));
    assert!(style["text"].is_null() && style["own"] == json!([]) && style["version"] == 0, "{style}");
    let rule = "Never cut my sign-off.";
    let changed = c.call("change_style", json!({"action": {"type": "setOwn", "text": rule}}));
    assert_eq!(changed["own"], json!([rule]));
    assert_eq!(changed["versions"][0]["label"], "AI: Added your rule");
    let file = std::fs::read_to_string(c.dir.join("data/nuzky/EDIT.md")).unwrap();
    assert!(file.contains(&format!("## Your rules\n\nThe creator's own instructions. They win over every rule below and over the guide.\n\n- {rule}\n")), "{file}");
    assert_eq!(c.rpc("resources/read", json!({"uri":"nuzky://style"}))["result"]["contents"][0]["text"], file.as_str());
    let edited = file.replace(rule, "Keep my sign-off.");
    let stale = c.error("change_style", json!({"action": {"type": "setText", "text": edited, "baseVersion": 0}}));
    assert!(stale.contains("STYLE_CHANGED"), "{stale}");
    let changed = c.call(
        "change_style",
        json!({"action": {"type": "setText", "text": edited, "baseVersion": changed["version"]}}),
    );
    assert_eq!(changed["own"], json!(["Keep my sign-off."]));
    assert!(c.error("change_style", json!({"action": {"type": "reset"}})).contains("only the creator"));
    // The style is the creator's, not the project's: nothing in the project changed.
    assert_eq!(c.call("get_state", json!({}))["revision"], 0);
    c.finish();
    let mut reader = Client::start(false, None, Some(None));
    assert!(reader.error("change_style", json!({"action": {"type": "setOwn", "text": "x"}})).contains("READ_ONLY"));
    assert!(!reader.dir.join("data/nuzky/EDIT.md").exists());
    reader.finish();
}

#[test]
fn a_creator_style_reaches_the_agent_and_without_one_nothing_changes() {
    let guide = include_str!("../../../skills/nuzky-edit/SKILL.md");
    let mut plain = Client::with_style(None);
    assert_eq!(plain.init["result"]["instructions"], guide);
    assert_eq!(plain.rpc("resources/list", json!({}))["result"]["resources"].as_array().unwrap().len(), 2);
    assert_eq!(plain.rpc("resources/read", json!({"uri":"nuzky://guide"}))["result"]["contents"][0]["text"], guide);
    assert!(
        plain.rpc("resources/read", json!({"uri":"nuzky://style"}))["error"]["message"]
            .as_str()
            .unwrap()
            .contains("nuzky style learn")
    );
    plain.finish();

    let style = "# Editing style\n\nKeep pauses under 120 ms.\n";
    let mut styled = Client::with_style(Some(style));
    // The style comes before the whole guide, so its rules are the first thing the agent reads.
    let style_first = |text: &str| {
        let at = |part: &str| text.find(part).unwrap_or_else(|| panic!("{part:?} is missing from {text}"));
        assert!(at(style) < at("# Edit with Nuzky"), "{text}");
        assert!(text.contains(guide), "{text}");
    };
    let instructions = styled.init["result"]["instructions"].as_str().unwrap().to_owned();
    assert!(instructions.starts_with("This creator has their own editing style"), "{instructions}");
    style_first(&instructions);
    let resources = styled.rpc("resources/list", json!({}));
    let uris: Vec<&str> =
        resources["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
    assert_eq!(uris, ["nuzky://guide", "nuzky://schema", "nuzky://style"]);
    assert_eq!(styled.rpc("resources/read", json!({"uri":"nuzky://style"}))["result"]["contents"][0]["text"], style);
    let read = styled.rpc("resources/read", json!({"uri":"nuzky://guide"}));
    assert_eq!(read["result"]["contents"][0]["text"], instructions);
    for (name, arguments) in
        [("edit_selected", json!({"goal":"Make a reel"})), ("rough_cut", json!({})), ("thumbnail", json!({}))]
    {
        let prompt = styled.rpc("prompts/get", json!({"name": name, "arguments": arguments}));
        style_first(prompt["result"]["messages"][0]["content"]["text"].as_str().unwrap());
    }
    styled.finish();
}

#[test]
fn disconnect_keeps_and_removes_checkpoint() {
    let mut c = Client::new(true);
    let run = c.call("begin_run", json!({"label":"disconnect"}));
    c.call(
        "apply_edits",
        json!({"run_id":run["run_id"],"request_id":"rename","edits":[{"type":"renameProject","name":"Kept"}]}),
    );
    c.finish();
    assert!(!c.dir.join("project.nuzky.checkpoint.json").exists());
    let disk: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.nuzky")).unwrap()).unwrap();
    assert_eq!(disk["name"], "Kept");
}

#[test]
fn recovery_tool_resolves_both_choices_over_stdio() {
    for (action, name, revision) in [("keep", "Original", 0), ("restore", "Before", 1)] {
        let mut c = Client::with_checkpoint(true, Some(Project::new("Before")));
        assert!(c.call("get_state", json!({}))["recovery_checkpoint"].is_string());
        let stamp = c.call("resolve_recovery", json!({"action": action}));
        assert_eq!(stamp["revision"], revision);
        let state = c.call("get_state", json!({}));
        assert_eq!(state["name"], name);
        assert!(state["recovery_checkpoint"].is_null());
        assert!(!c.dir.join("project.nuzky.checkpoint.json").exists());
        c.call("begin_run", json!({"label": "after recovery"}));
        c.finish();
    }
}

#[test]
fn retakes_answer_at_once_even_for_read_only_clients() {
    let mut c = Client::with_style(None);
    let tools = c.rpc("tools/list", json!({}));
    let analyze = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "analyze").unwrap();
    assert_eq!(analyze["inputSchema"]["required"], json!(["kind"]));
    let retakes = c.call("analyze", json!({"kind":"retakes"}));
    assert_eq!(retakes["time_basis"], "timeline");
    assert_eq!(retakes["transcript_key"], c.call("get_transcript", json!({}))["transcript_key"]);
    for list in ["groups", "fillers", "review", "suggested_delete"] {
        assert_eq!(retakes[list], json!([]), "{retakes}");
    }
    // So do the fillers of the whole timeline.
    let fillers = c.call("analyze", json!({"kind":"fillers"}));
    assert_eq!(fillers["transcript_key"], retakes["transcript_key"]);
    assert_eq!((&fillers["fillers"], &fillers["hesitations"]), (&json!([]), &json!([])), "{fillers}");
    // The other kinds still need an asset and start a job, which a read-only client cannot.
    assert!(c.error("analyze", json!({"kind":"retakes","asset_id":"a"})).contains("omit asset_id"));
    assert!(c.error("analyze", json!({"kind":"silences","asset_id":"a"})).contains("READ_ONLY"));
    assert!(c.error("analyze", json!({"kind":"fillers","asset_id":"a"})).contains("READ_ONLY"));
    c.finish();
    let mut writer = Client::start(true, None, Some(None));
    assert!(writer.error("analyze", json!({"kind":"silences"})).contains("asset_id is required"));
    writer.finish();
}

/// An "eee" recognition did not write lies between two words; analyze fillers names it by the
/// word before it, beside a filler word inside the sentence, and edit_transcript cuts it.
#[test]
fn fillers_and_hesitations_reach_agents_over_stdio() {
    use nuzky_session::transcripts::{Record, TranscriptStore, VERSION};
    let mut c = Client::start(true, None, Some(None));
    let mut samples = vec![0i16; 3 * 48_000];
    let mut sound = |from_us: i64, to_us: i64, hz: f64| {
        let first = (from_us * 48 / 1000) as usize;
        for (i, sample) in samples[first..(to_us * 48 / 1000) as usize].iter_mut().enumerate() {
            *sample = (0.4 * (i as f64 * hz / 48_000.0 * std::f64::consts::TAU).sin() * 32_767.0) as i16;
        }
    };
    let said = [
        (" Kamera", 300_000, 700_000),
        (" ehm", 800_000, 1_000_000),
        (" stojí", 1_100_000, 1_500_000),
        (" pevně.", 2_100_000, 2_500_000),
    ];
    for (_, from, to) in said {
        sound(from, to, 440.0);
    }
    sound(1_650_000, 1_950_000, 220.0);
    let words: Vec<Value> = said
        .iter()
        .map(|(text, from, to)| json!({"start_us":from,"end_us":to,"text":text,"probability":0.9}))
        .collect();
    let wav = c.dir.join("speech.wav");
    std::fs::write(&wav, wav_bytes(&samples)).unwrap();
    let video = c.dir.join("talk.mp4");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=360x640:r=30:d=3", "-i"])
        .arg(&wav)
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&video)
        .status()
        .expect("ffmpeg makes the test video");
    assert!(status.success());
    let run = c.call("begin_run", json!({"label":"place"}))["run_id"].clone();
    let asset_id = c.call("import_media", json!({"run_id":run,"paths":[video]}))["asset_ids"][0].clone();
    c.call("apply_edits", json!({"run_id":run,"request_id":"place","edits":[{"type":"addClip","assetId":asset_id}]}));
    c.call("end_run", json!({"run_id":run,"action":"keep"}));
    let asset: nuzky_engine::model::Asset =
        serde_json::from_value(c.call("get_state", json!({}))["assets"][0].clone()).unwrap();
    let store = TranscriptStore::at(c.dir.join("data/nuzky/transcripts")).unwrap();
    let record = Record {
        version: VERSION,
        fingerprint: store.fingerprint(&asset).unwrap(),
        duration_us: asset.duration_us,
        model: "fixture".into(),
        language: "cs".into(),
        words: serde_json::from_value(json!(words)).unwrap(),
        segments: vec![],
        alignment: None,
    };
    store.put(&asset, &record).unwrap();

    let waiting: Value = serde_json::from_str(&c.error("analyze", json!({"kind":"fillers"}))).unwrap();
    let waiting = waiting["error"].as_str().unwrap();
    assert!(waiting.starts_with("AUDIO_NOT_READY"), "{waiting}");
    let job_id = waiting.split("as job ").nth(1).and_then(|rest| rest.split(';').next()).unwrap();
    assert_eq!(wait_job(&mut c, &json!({"job_id":job_id}), Duration::from_secs(60), |_| false)["status"], "done");
    let found = c.call("analyze", json!({"kind":"fillers"}));
    assert_eq!(found["time_basis"], "timeline");
    assert_eq!(found["fillers"], json!([{"from":1,"to":1,"text":"ehm"}]), "{found}");
    let hesitation = &found["hesitations"][0];
    assert_eq!((found["hesitations"].as_array().unwrap().len(), &hesitation["after_word"]), (1, &json!(2)), "{found}");
    let (start, end) = (hesitation["start_us"].as_i64().unwrap(), hesitation["end_us"].as_i64().unwrap());
    assert!((1_600_000..=1_700_000).contains(&start) && (1_900_000..=2_000_000).contains(&end), "{found}");

    let key = c.call("get_transcript", json!({}))["transcript_key"].clone();
    let plan = c.call(
        "edit_transcript",
        json!({"transcript_key":key,"delete":[[1,1]],"delete_hesitations":[2],"dry_run":true}),
    );
    let cuts: Vec<(i64, i64)> = plan["ranges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["startUs"].as_i64().unwrap(), r["endUs"].as_i64().unwrap()))
        .collect();
    assert!(cuts.iter().any(|&(a, b)| a <= start && end <= b), "{plan}");
    let kept = [0, 2, 3].map(|i| (said[i].1, said[i].2));
    assert!(
        cuts.iter().all(|&(a, b)| kept.iter().all(|&(from, to)| b <= from || a >= to)),
        "a cut takes a kept word: {plan}"
    );
    let error = c.error("edit_transcript", json!({"transcript_key":key,"delete_hesitations":[0],"dry_run":true}));
    assert!(error.contains("INVALID_HESITATION"), "{error}");
    c.finish();
}

#[test]
fn headless_transcribe_without_models_points_to_transcript_tab_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    let video = c.dir.join("talk.mp4");
    assert!(
        Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=s=16x16:r=30:d=1"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-c:a",
                "aac",
            ])
            .arg(&video)
            .status()
            .unwrap()
            .success()
    );
    let run = c.call("begin_run", json!({"label": "Import speech"}))["run_id"].clone();
    let assets = c.call("import_media", json!({"run_id": run, "paths": [video]}));
    c.call(
        "apply_edits",
        json!({"run_id": run, "request_id": "place", "edits": [{"type": "addClip", "assetId": assets["asset_ids"][0]}]}),
    );
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    let error = c.error("transcribe", json!({}));
    assert!(error.contains("MODEL_MISSING") && error.contains("Transcript tab"), "{error}");
    let models = c.dir.join("data/nuzky/models");
    assert!(!models.exists() || std::fs::read_dir(models).unwrap().next().is_none());
    c.finish();
}

/// A 4 s take whose transcript, "Save 450 every month.", is in the data directory of `c`.
fn transcribed_take(c: &Client) -> nuzky_engine::model::Asset {
    use nuzky_session::transcripts::{Record, TranscriptStore, VERSION};

    let video = c.dir.join("talk.mp4");
    assert!(
        Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=s=16x16:r=30:d=4"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-c:a",
                "aac"
            ])
            .arg(&video)
            .status()
            .unwrap()
            .success()
    );
    let asset = nuzky_engine::media::probe(&video, "take".into()).unwrap();
    let store = TranscriptStore::at(c.dir.join("data/nuzky/transcripts")).unwrap();
    store
        .put(
            &asset,
            &Record {
                version: VERSION,
                fingerprint: store.fingerprint(&asset).unwrap(),
                duration_us: asset.duration_us,
                model: "fixture".into(),
                language: "en".into(),
                words: ["Save", "450", "every", "month."]
                    .iter()
                    .enumerate()
                    .map(|(i, text)| nuzky_engine::speech::Word {
                        start_us: 200_000 + i as i64 * 800_000,
                        end_us: 700_000 + i as i64 * 800_000,
                        text: (*text).into(),
                        probability: 0.9,
                    })
                    .collect(),
                segments: vec![],
                alignment: None,
            },
        )
        .unwrap();
    asset
}

#[test]
fn caption_style_packages_over_stdio() {
    use nuzky_engine::edit::caption_preset;

    let mut c = Client::start(true, None, Some(None));
    let asset = transcribed_take(&c);
    let mut own = caption_preset("neon_blue").unwrap().clone();
    own.name = "My neon".into();
    own.anim_out = own.anim_in;
    std::fs::write(c.dir.join("data/nuzky/caption-styles.json"), serde_json::to_vec(&vec![&own]).unwrap()).unwrap();
    let run = c.call("begin_run", json!({"label": "caption packages"}));
    c.call(
        "apply_edits",
        json!({"run_id": run["run_id"], "request_id": "place",
        "edits": [{"type": "addAssets", "assets": [asset]}, {"type": "addClip", "assetId": "take"}]}),
    );
    for (name, preset) in [("hormozi", caption_preset("hormozi").unwrap()), ("MY-NEON", &own)] {
        let made = c.call("build_captions", json!({"run_id": run["run_id"], "style_preset": name}));
        assert!(made["caption_count"].as_u64().unwrap() > 1);
        let state = c.call("get_state", json!({}));
        let clips = state["tracks"].as_array().unwrap().iter().find(|t| t["name"] == "Captions").unwrap()["clips"]
            .as_array()
            .unwrap();
        for clip in clips {
            assert_eq!(clip["content"]["style"]["fontFamily"], json!(preset.style.font_family));
            assert_eq!(clip["content"]["style"]["keywords"], json!(preset.style.keywords));
            assert_eq!(clip["animIn"], json!(preset.anim_in));
            assert_eq!(clip["animOut"], json!(preset.anim_out));
        }
        assert!(
            clips
                .iter()
                .flat_map(|clip| clip["content"]["words"].as_array().unwrap())
                .any(|word| word["text"] == "450" && word["key"] == true)
        );
    }
    let unknown = c.error("build_captions", json!({"run_id": run["run_id"], "style_preset": "missing_package"}));
    assert!(unknown.contains("My neon") && unknown.contains("hormozi") && unknown.contains("typewriter"), "{unknown}");
    c.call("end_run", json!({"run_id": run["run_id"], "action": "keep"}));
    c.finish();
}

const MISSING_FONT: &str = "Nuzky Test Font That Is Not Installed";

fn missing_font_warning() -> Value {
    json!([format!(
        "Font \"{MISSING_FONT}\" is not installed on this computer; its text is drawn in Inter. Install it or pick an installed font."
    )])
}

fn font_style(family: &str) -> Value {
    json!({"fontFamily": family, "fontSize": 80.0, "color": "#ffffff"})
}

/// A font this computer lacks is named once by the edits that give it to text, also when they are retried, and
/// get_state lists it; a bundled font and moving text in a missing one are not named.
#[test]
fn fonts_this_computer_lacks_are_named_over_stdio() {
    let mut c = Client::start(true, None, None);
    let run = c.call("begin_run", json!({"label": "fonts"}))["run_id"].clone();
    let bundled = c.call(
        "apply_edits",
        json!({"run_id": run, "request_id": "inter", "edits": [
            {"type": "addText", "startUs": 0, "text": "Hook", "style": font_style("Inter")}]}),
    );
    assert!(bundled.get("warnings").is_none(), "{bundled}");
    assert_eq!(c.call("get_state", json!({}))["missing_fonts"], json!([]));

    let titles = json!({"run_id": run, "request_id": "titles", "edits": [
        {"type": "addText", "startUs": 1_000_000, "text": "One", "style": font_style(MISSING_FONT)},
        {"type": "addText", "startUs": 2_000_000, "text": "Two", "style": font_style(MISSING_FONT)}]});
    let added = c.call("apply_edits", titles.clone());
    assert_eq!(added["warnings"], missing_font_warning());
    // A retry, as after a failed save or a lost answer, answers the same, warning included.
    assert_eq!(c.call("apply_edits", titles), added);
    let state = c.call("get_state", json!({}));
    assert_eq!(state["missing_fonts"], json!([MISSING_FONT]));
    let text = |state: &Value, text: &str| {
        state["tracks"].as_array().unwrap().iter().find_map(|track| {
            let clip = track["clips"].as_array().unwrap().iter().find(|clip| clip["content"]["text"] == text)?;
            Some(json!({"clip": clip, "trackId": track["id"], "trackKind": track["kind"],
                "keepInPlace": track["keepInPlace"]}))
        })
    };
    let (hook, one) = (text(&state, "Hook").unwrap(), text(&state, "One").unwrap());

    let moved = c.call(
        "apply_edits",
        json!({"run_id": run, "request_id": "move", "edits": [
            {"type": "moveClip", "clipId": one["clip"]["id"], "startUs": 3_000_000}]}),
    );
    assert!(moved.get("warnings").is_none(), "{moved}");
    let pasted = c.call(
        "apply_edits",
        json!({"run_id": run, "request_id": "paste", "edits": [
            {"type": "pasteAttributes", "source": one, "clipIds": [hook["clip"]["id"]]}]}),
    );
    assert_eq!(pasted["warnings"], missing_font_warning());
    let restyled = c.call(
        "apply_edits",
        json!({"run_id": run, "request_id": "restyle", "edits": [
            {"type": "restyleTrack", "trackId": hook["trackId"], "style": {"fontFamily": MISSING_FONT}}]}),
    );
    assert_eq!(restyled["warnings"], missing_font_warning());
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    c.finish();
}

/// Captions and reels in a font this computer lacks name it, a retried make_reels too.
#[test]
fn captions_and_reels_name_fonts_this_computer_lacks_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    let asset = transcribed_take(&c);
    let run = c.call("begin_run", json!({"label": "fonts"}))["run_id"].clone();
    c.call(
        "apply_edits",
        json!({"run_id": run, "request_id": "take", "edits": [
            {"type": "addAssets", "assets": [asset]}, {"type": "addClip", "assetId": "take"}]}),
    );
    let inter = c.call("build_captions", json!({"run_id": run, "style": font_style("Inter")}));
    assert!(inter.get("warnings").is_none(), "{inter}");
    let captions = c.call("build_captions", json!({"run_id": run, "style": font_style(MISSING_FONT)}));
    assert!(captions["caption_count"].as_u64().unwrap() > 1);
    assert_eq!(captions["warnings"], missing_font_warning());

    let key = c.call("get_transcript", json!({"detail": "sentences"}))["transcript_key"].clone();
    let proposed = c.call(
        "propose_reels",
        json!({"run_id": run, "transcript_key": key, "candidates": [
            {"from": 0, "to": 3, "title": "Save", "why": "The whole take.", "score": 0.5}]}),
    );
    let args = json!({"run_id": run, "request_id": "make", "ids": [proposed["candidates"][0]["id"]]});
    let made = c.call("make_reels", args.clone());
    assert_eq!(made["warnings"], missing_font_warning());
    assert_eq!(c.call("make_reels", args), made);
    let reel: Project =
        serde_json::from_slice(&std::fs::read(made["reels"][0]["path"].as_str().unwrap()).unwrap()).unwrap();
    assert!(reel.tracks.iter().flat_map(|track| &track.clips).any(|clip| matches!(&clip.content,
        nuzky_engine::model::ClipContent::Text { style, .. } if style.font_family.as_deref() == Some(MISSING_FONT))));
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    c.finish();
}

/// A learned style whose Settings give edit_transcript, build_captions and the reels' cut their values.
const SETTINGS_STYLE: &str = "# Editing style\n\nLearned from one video.\n\n## Settings\n\n| Setting | Value |\n|---|---|\n| edit_transcript shorten_pauses_us | 530000 |\n| Cuts per minute | 15.5 |\n| build_captions max_words | 3 |\n| build_captions max_chars | 30 |\n| build_captions style | {\"fontSize\":120,\"color\":\"#ffe14d\",\"bold\":true,\"strokeWidth\":0} |\n\n## Pauses\n\nShorten every pause longer than 530 ms to that length.\n\n";

/// Two sentences 2 s apart, with 0.1 s between their words and 1 s of silence after them, on a 720x1280 canvas, in
/// a run.
fn two_sentences(c: &mut Client) -> Value {
    let run = c.call("begin_run", json!({"label": "reel"}))["run_id"].clone();
    let said = [
        ("Everybody", 200_000, 600_000),
        ("saves", 700_000, 1_000_000),
        ("money", 1_100_000, 1_500_000),
        ("every", 1_600_000, 1_900_000),
        ("month.", 2_000_000, 2_400_000),
        ("Then", 4_400_000, 4_700_000),
        ("spend", 4_800_000, 5_100_000),
        ("it.", 5_200_000, 5_500_000),
    ];
    place_take(c, 6_500_000, &said, &run);
    let canvas = json!([{"type": "setCanvas", "width": 720, "height": 1280}]);
    c.call("apply_edits", json!({"run_id": run, "request_id": "canvas", "edits": canvas}));
    run
}

fn plan_cut(c: &mut Client, pause: Option<i64>) -> Value {
    let key = c.call("get_transcript", json!({}))["transcript_key"].clone();
    let mut args = json!({"transcript_key": key, "dry_run": true});
    if let Some(pause) = pause {
        args["shorten_pauses_us"] = json!(pause);
    }
    c.call("edit_transcript", args)
}

/// The reel of both sentences, proposed and made.
fn whole_reel(c: &mut Client, run: &Value) -> (Value, Value) {
    let key = c.call("get_transcript", json!({}))["transcript_key"].clone();
    let candidates = json!([{"from": 0, "to": 7, "title": "Money", "why": "All of it.", "score": 0.5}]);
    let proposed = c.call("propose_reels", json!({"run_id": run, "transcript_key": key, "candidates": candidates}));
    let made = c.call("make_reels", json!({"run_id": run, "ids": [proposed["candidates"][0]["id"]]}));
    let reel: Project =
        serde_json::from_slice(&std::fs::read(made["reels"][0]["path"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(json!(reel.duration_us()), proposed["candidates"][0]["durationUs"]);
    (proposed, made)
}

/// A reel is made with the pause length it was proposed with, whatever EDIT.md says by then: the style's when the
/// agent left it out, and the one the agent gave, such as a creator's rule for reels that overrides the style.
#[test]
fn a_reel_keeps_the_pause_it_was_proposed_with_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    let run = two_sentences(&mut c);
    let (plain, _) = whole_reel(&mut c, &run);
    let length = |reel: &Value| reel["candidates"][0]["durationUs"].as_i64().unwrap();
    let edit_md = c.dir.join("data/nuzky/EDIT.md");
    let mut propose_then_make = |pause: Option<i64>, style_then: &str| {
        std::fs::write(&edit_md, SETTINGS_STYLE).unwrap();
        let key = c.call("get_transcript", json!({}))["transcript_key"].clone();
        let candidates = json!([{"from": 0, "to": 7, "title": "Money", "why": "All of it.", "score": 0.5}]);
        let mut args = json!({"run_id": run, "transcript_key": key, "candidates": candidates});
        if let Some(pause) = pause {
            args["shorten_pauses_us"] = json!(pause);
        }
        let proposed = c.call("propose_reels", args);
        std::fs::write(&edit_md, SETTINGS_STYLE.replace("530000", style_then)).unwrap();
        let stored = c.call("get_state", json!({}))["reel_candidates"].clone();
        let made = c.call("make_reels", json!({"run_id": run, "ids": [proposed["candidates"][0]["id"]]}));
        assert!(made.get("from_style").is_none() && made.get("style_warnings").is_none(), "{made}");
        let reel: Project =
            serde_json::from_slice(&std::fs::read(made["reels"][0]["path"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(json!(reel.duration_us()), proposed["candidates"][0]["durationUs"], "{proposed} {stored}");
        (proposed, stored)
    };

    let (styled, stored) = propose_then_make(None, "900000");
    assert_eq!(length(&styled) - length(&plain), 230_000, "the 2 s pause shortens to 530 ms, not 300 ms");
    assert_eq!(styled["from_style"], json!({"shorten_pauses_us": 530_000}));
    let id = &styled["candidates"][0]["id"];
    let kept = stored.as_array().unwrap().iter().find(|c| &c["id"] == id).unwrap();
    assert_eq!(kept["shortenPausesUs"], 530_000, "{stored}");

    let (rule, _) = propose_then_make(Some(300_000), "fast");
    assert_eq!(length(&rule), length(&plain), "the pause the agent gave wins over the style's");
    assert!(rule.get("from_style").is_none() && rule.get("style_warnings").is_none(), "{rule}");
    assert_eq!(rule["candidates"][0]["shortenPausesUs"], 300_000);
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    c.finish();
}

/// Each caption's text and style.
fn captions_of(tracks: &Value) -> Vec<(String, Value)> {
    let track = tracks.as_array().unwrap().iter().find(|t| t["name"] == "Captions").unwrap();
    let clips = track["clips"].as_array().unwrap().iter();
    clips.map(|clip| (clip["content"]["text"].as_str().unwrap().to_owned(), clip["content"]["style"].clone())).collect()
}

fn most_words(captions: &[(String, Value)]) -> usize {
    captions.iter().map(|(text, _)| text.split(' ').count()).max().unwrap()
}

fn most_chars(captions: &[(String, Value)]) -> usize {
    captions.iter().map(|(text, _)| text.chars().count()).max().unwrap()
}

/// An agent that never reads EDIT.md and leaves the arguments out cuts with the creator's pause, captions with their
/// limits and look, and cuts reels with their pause. Arguments it gives win one by one, a row its tool cannot take
/// is skipped with a warning, and without the style every answer stays as it was.
#[test]
fn the_creators_settings_fill_in_what_an_agent_leaves_out_over_stdio() {
    let mut plain = Client::start(true, None, Some(None));
    let plain_run = two_sentences(&mut plain);
    let (plain_reel, plain_made) = whole_reel(&mut plain, &plain_run);
    assert!(plain_reel.get("from_style").is_none() && plain_made.get("from_style").is_none(), "{plain_made}");
    assert_eq!(plan_cut(&mut plain, None), plan_cut(&mut plain, Some(300_000)));
    let key = plain.call("get_transcript", json!({}))["transcript_key"].clone();
    plain.call("edit_transcript", json!({"run_id": plain_run, "transcript_key": key}));
    let built = plain.call("build_captions", json!({"run_id": plain_run}));
    let mut fields: Vec<&String> = built.as_object().unwrap().keys().collect();
    fields.sort();
    assert_eq!(fields, ["caption_count", "created", "removed", "revision", "session_epoch"]);
    let captions = captions_of(&plain.call("get_state", json!({}))["tracks"]);
    assert_eq!((most_words(&captions), most_chars(&captions) <= 15), (2, true), "{captions:?}");
    assert!(captions.iter().all(|(_, style)| style["fontSize"] == 95.0 && style["color"] == "#ffffff"));
    plain.call("end_run", json!({"run_id": plain_run, "action": "keep"}));
    plain.finish();

    let mut styled = Client::start(true, None, Some(Some(SETTINGS_STYLE)));
    let run = two_sentences(&mut styled);
    let planned = plan_cut(&mut styled, None);
    let tight = plan_cut(&mut styled, Some(300_000));
    let after = |plan: &Value| plan["duration_us"]["after"].as_i64().unwrap();
    assert_eq!(after(&planned) - after(&tight), 230_000, "the 2 s pause shortens to 530 ms, not 300 ms");
    assert_eq!(planned["from_style"], json!({"shorten_pauses_us": 530_000}));
    let mut same = planned.clone();
    same.as_object_mut().unwrap().remove("from_style");
    assert_eq!(same, plan_cut(&mut styled, Some(530_000)), "an explicit pause names nothing from the style");
    let (reel, made) = whole_reel(&mut styled, &run);
    let length = |reel: &Value| reel["candidates"][0]["durationUs"].as_i64().unwrap();
    assert_eq!(length(&reel) - length(&plain_reel), 230_000);
    assert_eq!(reel["from_style"], planned["from_style"]);
    assert!(made.get("from_style").is_none(), "the reel keeps the pause it was proposed with: {made}");

    let key = styled.call("get_transcript", json!({}))["transcript_key"].clone();
    let cut = styled.call("edit_transcript", json!({"run_id": run, "transcript_key": key}));
    assert_eq!(cut["duration_us"], planned["duration_us"]);

    // Rows a tool cannot take are skipped with a warning; EDIT.md is read again on every call.
    let edit_md = styled.dir.join("data/nuzky/EDIT.md");
    let broken = SETTINGS_STYLE.replace("| 530000 |", "| fast |").replace("| 3 |", "| 0 |").replace("120", "-1");
    std::fs::write(&edit_md, broken).unwrap();
    let mut warned = plan_cut(&mut styled, None);
    let warning = warned.as_object_mut().unwrap().remove("style_warnings").unwrap();
    assert!(warning[0].as_str().unwrap().contains("edit_transcript shorten_pauses_us"), "{warning}");
    assert_eq!(warned, plan_cut(&mut styled, Some(300_000)));
    let rebuilt = styled.call("build_captions", json!({"run_id": run}));
    assert_eq!(rebuilt["from_style"], json!({"max_chars": 30}));
    let warnings = rebuilt["style_warnings"].to_string();
    assert!(warnings.contains("build_captions max_words") && warnings.contains("build_captions style"), "{warnings}");
    let captions = captions_of(&styled.call("get_state", json!({}))["tracks"]);
    assert_eq!(most_words(&captions), 2, "{captions:?}");
    assert!(captions.iter().all(|(_, style)| style["fontSize"] == 95.0 && style["color"] == "#ffffff"));
    // An EDIT.md that is there but cannot be read gives the defaults with a warning, and only when they are used.
    std::fs::write(&edit_md, b"\xff\xfe").unwrap();
    let mut unreadable = plan_cut(&mut styled, None);
    let warning = unreadable.as_object_mut().unwrap().remove("style_warnings").unwrap();
    assert!(warning[0].as_str().unwrap().starts_with("EDIT.md could not be read"), "{warning}");
    assert_eq!(unreadable, plan_cut(&mut styled, Some(300_000)));
    std::fs::write(&edit_md, SETTINGS_STYLE).unwrap();

    // A preset replaces the learned look as a whole, and a limit given replaces only itself.
    let karaoke = styled.call("build_captions", json!({"run_id": run, "style_preset": "karaoke", "max_chars": 14}));
    assert_eq!(karaoke["from_style"], json!({"max_words": 3}));
    let captions = captions_of(&styled.call("get_state", json!({}))["tracks"]);
    assert_eq!((most_words(&captions), most_chars(&captions)), (3, 14), "{captions:?}");
    assert!(captions.iter().all(|(_, style)| style["fontSize"] == 95.0 && style["highlight"] == "#ffe14d"));

    let built = styled.call("build_captions", json!({"run_id": run}));
    assert_eq!((&built["from_style"]["max_words"], &built["from_style"]["max_chars"]), (&json!(3), &json!(30)));
    assert_eq!(built["from_style"]["style"]["fontSize"], 80.0, "120 px for 1080 across is 80 px for 720");
    assert!(built.get("style_warnings").is_none(), "{built}");
    styled.call("end_run", json!({"run_id": run, "action": "keep"}));
    styled.finish();
    let saved: Value = serde_json::from_slice(&std::fs::read(styled.dir.join("project.nuzky")).unwrap()).unwrap();
    let project: Project = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(json!(project.duration_us()), planned["duration_us"]["after"]);
    let captions = captions_of(&saved["tracks"]);
    assert_eq!(most_words(&captions), 3, "{captions:?}");
    assert!(most_chars(&captions) > 15, "{captions:?}");
    for (_, style) in &captions {
        assert_eq!(
            (&style["fontSize"], &style["color"], &style["bold"], &style["strokeWidth"]),
            (&json!(80.0), &json!("#ffe14d"), &json!(true), &json!(0.0)),
            "{style}"
        );
    }
}

/// A 4 s take whose words are stored as recognised, 0.8 s apart, and placed on the timeline by a
/// run. Words come from the store, so the file is never decoded and any bytes do. The client
/// must have a data directory of its own (`style: Some(..)`).
fn place_transcribed_take(c: &mut Client, said: &[&str], run_id: &Value) {
    let said: Vec<_> = said
        .iter()
        .enumerate()
        .map(|(i, text)| (*text, 200_000 + i as i64 * 800_000, 700_000 + i as i64 * 800_000))
        .collect();
    place_take(c, 4_000_000, &said, run_id);
}

/// A take of `duration_us` whose words (text, start, end) are stored and placed as above.
fn place_take(c: &mut Client, duration_us: i64, said: &[(&str, i64, i64)], run_id: &Value) {
    use nuzky_engine::{model::Asset, model::AssetKind, speech::Word};
    use nuzky_session::transcripts::{Record, TranscriptStore, VERSION};
    let path = c.dir.join("take.mov");
    std::fs::write(&path, b"take").unwrap();
    let asset = Asset {
        id: "take".into(),
        name: "take.mov".into(),
        path: path.to_string_lossy().into(),
        kind: AssetKind::Video,
        duration_us,
        width: 1080,
        height: 1920,
        fps: 30.0,
        has_audio: true,
        rotation: 0,
        mirror: false,
        credit: None,
        part: None,
    };
    let words = said
        .iter()
        .map(|&(text, start_us, end_us)| Word { start_us, end_us, text: text.into(), probability: 0.9 })
        .collect();
    let store = TranscriptStore::at(c.dir.join("data/nuzky/transcripts")).unwrap();
    let record = Record {
        version: VERSION,
        fingerprint: store.fingerprint(&asset).unwrap(),
        duration_us: asset.duration_us,
        model: "fixture".into(),
        language: "cs".into(),
        words,
        segments: vec![],
        alignment: None,
    };
    store.put(&asset, &record).unwrap();
    let edits = json!([{"type":"addAssets","assets":[asset]},{"type":"addClip","assetId":"take"}]);
    c.call("apply_edits", json!({"run_id":run_id,"request_id":"place","edits":edits}));
}

/// Each caption clip's id, text, and whether its timed words still spell it, so karaoke highlights it.
fn caption_clips(c: &mut Client) -> Vec<(String, String, bool)> {
    let state = c.call("get_state", json!({}));
    let track = state["tracks"].as_array().unwrap().iter().find(|t| t["name"] == "Captions").unwrap().clone();
    track["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|clip| {
            let text = clip["content"]["text"].as_str().unwrap().to_owned();
            let words = clip["content"]["words"].as_array().cloned().unwrap_or_default();
            let timed = words.iter().map(|w| w["text"].as_str().unwrap()).collect::<Vec<_>>().join(" ") == text;
            (clip["id"].as_str().unwrap().to_owned(), text, timed)
        })
        .collect()
}

fn caption_texts(c: &mut Client) -> Vec<String> {
    caption_clips(c).into_iter().map(|(_, text, _)| text).collect()
}

#[test]
fn correct_words_fixes_the_transcript_and_captions_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    let tools = c.rpc("tools/list", json!({}));
    let tool = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "correct_words").unwrap();
    assert_eq!(tool["inputSchema"]["required"], json!(["run_id", "transcript_key", "corrections"]));
    let run = c.call("begin_run", json!({"label":"fix a word"}));
    place_transcribed_take(&mut c, &["Mikrofon", "vejte", "co", "nejblíž."], &run["run_id"]);
    c.call("build_captions", json!({"run_id":run["run_id"]}));
    assert!(caption_texts(&mut c).iter().any(|t| t.contains("vejte")));
    let transcript = c.call("get_transcript", json!({}));
    assert_eq!(transcript["words"][1]["text"], "vejte");
    let args = json!({"run_id":run["run_id"],"request_id":"dejte","transcript_key":transcript["transcript_key"],
        "corrections":[{"i":1,"text":"dejte"}]});
    let fixed = c.call("correct_words", args.clone());
    assert_eq!(fixed["words"], json!([{"i":1,"before":"vejte","after":"dejte"}]));
    assert_eq!(fixed["captions_changed"].as_array().unwrap().len(), 1);
    assert_eq!(c.call("correct_words", args), fixed, "a retry with the same request_id corrects once");
    let shown = caption_texts(&mut c);
    assert!(shown.iter().any(|t| t.contains("dejte")) && !shown.iter().any(|t| t.contains("vejte")), "{shown:?}");
    let now = c.call("get_transcript", json!({}));
    assert_eq!(now["words"][1]["text"], "dejte");
    assert_eq!(now["transcript_key"], fixed["transcript_key"]);
    let stale = json!({"run_id":run["run_id"],"transcript_key":transcript["transcript_key"],"corrections":[{"i":2,"text":"to"}]});
    assert!(c.error("correct_words", stale).contains("SPEECH_CHANGED"));
    // Building the captions again reads the corrected word.
    c.call("build_captions", json!({"run_id":run["run_id"]}));
    assert_eq!(caption_texts(&mut c), shown);
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    c.finish();
    let disk: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.nuzky")).unwrap()).unwrap();
    assert_eq!(
        disk["wordCorrections"],
        json!([{"assetId":"take","sourceStartUs":1_000_000,"original":"vejte","text":"dejte"}])
    );
}

/// An agent off by one word once overwrote "něco" instead of "natchne" with a current
/// transcript_key. `expected` names the word the agent means, so a shifted index changes nothing.
#[test]
fn correct_words_refuses_a_word_that_reads_otherwise_than_expected_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    let run = c.call("begin_run", json!({"label":"fix a word"}));
    let run_id = run["run_id"].clone();
    place_transcribed_take(&mut c, &["Mikrofon", "něco", "natchne", "nejblíž."], &run_id);
    c.call("build_captions", json!({"run_id":run_id}));
    let transcript = c.call("get_transcript", json!({}));
    let key = transcript["transcript_key"].clone();
    let before = (c.call("get_state", json!({}))["revision"].clone(), caption_texts(&mut c));
    let unchanged = |c: &mut Client| {
        assert_eq!((c.call("get_state", json!({}))["revision"].clone(), caption_texts(c)), before);
        assert_eq!(c.call("get_transcript", json!({})), transcript);
    };
    let refused = |c: &mut Client, args: Value| {
        let error: Value = serde_json::from_str(&c.error("correct_words", args)).unwrap();
        error["error"].as_str().unwrap().to_owned()
    };

    let shifted =
        json!({"run_id":run_id,"transcript_key":key,"corrections":[{"i":1,"text":"nadchne","expected":"natchne"}]});
    assert_eq!(
        refused(&mut c, shifted),
        r#"WORD_MISMATCH: word 1 reads "něco", expected "natchne"; read get_transcript again"#
    );
    unchanged(&mut c);
    let mixed = json!({"run_id":run_id,"transcript_key":key,"corrections":[
        {"i":0,"text":"Mikrofón","expected":"Mikrofon"},{"i":3,"text":"nejbližší.","expected":"natchne"}]});
    assert!(refused(&mut c, mixed).starts_with("WORD_MISMATCH: word 3"));
    unchanged(&mut c);

    // Compared after trimming; a retry finishes the call although the word reads otherwise now.
    let right = json!({"run_id":run_id,"request_id":"nadchne","transcript_key":key,
        "corrections":[{"i":2,"text":"nadchne","expected":" natchne "}]});
    let fixed = c.call("correct_words", right.clone());
    assert_eq!(fixed["words"], json!([{"i":2,"before":"natchne","after":"nadchne"}]));
    assert_eq!(c.call("correct_words", right), fixed);
    assert!(caption_texts(&mut c).iter().any(|t| t.contains("nadchne")));

    let key = fixed["transcript_key"].clone();
    let original =
        json!({"run_id":run_id,"transcript_key":key,"corrections":[{"i":2,"text":"nadchne!","expected":"natchne"}]});
    assert!(refused(&mut c, original).starts_with(r#"WORD_MISMATCH: word 2 reads "nadchne""#));
    let again =
        json!({"run_id":run_id,"transcript_key":key,"corrections":[{"i":2,"text":"nadchne!","expected":"nadchne"}]});
    let fixed = c.call("correct_words", again);
    assert_eq!(fixed["words"], json!([{"i":2,"before":"nadchne","after":"nadchne!"}]));

    let unchecked =
        json!({"run_id":run_id,"transcript_key":fixed["transcript_key"],"corrections":[{"i":1,"text":"něco,"}]});
    assert_eq!(c.call("correct_words", unchecked)["words"], json!([{"i":1,"before":"něco","after":"něco,"}]));
    let words: Vec<_> = c.call("get_transcript", json!({}))["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(words, ["Mikrofon", "něco,", "nadchne!", "nejblíž."]);
    c.call("end_run", json!({"run_id":run_id,"action":"keep"}));
    c.finish();
}

#[test]
fn a_correction_with_more_words_than_a_caption_splits_and_its_parts_follow_corrections_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    let run = c.call("begin_run", json!({"label":"fix a word"}));
    place_transcribed_take(&mut c, &["Tak", "vzalbysi", "nebo", "ne."], &run["run_id"]);
    let correct = |c: &mut Client, text: &str| {
        let key = c.call("get_transcript", json!({}))["transcript_key"].clone();
        let args = json!({"run_id":run["run_id"],"transcript_key":key,"corrections":[{"i":1,"text":text}]});
        c.call("correct_words", args)["captions_changed"].as_array().unwrap().len()
    };
    correct(&mut c, "Vzal bys ji?");
    c.call("build_captions", json!({"run_id":run["run_id"],"max_words":2}));
    let captions = caption_clips(&mut c);
    assert!(captions.iter().all(|(_, t, _)| t.split_whitespace().count() <= 2), "{captions:?}");
    assert_eq!(caption_texts(&mut c).join(" "), "Tak Vzal bys ji? nebo ne.");
    assert!(captions.iter().all(|(.., timed)| *timed), "every part keeps its karaoke time: {captions:?}");
    let transcript = c.call("get_transcript", json!({}));
    assert_eq!(transcript["words"].as_array().unwrap().len(), 4, "the transcript keeps one word");
    assert_eq!(transcript["words"][1]["text"], "Vzal bys ji?");

    assert_eq!(correct(&mut c, "Vzala bys jí?"), 2);
    assert_eq!(caption_texts(&mut c), ["Tak", "Vzala bys", "jí?", "nebo", "ne."]);
    let captions = caption_clips(&mut c);
    assert!(captions.iter().all(|(.., timed)| *timed), "{captions:?}");
    correct(&mut c, "Vzal by sis ji?");
    assert_eq!(caption_texts(&mut c), ["Tak", "Vzal by", "sis ji?", "nebo", "ne."]);
    correct(&mut c, "Vzalbysji");
    assert_eq!(caption_texts(&mut c), ["Tak", "Vzalbysji", "nebo", "ne."]);
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    c.finish();
    let disk: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.nuzky")).unwrap()).unwrap();
    assert_eq!(
        disk["wordCorrections"],
        json!([{"assetId":"take","sourceStartUs":1_000_000,"original":"vzalbysi","text":"Vzalbysji"}])
    );
}

fn correct_second_word(c: &mut Client, run: &Value, text: &str) -> Value {
    let key = c.call("get_transcript", json!({}))["transcript_key"].clone();
    c.call("correct_words", json!({"run_id":run["run_id"],"transcript_key":key,"corrections":[{"i":1,"text":text}]}))
}

/// "Tak Vzal bys ji? nebo ne." with "Vzal bys ji?" one recognised word, its parts over two captions.
fn split_word_over_two_captions(c: &mut Client) -> (Value, Vec<String>) {
    let run = c.call("begin_run", json!({"label":"fix a word"}));
    place_transcribed_take(c, &["Tak", "vzalbysi", "nebo", "ne."], &run["run_id"]);
    correct_second_word(c, &run, "Vzal bys ji?");
    c.call("build_captions", json!({"run_id":run["run_id"],"max_words":2}));
    assert_eq!(caption_texts(c), ["Tak", "Vzal bys", "ji?", "nebo", "ne."]);
    (run, caption_clips(c).into_iter().map(|(id, ..)| id).collect())
}

#[test]
fn a_part_left_by_a_shorter_correction_still_takes_the_next_one_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    let run = c.call("begin_run", json!({"label":"fix a word"}));
    place_transcribed_take(&mut c, &["Tak", "vzalbysi", "nebo", "ne."], &run["run_id"]);
    correct_second_word(&mut c, &run, "Vzal bys ji?");
    c.call("build_captions", json!({"run_id":run["run_id"],"max_words":1}));
    correct_second_word(&mut c, &run, "Vzalbysji");
    // The part kept is the first, which ends before the middle of the word.
    assert_eq!(caption_texts(&mut c), ["Tak", "Vzalbysji", "nebo", "ne."]);
    let fixed = correct_second_word(&mut c, &run, "Vzala");
    assert_eq!(fixed["captions_changed"].as_array().unwrap().len(), 1, "{fixed}");
    assert_eq!(caption_texts(&mut c), ["Tak", "Vzala", "nebo", "ne."]);
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    c.finish();
}

#[test]
fn a_deleted_part_leaves_the_others_as_they_are_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    let (run, built) = split_word_over_two_captions(&mut c);
    let edits = json!([{"type":"deleteClips","clipIds":[built[2]]}]);
    c.call("apply_edits", json!({"run_id":run["run_id"],"request_id":"delete","edits":edits}));
    let fixed = correct_second_word(&mut c, &run, "Vzala bys jí?");
    assert_eq!(fixed["captions_changed"], json!([]));
    assert_eq!(fixed["captions_kept"], json!([built[1]]));
    assert!(fixed["note"].as_str().unwrap().contains("build_captions"), "{fixed}");
    assert_eq!(caption_texts(&mut c), ["Tak", "Vzal bys", "nebo", "ne."]);
    assert_eq!(c.call("get_transcript", json!({}))["words"][1]["text"], "Vzala bys jí?");
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    c.finish();
}

#[test]
fn a_part_typed_over_by_hand_is_never_overwritten_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    let (run, built) = split_word_over_two_captions(&mut c);
    let edits = json!([{"type":"updateClip","clipId":built[2],"text":"JI!!!"}]);
    c.call("apply_edits", json!({"run_id":run["run_id"],"request_id":"shout","edits":edits}));
    let fixed = correct_second_word(&mut c, &run, "Vzala bys jí?");
    assert_eq!(fixed["captions_changed"], json!([]));
    let mut kept = vec![built[1].clone(), built[2].clone()];
    kept.sort();
    assert_eq!(fixed["captions_kept"], json!(kept));
    assert_eq!(caption_texts(&mut c), ["Tak", "Vzal bys", "JI!!!", "nebo", "ne."]);
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    c.finish();
}

/// A 12 s talk of three sentences of four 0.4 s tone "words", the middle sentence 12 dB louder,
/// with its words stored as if recognised.
#[test]
fn emphasis_suggests_and_apply_zooms_punches_in_as_one_undo_over_stdio() {
    use nuzky_session::transcripts::{Record, TranscriptStore, VERSION};
    let mut c = Client::start(true, None, Some(None));
    let mut samples = vec![0i16; 12 * 48_000];
    let mut words = Vec::new();
    for sentence in 0..3i64 {
        for word in 0..4i64 {
            let start = 1_000_000 + sentence * 4_000_000 + word * 500_000;
            let level = if sentence == 1 { 0.4 } else { 0.1 };
            let first = (start * 48 / 1000) as usize;
            for (i, sample) in samples[first..first + 19_200].iter_mut().enumerate() {
                *sample = (level * (i as f64 * 440.0 / 48_000.0 * std::f64::consts::TAU).sin() * 32_767.0) as i16;
            }
            let end = if word == 3 { "." } else { "" };
            words.push(json!({"start_us": start, "end_us": start + 400_000, "text": format!(" w{sentence}{word}{end}"), "probability": 0.9}));
        }
    }
    let wav = c.dir.join("speech.wav");
    std::fs::write(&wav, wav_bytes(&samples)).unwrap();
    let video = c.dir.join("talk.mp4");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=360x640:r=30:d=12", "-i"])
        .arg(&wav)
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&video)
        .status()
        .expect("ffmpeg makes the test video");
    assert!(status.success());

    let run = c.call("begin_run", json!({"label":"place"}));
    let ids = c.call("import_media", json!({"run_id":run["run_id"],"paths":[video]}))["asset_ids"].clone();
    c.call(
        "apply_edits",
        json!({"run_id":run["run_id"],"request_id":"place","edits":[{"type":"addClip","assetId":ids[0]}]}),
    );
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    assert!(c.error("analyze", json!({"kind":"emphasis"})).contains("TRANSCRIPT_MISSING"));
    let asset: nuzky_engine::model::Asset =
        serde_json::from_value(c.call("get_state", json!({}))["assets"][0].clone()).unwrap();
    let store = TranscriptStore::at(c.dir.join("data/nuzky/transcripts")).unwrap();
    let record = Record {
        version: VERSION,
        fingerprint: store.fingerprint(&asset).unwrap(),
        duration_us: asset.duration_us,
        model: "fixture".into(),
        language: "cs".into(),
        words: serde_json::from_value(json!(words)).unwrap(),
        segments: vec![],
        alignment: None,
    };
    store.put(&asset, &record).unwrap();

    // Words stored earlier, no sound prepared and no app to prepare it: a job does, then it answers.
    let waiting: Value = serde_json::from_str(&c.error("analyze", json!({"kind":"emphasis"}))).unwrap();
    let waiting = waiting["error"].as_str().unwrap();
    assert!(waiting.starts_with("AUDIO_NOT_READY") && waiting.contains("talk.mp4"), "{waiting}");
    let job_id = waiting.split("as job ").nth(1).and_then(|rest| rest.split(';').next()).unwrap().to_owned();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while c.call("job", json!({"job_id":job_id,"action":"get"}))["status"] == "running" {
        assert!(std::time::Instant::now() < deadline, "preparing the sound timed out");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(c.call("job", json!({"job_id":job_id,"action":"get"}))["status"], "done");
    let found = c.call("analyze", json!({"kind":"emphasis"}));
    assert_eq!(found, c.call("analyze", json!({"kind":"emphasis"})), "the same words and sound gave another result");
    let key = c.call("get_transcript", json!({}))["transcript_key"].clone();
    assert_eq!(found["transcript_key"], key);
    // The loud sentence; the hook is less than 5 s before it, and 12 s allow one zoom.
    let zooms = found["zooms"].as_array().unwrap();
    assert_eq!(zooms.len(), 1, "{found}");
    assert_eq!(
        (zooms[0]["from"].clone(), zooms[0]["to"].clone(), zooms[0]["text"].clone()),
        (json!(4), json!(7), json!("w10 w11 w12 w13."))
    );
    assert_eq!((zooms[0]["score"].clone(), zooms[0]["scale"].clone()), (json!(0.6), json!(1.24)));

    let run = c.call("begin_run", json!({"label":"zoom"}));
    let stale = json!({"run_id":run["run_id"],"transcript_key":"stale","zooms":[{"from":4,"to":7,"scale":1.24}]});
    assert!(c.error("apply_zooms", stale).contains("SPEECH_CHANGED"));
    let args = json!({"run_id":run["run_id"],"request_id":"zoom","transcript_key":key,"zooms":[{"from":4,"to":7,"scale":1.24}]});
    let applied = c.call("apply_zooms", args.clone());
    assert_eq!(applied["ranges"], json!([{"startUs": 4_850_000, "endUs": 7_050_000, "scale": 1.24}]));
    assert_eq!(applied["skipped"], json!([]));
    // A retry gives the same answer without zooming again; other zooms under the same id are refused.
    assert_eq!(c.call("apply_zooms", args.clone()), applied);
    let mut other = args;
    other["zooms"][0]["scale"] = json!(1.3);
    assert!(c.error("apply_zooms", other).contains("REQUEST_CONFLICT"));
    let state = c.call("get_state", json!({}));
    let main: Vec<(i64, f64)> = state["tracks"][0]["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|clip| (clip["startUs"].as_i64().unwrap(), clip["content"]["transform"]["scale"].as_f64().unwrap()))
        .collect();
    assert_eq!(main.iter().map(|m| m.0).collect::<Vec<_>>(), [0, 4_850_000, 7_050_000]);
    assert!((main[1].1 - 1.24).abs() < 1e-6 && main[0].1 == 1.0 && main[2].1 == 1.0, "{main:?}");
    assert_eq!(c.call("get_transcript", json!({}))["transcript_key"], applied["transcript_key"]);
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    // One undo takes the whole run back.
    c.call("undo_run", json!({"run_id":run["run_id"]}));
    assert_eq!(c.call("get_state", json!({}))["tracks"][0]["clips"].as_array().unwrap().len(), 1);
    assert!(
        c.error("apply_zooms", json!({"run_id":"none","transcript_key":key,"zooms":[{"from":4,"to":7,"scale":1.2}]}))
            .contains("INVALID_RUN")
    );
    c.finish();
}

#[test]
#[ignore = "Requires tmp-test/speech.wav and installed small, Silero and English word timing models; run with scripts/isolated.sh"]
fn transcribe_edit_and_caption_real_media_over_stdio() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut c = Client::new(true);
    let media = c.dir.join("long-speech.mp4");
    assert!(
        Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(root.join("tmp-test/speech.wav"))
            .args([
                "-f",
                "lavfi",
                "-i",
                "color=s=16x16:r=1",
                "-af",
                "aresample=16000,apad=whole_dur=16,aloop=loop=7:size=256000",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-c:a",
                "aac",
                "-shortest"
            ])
            .arg(&media)
            .status()
            .unwrap()
            .success()
    );
    let run = c.call("begin_run", json!({"label":"import"}));
    let assets = c.call("import_media", json!({"run_id":run["run_id"],"paths":[media]}));
    let asset_id = &assets["asset_ids"][0];
    c.call(
        "apply_edits",
        json!({"run_id":run["run_id"],"request_id":"place","edits":[{"type":"addClip","assetId":asset_id}]}),
    );
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    let job = c.call("transcribe", json!({"asset_ids":[asset_id],"model":"small"}));
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    let (mut recognising, mut measuring, mut estimate) = (false, false, false);
    let mut previous = 0.0;
    loop {
        let status = c.call("job", json!({"job_id":job["job_id"],"action":"wait","timeout_s":1}));
        eprintln!("job progress: {status}");
        let progress = status["progress"].as_f64().unwrap_or(0.0);
        assert!(progress >= previous, "progress went backwards: {status}");
        previous = progress;
        if status["status"] == "running" {
            recognising |= status["phase"] == "transcribing" && progress > 0.0 && progress < 0.5;
            measuring |= status["phase"] == "measuring_word_times" && progress > 0.5 && progress < 1.0;
            estimate |= status["remaining_s"].as_u64().is_some();
        }
        if status["status"] == "done" {
            // The English word timing model from scripts/fixtures.sh measured the words.
            assert_eq!(status["result"]["assets"][0]["word_times"], "measured", "{status}");
            break;
        }
        assert_eq!(status["status"], "running", "{status}");
        assert!(std::time::Instant::now() < deadline, "transcription timed out");
    }
    assert!(
        recognising && measuring && estimate,
        "recognising={recognising}, measuring={measuring}, estimate={estimate}"
    );
    let at = std::time::Instant::now();
    assert_eq!(c.call("job", json!({"job_id":job["job_id"],"action":"wait"}))["status"], "done");
    assert!(at.elapsed() < Duration::from_millis(250));
    let transcript = c.call("get_transcript", json!({}));
    assert!(transcript["words"].as_array().unwrap().len() >= 2);
    assert_eq!(transcript["untranscribed"], json!([]));
    let run = c.call("begin_run", json!({"label":"words and captions"}));
    let args =
        json!({"run_id":run["run_id"],"transcript_key":transcript["transcript_key"],"delete":[[0,0]],"dry_run":true});
    let preview = c.call("edit_transcript", args.clone());
    assert_eq!(c.call("get_transcript", json!({}))["transcript_key"], transcript["transcript_key"]);
    let mut args = args;
    args["dry_run"] = json!(false);
    let edited = c.call("edit_transcript", args);
    assert_eq!(preview["duration_us"], edited["duration_us"]);
    let captions = c.call("build_captions", json!({"run_id":run["run_id"]}));
    assert!(captions["caption_count"].as_u64().unwrap() > 0);
    // Rebuilt as karaoke: each caption keeps its spoken words, which spell its text.
    assert!(
        c.error("build_captions", json!({"run_id":run["run_id"],"style_preset":"unknown_caption_package"}))
            .contains("karaoke")
    );
    let karaoke = c.call("build_captions", json!({"run_id":run["run_id"],"style_preset":"karaoke"}));
    assert_eq!(karaoke["caption_count"], captions["caption_count"]);
    let state = c.call("get_state", json!({}));
    let track = state["tracks"].as_array().unwrap().iter().find(|t| t["name"] == "Captions").unwrap();
    for clip in track["clips"].as_array().unwrap() {
        let content = &clip["content"];
        assert_eq!(content["style"]["highlight"], "#ffe14d", "{content}");
        let words: Vec<&str> =
            content["words"].as_array().unwrap().iter().map(|w| w["text"].as_str().unwrap()).collect();
        assert_eq!(words.join(" "), content["text"].as_str().unwrap());
    }
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    let slot = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(nuzky_analysis::models_dir().join(".recognition.lock"))
        .unwrap();
    slot.lock().unwrap();
    let job = c.call("transcribe", json!({"asset_ids":[asset_id],"model":"small"}));
    c.send(json!({"jsonrpc":"2.0", "id":99999, "method":"tools/call", "params":{"name":"job", "arguments":{"job_id":job["job_id"],"action":"wait","timeout_s":30}}}));
    std::thread::sleep(Duration::from_millis(50));
    let closed = std::time::Instant::now();
    c.finish();
    // rmcp would otherwise give a call in flight 5 s after its input ends.
    assert!(closed.elapsed() < Duration::from_secs(2), "wait held stdio shutdown open: {:?}", closed.elapsed());
}

/// Windows finds the empty vulkan-1.dll beside this copy of the CLI before the graphics driver's and cannot load it,
/// as on a computer without a Vulkan driver. Nuzky still starts and recognises speech, on the processor.
#[cfg(windows)]
#[test]
#[ignore = "Requires tmp-test/talk.mp4 and the small, Silero and English word timing models from scripts/fixtures.sh"]
fn recognises_speech_without_a_vulkan_loader() {
    let root = dunce::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    let fixtures = root.join("tmp-test/xdg/data/nuzky/models");
    let dir = std::env::temp_dir().join(format!("nuzky-mcp-{}", new_id()));
    let models = dir.join("data/nuzky/models");
    std::fs::create_dir_all(&models).unwrap();
    for file in ["ggml-small.bin", nuzky_analysis::VAD_MODEL, "wav2vec2-base-960h.gguf"] {
        std::fs::hard_link(fixtures.join(file), models.join(file))
            .or_else(|_| std::fs::copy(fixtures.join(file), models.join(file)).map(|_| ()))
            .unwrap();
    }
    let binary = dir.join("nuzky.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_nuzky"), &binary).unwrap();
    std::fs::write(dir.join("vulkan-1.dll"), b"").unwrap();
    let project = dir.join("project.nuzky");
    std::fs::write(&project, serde_json::to_vec(&Project::new("Original")).unwrap()).unwrap();
    let mut c = Client::spawn_binary(&binary, dir, &project, true, Some(None), &[]);
    let run = c.call("begin_run", json!({"label":"import"}));
    let ids =
        c.call("import_media", json!({"run_id":run["run_id"],"paths":[root.join("tmp-test/talk.mp4")]}))["asset_ids"]
            .clone();
    c.call(
        "apply_edits",
        json!({"run_id":run["run_id"],"request_id":"place","edits":[{"type":"addClip","assetId":ids[0]}]}),
    );
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    let job = c.call("transcribe", json!({"asset_ids":ids,"model":"small","language":"en"}));
    let done = wait_job(&mut c, &job, Duration::from_secs(600), |_| false);
    assert_eq!(done["status"], "done", "{done}");
    let transcript = c.call("get_transcript", json!({}));
    let words: Vec<String> = transcript["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["text"].as_str().unwrap().trim().trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .collect();
    assert!(words.contains(&"channel".into()) && words.contains(&"captions".into()), "{words:?}");
    c.finish();
}

#[test]
#[ignore = "Requires tmp-test/reel-1..3.mp4 from scripts/fixtures.sh and large-v3-turbo-q5_0 + Silero in tmp-test/xdg/data/nuzky/models"]
fn retakes_of_three_czech_takes_over_stdio() {
    let root = dunce::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    let model = root.join("tmp-test/xdg/data/nuzky/models/ggml-large-v3-turbo-q5_0.bin");
    let takes: Vec<PathBuf> = (1..=3).map(|n| root.join(format!("tmp-test/reel-{n}.mp4"))).collect();
    // Its own data directory, so no transcript stored by an earlier run is found, with the
    // Czech word timing model of scripts/fixtures.sh so recognition does not download it.
    let mut c = Client::start(true, None, Some(None));
    let models = c.dir.join("data/nuzky/models");
    std::fs::create_dir_all(&models).unwrap();
    let aligner = "wav2vec2-xls-r-300m-cs-250-q8_0.gguf";
    std::fs::hard_link(root.join("tmp-test/xdg/data/nuzky/models").join(aligner), models.join(aligner))
        .or_else(|_| {
            std::fs::copy(root.join("tmp-test/xdg/data/nuzky/models").join(aligner), models.join(aligner)).map(|_| ())
        })
        .unwrap();
    let run = c.call("begin_run", json!({"label":"takes"}));
    let ids = c.call("import_media", json!({"run_id":run["run_id"],"paths":takes}))["asset_ids"].clone();
    let place: Vec<Value> = ids.as_array().unwrap().iter().map(|id| json!({"type":"addClip","assetId":id})).collect();
    c.call("apply_edits", json!({"run_id":run["run_id"],"request_id":"place","edits":place}));
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    assert!(c.error("analyze", json!({"kind":"retakes"})).contains("TRANSCRIPT_MISSING"));

    let job = c.call("transcribe", json!({"language":"cs","model":model}));
    let deadline = std::time::Instant::now() + Duration::from_secs(900);
    loop {
        let status = c.call("job", json!({"job_id":job["job_id"],"action":"get"}));
        if status["status"] == "done" {
            break;
        }
        assert_eq!(status["status"], "running", "{status}");
        assert!(std::time::Instant::now() < deadline, "transcription timed out");
        std::thread::sleep(Duration::from_secs(2));
    }
    let transcript = c.call("get_transcript", json!({}));
    let analysis = c.call("analyze", json!({"kind":"retakes"}));
    eprintln!("{}", serde_json::to_string_pretty(&analysis).unwrap());
    assert_eq!(analysis, c.call("analyze", json!({"kind":"retakes"})), "the same words gave another analysis");
    assert_eq!(analysis["transcript_key"], transcript["transcript_key"]);
    assert_eq!(analysis["time_basis"], "timeline");
    let words: Vec<&str> =
        transcript["words"].as_array().unwrap().iter().map(|w| w["text"].as_str().unwrap().trim()).collect();
    let said =
        |from: &Value, to: &Value| words[from.as_u64().unwrap() as usize..=to.as_u64().unwrap() as usize].join(" ");
    let plain =
        |text: &str| text.to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == ' ').collect::<String>();

    // Exactly two restarted sentences, each keeping its last attempt.
    let groups = analysis["groups"].as_array().unwrap();
    let attempts: Vec<Vec<String>> = groups
        .iter()
        .map(|g| g["sentences"].as_array().unwrap().iter().map(|s| plain(&said(&s["from"], &s["to"]))).collect())
        .collect();
    assert_eq!(attempts.iter().map(Vec::len).collect::<Vec<_>>(), [3, 2], "{attempts:?}");
    // Recognition hears the first "Kamera musí stát" differently from run to run, e.g. "start".
    assert!(attempts[0].iter().all(|a| a.starts_with("dneska vám ukážu")), "{attempts:?}");
    assert!(attempts[1].iter().all(|a| a.starts_with("kamera musí st")), "{attempts:?}");
    assert_eq!(groups.iter().map(|g| g["keep"].as_u64().unwrap()).collect::<Vec<_>>(), [2, 1]);
    assert!(attempts[0][2].ends_with("za 10 minut"), "{attempts:?}");
    assert_eq!(attempts[1][1], "kamera musí stát pevně na stativu");
    // Only the filler words that start a sentence.
    let fillers: Vec<String> =
        analysis["fillers"].as_array().unwrap().iter().map(|f| plain(&said(&f["from"], &f["to"]))).collect();
    assert_eq!(fillers, ["ehm", "jakoby", "prostě"]);
    assert_eq!(analysis["review"], json!([]), "{analysis}");

    // suggested_delete plans as is, leaving each kept sentence once and no filler.
    let plan = c.call(
        "edit_transcript",
        json!({"transcript_key":transcript["transcript_key"],"delete":analysis["suggested_delete"],"dry_run":true}),
    );
    let text = plain(plan["preview_text"].as_str().unwrap());
    assert!(text.starts_with("dneska vám ukážu jak natočit video za 10 minut nejd"), "{text}");
    assert_eq!(text.matches("dneska").count(), 1, "{text}");
    assert_eq!(text.matches("kamera musí stát").count(), 1, "{text}");
    assert!(["ehm", "jakoby", "prostě"].iter().all(|f| !text.contains(f)), "{text}");
    c.finish();
}

#[test]
fn get_waveform_shows_what_a_cut_threw_away_over_stdio() {
    use base64::Engine as _;
    use nuzky_session::transcripts::{Record, TranscriptStore, VERSION};
    let mut c = Client::start(true, None, Some(None));
    let mut samples = vec![0i16; 4 * 48_000];
    let mut words = Vec::new();
    for (word, start) in [1_000_000i64, 1_600_000, 2_100_000, 3_000_000].into_iter().enumerate() {
        let first = (start * 48 / 1000) as usize;
        for (i, sample) in samples[first..first + 19_200].iter_mut().enumerate() {
            *sample = (0.5 * (i as f64 * 440.0 / 48_000.0 * std::f64::consts::TAU).sin() * 32_767.0) as i16;
        }
        words.push(json!({"start_us":start,"end_us":start + 400_000,"text":format!(" w{word}"),"probability":0.9}));
    }
    let wav = c.dir.join("speech.wav");
    std::fs::write(&wav, wav_bytes(&samples)).unwrap();
    let video = c.dir.join("talk.mp4");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=360x640:r=30:d=4", "-i"])
        .arg(&wav)
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&video)
        .status()
        .expect("ffmpeg makes the test video");
    assert!(status.success());
    let run = c.call("begin_run", json!({"label":"place"}))["run_id"].clone();
    let asset_id = c.call("import_media", json!({"run_id":run,"paths":[video]}))["asset_ids"][0].clone();
    c.call("apply_edits", json!({"run_id":run,"request_id":"place","edits":[{"type":"addClip","assetId":asset_id}]}));
    c.call("end_run", json!({"run_id":run,"action":"keep"}));
    let asset: nuzky_engine::model::Asset =
        serde_json::from_value(c.call("get_state", json!({}))["assets"][0].clone()).unwrap();
    let store = TranscriptStore::at(c.dir.join("data/nuzky/transcripts")).unwrap();
    store
        .put(
            &asset,
            &Record {
                version: VERSION,
                fingerprint: store.fingerprint(&asset).unwrap(),
                duration_us: asset.duration_us,
                model: "fixture".into(),
                language: "cs".into(),
                words: serde_json::from_value(json!(words)).unwrap(),
                segments: vec![],
                alignment: None,
            },
        )
        .unwrap();
    let run = c.call("begin_run", json!({"label":"cut and change speed"}))["run_id"].clone();
    c.call(
        "apply_edits",
        json!({"run_id":run,"request_id":"cut","edits":[
        {"type":"rippleDeleteRanges","ranges":[{"startUs":1_200_000,"endUs":1_800_000}]}]}),
    );
    let clip = c.call("get_state", json!({}))["tracks"][0]["clips"][1]["id"].clone();
    c.call(
        "apply_edits",
        json!({"run_id":run,"request_id":"split","edits":[
        {"type":"splitClip","clipId":clip,"atUs":1_950_000}]}),
    );
    let clip = c.call("get_state", json!({}))["tracks"][0]["clips"][2]["id"].clone();
    c.call(
        "apply_edits",
        json!({"run_id":run,"request_id":"speed","edits":[
        {"type":"updateClip","clipId":clip,"speed":2.0}]}),
    );
    c.call("end_run", json!({"run_id":run,"action":"keep"}));

    let args = json!({"range_us":[500_000,2_000_000],"width":800});
    let waiting: Value = serde_json::from_str(&c.error("get_waveform", args.clone())).unwrap();
    let waiting = waiting["error"].as_str().unwrap();
    assert!(waiting.starts_with("AUDIO_NOT_READY") && waiting.contains("talk.mp4"), "{waiting}");
    let job_id = waiting.split("as job ").nth(1).and_then(|rest| rest.split(';').next()).unwrap();
    let done = wait_job(&mut c, &json!({"job_id":job_id}), Duration::from_secs(60), |_| false);
    assert_eq!(done["status"], "done", "{done}");
    let result = c.rpc("tools/call", json!({"name":"get_waveform","arguments":args}));
    assert_ne!(result["result"]["isError"], true, "{result}");
    let content = &result["result"]["content"];
    let info: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(info["cuts"], json!([{"t_us":1_200_000,"split_words":["w0","w1"]}]), "{info}");
    assert_eq!(info["untranscribed"], json!([]));
    assert_eq!(info["range_us"], json!([500_000, 2_000_000]));
    assert_eq!(content[1]["mimeType"], "image/png");
    let bytes = base64::prelude::BASE64_STANDARD.decode(content[1]["data"].as_str().unwrap()).unwrap();
    let (frame, rgba) = decode_png(&bytes);
    assert_eq!((frame.width, frame.height), (800, 216));

    // 1.5 s heard plus the 0.6 s skipped source, shown once between the two cut lines.
    let x = |offset_us: i64| (offset_us as f64 / 2_100_000.0 * f64::from(frame.width)).round() as u32;
    let (left, right) = (x(700_000), x(1_300_000));
    for expected in [left, right] {
        assert!(
            (expected - 2..=expected + 2)
                .any(|x| (16..156).all(|y| pixel(&rgba, frame.width, x, y) == [0xff, 0x9f, 0x0a])),
            "no solid cut line near x={expected}"
        );
    }
    for y in [86 - 40, 86, 86 + 40] {
        assert_eq!(pixel(&rgba, frame.width, left - 10, y), [0x29, 0x97, 0xff], "heard w0 at y={y}");
        assert_eq!(pixel(&rgba, frame.width, left + 10, y), [0x6e, 0x6e, 0x75], "thrown w0 at y={y}");
    }
    assert!(
        [[0x0c, 0x0c, 0x0e], [0x2c, 0x2c, 0x30]].contains(&pixel(&rgba, frame.width, left + 10, 20)),
        "shade above the thrown waveform"
    );
    let silence = x(1_000_000);
    assert_eq!(pixel(&rgba, frame.width, silence, 86), [0x6e, 0x6e, 0x75]);
    assert!(
        (16u32..156)
            .filter(|y| y.abs_diff(86) > 2)
            .all(|y| pixel(&rgba, frame.width, silence, y) != [0x6e, 0x6e, 0x75]),
        "the discarded silence between w0 and w1 has a tall waveform"
    );
    c.finish();
}

/// 14 s of known pictures: three still patterns cut at 4 s and 8 s, moving footage from 12 s, and
/// silence with a loud tone from 6 s to 7 s; music on its own track sounds from 12.5 s to 13.5 s.
#[test]
fn activity_and_changes_find_what_happens_and_page_without_repeats_over_stdio() {
    let mut c = Client::new(true);
    let video = c.dir.join("scenes.mp4");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "smptebars=s=360x640:r=30:d=4"])
        .args(["-f", "lavfi", "-i", "rgbtestsrc=s=360x640:r=30:d=4"])
        .args(["-f", "lavfi", "-i", "colorchart=r=30:d=4"])
        .args(["-f", "lavfi", "-i", "testsrc2=s=360x640:r=30:d=2"])
        .args(["-f", "lavfi", "-i", "aevalsrc=if(between(t\\,6\\,7)\\,0.5*sin(2*PI*1000*t)\\,0):s=48000:d=14"])
        .args(["-filter_complex", "[0:v]setsar=1[a];[1:v]setsar=1[b];[2:v]scale=360:640,setsar=1[c];[3:v]setsar=1[d];[a][b][c][d]concat=n=4,format=yuv420p[v]"])
        .args(["-map", "[v]", "-map", "4:a", "-c:v", "libx264", "-preset", "ultrafast", "-g", "30", "-c:a", "aac"])
        .arg(&video)
        .status()
        .expect("ffmpeg makes the test video");
    assert!(status.success());
    let music = c.dir.join("music.wav");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1"])
        .arg(&music)
        .status()
        .expect("ffmpeg makes the music");
    assert!(status.success());
    let run = c.call("begin_run", json!({"label":"place"}));
    let ids = c.call("import_media", json!({"run_id":run["run_id"],"paths":[video, music]}))["asset_ids"].clone();
    let edits = json!([{"type":"addClip","assetId":ids[0]},{"type":"addClip","assetId":ids[1],"startUs":12_500_000}]);
    c.call("apply_edits", json!({"run_id":run["run_id"],"request_id":"place","edits":edits}));
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));

    // One point per 0.25 s, so every cut lands on a point.
    let activity = c.call("activity", json!({"points": 56}));
    assert_eq!((activity["points"].clone(), activity["step_us"].clone()), (json!(56), json!(250_000)), "{activity}");
    let peaks = |signal: &str| -> Vec<(i64, f64)> {
        activity["peaks"][signal]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["t_us"].as_i64().unwrap(), p["value"].as_f64().unwrap()))
            .collect()
    };
    let mut cuts = peaks("change");
    cuts.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut strongest: Vec<i64> = cuts.iter().take(3).map(|p| p.0).collect();
    strongest.sort();
    assert_eq!(strongest, [4_000_000, 8_000_000, 12_000_000], "{activity}");
    assert!(cuts[2].1 >= 18.0, "a cut scores like a scene cut: {activity}");
    let change = activity["change"].as_array().unwrap();
    assert!(
        change[1..16].iter().chain(&change[17..32]).all(|v| v.as_f64().unwrap() < 1.0),
        "stills do not change: {activity}"
    );
    let moving = peaks("motion");
    assert!(!moving.is_empty() && moving.iter().all(|p| p.0 >= 12_000_000), "{activity}");
    // The tone in the video and the music, whose sound the call prepares itself.
    let loud: Vec<bool> = peaks("loudness_db").iter().map(|p| (6_000_000..7_000_000).contains(&p.0)).collect();
    assert_eq!(loud, [true, false], "{activity}");
    assert!(peaks("loudness_db")[1].0 >= 12_500_000 && peaks("loudness_db")[1].0 < 13_500_000, "{activity}");
    assert!(activity["loudness_db"][4].as_f64().unwrap() < -60.0, "{activity}");

    // A frame per still and one after each cut; the rest looks the same and is skipped.
    let frames = |c: &mut Client, args: Value| {
        let result = c.rpc("tools/call", json!({"name":"inspect_frames","arguments":args}));
        assert_ne!(result["result"]["isError"], true, "{result}");
        let content = result["result"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2, "one sheet with the frames: {result}");
        assert_eq!(content[1]["mimeType"], "image/png");
        serde_json::from_str::<Value>(content[0]["text"].as_str().unwrap()).unwrap()
    };
    let stills = frames(&mut c, json!({"sample":"changes","range_us":[0, 12_000_000],"width":96}));
    assert_eq!(stills["times_us"], json!([0, 4_000_000, 8_000_000]), "{stills}");
    assert_eq!((stills["skipped"].clone(), stills["next"].clone()), (json!(45), Value::Null), "{stills}");
    // One frame a page: the cursor carries the kept frames, so no page repeats the one before.
    let mut pages =
        vec![frames(&mut c, json!({"sample":"changes","range_us":[0, 12_000_000],"max_frames":1,"width":96}))];
    while let Some(next) = pages.last().unwrap()["next"].as_str().map(str::to_owned) {
        assert!(pages.len() < 5, "{pages:?}");
        pages.push(frames(&mut c, json!({"sample":"changes","cursor":next,"max_frames":1,"width":96})));
    }
    let paged: Vec<Value> = pages.iter().flat_map(|p| p["times_us"].as_array().unwrap().clone()).collect();
    assert_eq!(json!(paged), stills["times_us"]);
    assert_eq!(pages.iter().map(|p| p["skipped"].as_u64().unwrap()).sum::<u64>(), 45);
    // Given times still render exactly those frames.
    let given = frames(&mut c, json!({"times_us":[0, 4_500_000],"width":96}));
    assert_eq!(given["times_us"], json!([0, 4_500_000]));
    assert!(given.get("next").is_none() && given.get("skipped").is_none());

    let cursor = pages[0]["next"].as_str().unwrap().to_owned();
    let mixed = c.error("inspect_frames", json!({"sample":"changes","cursor":cursor,"range_us":[0, 1_000_000]}));
    assert!(mixed.contains("INVALID_ARGUMENTS"), "{mixed}");
    assert!(c.error("inspect_frames", json!({"times_us":[0],"range_us":[0, 1_000_000]})).contains("INVALID_ARGUMENTS"));
    assert!(c.error("activity", json!({"range_us":[20_000_000, 30_000_000]})).contains("INVALID_RANGE"));
    let run = c.call("begin_run", json!({"label":"rename"}));
    c.call(
        "apply_edits",
        json!({"run_id":run["run_id"],"request_id":"rename","edits":[{"type":"renameProject","name":"Renamed"}]}),
    );
    let stale = c.error("inspect_frames", json!({"sample":"changes","cursor":cursor}));
    assert!(stale.contains("STALE_REVISION"), "{stale}");
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    c.finish();
}

/// 16-bit mono WAV at 48 kHz: the RIFF header, the format chunk and the samples.
fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + samples.len() as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    for field in [16u32, 1 | (1 << 16), 48_000, 96_000, 2 | (16 << 16)] {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
    bytes.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
    bytes
}

/// Polls a job until `done` says it is finished or `timeout` passes; returns the last state.
fn wait_job(c: &mut Client, job: &Value, timeout: Duration, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let status = c.call("job", json!({"job_id": job["job_id"], "action": "get"}));
        if done(&status) || status["status"] != "running" {
            return status;
        }
        assert!(std::time::Instant::now() < deadline, "job took too long: {status}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A still on the main track, so the timeline has a picture.
fn place_still(c: &mut Client) {
    let image = c.dir.join("still.ppm");
    std::fs::write(&image, b"P6\n2 2\n255\nabcdefghijkl").unwrap();
    let run = c.call("begin_run", json!({"label":"still"}));
    let ids = c.call("import_media", json!({"run_id":run["run_id"],"paths":[image]}))["asset_ids"].clone();
    c.call(
        "apply_edits",
        json!({"run_id":run["run_id"],"request_id":"place","edits":[{"type":"addClip","assetId":ids[0]}]}),
    );
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
}

/// A push-in on one clip and one over a "sentence" across a cut, with no keyframes to work out:
/// clips that already move are named, not changed, and one undo takes the run back.
#[test]
fn apply_motion_moves_clips_on_a_smooth_curve_and_skips_keyed_ones_over_stdio() {
    let mut c = Client::new(true);
    let tools = c.rpc("tools/list", json!({}));
    let tool = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "apply_motion").unwrap();
    assert_eq!(tool["inputSchema"]["required"], json!(["run_id", "kind", "strength"]));
    let image = c.dir.join("still.ppm");
    std::fs::write(&image, b"P6\n2 2\n255\nabcdefghijkl").unwrap();
    let run = c.call("begin_run", json!({"label":"motion"}))["run_id"].clone();
    let asset = c.call("import_media", json!({"run_id":run,"paths":[image]}))["asset_ids"][0].clone();
    let edits = json!([{"type":"addClip","assetId":asset},{"type":"addClip","assetId":asset}]);
    c.call("apply_edits", json!({"run_id":run,"request_id":"place","edits":edits}));
    let clips = |c: &mut Client| c.call("get_state", json!({}))["tracks"][0]["clips"].as_array().unwrap().clone();
    let (first, second) = {
        let placed = clips(&mut c);
        (placed[0]["id"].clone(), placed[1]["id"].clone())
    };

    let pushed = c.call("apply_motion", json!({"run_id":run,"clip_id":first,"kind":"pushIn","strength":0.1}));
    assert_eq!((&pushed["changed"], &pushed["skipped"]), (&json!([first]), &json!([])));
    let keys = clips(&mut c)[0]["keyframes"].clone();
    assert_eq!(
        keys.as_array().unwrap().iter().map(|k| (k["tUs"].clone(), k["ease"].clone())).collect::<Vec<_>>(),
        [(json!(0), json!("smooth")), (json!(3_000_000), json!("smooth"))]
    );
    assert_eq!(keys[0]["transform"]["scale"], 1.0);
    assert!((keys[1]["transform"]["scale"].as_f64().unwrap() - 1.1).abs() < 1e-6, "{keys}");

    let pushed_keys = keys;
    // A "sentence" from 2.5 s to 3.5 s: the first clip already moves; the second gets the move
    // from the start of the range, before the clip, so it starts halfway into it at the cut.
    let sentence = c
        .call("apply_motion", json!({"run_id":run,"range_us":[2_500_000, 3_500_000],"kind":"kenBurns","strength":0.1}));
    assert_eq!((&sentence["changed"], &sentence["skipped"]), (&json!([second]), &json!([first])));
    let keys = clips(&mut c)[1]["keyframes"].clone();
    assert_eq!((&keys[0]["tUs"], &keys[1]["tUs"]), (&json!(-500_000), &json!(500_000)));
    assert!((keys[1]["transform"]["scale"].as_f64().unwrap() - 1.1).abs() < 1e-6, "{keys}");
    assert_eq!(clips(&mut c)[0]["keyframes"], pushed_keys, "the clip that already moved keeps its motion");

    let wrong = c.error("apply_motion", json!({"run_id":run,"clip_id":first,"kind":"pushIn","strength":10}));
    assert!(wrong.contains("strength"), "{wrong}");
    let outside = c.error(
        "apply_motion",
        json!({"run_id":run,"range_us":[9_000_000, 10_000_000],"kind":"pullOut","strength":0.1}),
    );
    assert!(outside.contains("No video or image clip"), "{outside}");
    c.call("end_run", json!({"run_id":run,"action":"keep"}));
    c.call("undo_run", json!({"run_id":run}));
    assert!(clips(&mut c).is_empty());
    c.finish();
}

#[test]
fn reframe_names_missing_models_without_changing_the_project_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    place_still(&mut c);
    let run = c.call("begin_run", json!({"label":"reframe"}))["run_id"].clone();
    let before = std::fs::read(c.dir.join("project.nuzky")).unwrap();
    let revision = c.call("get_state", json!({}))["revision"].clone();
    let error = c.error("reframe", json!({"run_id":run,"format":"9:16"}));
    assert!(error.contains("MODEL_MISSING") && error.contains("yunet-2026may.onnx"), "{error}");
    let invalid = c.error("reframe", json!({"run_id":run,"format":"16:9"}));
    assert!(invalid.contains("INVALID_ARGUMENTS"), "{invalid}");
    assert_eq!(c.call("get_state", json!({}))["revision"], revision);
    assert_eq!(std::fs::read(c.dir.join("project.nuzky")).unwrap(), before);
    assert!(!c.dir.join("data/nuzky/models").exists());
    c.finish();
}

#[test]
#[ignore = "Requires tmp-test/wide-head.mp4 and the face model from scripts/fixtures.sh; run with scripts/isolated.sh"]
fn reframe_follows_a_wide_head_and_undoes_with_the_run_over_stdio() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let media = dunce::canonicalize(root.join("tmp-test/wide-head.mp4")).unwrap();
    let mut c = Client::new(true);
    let import = c.call("begin_run", json!({"label":"import"}))["run_id"].clone();
    let asset = c.call("import_media", json!({"run_id":import,"paths":[media]}))["asset_ids"][0].clone();
    c.call(
        "apply_edits",
        json!({"run_id":import,"request_id":"place","edits":[
            {"type":"setCanvas","width":1920,"height":1080}, {"type":"addClip","assetId":asset}
        ]}),
    );
    c.call("end_run", json!({"run_id":import,"action":"keep"}));
    let before = std::fs::read(c.dir.join("project.nuzky")).unwrap();
    let run = c.call("begin_run", json!({"label":"reframe"}))["run_id"].clone();
    let job = c.call("reframe", json!({"run_id":run,"format":"9:16"}));
    let done = wait_job(&mut c, &job, Duration::from_secs(60), |_| false);
    assert_eq!(done["status"], "done", "{done}");
    let state = c.call("get_state", json!({}));
    assert_eq!((&state["canvas"]["width"], &state["canvas"]["height"]), (&json!(1080), &json!(1920)));
    let clip = &state["tracks"][0]["clips"][0];
    assert!(clip["content"]["transform"]["scale"].as_f64().unwrap() > 3.0, "{clip}");
    assert_eq!(done["result"]["followed"], json!([clip["id"]]));
    assert_eq!(done["result"]["centred"], json!([]));
    assert_eq!(done["result"]["revision"], state["revision"]);
    assert_eq!(
        (&done["result"]["format"], &done["result"]["width"], &done["result"]["height"]),
        (&json!("9:16"), &json!(1080), &json!(1920))
    );
    c.call("end_run", json!({"run_id":run,"action":"keep"}));
    c.call("undo_run", json!({"run_id":run}));
    assert_eq!(std::fs::read(c.dir.join("project.nuzky")).unwrap(), before);

    let run = c.call("begin_run", json!({"label":"concurrent edit"}))["run_id"].clone();
    let job = c.call("reframe", json!({"run_id":run,"format":"4:5","clip_ids":[clip["id"]]}));
    c.call(
        "apply_edits",
        json!({"run_id":run,"request_id":"change","edits":[
            {"type":"setCanvas","width":1440,"height":1080}
        ]}),
    );
    let done = wait_job(&mut c, &job, Duration::from_secs(60), |_| false);
    assert_eq!(done["status"], "failed", "{done}");
    assert!(done["error"].as_str().unwrap().contains("Call reframe again"), "{done}");
    let changed = std::fs::read(c.dir.join("project.nuzky")).unwrap();
    let saved: Value = serde_json::from_slice(&changed).unwrap();
    assert_eq!(saved["canvas"]["width"], 1440);
    let job = c.call("reframe", json!({"run_id":run,"format":"1:1"}));
    c.call("job", json!({"job_id":job["job_id"],"action":"cancel"}));
    let done = wait_job(&mut c, &job, Duration::from_secs(10), |_| false);
    assert_eq!(done["status"], "cancelled", "{done}");
    assert_eq!(std::fs::read(c.dir.join("project.nuzky")).unwrap(), changed);
    c.finish();
}

#[test]
fn cover_tools_name_missing_models_and_never_download_them() {
    // Its own empty data directory: no models installed.
    let mut c = Client::start(true, None, Some(None));
    place_still(&mut c);
    let frames = c.error("analyze", json!({"kind":"thumbnail_frames"}));
    assert!(frames.contains("MODEL_MISSING: face detector") && frames.contains("nuzky vision-models"), "{frames}");
    let mask = c.error("segment_subject", json!({"time_us": 0}));
    assert!(mask.contains("MODEL_MISSING") && mask.contains("birefnet-lite.onnx"), "{mask}");
    assert!(!c.dir.join("data/nuzky/models").exists(), "nothing was downloaded");
    let format = c.error("analyze", json!({"kind":"thumbnail_frames","params":{"format":"4:3"}}));
    assert!(format.contains("INVALID_ARGUMENTS"), "{format}");
    let outside = c.error("segment_subject", json!({"time_us": 60_000_000}));
    assert!(outside.contains("INVALID_RANGE"), "{outside}");
    // An outline needs the person's mask, so the thumbnail cannot be drawn without the model.
    let run = c.call("begin_run", json!({"label":"cover"}))["run_id"].clone();
    let cover = json!({"format":"cover_9x16","timeUs":0,"outline":{"color":"#ffffff","width":8}});
    c.call("apply_edits", json!({"run_id":run,"request_id":"c","edits":[{"type":"setThumbnail","thumbnail":cover}]}));
    for (tool, args) in [
        ("inspect_thumbnail", json!({"format":"cover_9x16"})),
        ("export_thumbnail", json!({"format":"cover_9x16","path":"cover.png"})),
    ] {
        let error = c.error(tool, args);
        assert!(error.contains("MODEL_MISSING") && error.contains("birefnet-lite.onnx"), "{tool}: {error}");
    }
    assert!(!c.dir.join("cover.png").exists() && !c.dir.join("data/nuzky/models").exists());
    c.finish();
}

#[test]
#[ignore = "Requires tmp-test/face-thumb.mp4 and the vision models from scripts/fixtures.sh; run with scripts/isolated.sh"]
fn cover_frames_and_subject_mask_of_a_face_over_stdio() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let media = dunce::canonicalize(root.join("tmp-test/face-thumb.mp4")).unwrap();
    let mut c = Client::new(true);
    let run = c.call("begin_run", json!({"label":"import"}));
    let ids = c.call("import_media", json!({"run_id":run["run_id"],"paths":[media]}))["asset_ids"].clone();
    // The 12 s face five times: long enough that a stop has work left to skip.
    let place: Vec<Value> = (0..5).map(|_| json!({"type":"addClip","assetId":ids[0]})).collect();
    c.call("apply_edits", json!({"run_id":run["run_id"],"request_id":"place","edits":place}));
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));

    // Stopped while it looks at the timeline, the job ends at once.
    let job = c.call("analyze", json!({"kind":"thumbnail_frames"}));
    wait_job(&mut c, &job, Duration::from_secs(30), |s| s["phase"] == "looking" && s["progress"].as_f64() > Some(0.05));
    let asked = std::time::Instant::now();
    c.call("job", json!({"job_id": job["job_id"], "action": "cancel"}));
    let stopped = wait_job(&mut c, &job, Duration::from_secs(10), |_| false);
    assert_eq!(stopped["status"], "cancelled", "{stopped}");
    assert!(asked.elapsed() < Duration::from_secs(1), "cancelling took {:?}", asked.elapsed());

    let job = c.call("analyze", json!({"kind":"thumbnail_frames","params":{"format":"16:9"}}));
    let done = wait_job(&mut c, &job, Duration::from_secs(120), |_| false);
    assert_eq!(done["status"], "done", "{done}");
    let result = &done["result"];
    assert_eq!((result["format"].clone(), result["time_basis"].clone()), (json!("16:9"), json!("timeline")));
    let best = &result["candidates"][0];
    // [6,12) s of every 12 s holds the sharp open eyes; the blur and the blink come before.
    assert!(best["time_us"].as_i64().unwrap() % 12_000_000 >= 6_000_000, "{result}");
    assert!(best["parts"]["eyes_open"].as_f64().unwrap() > 0.9, "{result}");
    assert_eq!(best["faces"].as_array().unwrap().len(), 1, "{result}");
    assert!(best["subject_box"].is_array() && best["crops"]["16:9"].is_array(), "{result}");

    // A second mask waits for the first instead of adding up in memory.
    let job = c.call("segment_subject", json!({"time_us": 1_500_000}));
    wait_job(&mut c, &job, Duration::from_secs(30), |s| s["phase"] == "loading_model");
    let second = c.call("segment_subject", json!({"time_us": 4_500_000}));
    let waiting = wait_job(&mut c, &second, Duration::from_secs(10), |s| s["phase"] == "waiting_for_other_mask");
    assert_eq!(waiting["phase"], "waiting_for_other_mask", "{waiting}");
    // Stopped in the middle of the mask model, the job ends within a few seconds.
    wait_job(&mut c, &job, Duration::from_secs(30), |s| s["phase"] == "segmenting");
    std::thread::sleep(Duration::from_millis(300));
    let asked = std::time::Instant::now();
    c.call("job", json!({"job_id": job["job_id"], "action": "cancel"}));
    c.call("job", json!({"job_id": second["job_id"], "action": "cancel"}));
    for job in [&job, &second] {
        let stopped = wait_job(&mut c, job, Duration::from_secs(10), |_| false);
        assert_eq!(stopped["status"], "cancelled", "{stopped}");
    }
    // The model runs for over 2 s; stopping it mid-run takes a fraction of that.
    assert!(asked.elapsed() < Duration::from_secs(1), "cancelling took {:?}", asked.elapsed());

    let job = c.call("segment_subject", json!({"time_us": 7_500_000}));
    let done = wait_job(&mut c, &job, Duration::from_secs(120), |_| false);
    assert_eq!(done["status"], "done", "{done}");
    let mask = &done["result"];
    assert_eq!(
        (mask["width"].clone(), mask["height"].clone(), mask["person"].clone()),
        (json!(1080), json!(1920), json!(true))
    );
    let share = mask["subject_share"].as_f64().unwrap();
    assert!((0.6..0.8).contains(&share), "{mask}");
    assert!(
        PathBuf::from(mask["mask_path"].as_str().unwrap()).starts_with(&c.dir),
        "the mask is in this client's cache"
    );
    // The same frame of the same edit comes from the cache.
    let again = c.call("segment_subject", json!({"time_us": 7_500_000}));
    let again = wait_job(&mut c, &again, Duration::from_secs(30), |_| false);
    assert_eq!(
        (again["result"]["cached"].clone(), again["result"]["mask_path"].clone()),
        (json!(true), mask["mask_path"].clone())
    );
    c.finish();
}

/// The PNG of an inspect_thumbnail answer, with the answer's text.
fn inspect_thumbnail(c: &mut Client, args: Value) -> (Value, png::OutputInfo, Vec<u8>) {
    use base64::Engine as _;
    let result = c.rpc("tools/call", json!({"name":"inspect_thumbnail","arguments":args}));
    assert_ne!(result["result"]["isError"], true, "{result}");
    let content = &result["result"]["content"];
    let info: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    let bytes = base64::prelude::BASE64_STANDARD.decode(content[1]["data"].as_str().unwrap()).unwrap();
    let (frame, rgba) = decode_png(&bytes);
    (info, frame, rgba)
}

fn decode_png(bytes: &[u8]) -> (png::OutputInfo, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
    let mut rgba = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut rgba).unwrap();
    assert_eq!(frame.color_type, png::ColorType::Rgba);
    (frame, rgba)
}

/// Width and height from a JPEG's start-of-frame segment.
fn jpeg_size(bytes: &[u8]) -> (u16, u16) {
    assert_eq!(&bytes[..2], [0xff, 0xd8], "not a JPEG");
    let mut at = 2;
    loop {
        let (marker, length) = (bytes[at + 1], u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize);
        if (0xc0..=0xc2).contains(&marker) {
            let word = |i: usize| u16::from_be_bytes([bytes[at + i], bytes[at + i + 1]]);
            return (word(7), word(5));
        }
        at += 2 + length;
    }
}

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * width + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2]]
}

/// A cover and a YouTube thumbnail are set in a run, drawn, written to new files and undone with the run.
/// Their background is the frame as it is and no text is behind anyone, so no mask is needed.
#[test]
fn thumbnails_are_set_drawn_exported_and_undone_over_stdio() {
    let mut c = Client::new(true);
    let image = c.dir.join("grey.ppm");
    let mut ppm = b"P6\n64 64\n255\n".to_vec();
    ppm.extend([128u8; 64 * 64 * 3]);
    std::fs::write(&image, ppm).unwrap();
    let run = c.call("begin_run", json!({"label":"covers"}))["run_id"].clone();
    let asset = c.call("import_media", json!({"run_id":run,"paths":[image]}))["asset_ids"][0].clone();
    c.call("apply_edits", json!({"run_id":run,"request_id":"place","edits":[{"type":"addClip","assetId":asset}]}));
    let missing = c.error("inspect_thumbnail", json!({"format":"cover_9x16"}));
    assert!(missing.contains("THUMBNAIL_MISSING"), "{missing}");

    let hook = |y: f64| {
        json!({"text":"HOOK","style":{"fontSize":200,"color":"#00ff00","strokeWidth":0},
            "transform":{"x":0,"y":y,"scale":1,"rotation":0,"opacity":1}})
    };
    let set = |format: &str, texts: Value| json!({"type":"setThumbnail","thumbnail":{"format":format,"timeUs":1_000_000,"texts":texts}});
    let blurry =
        json!([{"type":"setThumbnail","thumbnail":{"format":"cover_9x16","timeUs":0,"background":{"blur":2}}}]);
    let bad = c.error("apply_edits", json!({"run_id":run,"request_id":"bad","edits":blurry}));
    assert!(bad.contains("INVALID_PROJECT") && bad.contains("blur"), "{bad}");
    let edits = json!([set("cover_9x16", json!([hook(-0.25)])), set("youtube_16x9", json!([hook(0.0)]))]);
    c.call("apply_edits", json!({"run_id":run,"request_id":"covers","edits":edits}));
    let state = c.call("get_state", json!({}));
    assert_eq!(state["thumbnails"].as_array().unwrap().len(), 2, "{state}");

    let (info, frame, rgba) = inspect_thumbnail(&mut c, json!({"format":"cover_9x16","width":270}));
    assert_eq!((frame.width, frame.height, &info["width"], &info["height"]), (270, 480, &json!(270), &json!(480)));
    assert_eq!(info["texts"], json!([{"text":"HOOK","behind":false,"hidden":0.0}]));
    // The hook a quarter of the height above the middle, the grey frame elsewhere.
    let green = |rgba: &[u8], width: u32, rows: std::ops::Range<u32>| {
        rows.flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(rgba, width, x, y) == [0, 255, 0])
            .count()
    };
    assert!(green(&rgba, 270, 100..140) > 300, "{}", green(&rgba, 270, 100..140));
    assert_eq!(green(&rgba, 270, 0..90) + green(&rgba, 270, 150..480), 0);
    assert_eq!(pixel(&rgba, 270, 135, 300), [128, 128, 128]);
    // The like and comment rail on the right is tinted red, the middle not.
    let (_, _, zones) = inspect_thumbnail(&mut c, json!({"format":"cover_9x16","width":270,"safe_zones":true}));
    let tinted = pixel(&zones, 270, 260, 240);
    assert!(tinted[0] > 150 && tinted[2] < 140, "{tinted:?}");
    assert_eq!(pixel(&zones, 270, 135, 300), [128, 128, 128]);

    let job = c.call("export_thumbnail", json!({"format":"youtube_16x9","path":"thumb.jpg"}));
    let done = wait_job(&mut c, &job, Duration::from_secs(30), |_| false);
    assert_eq!(done["status"], "done", "{done}");
    let jpeg = std::fs::read(c.dir.join("thumb.jpg")).unwrap();
    assert_eq!(jpeg_size(&jpeg), (1280, 720));
    assert!(jpeg.len() < 2 * 1024 * 1024 && done["result"]["bytes"] == jpeg.len(), "{done}");
    let taken = c.error("export_thumbnail", json!({"format":"youtube_16x9","path":"thumb.jpg"}));
    assert!(taken.contains("OUTPUT_EXISTS"), "{taken}");
    assert_eq!(std::fs::read(c.dir.join("thumb.jpg")).unwrap(), jpeg, "the first file is untouched");
    let wrong = c.error("export_thumbnail", json!({"format":"cover_9x16","path":"cover.gif"}));
    assert!(wrong.contains("INVALID_ARGUMENTS"), "{wrong}");
    let job = c.call("export_thumbnail", json!({"format":"cover_9x16","path":"cover.png"}));
    assert_eq!(wait_job(&mut c, &job, Duration::from_secs(30), |_| false)["status"], "done");
    let (cover, rgba) = decode_png(&std::fs::read(c.dir.join("cover.png")).unwrap());
    assert_eq!((cover.width, cover.height), (1080, 1920));
    assert!(green(&rgba, 1080, 400..560) > 16 * 300, "the cover at full size has the same hook");

    c.call("end_run", json!({"run_id":run,"action":"keep"}));
    let saved: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.nuzky")).unwrap()).unwrap();
    assert_eq!(saved["thumbnails"].as_array().unwrap().len(), 2);
    assert!(saved["thumbnails"][0].get("overlays").is_none(), "a cover without images saves no overlays");
    c.call("undo_run", json!({"run_id":run}));
    let state = c.call("get_state", json!({}));
    assert_eq!((state["thumbnails"].clone(), state["tracks"][0]["clips"].clone()), (json!([]), json!([])));
    c.finish();
}

/// A cover made from an image of the project, such as one an agent generated, shows the image instead of the
/// frame in the look, the export and inspect_image, and goes back to the frame when the image leaves the project.
#[test]
fn a_cover_is_made_from_an_image_over_stdio() {
    let mut c = Client::new(true);
    let ppm = |name: &str, (w, h): (usize, usize), rgb: [u8; 3]| {
        let path = c.dir.join(name);
        let mut bytes = format!("P6\n{w} {h}\n255\n").into_bytes();
        bytes.extend(rgb.repeat(w * h));
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let (grey, red) = (ppm("grey.ppm", (64, 64), [128; 3]), ppm("red.ppm", (30, 60), [255, 0, 0]));
    let run = c.call("begin_run", json!({"label":"generated cover"}))["run_id"].clone();
    let ids = c.call("import_media", json!({"run_id":run,"paths":[grey, red]}))["asset_ids"].clone();
    let (grey, red) = (ids[0].clone(), ids[1].clone());
    c.call("apply_edits", json!({"run_id":run,"request_id":"place","edits":[{"type":"addClip","assetId":grey}]}));
    let cover = |image: &Value| {
        json!([{"type":"setThumbnail","thumbnail":{"format":"cover_9x16","timeUs":0,"image":image,
            "frame":{"x":0,"y":0,"scale":1.125,"rotation":0,"opacity":1}}}])
    };
    let bad = c.error("apply_edits", json!({"run_id":run,"request_id":"bad","edits":cover(&json!("nope"))}));
    assert!(bad.contains("INVALID_PROJECT") && bad.contains("image"), "{bad}");
    c.call("apply_edits", json!({"run_id":run,"request_id":"cover","edits":cover(&red)}));

    let reddish = |p: [u8; 3]| p[0] > 230 && p[1] < 30 && p[2] < 30;
    let (info, frame, rgba) = inspect_thumbnail(&mut c, json!({"format":"cover_9x16","width":270}));
    assert_eq!(info["image"], red, "{info}");
    for (x, y) in [(135, 240), (5, 5), (265, 475)] {
        assert!(reddish(pixel(&rgba, frame.width, x, y)), "the image fills the cover at {x},{y}");
    }
    let job = c.call("export_thumbnail", json!({"format":"cover_9x16","path":"cover.png"}));
    assert_eq!(wait_job(&mut c, &job, Duration::from_secs(30), |_| false)["status"], "done");
    let (exported, rgba) = decode_png(&std::fs::read(c.dir.join("cover.png")).unwrap());
    assert!(reddish(pixel(&rgba, exported.width, 540, 960)), "the export draws the image too");

    use base64::Engine as _;
    let shown = c.rpc("tools/call", json!({"name":"inspect_image","arguments":{"asset_id":red}}));
    let content = &shown["result"]["content"];
    let info: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!((&info["width"], &info["height"]), (&json!(30), &json!(60)), "{info}");
    let (image, rgba) =
        decode_png(&base64::prelude::BASE64_STANDARD.decode(content[1]["data"].as_str().unwrap()).unwrap());
    assert!(reddish(pixel(&rgba, image.width, 15, 30)));
    let not_image = c.error("inspect_image", json!({"asset_id":"nope"}));
    assert!(not_image.contains("UNKNOWN_ASSET"), "{not_image}");

    c.call("apply_edits", json!({"run_id":run,"request_id":"remove","edits":[{"type":"removeAsset","assetId":red}]}));
    let (info, frame, rgba) = inspect_thumbnail(&mut c, json!({"format":"cover_9x16","width":270}));
    assert!(info["image"].is_null(), "{info}");
    assert_eq!(pixel(&rgba, frame.width, 135, 240), [128, 128, 128], "the cover is the frame again");
    c.call("end_run", json!({"run_id":run,"action":"keep"}));
    c.finish();
}

/// A logo with transparency goes on a cover made from a frame: the frame shows around it, its soft edge is never
/// darker than what is behind it, and the look, the export and the CLI draw it in the same place. A missing logo file
/// says so, and the logo leaves the cover with its asset.
#[test]
fn a_logo_goes_on_a_cover_over_stdio_and_the_cli() {
    let mut c = Client::new(true);
    // Grey in the cover's shape, so it fills the cover.
    let grey = c.dir.join("grey.ppm");
    let mut ppm = b"P6\n54 96\n255\n".to_vec();
    ppm.extend([128u8; 54 * 96 * 3]);
    std::fs::write(&grey, ppm).unwrap();
    // A transparent 400 px image with a magenta square in its middle half and, above it, thin magenta lines a pixel
    // wide, whose edges a shrink that ignores transparency would darken.
    let logo = c.dir.join("logo.png");
    let mut rgba = vec![0u8; 400 * 400 * 4];
    for y in 20..300 {
        for x in (100..300).filter(|x| y >= 100 || x % 4 == 0) {
            rgba[(y * 400 + x) * 4..][..4].copy_from_slice(&[255, 0, 255, 255]);
        }
    }
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 400, 400);
    encoder.set_color(png::ColorType::Rgba);
    encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
    std::fs::write(&logo, bytes).unwrap();

    let run = c.call("begin_run", json!({"label":"logo"}))["run_id"].clone();
    let ids = c.call("import_media", json!({"run_id":run,"paths":[grey, logo]}))["asset_ids"].clone();
    c.call("apply_edits", json!({"run_id":run,"request_id":"place","edits":[{"type":"addClip","assetId":ids[0]}]}));
    let cover = |asset: &Value| {
        json!([{"type":"setThumbnail","thumbnail":{"format":"cover_9x16","timeUs":0,
            "overlays":[{"assetId":asset,"transform":{"x":0.21,"y":-0.3,"scale":0.2,"rotation":0,"opacity":1}}]}}])
    };
    let bad = c.error("apply_edits", json!({"run_id":run,"request_id":"bad","edits":cover(&json!("nope"))}));
    assert!(bad.contains("INVALID_PROJECT") && bad.contains("image on a thumbnail"), "{bad}");
    c.call("apply_edits", json!({"run_id":run,"request_id":"cover","edits":cover(&ids[1])}));

    // The logo is 216 px with its square 108 px across, centred at 0.71 and 0.2 of the cover.
    let check = |rgba: &[u8], width: u32, what: &str| {
        let k = width as f64 / 1080.0;
        let (cx, cy) = (0.71 * 1080.0 * k, 0.2 * 1920.0 * k);
        let height = width * 16 / 9;
        let mut magenta = (0usize, 0.0, 0.0);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = pixel(rgba, width, x, y);
                // The square's rows but its edge ones, which the lines above it reach.
                if r > 240 && g < 15 && b > 240 && (y as f64 + 0.5 - cy).abs() < 54.0 * k - 1.0 {
                    magenta = (magenta.0 + 1, magenta.1 + x as f64 + 0.5, magenta.2 + y as f64 + 0.5);
                }
                // Over the grey frame a transparent edge only lightens red and blue.
                assert!(r >= 126 && b >= 126, "{what}: {r},{g},{b} at {x},{y} is darker than the frame");
            }
        }
        let side = 108.0 * k;
        let (n, x, y) = (magenta.0 as f64, magenta.1 / magenta.0 as f64, magenta.2 / magenta.0 as f64);
        assert!((n - side * (side - 2.0)).abs() < side * 4.0, "{what}: {n} magenta pixels, {side} px across");
        assert!((x - cx).abs() < 1.5 && (y - cy).abs() < 1.5, "{what}: centred at {x},{y}, not {cx},{cy}");
        let margin = ((cx - side * 0.75) as u32, cy as u32);
        assert_eq!(
            pixel(rgba, width, margin.0, margin.1),
            [128, 128, 128],
            "{what}: the frame shows around the square"
        );
    };
    let (_, frame, look) = inspect_thumbnail(&mut c, json!({"format":"cover_9x16","width":270}));
    check(&look, frame.width, "the look");
    let job = c.call("export_thumbnail", json!({"format":"cover_9x16","path":"cover.png"}));
    assert_eq!(wait_job(&mut c, &job, Duration::from_secs(30), |_| false)["status"], "done");
    let (_, exported) = decode_png(&std::fs::read(c.dir.join("cover.png")).unwrap());
    check(&exported, 1080, "the export");
    c.call("end_run", json!({"run_id":run,"action":"keep"}));

    let out = c.dir.join("cli.png");
    let cli = Command::new(env!("CARGO_BIN_EXE_nuzky"))
        .args(["thumbnail".as_ref(), c.dir.join("project.nuzky").as_os_str(), "cover_9x16".as_ref(), out.as_os_str()])
        .output()
        .unwrap();
    assert!(cli.status.success(), "{}", String::from_utf8_lossy(&cli.stderr));
    let (_, drawn) = decode_png(&std::fs::read(&out).unwrap());
    let off = drawn.iter().zip(&exported).filter(|(a, b)| a.abs_diff(**b) > 2).count();
    assert!(off < 1080 * 1920 / 1000, "{off} values differ between the CLI and MCP");

    let away = c.dir.join("away.png");
    std::fs::rename(&logo, &away).unwrap();
    let missing = c.error("inspect_thumbnail", json!({"format":"cover_9x16"}));
    assert!(missing.contains("MEDIA_MISSING") && missing.contains("logo.png"), "{missing}");
    let missing = c.error("export_thumbnail", json!({"format":"cover_9x16","path":"again.png"}));
    assert!(missing.contains("MEDIA_MISSING") && missing.contains("logo.png"), "{missing}");
    std::fs::rename(&away, &logo).unwrap();

    let run = c.call("begin_run", json!({"label":"no logo"}))["run_id"].clone();
    c.call(
        "apply_edits",
        json!({"run_id":run,"request_id":"remove","edits":[{"type":"removeAsset","assetId":ids[1]}]}),
    );
    let state = c.call("get_state", json!({}));
    assert!(state["thumbnails"][0].get("overlays").is_none(), "{state}");
    let (_, frame, rgba) = inspect_thumbnail(&mut c, json!({"format":"cover_9x16","width":270}));
    assert_eq!(pixel(&rgba, frame.width, 192, 96), [128, 128, 128], "the logo left the cover with its asset");

    // An image larger than Nuzky decodes it, white lines a pixel wide between transparent ones: shrunk, half of each
    // pixel is white, never a grey that hides them.
    let lines = c.dir.join("lines.png");
    let rgba: Vec<u8> = (0..64 * 4096).flat_map(|i| if i % 2 == 0 { [255; 4] } else { [0; 4] }).collect();
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 4096, 64);
    encoder.set_color(png::ColorType::Rgba);
    encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
    std::fs::write(&lines, bytes).unwrap();
    let id = c.call("import_media", json!({"run_id":run,"paths":[lines]}))["asset_ids"][0].clone();
    let wide = json!([{"type":"setThumbnail","thumbnail":{"format":"cover_9x16","timeUs":0,
        "overlays":[{"assetId":id,"transform":{"x":0,"y":0,"scale":1,"rotation":0,"opacity":1}}]}}]);
    c.call("apply_edits", json!({"run_id":run,"request_id":"lines","edits":wide}));
    let (_, frame, rgba) = inspect_thumbnail(&mut c, json!({"format":"cover_9x16","width":1080}));
    let row: Vec<u32> = (100..980).map(|x| pixel(&rgba, frame.width, x, 960)[1] as u32).collect();
    let mean = row.iter().sum::<u32>() as f64 / row.len() as f64;
    assert!((mean - 191.5).abs() < 8.0, "the lines over the grey frame average {mean}, not about 191");
    c.call("end_run", json!({"run_id":run,"action":"keep"}));
    c.finish();
}

/// inspect_image names a logo's exact colour from the file with its transparency, which the picture shows black,
/// and marks the white it sits on as the backdrop.
#[test]
fn inspect_image_names_a_logos_colours_over_stdio() {
    use base64::Engine as _;
    let mut c = Client::new(true);
    let logo = |name: &str, around: [u8; 4]| {
        let (w, h) = (400, 200);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let shape = (60..240).contains(&x) && (50..150).contains(&y);
                rgba.extend(if shape { [0xe5, 0x09, 0x14, 255] } else { around });
            }
        }
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, w as u32, h as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
        let path = c.dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let paths = [logo("logo.png", [0; 4]), logo("card.png", [255; 4])];
    let run = c.call("begin_run", json!({"label":"brand"}))["run_id"].clone();
    let ids = c.call("import_media", json!({"run_id":run,"paths":paths}))["asset_ids"].clone();
    let mut inspect = |id: &Value| {
        let shown = c.rpc("tools/call", json!({"name":"inspect_image","arguments":{"asset_id":id}}));
        let content = &shown["result"]["content"];
        let info: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        let png = base64::prelude::BASE64_STANDARD.decode(content[1]["data"].as_str().unwrap()).unwrap();
        (info, decode_png(&png))
    };

    let (info, (image, rgba)) = inspect(&ids[0]);
    assert_eq!((&info["width"], &info["height"]), (&json!(400), &json!(200)), "{info}");
    assert_eq!((pixel(&rgba, image.width, 5, 5), pixel(&rgba, image.width, 150, 100)), ([0; 3], [0xe5, 0x09, 0x14]));
    // Only the logo's red: no black from the transparency and no darker shades of its edges.
    assert_eq!(info["colors"], json!([{"hex": "#e50914", "share": 1.0}]), "{info}");
    assert_eq!(info["visible"], 0.23, "the transparency does not show: {info}");

    let (info, _) = inspect(&ids[1]);
    assert_eq!(info["visible"], 1.0, "{info}");
    let (white, red) = (&info["colors"][0], &info["colors"][1]);
    assert_eq!((&white["hex"], &white["background"]), (&json!("#ffffff"), &json!(true)), "{info}");
    assert!((0.72..0.8).contains(&white["share"].as_f64().unwrap()), "{info}");
    assert_eq!(red["hex"], "#e50914", "{info}");
    assert!(red.get("background").is_none(), "{info}");
    c.call("end_run", json!({"run_id":run,"action":"keep"}));
    c.finish();
}

/// A project saved before thumbnails opens, takes an edit and its undo, and is saved exactly as it was.
#[test]
fn a_project_from_before_thumbnails_saves_unchanged_over_stdio() {
    let mut c = Client::new(true);
    let path = c.dir.join("project.nuzky");
    let old = r##"{"version":1,"name":"Old","canvas":{"width":1080,"height":1920,"fps":30,"background":"#000000","backgroundBlur":0.0},"assets":[],"tracks":[{"id":"main","kind":"video","name":"Main","muted":false,"hidden":false,"keepInPlace":false,"clips":[]},{"id":"titles","kind":"text","name":"Text","muted":false,"hidden":false,"keepInPlace":false,"clips":[{"id":"title","startUs":0,"durationUs":3000000,"content":{"type":"text","text":"Ahoj","style":{"fontFamily":null,"fontSize":95.0,"color":"#ffffff","bold":false,"strokeWidth":7.5,"strokeColor":"#000000","background":null},"transform":{"x":0.0,"y":0.15,"scale":1.0,"rotation":0.0,"opacity":1.0}},"animIn":null,"animOut":null,"keyframes":[],"transitionIn":null}]}]}"##;
    c.finish();
    std::fs::write(&path, old).unwrap();
    let mut c = c.restart(true);
    let run = c.call("begin_run", json!({"label":"rename"}))["run_id"].clone();
    c.call("apply_edits", json!({"run_id":run,"request_id":"r","edits":[{"type":"renameProject","name":"New"}]}));
    c.call("end_run", json!({"run_id":run,"action":"keep"}));
    c.call("undo_run", json!({"run_id":run}));
    c.finish();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved, serde_json::from_str::<Value>(old).unwrap());
    assert!(saved.get("thumbnails").is_none());
}

/// Text behind a real face: where the mask has the person the picture shows, beside them the text. The cover
/// is saved with the project and draws the same after reopening, through MCP and the CLI.
#[test]
#[ignore = "Requires tmp-test/face-thumb.mp4 and the vision models from scripts/fixtures.sh; run with scripts/isolated.sh"]
fn text_behind_a_face_over_stdio_and_the_cli() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let media = dunce::canonicalize(root.join("tmp-test/face-thumb.mp4")).unwrap();
    let mut c = Client::new(true);
    let run = c.call("begin_run", json!({"label":"cover"}))["run_id"].clone();
    let ids = c.call("import_media", json!({"run_id":run,"paths":[media]}))["asset_ids"].clone();
    c.call("apply_edits", json!({"run_id":run,"request_id":"place","edits":[{"type":"addClip","assetId":ids[0]}]}));
    // Pure green over the top of the head, wider than the head.
    let cover = |behind: bool| {
        json!([{"type":"setThumbnail","thumbnail":{"format":"cover_9x16","timeUs":7_500_000,"texts":[{"text":"VESMÍR",
            "style":{"fontFamily":"Anton","fontSize":300,"color":"#00ff00","strokeWidth":0},
            "transform":{"x":0,"y":-0.3,"scale":1,"rotation":0,"opacity":1},"behind":behind}]}}])
    };
    c.call("apply_edits", json!({"run_id":run,"request_id":"front","edits":cover(false)}));
    let (info, _, front) = inspect_thumbnail(&mut c, json!({"format":"cover_9x16","width":1080}));
    assert_eq!(info["texts"][0]["hidden"], 0.0, "{info}");
    c.call("apply_edits", json!({"run_id":run,"request_id":"behind","edits":cover(true)}));
    // The mask of this frame is not made yet: inspecting starts it as a job.
    let waiting = c.error("inspect_thumbnail", json!({"format":"cover_9x16","width":1080}));
    assert!(waiting.contains("MASK_NOT_READY"), "{waiting}");
    let job = serde_json::from_str::<Value>(&waiting).unwrap()["error"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .skip_while(|w| *w != "job")
        .nth(1)
        .unwrap()
        .trim_end_matches(';')
        .to_owned();
    let done = wait_job(&mut c, &json!({"job_id": job}), Duration::from_secs(120), |_| false);
    assert_eq!(done["status"], "done", "{done}");
    let (info, _, behind) = inspect_thumbnail(&mut c, json!({"format":"cover_9x16","width":1080}));
    let hidden = info["texts"][0]["hidden"].as_f64().unwrap();
    assert!((0.1..0.9).contains(&hidden), "{info}");

    let mask = decode_gray(&std::fs::read(done["result"]["mask_path"].as_str().unwrap()).unwrap());
    let ink: Vec<usize> = (0..1080 * 1920).filter(|&i| front[i * 4..i * 4 + 3] == [0, 255, 0]).collect();
    let (person, beside): (Vec<usize>, Vec<usize>) =
        ink.iter().filter(|&&i| mask[i] == 255 || mask[i] == 0).partition(|&&i| mask[i] == 255);
    assert!(person.len() > 5_000 && beside.len() > 5_000, "{} {}", person.len(), beside.len());
    let green = |pixels: &[usize]| pixels.iter().filter(|&&i| behind[i * 4..i * 4 + 3] == [0, 255, 0]).count();
    assert_eq!(green(&person), 0, "the person covers the text");
    assert_eq!(green(&beside), beside.len(), "beside the person the text shows");
    let share = person.len() as f64 / ink.len() as f64;
    assert!((hidden - share).abs() < 0.05, "hidden {hidden}, measured {share}");
    c.call("end_run", json!({"run_id":run,"action":"keep"}));

    // Reopened, the saved cover draws the same, and the CLI draws it the same as well.
    let mut c = c.restart(false);
    let (_, _, again) = inspect_thumbnail(&mut c, json!({"format":"cover_9x16","width":1080}));
    assert_eq!(again, behind);
    let out = c.dir.join("cover.png");
    let cli = Command::new(env!("CARGO_BIN_EXE_nuzky"))
        .args(["thumbnail".as_ref(), c.dir.join("project.nuzky").as_os_str(), "cover_9x16".as_ref(), out.as_os_str()])
        .output()
        .unwrap();
    assert!(cli.status.success(), "{}", String::from_utf8_lossy(&cli.stderr));
    let (frame, drawn) = decode_png(&std::fs::read(&out).unwrap());
    assert_eq!((frame.width, frame.height), (1080, 1920));
    let off = drawn.iter().zip(&behind).filter(|(a, b)| a.abs_diff(**b) > 2).count();
    assert!(off < 1080 * 1920 / 1000, "{off} values differ between the CLI and MCP");
    c.finish();
}

fn decode_gray(bytes: &[u8]) -> Vec<u8> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
    let mut gray = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut gray).unwrap();
    assert_eq!(frame.color_type, png::ColorType::Grayscale);
    gray
}

fn long_talk(c: &mut Client) -> Vec<Value> {
    use nuzky_session::transcripts::{Record, TranscriptStore, VERSION};
    let video = c.dir.join("long-talk.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=s=192x108:r=2",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=8000:cl=stereo",
            "-t",
            "1060",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ])
        .arg(&video)
        .status()
        .expect("ffmpeg makes the long test video");
    assert!(status.success());
    let run = c.call("begin_run", json!({"label": "Place long talk"}))["run_id"].clone();
    let ids = c.call("import_media", json!({"run_id": run, "paths": [video]}))["asset_ids"].clone();
    c.call(
        "apply_edits",
        json!({"run_id": run, "request_id": "place", "edits": [{"type": "addClip", "assetId": ids[0]}]}),
    );
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    let mut words = Vec::new();
    let mut start = 0i64;
    for i in 0..2200 {
        let end = if i % 10 == 9 { "." } else { "" };
        let text = ["a", "story", "continues", "here"][i % 4];
        words.push(json!({"start_us": start, "end_us": start + 300_000,
            "text": format!(" {text}{end}"), "probability": 0.9}));
        start += 300_000 + if i % 10 == 9 { 700_000 } else { 100_000 };
    }
    let asset: nuzky_engine::model::Asset =
        serde_json::from_value(c.call("get_state", json!({}))["assets"][0].clone()).unwrap();
    let store = TranscriptStore::at(c.dir.join("data/nuzky/transcripts")).unwrap();
    store
        .put(
            &asset,
            &Record {
                version: VERSION,
                fingerprint: store.fingerprint(&asset).unwrap(),
                duration_us: asset.duration_us,
                model: "fixture".into(),
                language: "en".into(),
                words: serde_json::from_value(json!(words)).unwrap(),
                segments: vec![],
                alignment: None,
            },
        )
        .unwrap();
    words
}

#[test]
fn transcript_sentences_and_pages_preserve_long_talk_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    let stored = long_talk(&mut c);
    let full = c.call("get_transcript", json!({}));
    assert_eq!(
        full.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(),
        ["pauses", "revision", "sentences", "session_epoch", "transcript_key", "untranscribed", "words"]
    );
    assert_eq!(full["words"].as_array().unwrap().len(), 2200);
    for (i, word) in full["words"].as_array().unwrap().iter().enumerate() {
        assert_eq!(word["i"], i);
        assert_eq!(word["text"], stored[i]["text"]);
        assert_eq!(word["start_us"], stored[i]["start_us"]);
        assert_eq!(word["end_us"], stored[i]["end_us"]);
        assert_eq!(word["p"], json!(stored[i]["probability"].as_f64().unwrap() as f32));
    }
    assert_eq!(full["sentences"].as_array().unwrap().len(), 220);
    assert_eq!(c.call("get_transcript", json!({"detail": "full"})), full);
    let sentences = c.call("get_transcript", json!({"detail": "sentences"}));
    assert_eq!(sentences["sentences"], full["sentences"]);
    assert!(sentences.get("words").is_none() && sentences.get("pauses").is_none());
    assert!(sentences.get("next_cursor").is_none());
    let full_bytes = serde_json::to_vec(&full).unwrap().len();
    let sentence_bytes = serde_json::to_vec(&sentences).unwrap().len();
    eprintln!("Transcript JSON bytes: full={full_bytes}, sentences={sentence_bytes}");
    assert!(sentence_bytes * 3 <= full_bytes);

    let mut collected = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let page = c.call("get_transcript", json!({"detail": "sentences", "limit": 37, "cursor": cursor}));
        let batch = page["sentences"].as_array().unwrap();
        assert!(!batch.is_empty() && batch.len() <= 37);
        assert!(page.get("words").is_none() && page.get("pauses").is_none());
        collected.extend(batch.iter().cloned());
        cursor = page["next_cursor"].clone();
        assert!(cursor.is_string() || cursor.is_null());
        if cursor.is_null() {
            break;
        }
        assert!(collected.len() <= 220);
    }
    assert_eq!(collected.len(), 220);
    assert_eq!(json!(collected), full["sentences"]);
    collected.clear();
    cursor = Value::Null;
    loop {
        let page = c.call("get_transcript", json!({"limit": 50, "cursor": cursor}));
        let batch = page["sentences"].as_array().unwrap();
        assert!(!batch.is_empty() && batch.len() <= 50);
        let from = batch.first().unwrap()["from"].as_u64().unwrap();
        let to = batch.last().unwrap()["to"].as_u64().unwrap();
        assert!(
            page["pauses"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| { (from..=to).contains(&p["after_word"].as_u64().unwrap()) })
        );
        collected.extend(page["words"].as_array().unwrap().iter().cloned());
        cursor = page["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
        assert!(collected.len() <= 2200);
    }
    assert_eq!(collected.len(), 2200);
    assert_eq!(json!(collected), full["words"]);
    let first = c.call("get_transcript", json!({"detail": "sentences", "limit": 37}));
    let next = c.call("get_transcript", json!({"detail": "sentences", "cursor": first["next_cursor"]}));
    assert_eq!(next["sentences"], json!(full["sentences"].as_array().unwrap()[37..137]));
    let range = [150_000, 25_150_000];
    let ranged = c.call("get_transcript", json!({"range_us": range}));
    let page = c.call("get_transcript", json!({"range_us": range, "limit": 500}));
    for field in ["words", "sentences", "pauses"] {
        assert_eq!(page[field], ranged[field]);
    }
    assert!(page["next_cursor"].is_null());
    assert!(c.error("get_transcript", json!({"cursor": "garbage"})).contains("INVALID_ARGUMENTS"));
    assert!(c.error("get_transcript", json!({"limit": 0})).contains("INVALID_ARGUMENTS"));
    assert!(c.error("get_transcript", json!({"limit": 501})).contains("INVALID_ARGUMENTS"));
    assert!(c.error("get_transcript", json!({"cursor": "370.stale"})).contains("SPEECH_CHANGED"));
}

/// A long video with its own hard cuts: a proposed reel whose first word is said in the last moment of a shot starts
/// after the cut, also when the next shot differs as little as another framing of one camera, a made reel drops the
/// silent frames of the next shot at its end and of the video's first shot at its start, and a shot that changes
/// while a word is said stays, so no word is cut.
#[test]
fn reels_start_and_end_on_the_videos_own_cuts_over_stdio() {
    use nuzky_engine::{edit::MAIN_TRACK, model::ClipContent};
    use nuzky_session::transcripts::{Record, TranscriptStore, VERSION};
    let mut c = Client::start(true, None, Some(None));
    let video = c.dir.join("shots.mp4");
    let mut filter = "color=c=red:s=192x108:r=30:d=40".to_owned();
    for (frame, color) in
        [(3, "0x0055ff"), (309, "blue"), (598, "lime"), (807, "yellow"), (834, "magenta"), (900, "white")]
    {
        filter.push_str(&format!(",drawbox=color={color}:t=fill:enable='gte(n,{frame})'"));
    }
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", &filter])
        .args(["-f", "lavfi", "-i", "anullsrc=r=8000:cl=stereo", "-t", "40"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"])
        .args(["-g", "250", "-sc_threshold", "0", "-c:a", "aac"])
        .arg(&video)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let run = c.call("begin_run", json!({"label": "Place talk"}))["run_id"].clone();
    let ids = c.call("import_media", json!({"run_id": run, "paths": [video]}))["asset_ids"].clone();
    c.call(
        "apply_edits",
        json!({"run_id": run, "request_id": "place",
        "edits": [{"type": "addClip", "assetId": ids[0]}]}),
    );
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    let words: Vec<_> = [
        (200_000, 500_000, " Hello"),
        (600_000, 1_000_000, " there."),
        (10_000_000, 10_250_000, " Money"),
        (10_400_000, 10_600_000, " is"),
        (10_700_000, 10_900_000, " not"),
        (11_000_000, 11_500_000, " everything."),
        (12_000_000, 12_200_000, " It"),
        (12_300_000, 12_600_000, " buys"),
        (12_700_000, 13_000_000, " time."),
        (15_000_000, 15_400_000, " Later"),
        (15_500_000, 16_000_000, " on."),
        (18_000_000, 18_300_000, " Fear"),
        (18_400_000, 18_600_000, " is"),
        (18_700_000, 18_800_000, " a"),
        (19_500_000, 19_920_000, " liar."),
        (22_000_000, 22_500_000, " Anyway."),
        (25_000_000, 25_300_000, " Habits"),
        (25_400_000, 26_000_000, " stick."),
        (27_000_000, 27_300_000, " Small"),
        (27_400_000, 28_000_000, " wins."),
        (31_000_000, 31_500_000, " Bye."),
    ]
    .into_iter()
    .map(|(start, end, text)| json!({"start_us": start, "end_us": end, "text": text, "probability": 0.9}))
    .collect();
    let asset = serde_json::from_value(c.call("get_state", json!({}))["assets"][0].clone()).unwrap();
    let store = TranscriptStore::at(c.dir.join("data/nuzky/transcripts")).unwrap();
    store
        .put(
            &asset,
            &Record {
                version: VERSION,
                fingerprint: store.fingerprint(&asset).unwrap(),
                duration_us: asset.duration_us,
                model: "fixture".into(),
                language: "en".into(),
                words: serde_json::from_value(json!(words)).unwrap(),
                segments: vec![],
                alignment: None,
            },
        )
        .unwrap();
    let key = c.call("get_transcript", json!({}))["transcript_key"].clone();
    let run = c.call("begin_run", json!({"label": "Reels at picture cuts"}))["run_id"].clone();
    let candidates: Vec<_> = [(2, 8, "A"), (11, 14, "B"), (16, 19, "C"), (0, 1, "D")]
        .into_iter()
        .map(|(from, to, title)| json!({"from": from, "to": to, "title": title, "why": "One idea.", "score": 0.9}))
        .collect();
    let proposed = c.call("propose_reels", json!({"run_id": run, "transcript_key": key, "candidates": candidates}));
    let proposed = proposed["candidates"].as_array().unwrap();
    let ids: Vec<_> = proposed.iter().map(|p| p["id"].clone()).collect();
    let made = c.call("make_reels", json!({"run_id": run, "ids": ids}));
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    let state = c.call("get_state", json!({}));
    let made = made["reels"].as_array().unwrap();
    assert_eq!((proposed.len(), made.len()), (4, 4));
    let projects: Vec<Project> = made
        .iter()
        .map(|r| serde_json::from_slice(&std::fs::read(r["path"].as_str().unwrap()).unwrap()).unwrap())
        .collect();
    let clips: Vec<Vec<_>> = projects
        .iter()
        .map(|p| {
            p.tracks
                .iter()
                .find(|t| t.id == MAIN_TRACK)
                .unwrap()
                .clips
                .iter()
                .map(|clip| {
                    let ClipContent::Media { source_in_us, speed, .. } = clip.content else { panic!("expected video") };
                    (clip.start_us, source_in_us, clip.duration_us, speed)
                })
                .collect()
        })
        .collect();
    let source_end = |clip: &(i64, i64, i64, f32)| clip.1 + (clip.2 as f64 * f64::from(clip.3)).round() as i64;
    assert_eq!(proposed[0]["from"], 3);
    assert_eq!(proposed[0]["startUs"], 10_400_000);
    assert_eq!(proposed[0]["hook"], "is not everything.");
    let duration = |i: usize| projects[i].duration_us();
    for (i, reel) in made.iter().enumerate() {
        assert_eq!(reel["durationUs"], duration(i));
    }
    let shorter = proposed[1]["durationUs"].as_i64().unwrap() - duration(1);
    assert!((shorter - 106_667).abs() <= 2_000, "B shortened by {shorter} us");
    let saved = state["reel_candidates"].as_array().unwrap().iter().find(|r| r["id"] == ids[1]).unwrap();
    assert_eq!(saved["durationUs"], duration(1));
    assert!((source_end(clips[1].last().unwrap()) - 19_933_333).abs() <= 1_000, "{:?}", clips[1]);
    assert_eq!(duration(2), proposed[2]["durationUs"].as_i64().unwrap());
    assert_eq!(source_end(clips[2].last().unwrap()), 28_120_000);
    assert_eq!((proposed[3]["from"].as_i64(), clips[3][0].1), (Some(0), 100_000));
    assert_eq!(duration(3), proposed[3]["durationUs"].as_i64().unwrap() - 100_000);
    let frame = |i: usize, time_us: i64, expected: [u8; 3]| {
        let out = c.dir.join(format!("reel-{i}-{time_us}.png"));
        let output = Command::new(env!("CARGO_BIN_EXE_nuzky"))
            .arg("frame")
            .arg(made[i]["path"].as_str().unwrap())
            .arg((time_us as f64 / 1_000_000.0).to_string())
            .arg(&out)
            .arg("270")
            .env("NUZKY_DATA_DIR", c.dir.join("data/nuzky"))
            .env("HOME", c.dir.join("home"))
            .env("XDG_DATA_HOME", c.dir.join("data"))
            .env("XDG_CACHE_HOME", c.dir.join("cache"))
            .env("XDG_RUNTIME_DIR", c.dir.join("runtime"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let (info, rgba) = decode_png(&std::fs::read(out).unwrap());
        assert_eq!((info.width, info.height), (270, 480));
        let at = ((info.height / 2 * info.width + info.width / 2) * 4) as usize;
        let actual = &rgba[at..at + 3];
        assert!(
            actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 30),
            "reel {i} at {time_us} us: {actual:?} != {expected:?}"
        );
    };
    frame(0, 0, [0, 0, 255]);
    frame(1, duration(1) - 10_000, [0, 0, 255]);
    frame(2, duration(2) - 10_000, [255, 0, 255]);
    frame(3, 0, [0, 85, 255]);
    // Speech on a track kept in place does not move with the picture, so no cut of the picture moves the reel.
    let tracks = |c: &mut Client| c.call("get_state", json!({}))["tracks"].as_array().unwrap().clone();
    let clip = tracks(&mut c).iter().find(|t| t["id"] == MAIN_TRACK).unwrap()["clips"][0]["id"].clone();
    let run = c.call("begin_run", json!({"label": "Speech in place"}))["run_id"].clone();
    let edit = |c: &mut Client, edit: Value| {
        c.call("apply_edits", json!({"run_id": run, "request_id": edit["type"], "edits": [edit]}));
    };
    edit(&mut c, json!({"type": "detachAudio", "clipId": clip}));
    let sound = tracks(&mut c).iter().find(|t| t["kind"] == "audio").unwrap()["id"].clone();
    edit(&mut c, json!({"type": "updateTrack", "trackId": sound, "keepInPlace": true}));
    let key = c.call("get_transcript", json!({}))["transcript_key"].clone();
    let whole = json!([{"from": 0, "to": 20, "title": "E", "why": "All of it.", "score": 0.9}]);
    let proposed = c.call("propose_reels", json!({"run_id": run, "transcript_key": key, "candidates": whole}));
    let made = c.call("make_reels", json!({"run_id": run, "ids": [proposed["candidates"][0]["id"]]}));
    c.call("end_run", json!({"run_id": run, "action": "keep"}));
    let reel: Project =
        serde_json::from_slice(&std::fs::read(made["reels"][0]["path"].as_str().unwrap()).unwrap()).unwrap();
    let main = reel.tracks.iter().find(|t| t.id == MAIN_TRACK).unwrap();
    let ClipContent::Media { source_in_us, .. } = main.clips[0].content else { panic!("expected video") };
    assert_eq!(source_in_us, 0);
    c.finish();
}

/// A long talk becomes reels: proposals out of bounds are refused, three picked ones become projects beside it
/// with only their words on a 9:16 canvas, the talk and its video stay as they were, and undo takes it all back.
#[test]
fn reels_are_proposed_and_made_beside_a_long_talk_over_stdio() {
    let mut c = Client::start(true, None, Some(None));
    let words = long_talk(&mut c);
    let at = |i: u64, field: &str| words[i as usize][field].as_i64().unwrap();
    let transcript = c.call("get_transcript", json!({"detail": "sentences"}));
    let key = transcript["transcript_key"].clone();
    let sentences = transcript["sentences"].as_array().unwrap().clone();
    let first = |k: usize| sentences[k]["from"].as_u64().unwrap();
    let last = |k: usize| sentences[k]["to"].as_u64().unwrap();
    let reel = |a: usize, b: usize, title: &str| json!({"from": first(a), "to": last(b), "title": title, "why": "One idea and its payoff.", "score": 0.7});

    let run = c.call("begin_run", json!({"label": "Reels"}))["run_id"].clone();
    let propose = |candidates: Value| json!({"run_id": run, "transcript_key": key, "candidates": candidates});
    for (candidates, code) in [
        (json!([reel(0, 19, "Twenty sentences")]), "REEL_TOO_LONG"),
        (json!([reel(0, 4, "One"), reel(4, 8, "Two")]), "REEL_OVERLAP"),
        (
            json!([{"from": 2190, "to": 2209, "title": "Past the end", "why": "", "score": 0.5}]),
            "REEL_OUTSIDE_TRANSCRIPT",
        ),
        (
            json!([{"from": first(2), "to": last(6) - 1, "title": "Mid sentence", "why": "", "score": 0.5}]),
            "REEL_BOUNDARY",
        ),
    ] {
        let error = c.error("propose_reels", propose(candidates));
        assert!(error.contains(code), "{code}: {error}");
    }
    assert_eq!(c.call("get_state", json!({}))["reel_candidates"], json!([]));
    let picks = [(2, 6, "Sleep"), (40, 45, "Money"), (100, 113, "Fear")];
    let mut candidates: Vec<Value> = picks.iter().map(|&(a, b, t)| reel(a, b, t)).collect();
    candidates.push(reel(150, 152, "Habits"));
    let proposed = c.call("propose_reels", propose(json!(candidates)))["candidates"].as_array().unwrap().clone();
    assert_eq!(proposed.len(), 4);
    for (candidate, &(a, b, title)) in proposed.iter().zip(&picks) {
        let (from, to) = (first(a), last(b));
        assert_eq!((candidate["title"].as_str(), candidate["status"].as_str()), (Some(title), Some("proposed")));
        assert_eq!(
            (candidate["startUs"].as_i64(), candidate["endUs"].as_i64()),
            (Some(at(from, "start_us")), Some(at(to, "end_us")))
        );
        assert_eq!(candidate["hook"], sentences[a]["text"]);
        // 80 ms before the first word, 120 ms after the last, and the 700 ms pauses between sentences down to 300 ms.
        let expected = at(to, "end_us") - at(from, "start_us") + 200_000 - (b - a) as i64 * 400_000;
        assert!((candidate["durationUs"].as_i64().unwrap() - expected).abs() <= 1_000, "{candidate} {expected}");
    }
    assert!(proposed[2]["durationUs"].as_i64().unwrap() <= 60_000_000);
    c.call("end_run", json!({"run_id": run, "action": "keep"}));

    let source_path = c.dir.join("project.nuzky");
    let video = c.dir.join("long-talk.mp4");
    let source: Project = serde_json::from_slice(&std::fs::read(&source_path).unwrap()).unwrap();
    let video_bytes = std::fs::read(&video).unwrap();
    let run2 = c.call("begin_run", json!({"label": "Make reels"}))["run_id"].clone();
    let ids: Vec<Value> = proposed[..3].iter().map(|c| c["id"].clone()).collect();
    let args = json!({"run_id": run2, "request_id": "make", "ids": ids});
    let made = c.call("make_reels", args.clone());
    // A retry answers the same and writes nothing new.
    assert_eq!(c.call("make_reels", args), made);
    let reels = made["reels"].as_array().unwrap();
    assert_eq!(reels.len(), 3);
    let blurred = c.call("make_reels", json!({"run_id": run2, "ids": [proposed[3]["id"]], "canvas": "blur"}));
    assert!(c.error("make_reels", json!({"run_id": run2, "ids": [ids[0]]})).contains("REEL_MADE"));
    c.call("end_run", json!({"run_id": run2, "action": "keep"}));
    let names: Vec<_> = std::fs::read_dir(&c.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".nuzky"))
        .collect();
    assert_eq!(names.len(), 5, "{names:?}");

    for (i, made) in reels.iter().chain(blurred["reels"].as_array().unwrap()).enumerate() {
        let path = PathBuf::from(made["path"].as_str().unwrap());
        assert_eq!(path, dunce::canonicalize(&c.dir).unwrap().join(format!("project-reel-{}.nuzky", i + 1)));
        let reel: Project = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        nuzky_session::validate(&reel).unwrap();
        assert_eq!((reel.canvas.width, reel.canvas.height), (1080, 1920));
        assert_eq!(reel.duration_us(), proposed[i]["durationUs"].as_i64().unwrap());
        assert_eq!(made["durationUs"], proposed[i]["durationUs"]);
        assert_eq!(reel.name, proposed[i]["title"].as_str().unwrap());
        assert_eq!(reel.assets, source.assets);
        assert!(reel.reel_candidates.is_empty());
        let nuzky_engine::model::ClipContent::Media { transform, .. } = &reel.tracks[0].clips[0].content else {
            panic!()
        };
        // Crop fills the frame with the middle of the 16:9 picture; blur fits the whole of it.
        let (scale, blur) = if i < 3 { ((1920.0 / 108.0) / (1080.0 / 192.0), 0.0) } else { (1.0, 0.5) };
        assert!((transform.scale - scale).abs() < 1e-4 && reel.canvas.background_blur == blur, "{transform:?}");
    }
    // The reel holds exactly its sentences, for an agent working in it.
    let mut first_reel =
        Client::spawn_at(c.dir.clone(), &PathBuf::from(reels[0]["path"].as_str().unwrap()), false, Some(None));
    let inside = first_reel.call("get_transcript", json!({"detail": "sentences"}));
    assert_eq!(
        inside["sentences"].as_array().unwrap().iter().map(|s| &s["text"]).collect::<Vec<_>>(),
        sentences[2..=6].iter().map(|s| &s["text"]).collect::<Vec<_>>()
    );
    first_reel.finish();
    first_reel.dir = PathBuf::new();

    // The talk changed only in which candidates are made and where.
    let after: Project = serde_json::from_slice(&std::fs::read(&source_path).unwrap()).unwrap();
    let mut expected = source.clone();
    for (candidate, made) in
        expected.reel_candidates.iter_mut().zip(reels.iter().chain(blurred["reels"].as_array().unwrap()))
    {
        assert_eq!(candidate.id, made["id"].as_str().unwrap());
        candidate.status = nuzky_engine::model::ReelStatus::Made;
        candidate.project_path = Some(made["path"].as_str().unwrap().to_owned());
    }
    assert_eq!(after, expected);
    assert!(std::fs::read(&video).unwrap() == video_bytes, "the talk's video changed");

    // Undo takes the made marks back, then the proposals; the reels' projects stay on disk.
    c.call("undo_run", json!({"run_id": run2}));
    assert_eq!(
        serde_json::to_value(&c.call("get_state", json!({}))["reel_candidates"]).unwrap(),
        json!(source.reel_candidates)
    );
    c.call("undo_run", json!({"run_id": run}));
    assert_eq!(c.call("get_state", json!({}))["reel_candidates"], json!([]));
    assert!(c.dir.join("project-reel-3.nuzky").is_file());
    c.finish();
}
