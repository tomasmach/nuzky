//! Downloads publish only complete, verified files while holding the model's OS lock. An
//! interrupted download stays in `<model>.part` and continues with an HTTP range request.
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use capopen_session::jobs::check_cancel;
use sha2::{Digest, Sha256};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// A connection that sends nothing for this long fails; the next attempt resumes the partial file.
const STALL_TIMEOUT: Duration = if cfg!(test) { Duration::from_millis(500) } else { Duration::from_secs(30) };
/// Bounds how long an abandoned request thread can stay blocked. Slower downloads continue
/// with another range request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// Cancellation is noticed this quickly even while the connection is silent.
const POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy)]
pub struct Integrity {
    pub size: u64,
    pub sha256: &'static str,
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn checksum(hash: Sha256) -> String {
    hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn installed(path: &Path, expected: Integrity, cancel: &AtomicBool) -> Result<bool> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("Opening installed model"),
    };
    if file.metadata()?.len() != expected.size {
        return Ok(false);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 1 << 16];
    loop {
        check_cancel(cancel)?;
        let read = file.read(&mut buffer).context("Verifying installed model")?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(checksum(hash) == expected.sha256)
}

pub fn download(
    url: &str,
    path: &Path,
    expected: Integrity,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<PathBuf> {
    check_cancel(cancel)?;
    std::fs::create_dir_all(path.parent().context("Model needs a directory")?).context("Creating model directory")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(sidecar(path, ".lock"))
        .context("Opening model download lock")?;
    loop {
        check_cancel(cancel)?;
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(50)),
            Err(TryLockError::Error(error)) => return Err(error).context("Locking model download"),
        }
    }
    // Another process may have installed the model while we waited. Also repair older corrupt files.
    if installed(path, expected, cancel)? {
        return Ok(path.to_path_buf());
    }
    remove_old_parts(path);
    let part = sidecar(path, ".part");
    let mut resumed = std::fs::metadata(&part).is_ok_and(|file| file.len() > 0);
    loop {
        let result = transfer(url, &part, expected, cancel, &mut progress).and_then(|()| {
            ensure!(installed(&part, expected, cancel)?, "MODEL_INVALID: checksum mismatch");
            Ok(())
        });
        match result {
            Ok(()) => break,
            Err(error) if error.to_string().starts_with("MODEL_INVALID") => {
                let _ = std::fs::remove_file(&part);
                // The partial file of an earlier attempt may be what was wrong: start once from zero.
                if !std::mem::take(&mut resumed) {
                    return Err(error);
                }
            }
            // Cancelled, stalled or failed connections keep the partial file for the next attempt.
            Err(error) => return Err(error),
        }
    }
    check_cancel(cancel)?;
    std::fs::rename(&part, path).context("Installing downloaded model")?;
    Ok(path.to_path_buf())
}

/// Earlier versions downloaded into `<model>.<id>.part`, left behind by a crash.
fn remove_old_parts(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return };
    let prefix = format!("{}.", name.to_string_lossy());
    let current = format!("{prefix}part");
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if file.starts_with(&prefix) && file.ends_with(".part") && file != current {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Completes `part` to the expected size, continuing from the bytes it already holds.
fn transfer(
    url: &str,
    part: &Path,
    expected: Integrity,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(f32),
) -> Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(part)
        .context("Opening model download")?;
    let mut done = file.metadata().context("Reading partial model")?.len();
    if done > expected.size {
        done = 0;
    }
    progress(done as f32 / expected.size as f32);
    while done < expected.size {
        let from = done;
        let chunks = request(url, from)?;
        let mut heard = Instant::now();
        let ended = loop {
            check_cancel(cancel)?;
            match chunks.recv_timeout(POLL) {
                Ok(Chunk::Head { status, range_start, length }) => {
                    match (status, range_start) {
                        (200, _) => done = 0,
                        (206, Some(start)) if start == from => {}
                        _ => bail!("MODEL_INVALID: unexpected response {status}"),
                    }
                    if let Some(length) = length {
                        ensure!(length == expected.size - done, "MODEL_INVALID: unexpected download size");
                    }
                    file.set_len(done).context("Resuming model download")?;
                    file.seek(SeekFrom::Start(done)).context("Resuming model download")?;
                    heard = Instant::now();
                }
                Ok(Chunk::Data(bytes)) => {
                    done += bytes.len() as u64;
                    ensure!(done <= expected.size, "MODEL_INVALID: download exceeds expected size");
                    file.write_all(&bytes).context("Writing model download")?;
                    progress(done as f32 / expected.size as f32);
                    heard = Instant::now();
                }
                Ok(Chunk::End(result)) => break result,
                Err(RecvTimeoutError::Timeout) => ensure!(
                    heard.elapsed() < STALL_TIMEOUT,
                    "DOWNLOAD_STALLED: the server stopped sending the model; try again to continue"
                ),
                Err(RecvTimeoutError::Disconnected) => bail!("Model download stopped"),
            }
        };
        // A dropped or time-limited connection continues where it stopped; one that got nowhere fails.
        if done <= from && done < expected.size {
            ended?;
            bail!("Model download ended early");
        }
    }
    file.sync_all().context("Syncing model download")
}

