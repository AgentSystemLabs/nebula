//! `nebula browser` without its one dependency. ttyd may or may not be
//! installed on the machine running these tests, so PATH is scrubbed to make
//! the missing-ttyd path deterministic either way.

use std::process::Command;

#[test]
fn missing_ttyd_explains_the_dependency_and_fails() {
    let out = Command::new(env!("CARGO_BIN_EXE_nebula"))
        .arg("browser")
        .env("PATH", "")
        .output()
        .expect("failed to run nebula browser");

    assert!(!out.status.success(), "should exit non-zero without ttyd");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("needs ttyd"), "{stderr}");
    assert!(stderr.contains("brew install ttyd"), "{stderr}");
}

/// `--port 0` used to be refused before anything spawned. It now resolves to
/// a free port, so the run gets all the way to looking for ttyd — which is
/// the *only* thing left to fail once PATH is scrubbed.
#[test]
fn port_zero_now_resolves_and_reaches_the_ttyd_lookup() {
    let out = Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(["browser", "--port", "0"])
        .env("PATH", "")
        .output()
        .expect("failed to run nebula browser");

    assert!(!out.status.success(), "no ttyd on a scrubbed PATH");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("needs ttyd"),
        "got past port resolution: {stderr}"
    );
    assert!(
        !stderr.contains("fixed port"),
        "0 is no longer refused: {stderr}"
    );
}

/// `--public` is the flag for a nebula running on a remote box. It has to get
/// all the way past port resolution — which now probes 0.0.0.0 rather than
/// loopback — before ttyd is looked for.
#[test]
fn public_resolves_a_port_on_every_interface_and_reaches_the_ttyd_lookup() {
    let out = Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(["browser", "--public", "--port", "0"])
        .env("PATH", "")
        .output()
        .expect("failed to run nebula browser");

    assert!(!out.status.success(), "no ttyd on a scrubbed PATH");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("needs ttyd"), "{stderr}");
}

/// Serving a writable terminal to the network with no password is a thing to
/// say out loud, and it is said before ttyd is spawned so it survives a run
/// that fails for any other reason.
#[test]
fn a_public_bind_without_a_credential_warns() {
    let out = Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(["browser", "--public", "--port", "0"])
        .env("PATH", "")
        .output()
        .expect("failed to run nebula browser");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("WARNING"), "{stderr}");
    assert!(stderr.contains("--credential"), "{stderr}");
}

/// Loopback is unchanged: no warning, because there is nobody to warn about.
#[test]
fn the_default_loopback_bind_is_silent() {
    let out = Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(["browser", "--port", "0"])
        .env("PATH", "")
        .output()
        .expect("failed to run nebula browser");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("WARNING"), "{stderr}");
}

/// They are two spellings of one setting, so taking both is a mistake worth
/// an error rather than a silent winner.
#[test]
fn bind_and_public_cannot_both_be_given() {
    let out = Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(["browser", "--public", "--bind", "127.0.0.1"])
        .output()
        .expect("failed to run nebula browser");

    assert!(!out.status.success(), "should refuse both");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn a_bind_address_that_is_not_an_ip_is_rejected_by_the_parser() {
    let out = Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(["browser", "--bind", "example.com"])
        .output()
        .expect("failed to run nebula browser");

    assert!(!out.status.success(), "hostnames are not bind addresses");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid value"), "{stderr}");
}
