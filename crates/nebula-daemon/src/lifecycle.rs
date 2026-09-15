//! Daemon lifecycle: pidfile + flock liveness, socket path hygiene,
//! auto-spawn from the client side.

use anyhow::{Context, Result};
use nebula_core::paths;
use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How often a running daemon re-asserts its pidfile and buildstamp (see
/// [`PidfileLock::refresh`]). Far inside the three days macOS's cleaner
/// waits, and quick to repair a file something else deleted.
pub const RUNTIME_FILE_REFRESH: Duration = Duration::from_secs(60 * 60);

/// Guard holding the exclusive pidfile flock for the daemon's lifetime.
/// Lock possession — not file existence — is the liveness test.
pub struct PidfileLock {
    file: std::fs::File,
    path: PathBuf,
}

impl PidfileLock {
    /// Try to acquire the daemon lock. Returns None when another live daemon
    /// holds it.
    pub fn try_acquire() -> Result<Option<Self>> {
        ensure_runtime_dir()?;
        Self::acquire_at(&paths::pidfile_path())
    }

    fn acquire_at(path: &Path) -> Result<Option<Self>> {
        // No truncate: the flock decides ownership, content is informational.
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("open pidfile {}", path.display()))?;
        let ret = libc_flock(file.as_raw_fd());
        if ret != 0 {
            return Ok(None);
        }
        // Informational only; liveness is the lock.
        let _ = fs::write(path, format!("{}\n", std::process::id()));
        Ok(Some(Self {
            file,
            path: path.to_path_buf(),
        }))
    }

    /// Keep the pidfile where clients look for it for as long as the daemon
    /// runs. The default runtime dir is under /tmp, and macOS's tmp_cleaner
    /// deletes regular files there untouched for three days — sparing the
    /// socket beside them — so a daemon left up over a long weekend would
    /// lose the file every liveness check and `nebula kill` starts from
    /// (#68). A file still in place is touched so the cleaner passes it by;
    /// one already gone or replaced is re-created and locked again.
    pub fn refresh(&mut self) {
        if self.still_at_path() {
            let _ = self.file.set_modified(SystemTime::now());
            return;
        }
        // `None`: another daemon has taken the path since, and owns it now.
        if let Ok(Some(fresh)) = Self::acquire_at(&self.path) {
            *self = fresh;
        }
    }

    /// Whether the file at the pidfile path is the one this guard locked.
    fn still_at_path(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(&self.path), self.file.metadata()) {
            (Ok(on_disk), Ok(held)) => on_disk.dev() == held.dev() && on_disk.ino() == held.ino(),
            _ => false,
        }
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
/// stamp as "unknown build", which reads as stale). Returns the stamp for
/// [`rewrite_buildstamp`].
pub fn write_buildstamp() -> Option<String> {
    let stamp = exe_buildstamp()?;
    rewrite_buildstamp(&stamp);
    Some(stamp)
}

/// Put the startup stamp back, on the pidfile's refresh tick and for the same
/// cleaner. Never re-hashed: after an upgrade the file at `current_exe` is
/// the new build, and stamping that would pass a stale daemon off as fresh.
pub fn rewrite_buildstamp(stamp: &str) {
    let _ = fs::write(paths::buildstamp_path(), stamp);
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

    // #68: once the cleaner has deleted the pidfile, a client finds no
    // daemon at all. The refresh has to bring back a file that is both
    // readable and locked, or liveness checks still read "none running".
    #[test]
    fn refresh_recreates_and_relocks_a_deleted_pidfile() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("daemon.pid");
        let mut lock = PidfileLock::acquire_at(&path)
            .unwrap()
            .expect("lock is free");
        fs::remove_file(&path).unwrap();

        lock.refresh();

        assert_eq!(
            fs::read_to_string(&path).unwrap().trim(),
            std::process::id().to_string()
        );
        assert!(
            PidfileLock::acquire_at(&path).unwrap().is_none(),
            "the re-created pidfile must be locked"
        );
    }

    #[test]
    fn refresh_touches_the_pidfile_it_still_holds() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("daemon.pid");
        let mut lock = PidfileLock::acquire_at(&path)
            .unwrap()
            .expect("lock is free");
        // Aged past the cleaner's three days.
        let week_ago = SystemTime::now() - Duration::from_secs(7 * 24 * 60 * 60);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(week_ago)
            .unwrap();
        let inode = fs::metadata(&path).unwrap().ino();

        lock.refresh();

        let meta = fs::metadata(&path).unwrap();
        assert!(meta.modified().unwrap() > week_ago + Duration::from_secs(24 * 60 * 60));
        assert_eq!(meta.ino(), inode, "touched in place, not re-created");
        assert!(PidfileLock::acquire_at(&path).unwrap().is_none());
    }

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
