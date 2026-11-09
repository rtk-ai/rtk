use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn command(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    cmd.current_dir(dir)
        .env("RTK_TEE", "1")
        .env("RTK_TEE_DIR", dir.join("logs with 'quotes'"))
        .env("RTK_DB_PATH", dir.join("history.db"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env("RTK_NO_TOML", "1");
    cmd
}

fn pipe(dir: &Path, input: &str, tee: bool) -> Output {
    let mut child = command(dir)
        .args(["pipe", "-f", "ya-test"])
        .env("RTK_TEE", if tee { "1" } else { "0" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write input");
    child.wait_with_output().expect("wait for rtk")
}

fn saved_log(dir: &Path) -> PathBuf {
    let logs: Vec<_> = fs::read_dir(dir.join("logs with 'quotes'"))
        .expect("tee directory")
        .map(|entry| entry.expect("tee entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
        .collect();
    assert_eq!(
        logs.len(),
        1,
        "one full raw capture, not a second command run"
    );
    logs[0].clone()
}

#[test]
fn pipe_locations_refer_to_verbatim_stdin_with_duplicate_failures() {
    let dir = tempfile::tempdir().expect("tempdir");
    let raw = format!("warning\r\n\r\n\x1b[31m[fail] repeated\x1b[0m\r\nfile.py:5: error\r\nE   Expected: {}\r\n[fail] repeated", "x".repeat(10_000));
    let result = pipe(dir.path(), &raw, true);
    assert!(result.status.success(), "{:?}", result);
    let output = String::from_utf8(result.stdout).expect("utf8");
    assert!(output.contains("  3: [fail] repeated"), "{output}");
    assert!(output.contains("  6: [fail] repeated"), "{output}");
    assert_eq!(
        fs::read_to_string(saved_log(dir.path())).expect("saved log"),
        raw
    );
    assert!(
        output.len() * 5 < raw.len(),
        "recovery must preserve useful compression"
    );
}

#[test]
fn pipe_never_links_failures_beyond_the_saved_prefix() {
    let dir = tempfile::tempdir().expect("tempdir");
    let raw = format!(
        "[fail] saved\nE   Expected: {}\n[fail] omitted\n",
        "漢".repeat(400_000)
    );
    let result = pipe(dir.path(), &raw, true);
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).expect("utf8");
    assert!(output.contains("[partial output:"), "{output}");
    let index = output
        .split("[fail locations in saved output]")
        .nth(1)
        .expect("index");
    assert!(index.contains("  1: [fail] saved"));
    assert!(!index.contains("omitted"));
    let saved = fs::read_to_string(saved_log(dir.path())).expect("valid UTF-8 log");
    assert!(!saved.contains("[fail] omitted"));
}

#[test]
fn disabled_tee_has_no_source_locations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let raw = format!("[fail] failure\nE   Expected: {}\n", "x".repeat(10_000));
    let result = pipe(dir.path(), &raw, false);
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).expect("utf8");
    assert!(!output.contains("locations in saved output"));
    assert!(!dir.path().join("logs with 'quotes'").exists());
}

#[test]
fn small_pipe_output_stays_within_the_original_budget() {
    let dir = tempfile::tempdir().expect("tempdir");
    let raw = "[fail] tiny\nerror: failed\n";
    let result = pipe(dir.path(), raw, true);
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).expect("utf8"), raw);
}

#[test]
fn failed_capture_does_not_emit_a_broken_location() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("logs with 'quotes'"), "not a directory")
        .expect("block tee directory");
    let raw = format!("[fail] kept\nE   Expected: {}\n", "x".repeat(3000));
    let result = pipe(dir.path(), &raw, true);
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).expect("utf8");
    assert!(output.contains("[fail] kept"));
    assert!(!output.contains("locations in saved output"));
    assert!(!output.contains("[saved output:"));
}

