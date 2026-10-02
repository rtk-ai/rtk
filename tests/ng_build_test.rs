//! Process-level contracts for the native Angular build adapter.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

mod common;

const TABLE: &str = "Initial chunk files | Names         | Raw size  | Estimated transfer size\nmain-ABC123.js      | main          | 150.00 kB |                42.00 kB\n                   | Initial total | 150.00 kB |                42.00 kB\n";
const WORKSPACE: &str =
    r#"{"projects":{"app":{"architect":{"build":{"builder":"@angular/build:application"}}}}}"#;

fn fake_ng(dir: &Path) {
    fs::write(dir.join("angular.json"), WORKSPACE).expect("write finite workspace");
    let path = dir.join("ng");
    fs::write(
        &path,
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$NG_ARGV\"\ncat \"$NG_STDOUT\"\ncat \"$NG_STDERR\" >&2\nexit \"$NG_EXIT\"\n",
    )
    .expect("write fake ng");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod fake ng");
}

fn ng_command(dir: &Path, args: &[&str], stdout: &str, stderr: &str, code: i32) -> Command {
    fake_ng(dir);
    fs::write(dir.join("stdout"), stdout).expect("write stdout");
    fs::write(dir.join("stderr"), stderr).expect("write stderr");
    let path = std::env::join_paths(std::iter::once(dir.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("join PATH");
    let mut cmd = common::rtk_command();
    cmd.arg("ng")
        .args(args)
        .env("PATH", path)
        .env("NG_ARGV", dir.join("argv"))
        .env("NG_STDOUT", dir.join("stdout"))
        .env("NG_STDERR", dir.join("stderr"))
        .env("NG_EXIT", code.to_string())
        .env("RTK_TEE", "0")
        .current_dir(dir);
    cmd
}

fn run_ng(dir: &Path, args: &[&str], stdout: &str, stderr: &str, code: i32) -> Output {
    ng_command(dir, args, stdout, stderr, code)
        .output()
        .expect("run rtk ng")
}

#[test]
fn native_build_compacts_only_table_padding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run_ng(dir.path(), &["build"], TABLE, "", 0);
    assert!(out.status.success());
    let shown = String::from_utf8(out.stdout).expect("UTF-8 stdout");
    assert!(
        shown.contains("main-ABC123.js | main | 150.00 kB | 42.00 kB"),
        "{shown}"
    );
    assert!(shown.len() < TABLE.len());
}

#[test]
fn native_build_preserves_argv_and_exact_stderr() {
    let dir = tempfile::Builder::new()
        .prefix("angular path with spaces ")
        .tempdir()
        .expect("tempdir");
    let args = [
        "build",
        "my app",
        "--output-path",
        "dist folder",
        "--define",
        "LABEL='a b'",
        "--",
        "--watch",
    ];
    let diagnostics = "▲ [WARNING] bundle budget exceeded\n\n    src/app.ts:2:3\n      2 │ thing\n        ╵ ~~~~~\n";
    let out = run_ng(dir.path(), &args, TABLE, diagnostics, 17);
    assert_eq!(out.status.code(), Some(17));
    assert_eq!(out.stderr, diagnostics.as_bytes());
    let captured = fs::read(dir.path().join("argv")).expect("read argv");
    let expected: Vec<u8> = args.iter().flat_map(|arg| arg.bytes().chain([0])).collect();
    assert_eq!(captured, expected);
}

#[test]
fn successful_build_preserves_every_warning_even_above_the_shared_stderr_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let warnings = (0..100).map(|i| format!("▲ [WARNING] budget {i} exceeded: preserve this full diagnostic and source context\n")).collect::<String>();
    let out = run_ng(dir.path(), &["build"], TABLE, &warnings, 0);
    assert!(out.status.success());
    assert_eq!(out.stderr, warnings.as_bytes());
}

