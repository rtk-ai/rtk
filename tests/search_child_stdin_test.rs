//! What the search engine's stdin is wired to, in both directions.
//!
//! Withholding it matters on Windows: an MSYS engine can lose its operands to
//! Cygwin's `build_argv`, fall back to reading stdin, and block forever on a pipe
//! the parent never closes, outliving rtk (#4102). Passing it matters everywhere:
//! `grep -f -` reads its *pattern list* from stdin while still searching a file,
//! and `/dev/stdin` is an operand that names it.
//!
//! The mis-parse is Windows-only; "the engine reads stdin" is not. A stub engine
//! that ignores its argv and reports how many bytes it read reproduces both
//! directions on any platform, and fails when either one regresses.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Stub {
    dir: tempfile::TempDir,
}

impl Stub {
    /// A stand-in for an engine whose argv was mangled: it ignores its arguments,
    /// records that it ran, and reads stdin to EOF reporting the byte count.
    fn install(name: &str) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        let script = format!(
            "#!/bin/sh\n: > '{invoked}'\nwc -c > '{bytes}'\n",
            invoked = root.join("invoked").display(),
            bytes = root.join("bytes").display(),
        );
        let path = root.join(name);
        std::fs::write(&path, script).expect("write stub");
        let mut perms = std::fs::metadata(&path).expect("stat stub").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod stub");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn was_invoked(&self) -> bool {
        self.path().join("invoked").exists()
    }

    /// Bytes the stub read from stdin, or `None` if it never got that far.
    fn bytes_read(&self) -> Option<u64> {
        let raw = std::fs::read_to_string(self.path().join("bytes")).ok()?;
        raw.trim().parse().ok()
    }

    fn path_env(&self) -> std::ffi::OsString {
        let mut p = self.path().as_os_str().to_os_string();
        p.push(":");
        p.push(std::env::var_os("PATH").unwrap_or_default());
        p
    }
}

/// Wait for `child`, failing rather than wedging the suite if it never exits.
fn wait_with_deadline(child: &mut Child, what: &str) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => return status.code().unwrap_or(-1),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "{what}: rtk never exited — the engine is blocked on a stdin the \
                     parent never closes, which is the #4102 hang"
                );
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn spawn(stub: &Stub, args: &[&str], db: &PathBuf) -> Child {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .current_dir(stub.path())
        .env("PATH", stub.path_env())
        .env("RTK_DB_PATH", db)
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rtk")
}

/// The invocation names its own inputs: the engine must get no stdin, so holding
/// rtk's stdin open and never writing to it must not stop it exiting.
fn assert_engine_gets_no_stdin(args: &[&str], what: &str) {
    let stub = Stub::install("grep");
    std::fs::write(stub.path().join("q.txt"), "hello\n").expect("fixture");
    let db = tempfile::tempdir().expect("db dir");
    let db_path = db.path().join("rtk.db");

    let mut child = spawn(&stub, args, &db_path);
    // Held open and never written: a parent that keeps stdin open is what turns an
    // engine reading stdin into a hang rather than an EOF.
    let held = child.stdin.take();
    let code = wait_with_deadline(&mut child, what);
    drop(held);

    assert!(
        stub.was_invoked(),
        "{what}: the stub engine never ran — PATH did not take effect, so this test proves nothing"
    );
    assert_eq!(
        stub.bytes_read(),
        Some(0),
        "{what}: the engine was handed rtk's stdin and read from it"
    );
    assert!(
        (0..=2).contains(&code),
        "{what}: expected a grep-ish exit code, got {code}"
    );
}

/// The invocation names stdin — as an operand, or as a flag's value — so the
/// engine must receive it, and receive the bytes written.
fn assert_engine_gets_stdin(args: &[&str], what: &str) {
    let stub = Stub::install("grep");
    std::fs::write(stub.path().join("q.txt"), "hello\n").expect("fixture");
    let db = tempfile::tempdir().expect("db dir");
    let db_path = db.path().join("rtk.db");

    let mut child = spawn(&stub, args, &db_path);
    {
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(b"hello\n").expect("write stdin");
    } // dropped: EOF reaches the engine

    let code = wait_with_deadline(&mut child, what);
    assert!(
        stub.was_invoked(),
        "{what}: the stub engine never ran — PATH did not take effect, so this test proves nothing"
    );
    assert_eq!(
        stub.bytes_read(),
        Some(6),
        "{what}: the engine did not receive the 6 bytes written to rtk's stdin"
    );
    assert!(
        (0..=2).contains(&code),
        "{what}: expected a grep-ish exit code, got {code}"
    );
}

#[test]
fn a_file_operand_withholds_stdin_on_the_grouping_path() {
    assert_engine_gets_no_stdin(&["grep", "hello", "q.txt"], "grouping path");
}

#[test]
fn a_file_operand_withholds_stdin_on_the_passthrough_path() {
    // `-c` is a format flag, so this routes through passthrough instead.
    assert_engine_gets_no_stdin(&["grep", "-c", "hello", "q.txt"], "passthrough path");
}