#[cfg(unix)]
#[test]
fn short_live_output_does_not_expand_to_a_large_recovery_index() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let raw = format!("[fail] short\nerror: {}\n", "x".repeat(500));
    fs::write(dir.path().join("input"), &raw).expect("raw input");
    let ya = dir.path().join("ya");
    fs::write(&ya, "#!/bin/sh\ncat input\nexit 7\n").expect("fake ya");
    fs::set_permissions(&ya, fs::Permissions::from_mode(0o755)).expect("chmod");
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(&inherited));
    let result = command(dir.path())
        .args(["ya", "test", "fake-target"])
        .env("PATH", std::env::join_paths(paths).expect("PATH"))
        .output()
        .expect("run rtk ya");
    assert_eq!(result.status.code(), Some(7));
    let shown = result.stdout.len() + result.stderr.len();
    assert!(
        shown.div_ceil(4) <= raw.len().div_ceil(4),
        "{shown} bytes for {} input bytes",
        raw.len()
    );
}

#[cfg(unix)]
#[test]
fn quoted_read_command_opens_the_exact_saved_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let raw = format!(
        "noise\n[fail] selected\nE   Expected: {}\n",
        "x".repeat(3000)
    );
    let result = pipe(dir.path(), &raw, true);
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).expect("utf8");
    let command = output
        .lines()
        .find_map(|line| {
            line.strip_prefix("[read first failure: ")
                .and_then(|line| line.strip_suffix(']'))
        })
        .expect("read command");
    let read = Command::new("sh")
        .args(["-c", command])
        .output()
        .expect("read saved log");
    assert!(read.status.success());
    assert_eq!(
        String::from_utf8(read.stdout).expect("utf8"),
        raw.split_once('\n').expect("prefix").1
    );
}

#[test]
fn overflow_index_has_a_real_next_location() {
    let dir = tempfile::tempdir().expect("tempdir");
    let raw: String = (1..=30)
        .map(|i| format!("[fail] case-{i}\nE   Expected: {}\n", "x".repeat(500)))
        .collect();
    let result = pipe(dir.path(), &raw, true);
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).expect("utf8");
    let index = output
        .split("[fail locations in saved output]\n")
        .nth(1)
        .expect("index");
    let next_line: usize = index
        .lines()
        .find_map(|line| {
            line.split_once("next at line ")
                .map(|(_, n)| n.parse().expect("line number"))
        })
        .expect("continuation");
    assert!(raw
        .lines()
        .nth(next_line - 1)
        .expect("real physical line")
        .starts_with("[fail] case-"));
    let indexed = index
        .lines()
        .filter(|line| line.starts_with("  ") && !line.starts_with("  ..."))
        .count();
    assert!(indexed > 0 && indexed < 30);
    assert_eq!(next_line, indexed * 2 + 1);
}

#[cfg(unix)]
#[test]
fn live_locations_follow_saved_stdout_then_stderr_and_preserve_exit_code() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let ya = dir.path().join("ya");
    fs::write(&ya, "#!/bin/sh\nprintf '[fail] stderr-first\\n' >&2\nprintf 'error on stderr\\n' >&2\nsleep 0.05\nprintf '[fail] stdout-second\\n'\nprintf 'E   Expected: '\ni=0\nwhile [ $i -lt 2000 ]; do printf x; i=$((i + 1)); done\nprintf '\\n'\nexit 7\n").expect("write fake ya");
    fs::set_permissions(&ya, fs::Permissions::from_mode(0o755)).expect("chmod fake ya");
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(&inherited));
    let result = command(dir.path())
        .args(["ya", "test", "fake-target"])
        .env("PATH", std::env::join_paths(paths).expect("PATH"))
        .output()
        .expect("run rtk ya");
    assert_eq!(result.status.code(), Some(7));
    let output = String::from_utf8(result.stdout).expect("utf8");
    let saved = fs::read_to_string(saved_log(dir.path())).expect("saved log");
    assert_eq!(saved.lines().next(), Some("[fail] stdout-second"));
    assert_eq!(saved.lines().nth(2), Some("[fail] stderr-first"));
    assert!(output.contains("  1: [fail] stdout-second"), "{output}");
    assert!(output.contains("  3: [fail] stderr-first"), "{output}");
}
