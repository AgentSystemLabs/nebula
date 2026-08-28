//! `nebula tunnel` up to the point it would need a network. PATH is scrubbed
//! so the ssh lookup fails deterministically whether or not the machine
//! running these tests has an OpenSSH client, and `NEBULA_DATA_DIR` is
//! redirected because the command records the destination for the TUI's host
//! picker before it connects — into the real data dir otherwise.

use std::process::Command;

fn tunnel(args: &[&str]) -> std::process::Output {
    let data = tempfile::tempdir().expect("temp data dir");
    Command::new(env!("CARGO_BIN_EXE_nebula"))
        .arg("tunnel")
        .args(args)
        .env("PATH", "")
        .env("NEBULA_DATA_DIR", data.path())
        .output()
        .expect("failed to run nebula tunnel")
}

#[test]
fn missing_ssh_explains_the_dependency_and_fails() {
    let out = tunnel(&["user@example.invalid", "--port", "0"]);
    assert!(!out.status.success(), "should exit non-zero without ssh");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ssh not found on PATH"), "{stderr}");
}

/// The port is settled locally, before ssh is spawned, so the forward and the
/// URL are the same number — a taken one has to fail here rather than inside
/// a connection that already installed nebula on the far end.
#[test]
fn a_taken_local_port_fails_before_ssh_is_reached() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = held.local_addr().unwrap().port().to_string();
    let out = tunnel(&["user@example.invalid", "--port", &port]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("local port {port} is not free")),
        "{stderr}"
    );
    assert!(
        !stderr.contains("ssh not found"),
        "never got to ssh: {stderr}"
    );
}

/// The forward is announced before the connection, so a tunnel that hangs on
/// a slow remote still says which ports it is waiting on.
#[test]
fn the_forward_is_printed_before_connecting() {
    let out = tunnel(&["user@example.invalid", "--port", "0"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("connecting to user@example.invalid"),
        "{stdout}"
    );
    assert!(stdout.contains("127.0.0.1:"), "{stdout}");
}
