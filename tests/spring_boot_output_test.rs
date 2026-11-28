//! Preserve reports and diagnostics through the Spring Boot command route (#3771).

mod common;

use std::path::Path;
use std::process::Output;

fn java_output(jar: &str, fixture: &str, exit_code: i32) -> Output {
    let dir = tempfile::tempdir().expect("fixture directory");
    std::fs::write(dir.path().join("output.txt"), fixture).expect("write output fixture");
    write_java_stub(dir.path(), exit_code);

    let mut paths = vec![dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    common::rtk_command()
        .args(["java", "-jar", jar])
        .env("PATH", std::env::join_paths(paths).expect("fixture PATH"))
        .output()
        .expect("run java fixture through RTK")
}

#[cfg(windows)]
fn write_java_stub(dir: &Path, exit_code: i32) {
    std::fs::write(
        dir.join("java.cmd"),
        format!("@echo off\r\ntype \"%~dp0output.txt\"\r\nexit /b {exit_code}\r\n"),
    )
    .expect("write java stub");
}

#[cfg(unix)]
fn write_java_stub(dir: &Path, exit_code: i32) {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("java");
    std::fs::write(
        &path,
        format!("#!/bin/sh\ncat \"$(dirname \"$0\")/output.txt\"\nexit {exit_code}\n"),
    )
    .expect("write java stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("make java stub executable");
}

fn assert_output(output: &Output, expected: &str, exit_code: i32) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "{stdout:?} {stderr:?}"
    );
    assert_eq!(
        stdout.trim_end().lines().collect::<Vec<_>>(),
        expected.trim_end().lines().collect::<Vec<_>>(),
        "stderr={stderr:?}"
    );
}

#[test]
fn spring_boot_inline_fixtures_pass() {
    let output = common::rtk_command()
        .args(["verify", "--filter", "spring-boot", "--require-all"])
        .output()
        .expect("verify Spring Boot fixtures");
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("tests passed"));
}

#[test]
fn unrelated_spring_named_jars_keep_their_complete_reports() {
    let report = "Generating from spec.yaml\nWrote 12 files to ./out\n  model/User.java\n  api/DefaultApi.java\nSummary: 12 created, 0 skipped\nDone in 1.4s\n";
    for jar in [
        "offspring-tool.jar",
        "springboard.jar",
        "wellspring-etl.jar",
    ] {
        assert_output(&java_output(jar, report, 0), report, 0);
    }
}

#[test]
fn batch_report_after_the_old_cap_is_preserved_without_a_banner() {
    let mut report = String::from("2024-01-01 INFO Started BatchApp in 2.1 seconds\n");
    for index in 1..=100 {
        report.push_str(&format!("account {index}: reconciled\n"));
    }
    report.push_str("Final result: mismatches = 3\nids: 90412, 90855, 91003\nBatch complete.\n");
    let fixture = format!("  :: Spring Boot ::  (v3.2.0)\n{report}");
    assert_output(
        &java_output("spring-batch-report.jar", &fixture, 0),
        &report,
        0,
    );
}

#[test]
fn failure_preserves_late_stack_frames_root_cause_and_exit_status() {
    let mut diagnostic = String::from("2024-01-01 ERROR Application run failed\n");
    for index in 1..=100 {
        diagnostic.push_str(&format!("    at demo.Worker.step(Worker.java:{index})\n"));
    }
    diagnostic.push_str("Caused by: java.io.IOException: missing config\n    at demo.Config.load(Config.java:10)\n    ... 12 more\n");
    let fixture = format!("  :: Spring Boot ::  (v3.2.0)\n{diagnostic}");
    // The generic command route bypasses filtering on a nonzero exit, so the
    // complete raw output, including the banner, must remain available.
    assert_output(&java_output("spring-app.jar", &fixture, 7), &fixture, 7);
}
