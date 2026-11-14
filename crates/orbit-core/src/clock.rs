//! Wall-clock stamps, in the units the database and the on-disk caches
//! store them in.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch — the stamp every row's `created_at`
/// and `status_changed_at` carries, so the daemon's and the TUI's compare.
/// A clock set before 1970 reads as 0.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Whole seconds since the Unix epoch — the on-disk caches' stamp. A clock
/// set before 1970 reads as 0.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
