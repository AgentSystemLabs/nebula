//! `nebula sbx` up to the point it would need Docker. PATH is scrubbed so
//! the docker lookup fails the same way whether or not this machine has it.

use std::process::Command;

#[test]
fn missing_docker_explains_the_dependency_and_fails() {
    let out = Command::new(env!("CARGO_BIN_EXE_nebula"))
        .arg("sbx")
        .env("PATH", "")
        .output()
        .expect("failed to run nebula sbx");
    assert!(!out.status.success(), "should exit non-zero without docker");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("docker not found on PATH"), "{stderr}");
}