#[test]
fn unsupported_modes_and_explicit_detail_preserve_both_streams() {
    let dir = tempfile::tempdir().expect("tempdir");
    for args in [
        vec!["serve"],
        vec!["test"],
        vec!["b"],
        vec!["--help"],
        vec!["build", "--help"],
        vec!["build", "--verbose"],
        vec!["build", "--watch"],
        vec!["build", "--watch=false"],
        vec!["build", "--no-watch"],
        vec!["build", "--no-verbose"],
        vec!["build", "--json-help"],
        vec!["build", "-w"],
    ] {
        let out = run_ng(dir.path(), &args, TABLE, "unknown diagnostic\n", 3);
        assert_eq!(out.status.code(), Some(3), "{args:?}");
        assert_eq!(out.stdout, TABLE.as_bytes(), "{args:?}");
        assert_eq!(out.stderr, b"unknown diagnostic\n", "{args:?}");
    }
}

#[test]
fn unknown_empty_and_unterminated_output_stays_byte_identical() {
    let dir = tempfile::tempdir().expect("tempdir");
    for raw in [
        "",
        "\n\n",
        "custom builder output without final newline",
        "Initial chunk files | Names | Raw size\ninvalid | row | huge\n",
    ] {
        let out = run_ng(dir.path(), &["build"], raw, "", 0);
        assert_eq!(out.stdout, raw.as_bytes());
        assert!(out.stderr.is_empty());
    }
    let out = run_ng(dir.path(), &["build"], "", "stderr-only failure", 29);
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, b"stderr-only failure");
    assert_eq!(out.status.code(), Some(29));
}

#[test]
fn watch_and_serve_emit_before_the_child_exits() {
    use std::io::{BufRead, BufReader, Read};
    use std::sync::mpsc;
    use std::time::Duration;
    for args in [
        vec!["build", "--watch"],
        vec!["build", "--output-path", "--watch"],
        vec!["serve"],
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let release = dir.path().join("release");
        let mut cmd = ng_command(dir.path(), &args, "", "", 0);
        fs::write(dir.path().join("ng"), "#!/bin/sh\nprintf 'first visible line\\n'\nwhile [ ! -f \"$NG_RELEASE\" ]; do sleep 0.05; done\nprintf 'last visible line\\n'\n").expect("write live fake ng");
        let mut child = cmd
            .env("NG_RELEASE", &release)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn live ng");
        let stdout = child.stdout.take().expect("stdout pipe");
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut input = BufReader::new(stdout);
            let mut line = String::new();
            input.read_line(&mut line).expect("read live output");
            tx.send(line).expect("send first line");
            let mut tail = String::new();
            input.read_to_string(&mut tail).expect("drain live output");
        });
        let first = rx.recv_timeout(Duration::from_secs(5));
        fs::write(&release, "release").expect("release child even on regression");
        let status = child.wait().expect("wait live ng");
        reader.join().expect("reader thread");
        assert_eq!(
            first.expect("output must arrive before build completion"),
            "first visible line\n",
            "{args:?}"
        );
        assert!(status.success(), "{status}");
    }
}

#[test]
fn real_budget_failure_is_fully_recoverable_and_diagnostics_stay_on_stderr() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stdout = include_str!("fixtures/ng_build/budget-error.stdout.txt");
    let stderr = include_str!("fixtures/ng_build/budget-error.stderr.txt");
    let out = ng_command(dir.path(), &["build"], stdout, stderr, 1)
        .env_remove("RTK_TEE")
        .output()
        .expect("run with recovery");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stderr, stderr.as_bytes());
    let shown = String::from_utf8(out.stdout).expect("stdout UTF-8");
    let hash = shown
        .split("[full output: rtk recall ")
        .nth(1)
        .expect("recovery hint")
        .split(']')
        .next()
        .expect("hash");
    let recalled = common::rtk_command()
        .args(["recall", hash])
        .output()
        .expect("recall raw build");
    assert!(recalled.status.success());
    assert_eq!(recalled.stdout, format!("{stdout}{stderr}").as_bytes());
}

