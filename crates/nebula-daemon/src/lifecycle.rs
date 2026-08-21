//! Daemon lifecycle: pidfile + flock liveness, socket path hygiene,
//! auto-spawn from the client side.

use anyhow::{Context, Result};
use nebula_core::paths;
use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;

/// Guard holding the exclusive pidfile flock for the daemon's lifetime.
/// Lock possession — not file existence — is the liveness test.
pub struct PidfileLock {
    file: std::fs::File,
}

impl PidfileLock {
    /// Try to acquire the daemon lock. Returns None when another live daemon
    /// holds it.
    pub fn try_acquire() -> Result<Option<Self>> {
        ensure_runtime_dir()?;
        let path = paths::pidfile_path();
        // No truncate: the flock decides ownership, content is informational.
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open pidfile {}", path.display()))?;
        let ret = libc_flock(file.as_raw_fd());
        if ret != 0 {
            return Ok(None);
        }
        // Informational only; liveness is the lock.
        let _ = fs::write(&path, format!("{}\n", std::process::id()));
        Ok(Some(Self { file }))
    }

    pub fn is_daemon_alive() -> bool {
        match Self::try_acquire() {
            // We got the lock: nobody holds it. Release immediately by drop.
            Ok(Some(_guard)) => false,
            Ok(None) => true,
            Err(_) => false,
        }
    }
}

impl Drop for PidfileLock {
    fn drop(&mut self) {
        // flock released automatically on close; keep for explicitness.
        let _ = self.file.as_raw_fd();
    }
}

fn libc_flock(fd: i32) -> i32 {
    extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    unsafe { flock(fd, LOCK_EX | LOCK_NB) }
}

/// Record this process's binary fingerprint so installers can tell whether
/// the running daemon is already on the build they just installed. Called by
/// the daemon at startup; best-effort (staleness checks treat a missing
/// stamp as "unknown build", which reads as stale).
pub fn write_buildstamp() {
    if let Some(stamp) = exe_buildstamp() {
        let _ = fs::write(paths::buildstamp_path(), stamp);
    }
}

/// True when a live daemon is running different code than this binary — or
/// predates buildstamps entirely, so its build is unknown.
pub fn daemon_is_stale() -> bool {
    if !PidfileLock::is_daemon_alive() {
        return false;
    }
    match (
        fs::read_to_string(paths::buildstamp_path()).ok(),
        exe_buildstamp(),
    ) {
        (Some(recorded), Some(current)) => recorded.trim() != current,
        _ => true,
    }
}

/// Content fingerprint of this process's executable.
fn exe_buildstamp() -> Option<String> {
    fingerprint_file(&std::env::current_exe().ok()?)
}

/// FNV-style multiply-xor over 8-byte words, then the length — an identity
/// check, not security, and word-wide because the daemon hashes its own
/// ~30MB debug binary at startup under the e2e tests.
fn fingerprint_file(path: &Path) -> Option<String> {
    const PRIME: u64 = 0x100_0000_01b3;
    let bytes = fs::read(path).ok()?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut words = bytes.chunks_exact(8);
    for word in &mut words {
        hash = (hash ^ u64::from_le_bytes(word.try_into().unwrap())).wrapping_mul(PRIME);
    }
    let mut tail = [0u8; 8];
    tail[..words.remainder().len()].copy_from_slice(words.remainder());
    hash = (hash ^ u64::from_le_bytes(tail)).wrapping_mul(PRIME);
    hash = (hash ^ bytes.len() as u64).wrapping_mul(PRIME);
    Some(format!("{hash:016x}"))
}

/// Create the runtime dir with 0700 perms — this is the auth boundary.
pub fn ensure_runtime_dir() -> Result<()> {
    let dir = paths::runtime_dir();
    if !dir.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("create runtime dir {}", dir.display()))?;
    } else {
        let meta = fs::metadata(&dir)?;
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o700 {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Remove a socket file left behind by a dead daemon.
pub fn unlink_stale_socket(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_for_identical_content() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        // Same bytes at different paths/inodes — the cp+mv install dance.
        fs::write(&a, b"identical build bytes").unwrap();
        fs::write(&b, b"identical build bytes").unwrap();
        assert_eq!(fingerprint_file(&a), fingerprint_file(&b));
    }

    #[test]
    fn different_content_gets_a_different_fingerprint() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f");
        // Lengths off and on the 8-byte word boundary, plus zero-padding
        // ambiguity: "x" vs "x\0" must differ even though the padded tail
        // word is identical.
        let mut seen = std::collections::HashSet::new();
        for content in [&b""[..], b"x", b"x\0", b"12345678", b"123456789"] {
            fs::write(&file, content).unwrap();
            assert!(seen.insert(fingerprint_file(&file).unwrap()));
        }
    }
}
