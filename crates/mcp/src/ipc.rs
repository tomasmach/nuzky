//! Private, project-scoped Unix transport. The listener owns its Host until all clients drain.
use std::collections::HashMap;
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::{
    ffi::OsStrExt,
    fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    net::{UnixListener, UnixStream},
};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use capopen_engine::edit::new_id;
use capopen_session::host::Host;
use rmcp::model::CallToolResult;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::tools::{Access, Backend, Client, tool_error};

const PROTOCOL: u32 = 1;
const HELLO_TIMEOUT: Duration = Duration::from_secs(3);
const ACCEPT_INTERVAL: Duration = Duration::from_millis(20);
const MAX_LINE: u64 = 64 * 1024 * 1024;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;
pub const APP_CLOSED: &str = "APP_CLOSED: CapOpen was closed; restart the agent's MCP server";

fn uid() -> u32 {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // POSIX geteuid has no failure mode and does not access caller memory.
    unsafe { geteuid() }
}

pub fn socket_path(project: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(project).context("PROJECT_MISSING: resolving IPC path")?;
    let hash = canonical
        .as_os_str()
        .as_bytes()
        .iter()
        .fold(FNV_OFFSET, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(FNV_PRIME)
        });
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
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && meta.uid() == uid(),
        "IPC_UNAVAILABLE: directory is not owned by this user"
    );
    fs::set_permissions(path, Permissions::from_mode(0o700))?;
    Ok(())
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("IPC_UNAVAILABLE: removing endpoint"),
    }
}

fn send(stream: &mut UnixStream, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .context("IPC_CLOSED: writing message")
}

fn receive<T: serde::de::DeserializeOwned>(reader: &mut BufReader<UnixStream>) -> Result<T> {
    let mut line = String::new();
    reader
        .take(MAX_LINE + 1)
        .read_line(&mut line)
        .context("IPC_CLOSED: reading message")?;
    ensure!(!line.is_empty(), "IPC_CLOSED: peer disconnected");
    ensure!(
        line.len() as u64 <= MAX_LINE && line.ends_with('\n'),
        "PROTOCOL_MISMATCH: oversized or incomplete message"
    );
    serde_json::from_str(&line).context("PROTOCOL_MISMATCH: invalid message")
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
    // Keep the OS project lock until endpoint cleanup has finished.
    _host: Arc<Host>,
}

impl Listener {
    pub fn start(host: Arc<Host>) -> Result<Self> {
        let path = socket_path(&host.session.locked_path()?)?;
        Self::at(host, path)
    }

