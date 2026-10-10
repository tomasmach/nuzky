#![cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::os::unix::{fs::PermissionsExt, net::UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use nuzky_engine::{
    Project,
    edit::{EditCmd, new_id},
};
use nuzky_mcp::ipc::Listener;
use nuzky_session::{Mode, ProjectSession, host::Host};
use serde_json::{Value, json};

struct App {
    listener: Option<Listener>,
    host: Arc<Host>,
    dir: PathBuf,
    path: PathBuf,
    socket: PathBuf,
    /// False while the app restarts, so its files stay.
    remove: bool,
}
impl App {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("ipc-{}", new_id()));
        std::fs::create_dir_all(dir.join("nuzky")).unwrap();
        std::fs::set_permissions(dir.join("nuzky"), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("project.nuzky");
        let mut project = Project::new("Before");
        project
            .apply(EditCmd::AddText {
                start_us: 0,
                text: "IPC image".into(),
                style: nuzky_engine::model::TextStyle {
                    font_family: None,
                    font_size: 64.0,
                    color: "#ffffff".into(),
                    bold: false,
                    stroke_width: 0.0,
                    stroke_color: "#000000".into(),
                    background: None,
                    max_width: None,
                    highlight: None,
                    keywords: None,
                },
            })
            .unwrap();
        std::fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
        let session = ProjectSession::open(&path, Mode::Write, None).unwrap();
        Self::open(dir, path, session)
    }
    fn open(dir: PathBuf, path: PathBuf, session: ProjectSession) -> Self {
        let host = Arc::new(Host::new(session, dir.join("cache")).unwrap());
        let socket = dir.join("nuzky").join(nuzky_mcp::ipc::socket_path(&path).unwrap().file_name().unwrap());
        let listener = Some(Listener::at(host.clone(), socket.clone()).unwrap());
        Self { listener, host, dir, path, socket, remove: true }
    }
    /// Closes the project as quitting the app does, then opens it again.
    fn restart(mut self) -> Self {
        self.remove = false;
        let (dir, path) = (self.dir.clone(), self.path.clone());
        drop(self);
        // The listener lets go of the project off this thread, as in the app.
        let mut session = None;
        wait(|| {
            session = ProjectSession::open(&path, Mode::Write, None).ok();
            session.is_some()
        });
        Self::open(dir, path, session.unwrap())
    }
    fn hello(&self, token: &str, version: u32) -> Value {
        let mut stream = UnixStream::connect(&self.socket).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        writeln!(stream, "{}", json!({"nuzky":version,"token":token,"client":"test","access":"write"})).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.listener.take();
        if self.remove {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
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
        let project = app.path.to_string_lossy().into_owned();
        let mut args = vec!["--project", project.as_str()];
        if write {
            args.push("--allow-write");
        }
        Self::with(app, &args)
    }
    fn with(app: &App, args: &[&str]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_nuzky"));
        command.arg("mcp").args(args).env("XDG_RUNTIME_DIR", &app.dir);
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
        ("resolve_recovery", json!({"action":"keep"})),
    ] {
        reader.error(tool, args, "READ_ONLY");
    }
    // A reader may stop a job it started, such as a long activity, but only its own.
    reader.error("job", json!({"job_id":"none","action":"cancel"}), "UNKNOWN_JOB");
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
            nuzky_session::jobs::check_cancel(&cancel)?;
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
    assert!(!app.path.with_extension("nuzky.checkpoint.json").exists());
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

/// An agent configured once with `--current` attaches to whichever project the app has open, live,
/// and refuses to start rather than edit a project headless when no app has one open.
#[test]
fn current_attaches_to_the_open_project_and_refuses_without_an_app() {
    let mut app = App::new();
    let current = app.dir.join("nuzky/current");
    assert_eq!(std::fs::read_to_string(&current).unwrap(), std::fs::canonicalize(&app.path).unwrap().to_string_lossy());
    assert_eq!(std::fs::metadata(&current).unwrap().permissions().mode() & 0o777, 0o600);
    let mut bridge = Bridge::with(&app, &["--current", "--allow-write"]);
    assert_eq!(bridge.call("get_state", json!({}))["name"], "Before");
    let run = bridge.call("begin_run", json!({"label":"current"}));
    // The run is the app's own: the app's session shows it.
    assert_eq!(app.host.session.state().unwrap().open_run.unwrap().run_id, run["run_id"]);
    bridge.call("end_run", json!({"run_id":run["run_id"],"action":"keep"}));
    bridge.finish();
    app.listener.take();
    assert!(!current.exists(), "closing the project removes its name");
    let refused = Command::new(env!("CARGO_BIN_EXE_nuzky"))
        .args(["mcp", "--current", "--allow-write"])
        .env("XDG_RUNTIME_DIR", &app.dir)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("APP_NOT_RUNNING"), "{refused:?}");
}

