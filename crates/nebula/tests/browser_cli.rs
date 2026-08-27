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
