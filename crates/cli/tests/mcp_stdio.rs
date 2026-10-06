//! Builds and exercises the actual CLI executable over stdio.
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use capopen_engine::{Project, edit::new_id};
use serde_json::{Value, json};

struct Client {
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
        let dir = std::env::temp_dir().join(format!("capopen-mcp-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.capopen");
        std::fs::write(
            &path,
            serde_json::to_vec(&Project::new("Original")).unwrap(),
        )
        .unwrap();
        if let Some(project) = checkpoint {
            std::fs::write(dir.join("project.capopen.checkpoint.json"),
                serde_json::to_vec(&json!({"project": project})).unwrap()).unwrap();
        }
        let binary = env!("CARGO_BIN_EXE_capopen");
        let mut command = Command::new(binary);
        command
            .arg("mcp")
            .arg("--project")
            .arg(path)
            .arg("--cache")
            .arg(dir.join("cache"));
        if write {
            command.arg("--allow-write");
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
                let value = serde_json::from_str(&line)
                    .unwrap_or_else(|_| panic!("Non-JSON on MCP stdout: {line}"));
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            output,
            dir,
            seq: 0,
        };
        let init = client.rpc("initialize", json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"integration-test","version":"1"}}));
        assert_eq!(init["result"]["serverInfo"]["name"], "capopen");
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
            let value = self
                .output
                .recv_timeout(Duration::from_secs(20))
                .expect("MCP response timed out");
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
    fn finish(&mut self) {
        self.input.take();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "MCP did not exit on EOF"
            );
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
    assert_eq!(tools.len(), 13);
    let apply = tools.iter().find(|t| t["name"] == "apply_edits").unwrap();
    assert!(apply["inputSchema"]["$defs"]["EditCmd"].is_object());
    let captions = tools.iter().find(|t| t["name"] == "build_captions").unwrap();
    assert!(!captions["inputSchema"]["required"].as_array().unwrap().contains(&json!("style")));
    let recovery = tools.iter().find(|t| t["name"] == "resolve_recovery").unwrap();
    assert!(recovery["description"].as_str().unwrap().contains("Ask the user"));
    let before = c.call("get_state", json!({}));
    assert_eq!(before["name"], "Original");
    let run = c.call("begin_run", json!({"label":"integration"}));
    let args = json!({"run_id":run["run_id"],"request_id":"rename","expected_revision":run["revision"],"edits":[{"type":"renameProject","name":"Edited via MCP"}]});
    let applied = c.call("apply_edits", args.clone());
    assert_eq!(applied, c.call("apply_edits", args));
    assert_eq!(c.call("get_state", json!({}))["name"], "Edited via MCP");
    c.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    c.call("undo_run", json!({"run_id":run["run_id"]}));
    assert_eq!(c.call("get_state", json!({}))["name"], before["name"]);
    c.finish();
    let disk: Value =
        serde_json::from_slice(&std::fs::read(c.dir.join("project.capopen")).unwrap()).unwrap();
    assert_eq!(disk["name"], "Original");
}

#[test]
fn readonly_resources_prompts_and_clear_errors() {
    let mut c = Client::new(false);
    assert_eq!(c.call("get_state", json!({}))["read_only"], true);
    let error = c.rpc(
        "tools/call",
        json!({"name":"begin_run","arguments":{"label":"denied"}}),
    );
    assert_eq!(error["result"]["isError"], true);
    assert!(
        error["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("READ_ONLY")
    );
    let resources = c.rpc("resources/list", json!({}));
    assert_eq!(
        resources["result"]["resources"].as_array().unwrap().len(),
        2
    );
    let guide = c.rpc("resources/read", json!({"uri":"capopen://guide"}));
    assert!(
        guide["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("rippleDeleteRanges")
    );
    let prompt = c.rpc(
        "prompts/get",
        json!({"name":"edit_selected","arguments":{"goal":"Make a reel"}}),
    );
    assert!(
        prompt["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("Make a reel")
    );
    let error = c.rpc(
        "tools/call",
        json!({"name":"inspect_frames","arguments":{"times_us":[0]}}),
    );
    assert_eq!(error["result"]["isError"], true);
    c.finish();
}

#[test]
fn disconnect_keeps_and_removes_checkpoint() {
    let mut c = Client::new(true);
    let run = c.call("begin_run", json!({"label":"disconnect"}));
    c.call("apply_edits", json!({"run_id":run["run_id"],"request_id":"rename","edits":[{"type":"renameProject","name":"Kept"}]}));
    c.finish();
    assert!(!c.dir.join("project.capopen.checkpoint.json").exists());
    let disk: Value =
        serde_json::from_slice(&std::fs::read(c.dir.join("project.capopen")).unwrap()).unwrap();
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
