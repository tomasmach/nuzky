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
        let binary = env!("CARGO_BIN_EXE_nuzky");
        let mut command = Command::new(binary);
        command.arg("mcp").arg("--project").arg(project).arg("--cache").arg(dir.join("cache"));
        if write {
            command.arg("--allow-write");
        }
        if let Some(style) = style {
            let data = dir.join("data");
            std::fs::create_dir_all(data.join("nuzky")).unwrap();
            if let Some(text) = style {
                std::fs::write(data.join("nuzky/EDIT.md"), text).unwrap();
            }
            // macOS ignores XDG_DATA_HOME and reads ~/Library/Application Support, so there it is the same folder.
            #[cfg(target_os = "macos")]
            {
                std::fs::create_dir_all(dir.join("home/Library")).unwrap();
                std::os::unix::fs::symlink(&data, dir.join("home/Library/Application Support")).unwrap();
                command.env("HOME", dir.join("home"));
            }
            command.env("XDG_DATA_HOME", data);
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

/// The built-in sound effects need no network: listing finds them, and add_sound places one at the time
/// asked as an edit of the run, with its credit saved in the project and its file in Nuzky's library.
#[test]
fn built_in_sounds_are_listed_and_added_with_their_credit() {
    let mut c = Client::start(true, None, Some(None));
    let listed = c.call("search_sounds", json!({"query": "", "kind": "effect"}));
    let sounds = listed["sounds"].as_array().unwrap();
    assert!(sounds.len() >= 40, "{listed}");
    assert!(sounds.iter().all(|s| s["license"] == "cc0" && s["provider"] == "Built in"));
    assert!(sounds.iter().any(|s| s["id"] == "nuzky:whoosh" && s["category"] == "Whoosh"));
    // Music has nothing built in, and an empty query sends nothing anywhere.
    assert_eq!(c.call("search_sounds", json!({"query": " ", "kind": "music"}))["sounds"], json!([]));

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
    c.call("end_run", json!({"run_id": run, "action": "keep"}));

    let disk: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.nuzky")).unwrap()).unwrap();
    let assets = disk["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0]["credit"]["title"], "Whoosh");
    assert_eq!(assets[0]["credit"]["license"], "cc0");
    let file = PathBuf::from(assets[0]["path"].as_str().unwrap());
    assert!(file.starts_with(c.dir.join("data/nuzky/sounds")) && file.is_file(), "{file:?}");
    let clips: Vec<_> =
        disk["tracks"].as_array().unwrap().iter().flat_map(|t| t["clips"].as_array().unwrap()).collect();
    assert_eq!(clips.iter().map(|c| c["startUs"].as_i64().unwrap()).collect::<Vec<_>>(), [1_500_000, 4_000_000]);
}

#[test]
fn initialize_list_state_edit_end_undo_over_stdio() {
    let mut c = Client::new(true);
    let list = c.rpc("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 31);
    let search = tools.iter().find(|t| t["name"] == "search_sounds").unwrap();
    assert_eq!(search["annotations"]["openWorldHint"], true);
    assert_eq!(search["annotations"]["readOnlyHint"], true);
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
    // Ducking and Keep pitch are fields an agent finds in the schema and sets with updateClip.
    let schema = c.rpc("resources/read", json!({"uri":"nuzky://schema"}));
    let schema = schema["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(schema.contains("\"duckDb\"") && schema.contains("\"keepPitch\""));
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
        [
            "thumbnail_frames",
            "segment_subject",
            "setThumbnail",
            "inspect_thumbnail",
            "export_thumbnail",
            "\"Behind head\""
        ]
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

// Only on Linux does XDG_DATA_HOME choose where Nuzky looks for EDIT.md.
#[cfg(target_os = "linux")]
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
    let instructions = styled.init["result"]["instructions"].as_str().unwrap().to_owned();
    assert!(instructions.starts_with("This creator has their own editing style, nuzky://style"), "{instructions}");
    assert!(instructions.contains(guide) && instructions.ends_with(style));
    let resources = styled.rpc("resources/list", json!({}));
    let uris: Vec<&str> =
        resources["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
    assert_eq!(uris, ["nuzky://guide", "nuzky://schema", "nuzky://style"]);
    assert_eq!(styled.rpc("resources/read", json!({"uri":"nuzky://style"}))["result"]["contents"][0]["text"], style);
    assert!(
        styled.rpc("resources/read", json!({"uri":"nuzky://guide"}))["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .ends_with(style)
    );
    let prompt = styled.rpc("prompts/get", json!({"name":"edit_selected","arguments":{"goal":"Make a reel"}}));
    assert!(prompt["result"]["messages"][0]["content"]["text"].as_str().unwrap().ends_with(style));
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
    // The other kinds still need an asset and start a job, which a read-only client cannot.
    assert!(c.error("analyze", json!({"kind":"retakes","asset_id":"a"})).contains("omit asset_id"));
    assert!(c.error("analyze", json!({"kind":"silences","asset_id":"a"})).contains("READ_ONLY"));
    c.finish();
    let mut writer = Client::start(true, None, Some(None));
    assert!(writer.error("analyze", json!({"kind":"fillers"})).contains("asset_id is required"));
    writer.finish();
}

// Only on Linux does XDG_DATA_HOME choose where Nuzky keeps transcripts.
#[cfg(target_os = "linux")]
#[test]
fn correct_words_fixes_the_transcript_and_captions_over_stdio() {
    use nuzky_engine::{model::Asset, model::AssetKind, speech::Word};
    use nuzky_session::transcripts::{Record, TranscriptStore, VERSION};
    let mut c = Client::start(true, None, Some(None));
    // Words come from the store, so the file is never decoded and any bytes do.
    let path = c.dir.join("take.mov");
    std::fs::write(&path, b"take").unwrap();
    let asset = Asset {
        id: "take".into(),
        name: "take.mov".into(),
        path: path.to_string_lossy().into(),
        kind: AssetKind::Video,
        duration_us: 4_000_000,
        width: 1080,
        height: 1920,
        fps: 30.0,
        has_audio: true,
        rotation: 0,
        mirror: false,
        credit: None,
    };
    let said = ["Mikrofon", "vejte", "co", "nejblíž."];
    let words = said
        .iter()
        .enumerate()
        .map(|(i, text)| Word {
            start_us: 200_000 + i as i64 * 800_000,
            end_us: 700_000 + i as i64 * 800_000,
            text: (*text).into(),
            probability: 0.9,
        })
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

    let tools = c.rpc("tools/list", json!({}));
    let tool = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "correct_words").unwrap();
    assert_eq!(tool["inputSchema"]["required"], json!(["run_id", "transcript_key", "corrections"]));
    let run = c.call("begin_run", json!({"label":"fix a word"}));
    let edits = json!([{"type":"addAssets","assets":[asset]},{"type":"addClip","assetId":"take"}]);
    c.call("apply_edits", json!({"run_id":run["run_id"],"request_id":"place","edits":edits}));
    let captions = |c: &mut Client| {
        let state = c.call("get_state", json!({}));
        let track = state["tracks"].as_array().unwrap().iter().find(|t| t["name"] == "Captions").unwrap().clone();
        track["clips"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["content"]["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    c.call("build_captions", json!({"run_id":run["run_id"]}));
    assert!(captions(&mut c).iter().any(|t| t.contains("vejte")));
    let transcript = c.call("get_transcript", json!({}));
    assert_eq!(transcript["words"][1]["text"], "vejte");
    let args = json!({"run_id":run["run_id"],"request_id":"dejte","transcript_key":transcript["transcript_key"],
        "corrections":[{"i":1,"text":"dejte"}]});
    let fixed = c.call("correct_words", args.clone());
    assert_eq!(fixed["words"], json!([{"i":1,"before":"vejte","after":"dejte"}]));
    assert_eq!(fixed["captions_changed"].as_array().unwrap().len(), 1);
    assert_eq!(c.call("correct_words", args), fixed, "a retry with the same request_id corrects once");
    let shown = captions(&mut c);
    assert!(shown.iter().any(|t| t.contains("dejte")) && !shown.iter().any(|t| t.contains("vejte")), "{shown:?}");
    let now = c.call("get_transcript", json!({}));
    assert_eq!(now["words"][1]["text"], "dejte");
    assert_eq!(now["transcript_key"], fixed["transcript_key"]);
    let stale = json!({"run_id":run["run_id"],"transcript_key":transcript["transcript_key"],"corrections":[{"i":2,"text":"to"}]});
    assert!(c.error("correct_words", stale).contains("SPEECH_CHANGED"));
    // Building the captions again reads the corrected word.
    c.call("build_captions", json!({"run_id":run["run_id"]}));
    assert_eq!(captions(&mut c), shown);
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    c.finish();
    let disk: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.nuzky")).unwrap()).unwrap();
    assert_eq!(
        disk["wordCorrections"],
        json!([{"assetId":"take","sourceStartUs":1_000_000,"original":"vejte","text":"dejte"}])
    );
}

/// A 12 s talk of three sentences of four 0.4 s tone "words", the middle sentence 12 dB louder,
/// with its words stored as if recognised. Only on Linux does XDG_DATA_HOME choose the store.
#[cfg(target_os = "linux")]
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
    // 16-bit mono WAV: the RIFF header, the format chunk and the samples.
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + samples.len() as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    for field in [16u32, 1 | (1 << 16), 48_000, 96_000, 2 | (16 << 16)] {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
    bytes.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
    std::fs::write(&wav, bytes).unwrap();
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
#[ignore = "Requires tmp-test/talk.mp4 and installed small + Silero models; run with XDG_DATA_HOME=tmp-test/xdg/data"]
fn transcribe_edit_and_caption_real_media_over_stdio() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let media = root.join("tmp-test/talk.mp4").canonicalize().unwrap();
    let mut c = Client::new(true);
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
    loop {
        let status = c.call("job", json!({"job_id":job["job_id"],"action":"get"}));
        if status["status"] == "done" {
            // The English word timing model from scripts/fixtures.sh measured the words.
            assert_eq!(status["result"]["assets"][0]["word_times"], "measured", "{status}");
            break;
        }
        assert_eq!(status["status"], "running", "{status}");
        assert!(std::time::Instant::now() < deadline, "transcription timed out");
        std::thread::sleep(Duration::from_secs(2));
    }
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
    assert!(c.error("build_captions", json!({"run_id":run["run_id"],"style_preset":"neon"})).contains("karaoke"));
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
    c.finish();
}

#[test]
#[ignore = "Requires tmp-test/reel-1..3.mp4 from scripts/fixtures.sh and large-v3-turbo-q5_0 + Silero in tmp-test/xdg/data/nuzky/models"]
fn retakes_of_three_czech_takes_over_stdio() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
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
#[ignore = "Requires tmp-test/face-thumb.mp4 and the vision models from scripts/fixtures.sh; run with XDG_DATA_HOME=tmp-test/xdg/data"]
fn cover_frames_and_subject_mask_of_a_face_over_stdio() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let media = root.join("tmp-test/face-thumb.mp4").canonicalize().unwrap();
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
    c.call("undo_run", json!({"run_id":run}));
    let state = c.call("get_state", json!({}));
    assert_eq!((state["thumbnails"].clone(), state["tracks"][0]["clips"].clone()), (json!([]), json!([])));
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
#[ignore = "Requires tmp-test/face-thumb.mp4 and the vision models from scripts/fixtures.sh; run with XDG_DATA_HOME=tmp-test/xdg/data"]
fn text_behind_a_face_over_stdio_and_the_cli() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let media = root.join("tmp-test/face-thumb.mp4").canonicalize().unwrap();
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

/// A long talk becomes reels: proposals out of bounds are refused, three picked ones become projects beside it
/// with only their words on a 9:16 canvas, the talk and its video stay as they were, and undo takes it all back.
#[cfg(target_os = "linux")]
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
        assert_eq!(path, c.dir.join(format!("project-reel-{}.nuzky", i + 1)));
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
