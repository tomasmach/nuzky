use super::*;
use capopen_engine::Project;
use capopen_session::{Mode, ProjectSession};
use std::os::unix::fs::symlink;

struct Fixture {
    dir: PathBuf,
    path: PathBuf,
    socket: PathBuf,
    host: Arc<Host>,
    _cleanup: Cleanup,
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("ipc-hardening-{}", new_id()));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("project.capopen");
        fs::write(&path, serde_json::to_vec(&Project::new("fixture")).unwrap()).unwrap();
        let host =
            Arc::new(Host::new(ProjectSession::open(&path, Mode::Write, None).unwrap(), dir.join("cache")).unwrap());
        Self { socket: dir.join("project.sock"), _cleanup: Cleanup(dir.clone()), dir, path, host }
    }
    fn listener(&self) -> Listener {
        Listener::at(self.host.clone(), self.socket.clone()).unwrap()
    }
    fn hello(&self) -> Value {
        json!({"capopen":1,"token":fs::read_to_string(self.socket.with_extension("token")).unwrap(),"client":"test","access":"write"})
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Arc::strong_count(&self.host) > 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
fn value(reader: &mut BufReader<UnixStream>) -> Value {
    receive(reader, MAX_LINE_BYTES, Some(Instant::now() + Duration::from_secs(5))).unwrap()
}

#[test]
fn prepared_endpoint_cannot_exfiltrate_a_symlinked_token() {
    let f = Fixture::new();
    let fake = UnixListener::bind(&f.socket).unwrap();
    let secret = f.dir.join("private-key");
    fs::write(&secret, "a".repeat(security::TOKEN_LEN)).unwrap();
    fs::set_permissions(&secret, Permissions::from_mode(0o600)).unwrap();
    symlink(&secret, f.socket.with_extension("token")).unwrap();
    let error = Remote::at(&f.socket, false).err().unwrap();
    assert!(error.to_string().starts_with("IPC_UNTRUSTED"));
    let (mut peer, _) = fake.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let mut sent = Vec::new();
    peer.read_to_end(&mut sent).unwrap();
    assert!(sent.is_empty(), "no hello or token may be sent");
    fs::set_permissions(&f.dir, Permissions::from_mode(0o755)).unwrap();
    assert!(Remote::at(&f.socket, false).is_err());
    assert!(Listener::at(f.host.clone(), f.socket.clone()).is_err());
    assert_eq!(fs::metadata(&f.dir).unwrap().permissions().mode() & 0o777, 0o755);
}

#[test]
fn fallback_requires_absence_not_rejection_mismatch_or_timeout() {
    let f = Fixture::new();
    assert!(Remote::at(&f.socket, true).unwrap().is_none());
    let fake = UnixListener::bind(&f.socket).unwrap();
    drop(fake);
    assert!(Remote::at(&f.socket, true).unwrap().is_none());
    fs::remove_file(&f.socket).unwrap();
    security::create_token(&f.socket.with_extension("token"))
        .unwrap()
        .write_all("a".repeat(security::TOKEN_LEN).as_bytes())
        .unwrap();
    for (reply, code) in [
        (Some(json!({"ok":false,"error":"UNAUTHORIZED: wrong token"})), "UNAUTHORIZED"),
        (Some(json!({"ok":false,"error":"PROTOCOL_MISMATCH: old server"})), "PROTOCOL_MISMATCH"),
        (Some(json!({"ok":true})), "PROTOCOL_MISMATCH"),
        (None, "IPC_UNTRUSTED"),
    ] {
        let fake = UnixListener::bind(&f.socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = fake.accept().unwrap();
            let _: Value = value(&mut BufReader::new(stream.try_clone().unwrap()));
            if let Some(reply) = reply {
                send(&mut stream, &reply).unwrap();
            } else {
                std::thread::sleep(HELLO_TIMEOUT + Duration::from_millis(100));
            }
        });
        let error = Remote::at(&f.socket, false).err().unwrap();
        assert!(error.to_string().starts_with(code), "{error:#}");
        server.join().unwrap();
        fs::remove_file(&f.socket).unwrap();
    }
}

#[test]
fn connections_and_hello_lines_are_bounded() {
    let f = Fixture::new();
    let listener = f.listener();
    let mut connections = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let mut stream = UnixStream::connect(&f.socket).unwrap();
        send(&mut stream, &f.hello()).unwrap();
        assert_eq!(value(&mut BufReader::new(stream.try_clone().unwrap()))["ok"], true);
        connections.push(stream);
    }
    let mut extra = BufReader::new(UnixStream::connect(&f.socket).unwrap());
    assert!(value(&mut extra)["error"].as_str().unwrap().starts_with("IPC_BUSY"));
    connections.clear();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !listener.connections.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut stream = UnixStream::connect(&f.socket).unwrap();
    stream.write_all(&vec![b' '; MAX_HELLO_BYTES + 1]).unwrap();
    assert!(value(&mut BufReader::new(stream))["error"].as_str().unwrap().starts_with("PROTOCOL_MISMATCH"));
    drop(listener);
}

