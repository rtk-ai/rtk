mod common;

use std::fs;
use std::path::Path;
use std::process::Output;

// A realistic package description makes the successful summary smaller than
// its manifest, so the summary assertions do not depend on raw-fallback size.
const CARGO: &str = r#"[package]
name = "demo"
version = "0.1.0"
description = "A polyglot reporting application with a Rust processing engine, a Node.js interface, and a Go service. This manifest describes the engine and its dependencies while the neighboring manifests describe the other components of the same application."

[dependencies]
serde = "1.0"
regex = "1"
"#;

const GO: &str = "module example.com/demo\n\ngo 1.22\n\nrequire github.com/pkg/errors v0.9.1\n";
const BAD_JSON: &str = r#"{"name":"demo","dependencies":{"react":"^18"},}"#;

fn deps(dir: &Path) -> Output {
    common::rtk_command()
        .arg("deps")
        .arg(dir)
        .output()
        .expect("run rtk deps")
}

#[test]
fn malformed_json_keeps_summaries_before_and_after_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("Cargo.toml"), CARGO).expect("write Cargo.toml");
    fs::write(dir.path().join("package.json"), BAD_JSON).expect("write package.json");
    fs::write(dir.path().join("go.mod"), GO).expect("write go.mod");

    let output = deps(dir.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout:?} {stderr:?}");
    assert!(stdout.contains("Rust (Cargo.toml):"), "{stdout:?}");
    assert!(stdout.contains("serde (1.0)"), "{stdout:?}");
    assert!(stdout.contains("Go (go.mod):"), "{stdout:?}");
    assert!(
        stdout.contains("github.com/pkg/errors v0.9.1"),
        "{stdout:?}"
    );
    assert!(stderr.contains("package.json"), "{stderr:?}");
    assert!(stderr.contains("trailing comma"), "{stderr:?}");
}

#[test]
fn unreadable_first_manifest_does_not_block_later_manifests() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Invalid UTF-8 is a deterministic read failure on every platform,
    // unlike chmod-based tests that behave differently under root/Windows.
    fs::write(dir.path().join("Cargo.toml"), [0xff, 0xfe]).expect("write unreadable Cargo.toml");
    fs::write(dir.path().join("requirements.txt"), "requests==2.32.0\n")
        .expect("write requirements.txt");

    let output = deps(dir.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout:?} {stderr:?}");
    assert!(stdout.contains("requests==2.32.0"), "{stdout:?}");
    assert!(stderr.contains("Cargo.toml"), "{stderr:?}");
}

#[test]
fn unreadable_last_manifest_does_not_discard_earlier_summary() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("Cargo.toml"), CARGO).expect("write Cargo.toml");
    fs::write(dir.path().join("go.mod"), [0xff]).expect("write unreadable go.mod");

    let output = deps(dir.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout:?} {stderr:?}");
    assert!(stdout.contains("serde (1.0)"), "{stdout:?}");
    assert!(stderr.contains("go.mod"), "{stderr:?}");
}

#[test]
fn warning_survives_raw_fallback_for_a_tiny_valid_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("package.json"), BAD_JSON).expect("write package.json");
    fs::write(dir.path().join("requirements.txt"), "requests\n").expect("write requirements.txt");

    let output = deps(dir.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout:?} {stderr:?}");
    assert!(stdout.contains("requests"), "{stdout:?}");
    assert!(stderr.contains("package.json"), "{stderr:?}");
}

#[test]
fn only_malformed_manifest_still_returns_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("package.json"), BAD_JSON).expect("write package.json");

    let output = deps(dir.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr:?}");
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
    assert!(stderr.contains("package.json"), "{stderr:?}");
    assert!(stderr.contains("trailing comma"), "{stderr:?}");
}

#[test]
fn all_unreadable_manifests_report_every_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifests = [
        "Cargo.toml",
        "package.json",
        "requirements.txt",
        "pyproject.toml",
        "go.mod",
    ];
    for name in manifests {
        fs::write(dir.path().join(name), [0xff]).expect("write invalid UTF-8");
    }

    let output = deps(dir.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr:?}");
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
    for name in manifests {
        assert!(stderr.contains(name), "missing {name}: {stderr:?}");
    }
}

#[test]
fn valid_manifests_still_produce_summaries_without_warnings() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("Cargo.toml"), CARGO).expect("write Cargo.toml");
    fs::write(dir.path().join("go.mod"), GO).expect("write go.mod");

    let output = deps(dir.path());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout:?} {stderr:?}");
    assert!(stdout.contains("Rust (Cargo.toml):"), "{stdout:?}");
    assert!(stdout.contains("Go (go.mod):"), "{stdout:?}");
    assert!(!stderr.contains("rtk deps: warning:"), "{stderr:?}");
}