/// Three agent runs and an edit in the app are versions that outlive the app. undo_to over stdio
/// brings back exactly the project after the first run as one step, Undo in the app takes it back,
/// and after a restart every version is still there.
#[test]
fn versions_restore_an_earlier_run_and_outlive_a_restart() {
    let app = App::new();
    let mut agent = Bridge::new(&app, true);
    let mut runs = Vec::new();
    for name in ["First run", "Second run", "Third run"] {
        let run = agent.call("begin_run", json!({"label": name}))["run_id"].clone();
        let edit = json!({"run_id":run,"request_id":"rename","edits":[{"type":"renameProject","name":name}]});
        agent.call("apply_edits", edit);
        agent.call("end_run", json!({"run_id":run,"action":"keep"}));
        runs.push(run);
    }
    app.host.session.edit(vec![EditCmd::RenameProject { name: "By hand".into() }], None, Default::default()).unwrap();
    let before = agent.call("list_history", json!({}));
    let labels = |list: &Value| -> Vec<String> {
        list["versions"].as_array().unwrap().iter().map(|v| v["label"].as_str().unwrap().to_owned()).collect()
    };
    assert_eq!(labels(&before), ["Rename project", "Third run", "Second run", "First run", "Opened"]);
    let first = before["versions"][3].clone();
    assert_eq!(first["run_id"], runs[0]);
    assert!(before["versions"][0]["run_id"].is_null(), "the edit in the app is the user's");
    assert_eq!(before["current_hash"], before["versions"][0]["hash"]);

    // The jump waits for a run to end, with the code the app already explains.
    let open = agent.call("begin_run", json!({"label":"open"}))["run_id"].clone();
    agent.error("undo_to", json!({"index":first["index"]}), "RUN_ACTIVE");
    agent.call("end_run", json!({"run_id":open,"action":"keep"}));
    agent.error("undo_to", json!({"index":999}), "UNKNOWN_VERSION");
    let prefix = &first["hash"].as_str().unwrap()[..6];
    let restored = agent.call("undo_to", json!({"hash":prefix}));
    assert_eq!(
        (restored["changed"].clone(), restored["restored"]["index"].clone()),
        (json!(true), first["index"].clone())
    );
    assert_eq!(app.host.session.state().unwrap().project.name, "First run");
    let jumped = agent.call("list_history", json!({}));
    assert_eq!(jumped["current_hash"], first["hash"], "the project is exactly the version after the first run");
    assert_eq!(jumped["versions"][0]["label"], "Restore: First run");
    assert_eq!(jumped["versions"][0]["hash"], first["hash"]);
    assert_eq!(labels(&jumped)[1..], labels(&before), "the versions after it stay");

    app.host.session.undo().unwrap();
    assert_eq!(app.host.session.state().unwrap().project.name, "By hand");
    let undone = agent.call("list_history", json!({}));
    assert_eq!(undone["current_hash"], before["current_hash"], "Undo in the app takes the jump back");
    agent.finish();

    let app = app.restart();
    let mut agent = Bridge::new(&app, false);
    let reopened = agent.call("list_history", json!({"limit":200}));
    assert_eq!(reopened["versions"], undone["versions"], "every version outlives the restart");
    assert_eq!(reopened["current_hash"], undone["current_hash"]);
    agent.error("undo_to", json!({"index":first["index"]}), "READ_ONLY");
    agent.finish();
    let mut agent = Bridge::new(&app, true);
    agent.call("undo_to", json!({"index":first["index"]}));
    assert_eq!(app.host.session.state().unwrap().project.name, "First run");
    agent.finish();
}
