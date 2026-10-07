//! Private, project-scoped Unix transport. The listener owns its Host until all clients drain.
use std::collections::HashMap;
use std::fs::{self, DirBuilder, File, Permissions};
use std::io::{BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::{
    ffi::OsStrExt,
    fs::{DirBuilderExt, PermissionsExt},
    net::{UnixListener, UnixStream},
};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use capopen_engine::edit::new_id;
use capopen_session::{
    hash::{FNV_OFFSET, hash_bytes},
    host::Host,
};
use rmcp::model::CallToolResult;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::limits::MAX_LINE_BYTES;
use crate::tools::{Access, Backend, Client, tool_error};
mod security;
mod wire;
mod workers;
use security::uid;
use wire::{receive, send};
use workers::Workers;
#[cfg(test)]
mod tests;

const PROTOCOL: u32 = 1;
const HELLO_TIMEOUT: Duration = Duration::from_secs(3);
const ACCEPT_INTERVAL: Duration = Duration::from_millis(20);
const MAX_HELLO_BYTES: usize = 4096;
const MAX_CONNECTIONS: usize = 8;
pub const APP_CLOSED: &str = "APP_CLOSED: CapOpen was closed; restart the agent's MCP server";

pub fn socket_path(project: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(project).context("PROJECT_MISSING: resolving IPC path")?;
    let mut hash = FNV_OFFSET;
    hash_bytes(&mut hash, canonical.as_os_str().as_bytes());
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(|p| PathBuf::from(p).join("capopen"))
        .unwrap_or_else(|| std::env::temp_dir().join(format!("capopen-{}", uid())));
    Ok(directory.join(format!("{hash:016x}.sock")))
}

fn private_directory(path: &Path) -> Result<()> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).context("IPC_UNAVAILABLE: creating private directory"),
    }
    security::directory(path)
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("IPC_UNAVAILABLE: removing endpoint"),
    }
}

#[derive(Deserialize, Serialize)]
struct Hello {
    capopen: u32,
    token: String,
    client: String,
    access: Access,
}
#[derive(Deserialize, Serialize)]
struct Request {
    id: u64,
    tool: String,
    args: Value,
}
#[derive(Deserialize, Serialize)]
struct Response {
    id: u64,
    result: CallToolResult,
}

pub struct Listener {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    connections: Arc<Mutex<HashMap<String, UnixStream>>>,
    worker: Option<JoinHandle<()>>,
}

impl Listener {
    pub fn start(host: Arc<Host>) -> Result<Self> {
        let path = socket_path(&host.session.locked_path()?)?;
        Self::at(host, path)
    }

