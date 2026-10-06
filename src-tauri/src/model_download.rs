//! Downloads publish only complete, verified files while holding the model's OS lock.
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use capopen_session::jobs::check_cancel;
use sha2::{Digest, Sha256};

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
    progress(0.0);
    let response = ureq::get(url).call().context("Requesting model")?;
    if let Some(size) = response.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse::<u64>().ok()) {
        ensure!(size == expected.size, "MODEL_INVALID: unexpected download size");
    }
    let mut reader = response.into_body().into_reader();
    let tmp = sidecar(path, &format!(".{}.part", capopen_engine::edit::new_id()));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp).context("Creating model download")?;
    let result = (|| {
        let mut buf = [0; 1 << 16];
        let mut done = 0u64;
        let mut hash = Sha256::new();
        loop {
            check_cancel(cancel)?;
            let n = reader.read(&mut buf).context("Reading model download")?;
            if n == 0 {
                break;
            }
            done += n as u64;
            ensure!(done <= expected.size, "MODEL_INVALID: download exceeds expected size");
            file.write_all(&buf[..n]).context("Writing model download")?;
            hash.update(&buf[..n]);
            progress(done as f32 / expected.size as f32);
        }
        ensure!(done == expected.size, "MODEL_INVALID: incomplete download");
        ensure!(checksum(hash) == expected.sha256, "MODEL_INVALID: checksum mismatch");
        file.sync_all().context("Syncing model download")?;
        drop(file);
        check_cancel(cancel)?;
        std::fs::rename(&tmp, path).context("Installing downloaded model")?;
        Ok(path.to_path_buf())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
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
    fn failed_or_cancelled_downloads_preserve_existing_files_and_clean_only_their_temp() {
        for (body, advertised, cancel_after_read) in [
            (&b"short"[..], 5, false),
            (&b"short"[..], 8, false),
            (&b"too long!"[..], 9, false),
            (&b"corrupt!"[..], 8, false),
            (&b"model-v1"[..], 8, true),
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
                1
            );
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
