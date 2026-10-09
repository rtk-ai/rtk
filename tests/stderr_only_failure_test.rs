//! Stdout-only filters parse structured stdout, but a tool's diagnostics often go to
//! stderr. Dropping that stream leaves a failing command looking silent; inventing a
//! stdout message in its place is just as wrong. These pin both halves, along with the
//! line endings rtk prints around a stub tool's output.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

mod common;

/// Writes an executable `name` in `dir` that runs `body`.
fn fake_tool(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{}\n", body)).expect("write fake tool");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake tool");
}

/// Runs rtk with `dir` first on PATH, so the fake tool shadows any real one.
fn rtk_with(dir: &Path, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    common::rtk_command()
        .args(args)
        .env("PATH", path)
        .output()
        .expect("run rtk")
}

/// What a failed assertion needs to show: both streams and the exit status.
fn context(out: &Output) -> String {
    format!(
        "exit {:?}, stdout {:?}, stderr {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A golangci-lint that answers `--version`, then fails the way a config or build
/// error does: everything on stderr, stdout empty, non-zero exit.
fn fake_golangci(dir: &Path, failure: &str) {
    fake_tool(
        dir,
        "golangci-lint",
        &format!(
            r#"if [ "$1" = "--version" ]; then
  echo "golangci-lint has version 1.64.8"
  exit 0
fi
{failure}"#
        ),
    );
}

#[test]
fn golangci_lint_stderr_only_failure_reaches_the_user() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_golangci(
        dir.path(),
        r#"echo 'level=error msg="context loading failed: failed to load packages"' >&2
exit 3"#,
    );

    let out = rtk_with(dir.path(), &["golangci-lint", "run"]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("context loading failed"),
        "the tool's own error must reach the user; {}",
        context(&out)
    );
    assert_eq!(
        out.status.code(),
        Some(3),
        "exit code must propagate; {}",
        context(&out)
    );
}

/// The command said nothing on stdout, so neither does rtk. A filter's "I got nothing"
/// placeholder is not a diagnostic — the real one is on stderr.
#[test]
fn no_invented_stdout_diagnostic_when_the_tool_used_stderr() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(
        dir.path(),
        "prettier",
        r#"echo "Checking formatting..." >&2
echo "[warn] src/a.js" >&2
echo "Code style issues found in the above file(s)." >&2
exit 1"#,
    );

    let out = rtk_with(dir.path(), &["prettier", "--check", "."]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stdout.trim().is_empty(),
        "prettier ran fine and reported on stderr; stdout must not claim otherwise; {}",
        context(&out)
    );
    assert!(
        stderr.contains("[warn] src/a.js"),
        "prettier's report must reach the user; {}",
        context(&out)
    );
}

/// stderr goes out on its own stream, so it must not also be replayed on stdout.
#[test]
fn stderr_is_not_duplicated_onto_stdout() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(
        dir.path(),
        "ruff",
        r#"echo "stdout line"
echo "stderr diagnostic" >&2
exit 1"#,
    );

    let out = rtk_with(dir.path(), &["ruff", "check", "."]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stdout.contains("stderr diagnostic"),
        "stderr must not be replayed on stdout; {}",
        context(&out)
    );
    assert!(
        stderr.contains("stderr diagnostic"),
        "stderr must still reach the user; {}",
        context(&out)
    );
}

/// A command that genuinely says nothing must stay silent.
#[test]
fn silent_command_stays_silent() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(dir.path(), "ruff", "exit 0");

    let out = rtk_with(dir.path(), &["ruff", "check", "."]);

    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "a silent command must not gain output; {}",
        context(&out)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).trim().is_empty(),
        "a silent command must not gain diagnostics; {}",
        context(&out)
    );
    assert_eq!(out.status.code(), Some(0), "{}", context(&out));
}

/// Not a golangci-lint quirk: every stdout-only filter forwards stderr.
#[test]
fn stderr_forwarding_is_not_golangci_specific() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(
        dir.path(),
        "ruff",
        r#"echo "ruff failed hard" >&2
exit 2"#,
    );

    let out = rtk_with(dir.path(), &["ruff", "check", "."]);

    assert!(
        String::from_utf8_lossy(&out.stderr).contains("ruff failed hard"),
        "stderr must reach the user for any stdout-only filter; {}",
        context(&out)
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "exit code must propagate; {}",
        context(&out)
    );
}

/// golangci-lint exits 1 when it merely found lint issues. RTK reports them and
/// returns 0, so the linter running successfully does not fail a build.
#[test]
fn lint_issues_are_summarised_and_exit_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let issues = r#"{"Issues":[{"FromLinter":"errcheck","Text":"unchecked","Pos":{"Filename":"main.go","Line":1,"Column":1},"SourceLines":["x()"],"Severity":""}]}"#;
    fake_golangci(dir.path(), &format!("echo '{issues}'\nexit 1"));

    let out = rtk_with(dir.path(), &["golangci-lint", "run"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("1 issues in 1 files") && stdout.contains("errcheck"),
        "expected an issue summary; {}",
        context(&out)
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "exit 1 means issues found, which RTK reports without failing; {}",
        context(&out)
    );
}

/// The guard's promise is about what reaches the user, so it has to hold at the printer
/// too: a summary longer than the command's output falls back to that output, and the
/// bytes rtk emits for it must be the tool's own, not one more. `go vet` runs in the
/// tool's own output format, so the fallback is the command's bytes and not a format
/// rtk chose for it.
#[test]
fn guard_fallback_emits_exactly_the_command_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(dir.path(), "go", "printf 'ok\\n'");

    let out = rtk_with(dir.path(), &["go", "vet", "./..."]);

    assert_eq!(out.stdout, b"ok\n", "guard fallback; {}", context(&out));
}

/// A filter that leaves its last line unterminated gets exactly one newline from the
/// printer.
#[test]
fn filtered_report_ends_with_exactly_one_newline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let issues = r#"{"Issues":[{"FromLinter":"errcheck","Text":"unchecked","Pos":{"Filename":"main.go","Line":1,"Column":1},"SourceLines":["x()"],"Severity":""}]}"#;
    fake_golangci(dir.path(), &format!("echo '{issues}'"));

    let out = rtk_with(dir.path(), &["golangci-lint", "run"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("errcheck"),
        "expected a summary; {}",
        context(&out)
    );
    assert!(
        stdout.ends_with('\n'),
        "summary must be terminated; {}",
        context(&out)
    );
    assert!(
        !stdout.ends_with("\n\n"),
        "one terminator, not two; {}",
        context(&out)
    );
}

/// ruff's `[]` is the empty form of the JSON rtk asked ruff for, so the clean-run summary
/// replaces it even though it is longer, and is printed with its one terminator. ruff runs
/// without a tee label: this is the plain print path, not the recovery-hint one.
#[test]
fn injected_clean_output_prints_the_summary_once_terminated() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(dir.path(), "ruff", "echo '[]'");

    let out = rtk_with(dir.path(), &["ruff", "check"]);

    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Ruff: No issues found\n",
        "{}",
        context(&out)
    );
}

/// The same plain print path when the guard falls back: `ruff format`'s own report is
/// shorter than its summary, and it is printed as ruff wrote it, not one newline longer.
#[test]
fn plain_path_fallback_emits_exactly_the_command_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    fake_tool(dir.path(), "ruff", "echo '1 file left unchanged'");

    let out = rtk_with(dir.path(), &["ruff", "format"]);

    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1 file left unchanged\n",
        "{}",
        context(&out)
    );
}