#[test]
fn tracking_counts_preserved_stderr_and_actual_filtered_stdout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_dir = dir.path().join("data");
    fs::create_dir(&db_dir).expect("create isolated DB directory");
    let db = db_dir.join("history.db");
    let stderr = "▲ [WARNING] preserve every budget diagnostic\n";
    let out = ng_command(dir.path(), &["build"], TABLE, stderr, 0)
        .env("RTK_DB_PATH", &db)
        .output()
        .expect("run tracked build");
    assert!(out.status.success());
    let conn = rusqlite::Connection::open(db).expect("open tracking DB");
    let (input, output): (usize, usize) = conn
        .query_row(
            "SELECT input_tokens, output_tokens FROM commands ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("tracked ng build");
    assert_eq!(input, (TABLE.len() + stderr.len()).div_ceil(4));
    assert_eq!(output, (out.stdout.len() + out.stderr.len()).div_ceil(4));
}

#[test]
fn configured_watch_and_uncertain_workspaces_pass_through() {
    let dir = tempfile::tempdir().expect("tempdir");
    for workspace in [
        Some(WORKSPACE.replace("\"builder\"", "\"options\":{\"watch\":true},\"builder\"")),
        Some(WORKSPACE.replace(
            "\"builder\"",
            "\"configurations\":{\"live\":{\"watch\":true}},\"builder\"",
        )),
        Some(format!("// JSONC: intentionally not guessed\n{WORKSPACE}")),
        Some(WORKSPACE.replace("@angular/build:application", "custom:potentially-live")),
        Some("{ malformed configuration".to_string()),
        None,
    ] {
        let mut cmd = ng_command(dir.path(), &["build"], TABLE, "keep diagnostics", 0);
        match workspace {
            Some(ref content) => {
                fs::write(dir.path().join("angular.json"), content).expect("write workspace")
            }
            None => fs::remove_file(dir.path().join("angular.json")).expect("remove workspace"),
        }
        let out = cmd.output().expect("run uncertain workspace");
        assert_eq!(out.stdout, TABLE.as_bytes(), "{workspace:?}");
        assert_eq!(out.stderr, b"keep diagnostics");
    }
}

#[test]
fn known_workspace_is_found_from_a_child_directory_and_hidden_filename() {
    let dir = tempfile::tempdir().expect("tempdir");
    let nested = dir.path().join("src");
    fs::create_dir(&nested).expect("create nested directory");
    let mut cmd = ng_command(dir.path(), &["build"], TABLE, "", 0);
    fs::rename(
        dir.path().join("angular.json"),
        dir.path().join(".angular.json"),
    )
    .expect("hidden workspace");
    let out = cmd.current_dir(nested).output().expect("run nested build");
    assert!(out.status.success());
    assert!(out.stdout.len() < TABLE.len());
}

#[test]
fn signalled_build_flushes_both_streams_and_does_not_leave_its_child_running() {
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("ready");
    let pid_file = dir.path().join("pid");
    let mut cmd = ng_command(dir.path(), &["build"], "", "", 0);
    fs::write(dir.path().join("ng"), "#!/bin/sh\nprintf 'captured stdout before cancellation\\n'\nprintf 'important stderr before cancellation' >&2\nprintf '%s' \"$$\" > \"$NG_PID\"\ntouch \"$NG_READY\"\nexec sleep 300\n").expect("write cancellable ng");
    let mut child = cmd
        .env("NG_READY", &marker)
        .env("NG_PID", &pid_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cancellable build");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(marker.exists(), "ng did not start");
    let ng_pid = fs::read_to_string(pid_file).expect("read ng PID");
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .expect("signal rtk")
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().expect("poll rtk").is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ng_alive = Command::new("kill")
        .args(["-0", &ng_pid])
        .stderr(Stdio::null())
        .status()
        .expect("check ng PID")
        .success();
    if ng_alive {
        let _ = Command::new("kill").args(["-KILL", &ng_pid]).status();
    }
    if child.try_wait().expect("check rtk").is_none() {
        child.kill().expect("cleanup stalled rtk");
    }
    let out = child.wait_with_output().expect("collect signalled build");
    assert!(!ng_alive, "cancelled ng process survived rtk");
    assert_eq!(out.stdout, b"captured stdout before cancellation\n");
    assert_eq!(out.stderr, b"important stderr before cancellation");
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(out.status.signal(), Some(15));
}
