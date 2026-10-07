//! Builds and exercises the actual CLI executable over stdio.
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use capopen_engine::{Project, edit::new_id};
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
        let dir = std::env::temp_dir().join(format!("capopen-mcp-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.capopen");
        std::fs::write(&path, serde_json::to_vec(&Project::new("Original")).unwrap()).unwrap();
        if let Some(project) = checkpoint {
            std::fs::write(
                dir.join("project.capopen.checkpoint.json"),
                serde_json::to_vec(&json!({"project": project})).unwrap(),
            )
            .unwrap();
        }
        let binary = env!("CARGO_BIN_EXE_capopen");
        let mut command = Command::new(binary);
        command.arg("mcp").arg("--project").arg(path).arg("--cache").arg(dir.join("cache"));
        if write {
            command.arg("--allow-write");
        }
        if let Some(style) = style {
            let data = dir.join("data");
            std::fs::create_dir_all(data.join("capopen")).unwrap();
            if let Some(text) = style {
                std::fs::write(data.join("capopen/EDIT.md"), text).unwrap();
            }
            command.env("XDG_DATA_HOME", data);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Build capopen-cli first");
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
        assert_eq!(init["result"]["serverInfo"]["name"], "capopen");
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
fn initialize_list_state_edit_end_undo_over_stdio() {
    let mut c = Client::new(true);
    let list = c.rpc("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 16);
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
    let disk: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.capopen")).unwrap()).unwrap();
    assert_eq!(disk["name"], "Original");
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
    let guide = c.rpc("resources/read", json!({"uri":"capopen://guide"}));
    assert!(guide["result"]["contents"][0]["text"].as_str().unwrap().contains("rippleDeleteRanges"));
    let prompt = c.rpc("prompts/get", json!({"name":"edit_selected","arguments":{"goal":"Make a reel"}}));
    assert!(prompt["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("Make a reel"));
    // One prompt starts the whole rough cut; the user's wishes come with it.
    let names: Vec<_> = c.rpc("prompts/list", json!({}))["result"]["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].clone())
        .collect();
    assert_eq!(names, [json!("edit_selected"), json!("rough_cut")]);
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
    let error = c.rpc("tools/call", json!({"name":"inspect_frames","arguments":{"times_us":[0]}}));
    assert_eq!(error["result"]["isError"], true);
    c.finish();
}

// Only on Linux does XDG_DATA_HOME choose where CapOpen looks for EDIT.md.
#[cfg(target_os = "linux")]
#[test]
fn a_creator_style_reaches_the_agent_and_without_one_nothing_changes() {
    let guide = include_str!("../../../skills/capopen-edit/SKILL.md");
    let mut plain = Client::with_style(None);
    assert_eq!(plain.init["result"]["instructions"], guide);
    assert_eq!(plain.rpc("resources/list", json!({}))["result"]["resources"].as_array().unwrap().len(), 2);
    assert_eq!(plain.rpc("resources/read", json!({"uri":"capopen://guide"}))["result"]["contents"][0]["text"], guide);
    assert!(
        plain.rpc("resources/read", json!({"uri":"capopen://style"}))["error"]["message"]
            .as_str()
            .unwrap()
            .contains("capopen style learn")
    );
    plain.finish();

    let style = "# Editing style\n\nKeep pauses under 120 ms.\n";
    let mut styled = Client::with_style(Some(style));
    let instructions = styled.init["result"]["instructions"].as_str().unwrap().to_owned();
    assert!(instructions.starts_with("This creator has their own editing style, capopen://style"), "{instructions}");
    assert!(instructions.contains(guide) && instructions.ends_with(style));
    let resources = styled.rpc("resources/list", json!({}));
    let uris: Vec<&str> =
        resources["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
    assert_eq!(uris, ["capopen://guide", "capopen://schema", "capopen://style"]);
    assert_eq!(styled.rpc("resources/read", json!({"uri":"capopen://style"}))["result"]["contents"][0]["text"], style);
    assert!(
        styled.rpc("resources/read", json!({"uri":"capopen://guide"}))["result"]["contents"][0]["text"]
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
    assert!(!c.dir.join("project.capopen.checkpoint.json").exists());
    let disk: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.capopen")).unwrap()).unwrap();
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
        assert!(!c.dir.join("project.capopen.checkpoint.json").exists());
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

// Only on Linux does XDG_DATA_HOME choose where CapOpen keeps transcripts.
#[cfg(target_os = "linux")]
#[test]
fn correct_words_fixes_the_transcript_and_captions_over_stdio() {
    use capopen_engine::{model::Asset, model::AssetKind, speech::Word};
    use capopen_session::transcripts::{Record, TranscriptStore, VERSION};
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
    let store = TranscriptStore::at(c.dir.join("data/capopen/transcripts")).unwrap();
    let record = Record {
        version: VERSION,
        fingerprint: store.fingerprint(&asset).unwrap(),
        duration_us: asset.duration_us,
        model: "fixture".into(),
        language: "cs".into(),
        words,
        segments: vec![],
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
    let disk: Value = serde_json::from_slice(&std::fs::read(c.dir.join("project.capopen")).unwrap()).unwrap();
    assert_eq!(
        disk["wordCorrections"],
        json!([{"assetId":"take","sourceStartUs":1_000_000,"original":"vejte","text":"dejte"}])
    );
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
#[ignore = "Requires tmp-test/reel-1..3.mp4 from scripts/fixtures.sh and large-v3-turbo-q5_0 + Silero in tmp-test/xdg/data/capopen/models"]
fn retakes_of_three_czech_takes_over_stdio() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let model = root.join("tmp-test/xdg/data/capopen/models/ggml-large-v3-turbo-q5_0.bin");
    let takes: Vec<PathBuf> = (1..=3).map(|n| root.join(format!("tmp-test/reel-{n}.mp4"))).collect();
    // Its own data directory, so no transcript stored by an earlier run is found.
    let mut c = Client::start(true, None, Some(None));
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
