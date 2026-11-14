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

/// `epoch_secs` on this machine's local wall clock, as `HH:MM` — when a
/// plan limit resets. Falls back to UTC if the C library cannot convert it.
pub fn local_hh_mm(epoch_secs: i64) -> String {
    let t = epoch_secs as libc::time_t;
    // SAFETY: `localtime_r` reads only `t` and writes only the `tm` it is
    // handed; an all-zero `tm` is a valid value of that plain C struct.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if converted {
        return format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    }
    let day = epoch_secs.rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3600, day % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_hh_mm_is_a_clock_reading() {
        let s = local_hh_mm(1_738_425_600);
        assert_eq!(s.len(), 5, "{s}");
        assert_eq!(&s[2..3], ":");
        let h: u32 = s[..2].parse().unwrap();
        let m: u32 = s[3..].parse().unwrap();
        assert!(h < 24, "{s}");
        // 1738425600 is on the hour; every zone offset is a multiple of
        // a quarter hour.
        assert!(m % 15 == 0, "{s}");
    }
}