#[test]
fn oversized_request_receives_an_error_with_its_id() {
    let f = Fixture::new();
    let (server, mut client) = UnixStream::pair().unwrap();
    let backend =
        Arc::new(Backend::shared(f.host.clone(), &f.path, Client { id: new_id(), access: Access::Write }).unwrap());
    let worker = std::thread::spawn(move || {
        let mut reader = BufReader::new(server.try_clone().unwrap());
        serve_requests(&mut reader, backend, Arc::new(Mutex::new(server)), 128).unwrap();
    });
    let request = json!({"id":91,"tool":"get_state","args":{"payload":"x".repeat(200)}});
    // Preserve id before the large argument rather than relying on map ordering.
    let bytes = format!("{{\"id\":91,\"tool\":\"get_state\",\"args\":{}}}\n", request["args"]);
    client.write_all(bytes.as_bytes()).unwrap();
    let reply = value(&mut BufReader::new(client));
    assert_eq!(reply["id"], 91);
    assert!(reply["result"].to_string().contains("REQUEST_TOO_LARGE"));
    worker.join().unwrap();
}

#[test]
fn oversized_inspection_is_a_tool_error_and_connection_survives() {
    let f = Fixture::new();
    let listener = f.listener();
    let remote = Remote::at(&f.socket, true).unwrap().unwrap();
    let run = remote.call("begin_run".into(), json!({"label":"frames"})).unwrap().structured_content.unwrap();
    let result = remote.call("apply_edits".into(), json!({"run_id":run["run_id"],"request_id":"caption","edits":[{"type":"addText","startUs":0,"text":"test","style":{"fontSize":64,"color":"#fff","bold":false,"strokeWidth":0,"strokeColor":"#000","background":null}}]})).unwrap();
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let result = remote.call("inspect_frames".into(), json!({"times_us":vec![0;16],"width":1280})).unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(serde_json::to_string(&result).unwrap().contains("RESULT_TOO_LARGE"));
    let error = remote.call("get_state".into(), json!({"payload":"x".repeat(MAX_LINE_BYTES)})).unwrap_err();
    assert!(error.to_string().starts_with("REQUEST_TOO_LARGE"));
    assert_ne!(remote.call("get_state".into(), json!({})).unwrap().is_error, Some(true));
    drop(remote);
    drop(listener);
}

#[test]
fn remote_disconnect_cancels_its_jobs_and_keeps_its_run() {
    let f = Fixture::new();
    let listener = f.listener();
    let remote = Remote::at(&f.socket, true).unwrap().unwrap();
    let run = remote.call("begin_run".into(), json!({"label":"owner"})).unwrap().structured_content.unwrap();
    let job = remote.call("transcribe".into(), json!({"asset_ids":[]})).unwrap().structured_content.unwrap();
    assert_eq!(job["run_id"], run["run_id"]);
    let (release, wait) = mpsc::channel();
    let held = f
        .host
        .start_job(
            job["owner"].as_str().unwrap(),
            Some(run["run_id"].as_str().unwrap()),
            "test",
            f.host.session.state().unwrap().stamp,
            move |_, _| {
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(json!({}))
            },
        )
        .unwrap();
    drop(remote);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let cancelled = f.host.jobs.get(held["job_id"].as_str().unwrap(), false).unwrap()["cancel_requested"] == true;
        if cancelled && f.host.session.state().unwrap().open_run.is_none() {
            break;
        }
        assert!(Instant::now() < deadline, "disconnect did not cancel the client job and finish the run");
        std::thread::sleep(Duration::from_millis(10));
    }
    release.send(()).unwrap();
    drop(listener);
}

#[test]
fn cancelled_transcription_never_loads_models() {
    let missing = Path::new("/nonexistent-capopen-model");
    let asset = capopen_engine::model::Asset {
        id: "cancelled".into(),
        name: "cancelled".into(),
        path: "/nonexistent-capopen-source".into(),
        kind: capopen_engine::model::AssetKind::Audio,
        duration_us: 1,
        width: 0,
        height: 0,
        fps: 0.0,
        has_audio: true,
        rotation: 0,
    };
    let error = capopen_analysis::transcribe_words_cancellable(
        capopen_analysis::AudioSource::Asset { asset: &asset, cache: missing },
        missing,
        missing,
        "auto",
        || true,
    )
    .unwrap_err();
    assert!(error.to_string().starts_with("CANCELLED"));
}

#[test]
fn token_is_ready_before_the_socket_and_a_failed_bind_leaves_neither() {
    let f = Fixture::new();
    // Unix socket paths are limited to about 108 bytes, so binding this one fails.
    let long = f.dir.join(format!("{}.sock", "x".repeat(120)));
    let error = Listener::at(f.host.clone(), long.clone()).err().unwrap();
    assert!(error.to_string().starts_with("IPC_UNAVAILABLE"), "{error:#}");
    assert!(!long.with_extension("token").exists() && !long.exists());
    let listener = f.listener();
    assert!(f.socket.exists() && f.socket.with_extension("token").exists());
    let remote = Remote::at(&f.socket, false).unwrap().unwrap();
    assert_ne!(remote.call("get_state".into(), json!({})).unwrap().is_error, Some(true));
    drop(remote);
    drop(listener);
    assert!(!f.socket.exists() && !f.socket.with_extension("token").exists());
}
