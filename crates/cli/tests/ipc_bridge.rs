#![cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::os::unix::{fs::PermissionsExt, net::UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use capopen_engine::{
    Project,
    edit::{EditCmd, new_id},
};
use capopen_mcp::ipc::Listener;
use capopen_session::{Mode, ProjectSession, host::Host};
use serde_json::{Value, json};

struct App {
    listener: Option<Listener>,
    host: Arc<Host>,
    dir: PathBuf,
    path: PathBuf,
    socket: PathBuf,
}
impl App {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("ipc-{}", new_id()));
        std::fs::create_dir_all(dir.join("capopen")).unwrap();
        std::fs::set_permissions(dir.join("capopen"), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("project.capopen");
        let mut project = Project::new("Before");
        project
            .apply(EditCmd::AddText {
                start_us: 0,
                text: "IPC image".into(),
                style: capopen_engine::model::TextStyle {
                    font_family: None,
                    font_size: 64.0,
                    color: "#ffffff".into(),
                    bold: false,
                    stroke_width: 0.0,
                    stroke_color: "#000000".into(),
                    background: None,
                    max_width: None,
                    highlight: None,
                },
            })
            .unwrap();
        std::fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
        let host =
            Arc::new(Host::new(ProjectSession::open(&path, Mode::Write, None).unwrap(), dir.join("cache")).unwrap());
        let socket = dir.join("capopen").join(capopen_mcp::ipc::socket_path(&path).unwrap().file_name().unwrap());
        let listener = Some(Listener::at(host.clone(), socket.clone()).unwrap());
        Self { listener, host, dir, path, socket }
    }
    fn hello(&self, token: &str, version: u32) -> Value {
        let mut stream = UnixStream::connect(&self.socket).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        writeln!(stream, "{}", json!({"capopen":version,"token":token,"client":"test","access":"write"})).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.listener.take();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Bridge {
    child: Child,
    input: Option<ChildStdin>,
    output: mpsc::Receiver<Value>,
    id: u64,
}
impl Bridge {
    fn new(app: &App, write: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_capopen"));
        command.args(["mcp", "--project"]).arg(&app.path).env("XDG_RUNTIME_DIR", &app.dir);
        if write {
            command.arg("--allow-write");
        }
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(serde_json::from_str(&line).unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut bridge = Self { child, input, output, id: 0 };
        bridge.rpc(
            "initialize",
            json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"ipc-test","version":"1"}}),
        );
        writeln!(bridge.input.as_mut().unwrap(), "{}", json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .unwrap();
        bridge
    }
    fn send(&mut self, method: &str, params: Value) -> u64 {
        self.id += 1;
        writeln!(
            self.input.as_mut().unwrap(),
            "{}",
            json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params})
        )
        .unwrap();
        self.id
    }
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.send(method, params);
        loop {
            let value = self.output.recv_timeout(Duration::from_secs(30)).unwrap();
            if value["id"] == self.id {
                assert!(value.get("error").is_none(), "{value}");
                return value["result"].clone();
            }
        }
    }
    fn result(&mut self, tool: &str, args: Value) -> Value {
        self.rpc("tools/call", json!({"name":tool,"arguments":args}))
    }
    fn call(&mut self, tool: &str, args: Value) -> Value {
        let result = self.result(tool, args);
        assert_ne!(result["isError"], true, "{result}");
        result["structuredContent"].clone()
    }
    fn error(&mut self, tool: &str, args: Value, code: &str) {
        let result = self.result(tool, args);
        assert_eq!(result["isError"], true, "{result}");
        assert!(result.to_string().contains(code), "{result}");
    }
    fn finish(&mut self) {
        self.input.take();
        wait(|| {
            self.child.try_wait().unwrap().is_some_and(|status| {
                assert!(status.success());
                true
            })
        });
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn permissions(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn real_bridge_shares_app_and_preserves_access_runs_images_and_disconnect() {
    let mut app = App::new();
    let token_path = app.socket.with_extension("token");
    let token = std::fs::read_to_string(&token_path).unwrap();
    assert_eq!(token.len(), 64);
    assert_eq!(permissions(app.socket.parent().unwrap()), 0o700);
    assert_eq!(permissions(&app.socket), 0o600);
    assert_eq!(permissions(&token_path), 0o600);
    assert!(app.hello("wrong", 1)["error"].as_str().unwrap().starts_with("UNAUTHORIZED"));
    assert!(app.hello(&token, 2)["error"].as_str().unwrap().starts_with("PROTOCOL_MISMATCH"));
    let mut writer = Bridge::new(&app, true);
    let mut reader = Bridge::new(&app, false);
    let mut other = Bridge::new(&app, true);
    let epoch = app.host.session.state().unwrap().stamp.session_epoch;
    assert_eq!(writer.call("get_state", json!({}))["session_epoch"], epoch);
    assert_eq!(reader.call("get_state", json!({}))["read_only"], true);
    for (tool, args) in [
        ("begin_run", json!({"label":"denied"})),
        ("transcribe", json!({})),
        ("job", json!({"job_id":"none","action":"cancel"})),
        ("resolve_recovery", json!({"action":"keep"})),
    ] {
        reader.error(tool, args, "READ_ONLY");
    }
    let run = writer.call("begin_run", json!({"label":"live run"}))["run_id"].clone();
    let edit = json!({"run_id":run,"request_id":"rename","edits":[{"type":"renameProject","name":"Live"}]});
    reader.error("apply_edits", edit.clone(), "READ_ONLY");
    other.error("apply_edits", edit.clone(), "INVALID_RUN");
    other.finish();
    reader.finish();
    assert_eq!(app.host.session.state().unwrap().open_run.unwrap().run_id, run);
    writer.call("apply_edits", edit.clone());
    assert_eq!(app.host.session.state().unwrap().project.name, "Live");
    assert!(app.host.session.edit(vec![], None, Default::default()).unwrap_err().to_string().contains("RUN_ACTIVE"));
    let frame_id = writer.send("tools/call", json!({"name":"inspect_frames","arguments":{"times_us":[0],"width":96}}));
    let state_id = writer.send("tools/call", json!({"name":"get_state","arguments":{}}));
    let mut results = std::collections::HashMap::new();
    while results.len() < 2 {
        let value = writer.output.recv_timeout(Duration::from_secs(30)).unwrap();
        if let Some(id) = value["id"].as_u64() {
            results.insert(id, value["result"].clone());
        }
    }
    assert_eq!(results[&state_id]["structuredContent"]["name"], "Live");
    let frames = &results[&frame_id];
    assert_ne!(frames["isError"], true, "{frames}");
    let image = frames["content"].as_array().unwrap().iter().find(|c| c["type"] == "image").unwrap();
    assert_eq!(image["mimeType"], "image/png");
    assert!(image["data"].as_str().unwrap().starts_with("iVBORw0KGgo"));
    let job = app
        .host
        .jobs
        .start("test-client", run.as_str(), "test", app.host.session.state().unwrap().stamp, |cancel, _| {
            while !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            capopen_session::jobs::check_cancel(&cancel)?;
            Ok(json!({}))
        })
        .unwrap();
    app.host.stop_run().unwrap();
    wait(|| app.host.jobs.get(job["job_id"].as_str().unwrap(), false).unwrap()["status"] == "cancelled");
    for tool in ["apply_edits", "build_captions", "end_run"] {
        writer.error(tool, edit.clone(), "RUN_STOPPED");
    }
    writer.call("undo_run", json!({"run_id":run}));
    assert_eq!(app.host.session.state().unwrap().project.name, "Before");
    let run = writer.call("begin_run", json!({"label":"keep on disconnect"}))["run_id"].clone();
    writer.call(
        "apply_edits",
        json!({"run_id":run,"request_id":"keep","edits":[{"type":"renameProject","name":"Kept"}]}),
    );
    writer.finish();
    wait(|| app.host.session.state().unwrap().open_run.is_none());
    assert_eq!(app.host.session.state().unwrap().project.name, "Kept");
    assert!(!app.path.with_extension("capopen.checkpoint.json").exists());
    app.host.session.undo_run(run.as_str().unwrap()).unwrap();
    assert_eq!(app.host.session.state().unwrap().project.name, "Before");
    let mut attached = Bridge::new(&app, true);
    app.listener.take();
    assert!(!app.socket.exists() && !token_path.exists());
    attached.error("get_state", json!({}), "APP_CLOSED");
    app.listener = Some(Listener::at(app.host.clone(), app.socket.clone()).unwrap());
    assert_ne!(std::fs::read_to_string(&token_path).unwrap(), token);
    assert!(app.hello(&token, 1)["error"].as_str().unwrap().starts_with("UNAUTHORIZED"));
    attached.error("get_state", json!({}), "APP_CLOSED");
    attached.finish();
}
