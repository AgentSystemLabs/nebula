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
