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

#[test]
fn port_zero_is_rejected() {
    let out = Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(["browser", "--port", "0"])
        .env("PATH", "")
        .output()
        .expect("failed to run nebula browser");

    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("fixed port"), "{stderr}");
}