    pub fn at(host: Arc<Host>, path: PathBuf) -> Result<Self> {
        let project = host.session.locked_path()?;
        private_directory(path.parent().context("IPC_UNAVAILABLE: no socket directory")?)?;
        remove(&path)?;
        remove(&path.with_extension("token"))?;
        // Dropping the owner on any error below removes both endpoint files.
        let mut owner = Self { path, stop: Arc::default(), connections: Arc::default(), worker: None };
        let mut random = [0u8; 32];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let token = random.iter().map(|b| format!("{b:02x}")).collect::<String>();
        // A client that finds the socket must also find its token.
        let mut token_file = security::create_token(&owner.path.with_extension("token"))?;
        token_file.write_all(token.as_bytes())?;
        let listener = UnixListener::bind(&owner.path).context("IPC_UNAVAILABLE: binding socket")?;
        fs::set_permissions(&owner.path, Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let stop = owner.stop.clone();
        let connections = owner.connections.clone();
        owner.worker = Some(
            std::thread::Builder::new()
                .name("capopen-ipc".into())
                .spawn(move || {
                    std::thread::scope(|scope| {
                        while !stop.load(Ordering::Acquire) {
                            match listener.accept() {
                                Ok((mut stream, _)) => {
                                    if connections.lock().unwrap().len() >= MAX_CONNECTIONS {
                                        let _ = stream.set_write_timeout(Some(ACCEPT_INTERVAL));
                                        let _ = send(
                                            &mut stream,
                                            &json!({"ok":false,"error":"IPC_BUSY: connection limit reached"}),
                                        );
                                        continue;
                                    }
                                    let id = new_id();
                                    let copy = match stream.try_clone() {
                                        Ok(copy) => copy,
                                        Err(error) => {
                                            eprintln!("IPC_UNAVAILABLE: tracking connection: {error}");
                                            continue;
                                        }
                                    };
                                    connections.lock().unwrap().insert(id.clone(), copy);
                                    let connections = connections.clone();
                                    let (host, project, token) = (host.clone(), project.clone(), token.clone());
                                    scope.spawn(move || {
                                        if let Err(e) = connection(stream, host, &project, &token) {
                                            eprintln!("IPC_CLIENT: {e:#}");
                                        }
                                        connections.lock().unwrap().remove(&id);
                                    });
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                    std::thread::sleep(ACCEPT_INTERVAL)
                                }
                                Err(e) => {
                                    eprintln!("IPC_UNAVAILABLE: {e}");
                                    break;
                                }
                            }
                        }
                        for stream in connections.lock().unwrap().values() {
                            let _ = stream.shutdown(Shutdown::Both);
                        }
                    });
                })
                .context("IPC_UNAVAILABLE: starting listener")?,
        );
        Ok(owner)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for stream in self.connections.lock().unwrap().values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        // The listener thread drains its scoped clients and retains the project lock off the UI thread.
        self.worker.take();
        for path in [&self.path, &self.path.with_extension("token")] {
            if let Err(e) = remove(path) {
                eprintln!("{e:#}");
            }
        }
    }
}

fn connection(mut stream: UnixStream, host: Arc<Host>, project: &Path, token: &str) -> Result<()> {
    security::peer(&stream)?;
    stream.set_write_timeout(Some(HELLO_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let hello =
        receive::<Hello>(&mut reader, MAX_HELLO_BYTES, Some(Instant::now() + HELLO_TIMEOUT)).and_then(|hello| {
            ensure!(hello.capopen == PROTOCOL, "PROTOCOL_MISMATCH: expected capopen {PROTOCOL}");
            ensure!(security::equal_token(&hello.token, token), "UNAUTHORIZED: invalid token");
            Ok(hello)
        });
    let hello = match hello {
        Ok(hello) => hello,
        Err(e) => {
            send(&mut stream, &json!({"ok":false,"error":format!("{e:#}")}))?;
            return Ok(());
        }
    };
    let backend = Arc::new(Backend::shared(host, project, Client { id: new_id(), access: hello.access })?);
    send(&mut stream, &json!({"ok":true,"session_epoch":backend.host.session.stamp().session_epoch}))?;
    stream.set_read_timeout(None)?;
    serve_requests(&mut reader, backend, Arc::new(Mutex::new(stream)), MAX_LINE_BYTES)
}

fn serve_requests(
    reader: &mut BufReader<UnixStream>,
    backend: Arc<Backend>,
    writer: Arc<Mutex<UnixStream>>,
    line_limit: usize,
) -> Result<()> {
    let mut workers = Workers::default();
    while let Ok(line) = wire::line(reader, line_limit, None) {
        if !line.complete {
            let result = tool_error(&backend, "REQUEST_TOO_LARGE: IPC line limit exceeded".into());
            let _ = send(&mut writer.lock().unwrap(), &json!({"id":wire::request_id(&line.bytes),"result":result}));
            break;
        }
        let request: Request = match serde_json::from_slice(&line.bytes) {
            Ok(request) => request,
            Err(error) => {
                let result = tool_error(&backend, format!("PROTOCOL_MISMATCH: {error}"));
                let _ = send(&mut writer.lock().unwrap(), &json!({"id":wire::request_id(&line.bytes),"result":result}));
                continue;
            }
        };
        let (request_backend, request_writer) = (backend.clone(), writer.clone());
        let id = request.id;
        if let Err(error) = workers.start(move || {
            let (backend, writer) = (request_backend, request_writer);
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.call(&request.tool, request.args)))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("TOOL_FAILED: worker panicked")))
                    .unwrap_or_else(|e| tool_error(&backend, format!("{e:#}")));
            if send(&mut writer.lock().unwrap(), &Response { id, result }).is_err() {
                let _ = writer.lock().unwrap().shutdown(Shutdown::Both);
            }
        }) {
            let result = tool_error(&backend, error.to_string());
            if send(&mut writer.lock().unwrap(), &Response { id, result }).is_err() {
                break;
            }
        }
    }
    backend.close();
    drop(workers);
    backend.disconnect()
}