    pub fn at(host: Arc<Host>, path: PathBuf) -> Result<Self> {
        let project = host.session.locked_path()?;
        private_directory(
            path.parent()
                .context("IPC_UNAVAILABLE: no socket directory")?,
        )?;
        remove(&path)?;
        remove(&path.with_extension("token"))?;
        let listener = UnixListener::bind(&path).context("IPC_UNAVAILABLE: binding socket")?;
        let mut owner = Self {
            path,
            stop: Arc::default(),
            connections: Arc::default(),
            worker: None,
            _host: host.clone(),
        };
        fs::set_permissions(&owner.path, Permissions::from_mode(0o600))?;
        let mut random = [0u8; 32];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let token = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let mut token_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(owner.path.with_extension("token"))?;
        token_file.write_all(token.as_bytes())?;
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
                                Ok((stream, _)) => {
                                    let id = new_id();
                                    let copy = match stream.try_clone() {
                                        Ok(copy) => copy,
                                        Err(error) => {
                                            eprintln!(
                                                "IPC_UNAVAILABLE: tracking connection: {error}"
                                            );
                                            continue;
                                        }
                                    };
                                    connections.lock().unwrap().insert(id.clone(), copy);
                                    let connections = connections.clone();
                                    let (host, project, token) =
                                        (host.clone(), project.clone(), token.clone());
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
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        for path in [&self.path, &self.path.with_extension("token")] {
            if let Err(e) = remove(path) {
                eprintln!("{e:#}");
            }
        }
    }
}

fn connection(mut stream: UnixStream, host: Arc<Host>, project: &Path, token: &str) -> Result<()> {
    stream.set_read_timeout(Some(HELLO_TIMEOUT))?;
    stream.set_write_timeout(Some(HELLO_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let hello = receive::<Hello>(&mut reader)
        .map_err(|error| anyhow::anyhow!("PROTOCOL_MISMATCH: invalid hello: {error:#}"))
        .and_then(|hello| {
            ensure!(
                hello.capopen == PROTOCOL,
                "PROTOCOL_MISMATCH: expected capopen {PROTOCOL}"
            );
            ensure!(hello.token == token, "UNAUTHORIZED: invalid token");
            Ok(hello)
        });
    let hello = match hello {
        Ok(hello) => hello,
        Err(e) => {
            send(&mut stream, &json!({"ok":false,"error":format!("{e:#}")}))?;
            return Ok(());
        }
    };
    let backend = Backend::shared(
        host,
        project,
        Client {
            id: new_id(),
            access: hello.access,
        },
    )?;
    send(
        &mut stream,
        &json!({"ok":true,"session_epoch":backend.host.session.state()?.stamp.session_epoch}),
    )?;
    stream.set_read_timeout(None)?;
    let writer = Mutex::new(stream);
    std::thread::scope(|scope| {
        while let Ok(request) = receive::<Request>(&mut reader) {
            let (backend, writer) = (&backend, &writer);
            scope.spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    backend.call(&request.tool, request.args)
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("TOOL_FAILED: worker panicked")))
                .unwrap_or_else(|e| tool_error(backend, format!("{e:#}")));
                if send(
                    &mut writer.lock().unwrap(),
                    &Response {
                        id: request.id,
                        result,
                    },
                )
                .is_err()
                {
                    let _ = writer.lock().unwrap().shutdown(Shutdown::Both);
                }
            });
        }
    });
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
    pub fn connect(project: &Path, allow_write: bool) -> Result<Self> {
        let path = socket_path(project)?;
        let token = fs::read_to_string(path.with_extension("token"))
            .context("IPC_UNAVAILABLE: reading token")?;
        let mut stream = UnixStream::connect(path).context("IPC_UNAVAILABLE: connecting")?;
        stream.set_read_timeout(Some(HELLO_TIMEOUT))?;
        stream.set_write_timeout(Some(HELLO_TIMEOUT))?;
        send(
            &mut stream,
            &Hello {
                capopen: PROTOCOL,
                token,
                client: "capopen-mcp".into(),
                access: if allow_write {
                    Access::Write
                } else {
                    Access::ReadOnly
                },
            },
        )?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let hello: Value = receive(&mut reader)?;
        ensure!(
            hello["ok"] == true && hello["session_epoch"].is_string(),
            "IPC_UNAVAILABLE: {}",
            hello["error"]
        );
        stream.set_read_timeout(None)?;
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (requests, ended) = (pending.clone(), closed.clone());
        let worker = std::thread::Builder::new()
            .name("capopen-ipc-results".into())
            .spawn(move || {
                while let Ok(response) = receive::<Response>(&mut reader) {
                    if let Some(tx) = requests.lock().unwrap().remove(&response.id) {
                        let _ = tx.send(response.result);
                    }
                }
                ended.store(true, Ordering::Release);
                requests.lock().unwrap().clear();
            })
            .context("IPC_UNAVAILABLE: starting response reader")?;
        Ok(Self {
            writer: Mutex::new(stream),
            pending,
            closed,
            sequence: AtomicU64::new(0),
            reader: Some(worker),
        })
    }

    pub fn call(&self, tool: String, args: Value) -> Result<CallToolResult> {
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            ensure!(!self.closed.load(Ordering::Acquire), APP_CLOSED);
            pending.insert(id, tx);
        }
        if send(
            &mut self.writer.lock().unwrap(),
            &Request { id, tool, args },
        )
        .is_err()
        {
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
