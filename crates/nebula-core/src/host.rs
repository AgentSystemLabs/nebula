//! Machine identity for the status bar: which host is this nebula running on,
//! and did we get here over ssh?

/// Short machine hostname: gethostname(2), then the `HOSTNAME` env var, then
/// "unknown". Domain suffix is stripped ("mbp.local" -> "mbp").
pub fn hostname() -> String {
    gethostname_syscall()
        .or_else(|| crate::env::non_empty("HOSTNAME"))
        .map(|raw| short_name(&raw).to_string())
        .unwrap_or_else(|| UNKNOWN_HOST.to_string())
}

const UNKNOWN_HOST: &str = "unknown";

/// True when running inside an ssh session — sshd sets these for both
/// `nebula ssh` launches and a manual `ssh host` followed by `nebula`.
pub fn is_remote_session() -> bool {
    is_remote_from(
        std::env::var("SSH_CONNECTION").ok().as_deref(),
        std::env::var("SSH_TTY").ok().as_deref(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WslFlavor {
    Wsl1,
    Wsl2,
}

/// Detect Windows Subsystem for Linux and whether it is backed by the real
/// Linux kernel (WSL2). WSL1 reports "Microsoft" in the kernel strings but
/// not the WSL2-specific "microsoft-standard" marker.
#[cfg(target_os = "linux")]
pub fn wsl_flavor() -> Option<WslFlavor> {
    wsl_flavor_from(
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .as_deref(),
        std::fs::read_to_string("/proc/version").ok().as_deref(),
    )
}

#[cfg(not(target_os = "linux"))]
pub fn wsl_flavor() -> Option<WslFlavor> {
    None
}

fn short_name(full: &str) -> &str {
    full.split('.').next().unwrap_or(full)
}

fn is_remote_from(conn: Option<&str>, tty: Option<&str>) -> bool {
    conn.is_some_and(|v| !v.is_empty()) || tty.is_some_and(|v| !v.is_empty())
}

fn wsl_flavor_from(osrelease: Option<&str>, version: Option<&str>) -> Option<WslFlavor> {
    let osrelease = osrelease.unwrap_or_default().to_ascii_lowercase();
    let version = version.unwrap_or_default().to_ascii_lowercase();
    let text = format!("{osrelease}\n{version}");
    if !text.contains("microsoft") {
        return None;
    }
    if text.contains("wsl2") || text.contains("microsoft-standard") {
        Some(WslFlavor::Wsl2)
    } else {
        Some(WslFlavor::Wsl1)
    }
}

// Avoid a libc dependency in this dep-light crate for one call.
fn gethostname_syscall() -> Option<String> {
    extern "C" {
        fn gethostname(name: *mut core::ffi::c_char, len: usize) -> i32;
    }
    let mut buf = [0u8; 256];
    let rc = unsafe { gethostname(buf.as_mut_ptr() as *mut core::ffi::c_char, buf.len() - 1) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).into_owned();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_name_strips_domain() {
        assert_eq!(short_name("mbp.local"), "mbp");
        assert_eq!(short_name("web01.internal.example.com"), "web01");
        assert_eq!(short_name("plain"), "plain");
        assert_eq!(short_name(""), "");
    }

    #[test]
    fn remote_detection_requires_non_empty() {
        assert!(!is_remote_from(None, None));
        assert!(!is_remote_from(Some(""), Some("")));
        assert!(is_remote_from(Some("1.2.3.4 50000 5.6.7.8 22"), None));
        assert!(is_remote_from(None, Some("/dev/pts/3")));
    }

    #[test]
    fn wsl_detection_classifies_kernel_strings() {
        assert_eq!(
            wsl_flavor_from(Some("5.15.167.4-microsoft-standard-WSL2"), None),
            Some(WslFlavor::Wsl2)
        );
        assert_eq!(
            wsl_flavor_from(
                Some("4.4.0-19041-Microsoft"),
                Some("Linux version 4.4.0-19041-Microsoft")
            ),
            Some(WslFlavor::Wsl1)
        );
        assert_eq!(wsl_flavor_from(Some("6.12.94+"), None), None);
    }

    #[test]
    fn hostname_is_non_empty() {
        assert!(!hostname().is_empty());
    }
}