type Pending = Arc<Mutex<HashMap<u64, mpsc::Sender<CallToolResult>>>>;
pub struct Remote {
    writer: Mutex<UnixStream>,
    pending: Pending,
    closed: Arc<AtomicBool>,
    sequence: AtomicU64,
    reader: Option<JoinHandle<()>>,
}

impl Remote {
    pub fn connect(project: &Path, allow_write: bool) -> Result<Option<Self>> {
        Self::at(&socket_path(project)?, allow_write)
    }

    fn at(path: &Path, allow_write: bool) -> Result<Option<Self>> {
        let directory = path.parent().context("IPC_UNTRUSTED: missing endpoint directory")?;
        match fs::symlink_metadata(directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            _ => security::directory(directory)?,
        }
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            _ => security::socket(path)?,
        }
        let mut stream = match UnixStream::connect(path) {
            Ok(stream) => stream,
            Err(error)
                if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error).context("IPC_UNTRUSTED: connecting to endpoint"),
        };
        security::peer(&stream)?;
        let token = security::token(&path.with_extension("token"))?;
        stream.set_write_timeout(Some(HELLO_TIMEOUT))?;
        let deadline = Instant::now() + HELLO_TIMEOUT;
        send(
            &mut stream,
            &Hello {
                capopen: PROTOCOL,
                token,
                client: "capopen-mcp".into(),
                access: if allow_write { Access::Write } else { Access::ReadOnly },
            },
        )?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let hello: Value = receive(&mut reader, MAX_HELLO_BYTES, Some(deadline))?;
        if hello["ok"] != true {
            let error = hello["error"].as_str().unwrap_or("PROTOCOL_MISMATCH: missing hello response");
            ensure!(!error.starts_with("UNAUTHORIZED:"), "UNAUTHORIZED: endpoint rejected the token");
            ensure!(!error.starts_with("PROTOCOL_MISMATCH:"), "PROTOCOL_MISMATCH: endpoint rejected the protocol");
            anyhow::bail!("IPC_UNTRUSTED: endpoint rejected connection");
        }
        ensure!(hello["session_epoch"].is_string(), "PROTOCOL_MISMATCH: missing session epoch");
        stream.set_read_timeout(None)?;
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (requests, ended) = (pending.clone(), closed.clone());
        let worker = std::thread::Builder::new()
            .name("capopen-ipc-results".into())
            .spawn(move || {
                while let Ok(response) = receive::<Response>(&mut reader, MAX_LINE_BYTES, None) {
                    if let Some(tx) = requests.lock().unwrap().remove(&response.id) {
                        let _ = tx.send(response.result);
                    }
                }
                ended.store(true, Ordering::Release);
                requests.lock().unwrap().clear();
            })
            .context("IPC_UNAVAILABLE: starting response reader")?;
        Ok(Some(Self {
            writer: Mutex::new(stream),
            pending,
            closed,
            sequence: AtomicU64::new(0),
            reader: Some(worker),
        }))
    }

    pub fn call(&self, tool: String, args: Value) -> Result<CallToolResult> {
        ensure!(!self.closed.load(Ordering::Acquire), APP_CLOSED);
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        let bytes = wire::encode(&Request { id, tool, args })
            .context("REQUEST_TOO_LARGE: cannot encode IPC request within transport limit")?;
        let (tx, rx) = mpsc::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            ensure!(!self.closed.load(Ordering::Acquire), APP_CLOSED);
            pending.insert(id, tx);
        }
        if self.writer.lock().unwrap().write_all(&bytes).is_err() {
            self.closed.store(true, Ordering::Release);
            self.pending.lock().unwrap().clear();
            let _ = self.writer.lock().unwrap().shutdown(Shutdown::Both);
            anyhow::bail!(APP_CLOSED);
        }
        rx.recv().context(APP_CLOSED)
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        let _ = self.writer.lock().unwrap().shutdown(Shutdown::Both);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
