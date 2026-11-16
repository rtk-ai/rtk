//! Exercise CLI routing and both output streams, not just the Cargo formatter.
#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

fn cargo_fixture(args: &[&str], stdout: &str, stderr: &str, code: i32) -> Output {
    let dir = common::tempdir();
    fs::write(dir.path().join("stdout.txt"), stdout).unwrap();
    fs::write(dir.path().join("stderr.txt"), stderr).unwrap();
    let cargo = dir.path().join("cargo");
    fs::write(
        &cargo,
        format!("#!/bin/sh\n/bin/cat stdout.txt\n/bin/cat stderr.txt >&2\nexit {code}\n"),
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    common::rtk_command()
        .current_dir(dir.path())
        .env("PATH", dir.path())
        .env("RTK_SUPPRESS_HOOK_WARNING", "1")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn nocapture_keeps_the_complete_panic_on_stderr_and_exit_101() {
    let message = format!("PANIC_BEGIN_{}_PANIC_END", "detail_".repeat(100));
    let diagnostic = format!(
        "thread 'named_failure' panicked at src/lib.rs:6:30:\n{message}\nstack backtrace:\n   0: 0x1234 - trace_level_three\n       at src/lib.rs:6:30\n   1: 0x4567 - trace_level_two\n       at src/lib.rs:4:28\n   2: 0x7890 - trace_level_one\n       at src/lib.rs:2:28\nerror: test failed, to rerun pass `--lib`\n"
    );
    let result = cargo_fixture(
        &["cargo", "test", "--offline", "--", "--nocapture"],
        "running 1 test\ntest named_failure ... FAILED\n",
        &format!("   Compiling demo v0.1.0\n{diagnostic}"),
        101,
    );
    assert_eq!(result.status.code(), Some(101));
    let stderr = String::from_utf8(result.stderr).unwrap();
    let stdout = String::from_utf8(result.stdout).unwrap();
    assert!(stderr.contains(&diagnostic), "{stderr}");
    assert!(!stdout.contains("PANIC_BEGIN"), "{stdout}");
    assert!(
        !stderr.contains("   Compiling demo"),
        "still filters progress"
    );
}

#[test]
fn json_build_keeps_each_diagnostic_after_the_old_cap() {
    let diagnostics: Vec<_> = (1..=21)
        .map(|i| {
            format!(
                "error: UNIQUE_{i}_BEGIN_{}_END\n --> src/lib.rs:{i}:1\n",
                "detail_".repeat(70)
            )
        })
        .collect();
    let records = diagnostics.iter().map(|d| {
        serde_json::json!({"reason":"compiler-message","message":{"level":"error","message":d,"rendered":d}}).to_string()
    }).collect::<Vec<_>>().join("\n");
    let result = cargo_fixture(
        &["cargo", "build", "--offline", "--message-format=json"],
        &format!("{records}\n"),
        "error: could not compile `demo` (lib) due to 21 previous errors\n",
        101,
    );
    assert_eq!(result.status.code(), Some(101));
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    for diagnostic in diagnostics {
        assert!(
            output.contains(diagnostic.trim_end()),
            "missing {diagnostic}"
        );
    }
    assert!(output.contains("error: could not compile `demo` (lib) due to 21 previous errors"));
}

#[test]
fn passing_tests_still_compress_progress_and_preserve_counts() {
    let result = cargo_fixture(
        &["cargo", "test", "--offline"],
        "running 1 test\ntest named_success ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n",
        "   Compiling demo v0.1.0\n    Finished test [unoptimized + debuginfo] target(s) in 0.5s\n",
        0,
    );
    assert_eq!(result.status.code(), Some(0));
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.contains("1 passed"), "{output}");
    assert!(!output.contains("Compiling"), "{output}");
    assert!(!output.contains("test named_success ... ok"), "{output}");
}