enum Chunk {
    Head { status: u16, range_start: Option<u64>, length: Option<u64> },
    Data(Vec<u8>),
    End(Result<()>),
}

/// Runs one request on its own thread, so a silent connection never delays cancellation.
fn request(url: &str, from: u64) -> Result<Receiver<Chunk>> {
    let (tx, rx) = mpsc::sync_channel(8);
    let url = url.to_owned();
    std::thread::Builder::new()
        .name("capopen-model-download".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_resolve(Some(CONNECT_TIMEOUT))
                .timeout_connect(Some(CONNECT_TIMEOUT))
                .timeout_send_request(Some(CONNECT_TIMEOUT))
                .timeout_recv_response(Some(RESPONSE_TIMEOUT))
                .timeout_recv_body(Some(REQUEST_TIMEOUT))
                .build()
                .into();
            // Byte ranges address the stored file, never a compressed transfer.
            let mut call = agent.get(&url).header("Accept-Encoding", "identity");
            if from > 0 {
                call = call.header("Range", format!("bytes={from}-"));
            }
            let response = match call.call() {
                Ok(response) => response,
                Err(ureq::Error::StatusCode(416)) => {
                    let _ = tx.send(Chunk::End(Err(anyhow::anyhow!("MODEL_INVALID: server refused to resume"))));
                    return;
                }
                Err(error) => {
                    let _ = tx.send(Chunk::End(Err(error).context("Requesting model")));
                    return;
                }
            };
            let header = |name: &str| response.headers().get(name).and_then(|value| value.to_str().ok());
            let range_start =
                header("content-range").and_then(|range| range.strip_prefix("bytes ")?.split('-').next()?.parse().ok());
            let head = Chunk::Head {
                status: response.status().as_u16(),
                range_start,
                length: header("content-length").and_then(|value| value.parse().ok()),
            };
            if tx.send(head).is_err() {
                return;
            }
            let mut reader = response.into_body().into_reader();
            loop {
                let mut buffer = vec![0; 1 << 16];
                let chunk = match reader.read(&mut buffer) {
                    Ok(0) => Chunk::End(Ok(())),
                    Ok(read) => {
                        buffer.truncate(read);
                        Chunk::Data(buffer)
                    }
                    Err(error) => Chunk::End(Err(error).context("Reading model download")),
                };
                let end = matches!(chunk, Chunk::End(_));
                if tx.send(chunk).is_err() || end {
                    return;
                }
            }
        })
        .context("Starting model download")?;
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(Instant::now() < deadline, "download test timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    const INTEGRITY: Integrity =
        Integrity { size: 8, sha256: "1a1f4502024df8a68d12e64bb2364ad6308d04ed0a7d5e8300a676ec70867140" };

    fn serve(listener: TcpListener, bytes: &'static [u8], advertised: usize) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {advertised}\r\nConnection: close\r\n\r\n").unwrap();
            socket.write_all(bytes).ok();
        })
    }

    #[test]
    fn download_rejects_same_size_corruption() {
        let dir = std::env::temp_dir().join(format!("capopen-download-{}", capopen_engine::edit::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
        let server = serve(listener, b"corrupt!", 8);
        let result = download(&url, &path, INTEGRITY, &AtomicBool::new(false), |_| {});
        server.join().unwrap();
        let published = path.exists();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(result.is_err(), "corrupt response was accepted");
        assert!(!published, "corrupt model was installed");
    }
    #[test]
    fn failed_or_cancelled_downloads_preserve_existing_files_and_keep_only_resumable_parts() {
        for (body, advertised, cancel_after_read, resumable) in [
            (&b"short"[..], 5, false, false),
            (&b"short"[..], 8, false, true),
            (&b"too long!"[..], 9, false, false),
            (&b"corrupt!"[..], 8, false, false),
            (&b"model-v1"[..], 8, true, true),
        ] {
            let dir = std::env::temp_dir().join(format!("capopen-download-{}", capopen_engine::edit::new_id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("model.bin");
            std::fs::write(&path, b"old invalid file").unwrap();
            let foreign = path.with_extension("part");
            std::fs::write(&foreign, b"another process owns this").unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
            let server = serve(listener, body, advertised);
            let cancel = AtomicBool::new(false);
            let result = download(&url, &path, INTEGRITY, &cancel, |fraction| {
                if cancel_after_read && fraction > 0.0 {
                    cancel.store(true, Ordering::Relaxed);
                }
            });
            server.join().unwrap();
            assert!(result.is_err());
            if cancel_after_read {
                assert!(result.unwrap_err().to_string().starts_with("CANCELLED:"));
            }
            assert_eq!(std::fs::read(&path).unwrap(), b"old invalid file");
            assert_eq!(std::fs::read(&foreign).unwrap(), b"another process owns this");
            assert_eq!(
                std::fs::read_dir(&dir)
                    .unwrap()
                    .filter(|entry| entry.as_ref().unwrap().path().extension().is_some_and(|ext| ext == "part"))
                    .count(),
                1 + resumable as usize
            );
            assert_eq!(sidecar(&path, ".part").exists(), resumable);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    type Script = Box<dyn FnOnce(&mut std::net::TcpStream, &str) + Send>;

    /// Answers each connection with the next script and returns the request heads it read.
    fn scripted(listener: TcpListener, scripts: Vec<Script>) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let mut heads = Vec::new();
            for script in scripts {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut head = Vec::new();
                let mut byte = [0];
                while !head.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                }
                let head = String::from_utf8(head).unwrap().to_ascii_lowercase();
                script(&mut socket, &head);
                heads.push(head);
            }
            heads
        })
    }

    /// Serves the rest of "model-v1" for `Range: bytes=4-`, otherwise the whole file.
    fn resume(socket: &mut std::net::TcpStream, head: &str) {
        if head.contains("range: bytes=4-") {
            write!(
                socket,
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/8\r\nContent-Length: 4\r\n\r\nl-v1"
            )
            .unwrap();
        } else {
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nmodel-v1").unwrap();
        }
    }

    #[test]
    fn silent_connections_cancel_promptly_and_the_next_attempt_resumes() {
        for sends_part in [false, true] {
            let dir = std::env::temp_dir().join(format!("capopen-download-{}", capopen_engine::edit::new_id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("model.bin");
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
            let (release, held) = std::sync::mpsc::channel::<()>();
            let server = scripted(
                listener,
                vec![
                    Box::new(move |socket, _| {
                        if sends_part {
                            write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nmode").unwrap();
                        }
                        let _ = held.recv();
                    }),
                    Box::new(resume),
                ],
            );
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            let (worker_cancel, worker_url, worker_path) = (cancel.clone(), url.clone(), path.clone());
            let worker =
                std::thread::spawn(move || download(&worker_url, &worker_path, INTEGRITY, &worker_cancel, |_| {}));
            let part = sidecar(&path, ".part");
            if sends_part {
                wait_until(|| std::fs::read(&part).is_ok_and(|bytes| bytes == b"mode"));
            } else {
                std::thread::sleep(Duration::from_millis(200));
            }
            cancel.store(true, Ordering::Relaxed);
            let began = Instant::now();
            let error = worker.join().unwrap().unwrap_err();
            assert!(error.to_string().starts_with("CANCELLED:"), "{error:#}");
            assert!(began.elapsed() < Duration::from_secs(1), "cancel took {:?}", began.elapsed());
            release.send(()).unwrap();
            download(&url, &path, INTEGRITY, &AtomicBool::new(false), |_| {}).unwrap();
            let heads = server.join().unwrap();
            assert_eq!(heads[1].contains("range: bytes=4-"), sends_part);
            assert_eq!(std::fs::read(&path).unwrap(), b"model-v1");
            assert!(!part.exists());
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn stalled_download_fails_and_keeps_its_part() {
        let dir = std::env::temp_dir().join(format!("capopen-download-{}", capopen_engine::edit::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
        let (release, held) = std::sync::mpsc::channel::<()>();
        let server = scripted(
            listener,
            vec![Box::new(move |socket, _| {
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nmode").unwrap();
                let _ = held.recv();
            })],
        );
        let began = Instant::now();
        let error = download(&url, &path, INTEGRITY, &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(error.to_string().starts_with("DOWNLOAD_STALLED:"), "{error:#}");
        assert!(began.elapsed() < STALL_TIMEOUT + Duration::from_secs(1));
        assert_eq!(std::fs::read(sidecar(&path, ".part")).unwrap(), b"mode");
        release.send(()).unwrap();
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ignored_range_or_corrupt_part_restarts_from_zero_and_cleans_old_parts() {
        // The server ignores the range and sends the whole file, or the partial file was wrong:
        // the checksum fails and the download starts over.
        for (partial, ranges) in [(&b"mode"[..], &[true][..]), (b"XXXX", &[true, false])] {
            let script = || -> Vec<Script> {
                if ranges.len() == 1 {
                    vec![Box::new(|socket, _| resume(socket, ""))]
                } else {
                    vec![Box::new(resume), Box::new(resume)]
                }
            };
            let dir = std::env::temp_dir().join(format!("capopen-download-{}", capopen_engine::edit::new_id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("model.bin");
            std::fs::write(sidecar(&path, ".part"), partial).unwrap();
            let old = sidecar(&path, ".0123456789abcdef.part");
            std::fs::write(&old, b"left by a crash").unwrap();
            let foreign = path.with_extension("part");
            std::fs::write(&foreign, b"another model").unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
            let server = scripted(listener, script());
            download(&url, &path, INTEGRITY, &AtomicBool::new(false), |_| {}).unwrap();
            let heads = server.join().unwrap();
            assert_eq!(heads.iter().map(|head| head.contains("range: bytes=4-")).collect::<Vec<_>>(), ranges);
            assert_eq!(std::fs::read(&path).unwrap(), b"model-v1");
            assert!(!sidecar(&path, ".part").exists() && !old.exists());
            assert_eq!(std::fs::read(&foreign).unwrap(), b"another model");
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn corrupt_installed_model_is_replaced_and_valid_model_is_reused_offline() {
        let dir = std::env::temp_dir().join(format!("capopen-download-{}", capopen_engine::edit::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        std::fs::write(&path, b"corrupt!").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
        let server = serve(listener, b"model-v1", 8);
        let cancel = AtomicBool::new(false);
        download(&url, &path, INTEGRITY, &cancel, |_| {}).unwrap();
        server.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"model-v1");
        download("invalid URL", &path, INTEGRITY, &cancel, |_| panic!("no download needed")).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn download_child() {
        let Some(dir) = std::env::var_os("CAPOPEN_MODEL_DOWNLOAD_TEST") else { return };
        let dir = PathBuf::from(dir);
        let url = std::fs::read_to_string(dir.join("url")).unwrap();
        std::fs::write(dir.join(format!("ready-{}", std::process::id())), b"").unwrap();
        let path = download(&url, &dir.join("model.bin"), INTEGRITY, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"model-v1");
    }

    #[test]
    fn downloads_serialize_across_processes_and_recheck_after_lock() {
        let dir = std::env::temp_dir().join(format!("capopen-download-{}", capopen_engine::edit::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        std::fs::write(dir.join("url"), format!("http://{}/model.bin", listener.local_addr().unwrap())).unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(sidecar(&path, ".lock"))
            .unwrap();
        lock.lock().unwrap();
        let spawning = crate::CHILD_SPAWN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut children: Vec<_> = (0..2)
            .map(|_| {
                Child(
                    std::process::Command::new(std::env::current_exe().unwrap())
                        .args(["--exact", "model_download::tests::download_child", "--nocapture"])
                        .env("CAPOPEN_MODEL_DOWNLOAD_TEST", &dir)
                        .spawn()
                        .unwrap(),
                )
            })
            .collect();
        wait_until(|| children.iter().all(|child| dir.join(format!("ready-{}", child.0.id())).exists()));
        // Running children have replaced the copies of this process's open files.
        drop(spawning);
        std::thread::sleep(Duration::from_millis(150));
        assert!(children.iter_mut().all(|child| child.0.try_wait().unwrap().is_none()));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "request started without the lock"
        );
        assert!(!path.exists());
        let server_listener = listener.try_clone().unwrap();
        // serve() uses a blocking accept; the original becomes nonblocking again after the response.
        server_listener.set_nonblocking(false).unwrap();
        let server = serve(server_listener, b"model-v1", 8);
        drop(lock);
        for child in &mut children {
            wait_until(|| child.0.try_wait().unwrap().is_some());
            assert!(child.0.wait().unwrap().success());
        }
        server.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "second process downloaded again"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"model-v1");
        assert!(
            !std::fs::read_dir(&dir).unwrap().any(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "part"))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancelling_while_waiting_for_model_lock_does_not_start_a_download() {
        let dir = std::env::temp_dir().join(format!("capopen-download-{}", capopen_engine::edit::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(sidecar(&path, ".lock"))
            .unwrap();
        lock.lock().unwrap();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || download("invalid URL", &path, INTEGRITY, &worker_cancel, |_| {}));
        std::thread::sleep(Duration::from_millis(100));
        cancel.store(true, Ordering::Relaxed);
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.to_string().starts_with("CANCELLED:"));
        drop(lock);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
