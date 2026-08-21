//! Small memory probes shared by the daemon and the TUI client. All of them
//! shell out (macOS has no /proc) and only run on the metrics modal's slow
//! poll, never on a hot path.

/// Resident set size of one process, bytes.
pub fn process_rss_bytes(pid: u32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    text.trim().parse::<u64>().ok().map(|kb| kb * 1024)
}

/// Physical memory installed on this machine, bytes.
pub fn system_total_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        String::from_utf8(out.stdout).ok()?.trim().parse().ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    }
}
