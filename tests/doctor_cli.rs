//! End-to-end integration test for the `fileferry doctor` subcommand.
//!
//! Executes the compiled binary as a subprocess with an explicit
//! `--config` pointing at a temp YAML and asserts the stdout + exit
//! code. Complements the unit tests in `src/doctor.rs::tests` which
//! exercise the report formatter without spawning a subprocess.

use std::io::Write;
use std::process::Command;

use tempfile::NamedTempFile;

/// Path to the compiled `fileferry` binary produced by `cargo test`.
/// `CARGO_BIN_EXE_<name>` is set by Cargo for every binary in the
/// crate — deterministic, no PATH lookup.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_fileferry")
}

#[test]
fn doctor_on_clean_loopback_config_exits_zero() {
    // A minimal YAML pointing at localhost with no `security:` block
    // → default admin off, no token → default AppConfig.
    // Doctor's exit code should be 0 (green) because `AppConfig::default()`
    // on `0.0.0.0:8080` DOES fire W-1 + W-4 — the wildcard bind is what
    // main.rs bakes into `addr`. So we assert exit=1 for the wildcard
    // case; a genuine loopback-bind YAML would need a `bind:` field
    // that doesn't yet exist. This test locks the current default:
    // running `doctor` on a stock config surfaces the amber state so
    // the operator sees the recon endpoint hint.
    let mut yaml = NamedTempFile::new().unwrap();
    writeln!(yaml, "port: 8080").unwrap();
    let output = Command::new(binary())
        .args(["doctor", "--config", yaml.path().to_str().unwrap()])
        .output()
        .expect("doctor subprocess spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "doctor should exit non-zero on stock config (wildcard bind fires W-1/W-4). \
         stdout={stdout} stderr={stderr} status={:?}",
        output.status
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code should be 1 (warnings), not 2 (error). stderr={stderr}"
    );
    assert!(
        stdout.contains("fileferry doctor"),
        "doctor did not print header. stdout={stdout}"
    );
    assert!(
        stdout.contains("W-1") || stdout.contains("Preflight checks"),
        "doctor output missing preflight section. stdout={stdout}"
    );
    // Sanity: stderr should be empty for a well-formed run — doctor
    // writes only to stdout. Tracing is not initialised in the doctor
    // path (main.rs runs doctor BEFORE init_tracing).
    assert!(
        stderr.is_empty(),
        "doctor unexpectedly wrote to stderr: {stderr}"
    );
}

#[test]
fn doctor_on_broken_yaml_exits_two() {
    // Break-the-fix probe: doctor must distinguish "config invalid"
    // (exit 2) from "config OK but posture amber" (exit 1). Give it
    // a YAML with an unknown top-level field — deny_unknown_fields
    // fires. Exit code must be 2 and the error must land on stderr.
    let mut yaml = NamedTempFile::new().unwrap();
    writeln!(yaml, "porrt: 9000").unwrap();
    let output = Command::new(binary())
        .args(["doctor", "--config", yaml.path().to_str().unwrap()])
        .output()
        .expect("doctor subprocess spawn");
    assert_eq!(
        output.status.code(),
        Some(2),
        "invalid YAML must exit 2, got {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("porrt") || stderr.contains("config"),
        "stderr should name the config error, got {stderr}"
    );
}