#[test]
fn an_explicit_dash_operand_receives_stdin() {
    assert_engine_gets_stdin(&["grep", "hello", "-"], "explicit dash");
}

#[test]
fn a_pattern_file_of_dash_receives_stdin() {
    // The patterns come from stdin while `q.txt` is still the file searched, so
    // the operand alone does not show that stdin is an input.
    assert_engine_gets_stdin(&["grep", "-f", "-", "q.txt"], "-f -");
    assert_engine_gets_stdin(&["grep", "--file=-", "q.txt"], "--file=-");
}

#[test]
fn a_dev_stdin_operand_receives_stdin() {
    assert_engine_gets_stdin(&["grep", "hello", "/dev/stdin"], "/dev/stdin operand");
}

/// Whether the `grep` on PATH accepts `--exclude-from`, asked by trying it.
///
/// Not by reading `--version`: macOS reports "BSD grep, GNU compatible", which
/// contains "GNU" while rejecting the flag, so the vendor string answers the wrong
/// question. A rejected flag produces no output, which would read as "the shape
/// changed" rather than "this grep does not have that flag".
///
/// Exit 1 means "accepted, no match"; only a usage error (2) means unsupported.
fn grep_takes_exclude_from() -> bool {
    Command::new("grep")
        .args(["--exclude-from=/dev/null", "x"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|st| matches!(st.code(), Some(0) | Some(1)))
        .unwrap_or(false)
}

/// Wiring stdin through must not change the *output shape*.
///
/// Whether stdin carries the searched data is a narrower question than whether
/// the engine needs stdin at all: a command searching a named file while reading
/// an exclusion list from stdin still wants the grouped, capped, tee'd form.
/// Keying the router on the stdio plan instead sent those down the streaming
/// path, which folds nothing — a silent doubling of rtk's own output metric.
///
/// Uses the real engine, not the stub, because the shape is what is asserted.
#[test]
fn naming_stdin_in_a_flag_does_not_lose_the_grouped_output_shape() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = tempfile::tempdir().expect("db dir");
    let data = dir.path().join("data.txt");
    let body: String = (1..=60).map(|i| format!("hello {i}\n")).collect();
    std::fs::write(&data, body).expect("fixture");

    let run = |args: &[&str], stdin: Option<&str>| -> usize {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(args)
            .current_dir(dir.path())
            .env("RTK_DB_PATH", db.path().join("rtk.db"))
            .env("LC_ALL", "C")
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn rtk");
        if let Some(text) = stdin {
            let mut handle = child.stdin.take().expect("stdin");
            handle.write_all(text.as_bytes()).expect("write stdin");
        }
        let out = child.wait_with_output().expect("wait");
        String::from_utf8_lossy(&out.stdout).lines().count()
    };

    // Guarding on the engine this drives: a missing grep would make every run
    // produce nothing, and `0 == 0` would pass while asserting nothing at all.
    if !Command::new("grep")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return;
    }

    // The reference: a named file, stdin nowhere in the invocation. The lower
    // bound is what makes "nothing was produced" a failure rather than a pass.
    let grouped = run(&["grep", "hello", "data.txt"], None);
    assert!(
        (3..60).contains(&grouped),
        "reference run produced {grouped} of 60 lines, so this test cannot tell the shapes apart"
    );

    // Same search, with an exclusion list arriving on stdin. GNU-only flag.
    if grep_takes_exclude_from() {
        let with_stdin_flag = run(
            &["grep", "--exclude-from=-", "hello", "data.txt"],
            Some("*.none\n"),
        );
        assert_eq!(
            with_stdin_flag, grouped,
            "naming stdin in a flag changed the output shape"
        );
    } else {
        eprintln!("skipped the --exclude-from leg: this grep does not accept the flag");
    }

    // And with the operand naming stdin, where the data really does arrive there
    // but is still one named stream rather than a pipe rtk should stream.
    let via_device = run(
        &["grep", "hello", "/dev/stdin"],
        Some(&{ (1..=60).map(|i| format!("hello {i}\n")).collect::<String>() }),
    );
    assert_eq!(
        via_device, grouped,
        "a /dev/stdin operand changed the output shape"
    );
}

/// `-f /dev/null` is the idiomatic "no patterns", and `/dev/null` is not a stream,
/// so the guard applies to it. The unit tests cannot assert this — under
/// `cargo test` the harness's own stdin is `/dev/null`, which makes it genuinely
/// this process's stdin — but here rtk's stdin is a pipe, so the two differ.
#[test]
fn a_dev_null_flag_value_is_not_a_stream() {
    assert_engine_gets_no_stdin(&["grep", "-f", "/dev/null", "q.txt"], "-f /dev/null");
    assert_engine_gets_no_stdin(
        &["grep", "--exclude-from=/dev/null", "hello", "q.txt"],
        "--exclude-from=/dev/null",
    );
}
