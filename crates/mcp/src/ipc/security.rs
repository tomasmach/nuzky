use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::{
    fd::AsRawFd,
    unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
        net::UnixStream,
    },
};
use std::path::Path;

use anyhow::{Context, Result, ensure};

pub(super) const TOKEN_LEN: usize = 64;

pub(super) fn uid() -> u32 {
    // POSIX geteuid has no failure mode and does not access caller memory.
    unsafe { libc::geteuid() }
}

pub(super) fn directory(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).context("IPC_UNTRUSTED: checking directory")?;
    ensure!(
        meta.is_dir() && meta.uid() == uid() && meta.mode() & 0o7777 == 0o700,
        "IPC_UNTRUSTED: endpoint directory must be owned by this user with mode 0700"
    );
    Ok(())
}

pub(super) fn socket(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).context("IPC_UNTRUSTED: checking socket")?;
    ensure!(meta.file_type().is_socket() && meta.uid() == uid(), "IPC_UNTRUSTED: socket must be owned by this user");
    Ok(())
}

pub(super) fn token(path: &Path) -> Result<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .context("IPC_UNTRUSTED: opening token without following symlinks")?;
    let meta = file.metadata().context("IPC_UNTRUSTED: checking token descriptor")?;
    ensure!(
        meta.is_file() && meta.uid() == uid() && meta.mode() & 0o7777 == 0o600 && meta.len() == TOKEN_LEN as u64,
        "IPC_UNTRUSTED: token must be a private 0600 regular file of exactly 64 bytes"
    );
    let mut bytes = Vec::with_capacity(TOKEN_LEN + 1);
    file.take((TOKEN_LEN + 1) as u64).read_to_end(&mut bytes).context("IPC_UNTRUSTED: reading token")?;
    ensure!(
        bytes.len() == TOKEN_LEN && bytes.iter().all(u8::is_ascii_hexdigit),
        "IPC_UNTRUSTED: token must contain 64 hex digits"
    );
    String::from_utf8(bytes).context("IPC_UNTRUSTED: invalid token encoding")
}

pub(super) fn create_token(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_EXCL)
        .open(path)
        .context("IPC_UNTRUSTED: creating exclusive token file")
}

pub(super) fn peer(stream: &UnixStream) -> Result<()> {
    verify_uid(peer_uid(stream)?)
}

fn verify_uid(peer: u32) -> Result<()> {
    ensure!(peer == uid(), "IPC_UNTRUSTED: peer belongs to a different user");
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid(stream: &UnixStream) -> Result<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut length = std::mem::size_of_val(&cred) as libc::socklen_t;
    // The kernel writes at most length bytes into the correctly sized ucred.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    ensure!(
        rc == 0 && length as usize == std::mem::size_of_val(&cred),
        "IPC_UNTRUSTED: cannot verify peer credentials"
    );
    Ok(cred.uid)
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn peer_uid(stream: &UnixStream) -> Result<u32> {
    let (mut user, mut group) = (0, 0);
    // getpeereid writes into the two valid uid_t/gid_t outputs.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut user, &mut group) };
    ensure!(rc == 0, "IPC_UNTRUSTED: cannot verify peer credentials");
    Ok(user)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
fn peer_uid(_: &UnixStream) -> Result<u32> {
    anyhow::bail!("IPC_UNTRUSTED: peer credentials are unsupported on this platform")
}

pub(super) fn equal_token(left: &str, right: &str) -> bool {
    if left.len() != TOKEN_LEN || right.len() != TOKEN_LEN {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.bytes().zip(right.bytes()) {
        difference |= std::hint::black_box(a ^ b);
    }
    std::hint::black_box(difference) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn reject_symlink_token_wrong_modes_and_foreign_directory() {
        let dir = std::env::temp_dir().join(format!("ipc-trust-{}", capopen_engine::edit::new_id()));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        directory(&dir).unwrap();
        let secret = dir.join("secret");
        fs::write(&secret, "PRIVATE KEY MUST NEVER BE SENT").unwrap();
        let path = dir.join("token");
        symlink(&secret, &path).unwrap();
        assert!(token(&path).unwrap_err().to_string().starts_with("IPC_UNTRUSTED"));
        assert!(create_token(&path).is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, "a".repeat(TOKEN_LEN)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(token(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(token(&path).unwrap().len(), TOKEN_LEN);
        for text in ["a".repeat(TOKEN_LEN - 1), "g".repeat(TOKEN_LEN), "a".repeat(TOKEN_LEN + 1)] {
            fs::write(&path, text).unwrap();
            assert!(token(&path).is_err());
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(directory(&dir).is_err());
        let link = dir.with_extension("link");
        symlink(&dir, &link).unwrap();
        assert!(directory(&link).is_err());
        fs::remove_file(link).unwrap();
        // / is foreign-owned for unprivileged test runs. Root can test a foreign uid directly.
        if uid() != 0 {
            assert!(directory(Path::new("/")).is_err());
        }
        assert!(verify_uid(uid().wrapping_add(1)).is_err());
        let (a, _) = UnixStream::pair().unwrap();
        peer(&a).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn token_comparison_checks_every_position_and_length() {
        let token = "a".repeat(TOKEN_LEN);
        assert!(equal_token(&token, &token));
        for i in 0..TOKEN_LEN {
            let mut other = token.clone().into_bytes();
            other[i] = b'b';
            assert!(!equal_token(&token, std::str::from_utf8(&other).unwrap()));
        }
        assert!(!equal_token(&token, &token[..TOKEN_LEN - 1]));
        assert!(!equal_token("", ""));
    }
}
