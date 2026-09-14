//! End-to-end exit-code contract for `rtk diff`.
//!
//! The in-process tests assert on what `diff_cmd::run` returns. That is not the
//! same claim as "the shell sees it": the value still has to survive `run_cli`
//! and `std::process::exit`. These spawn the real binary and read `$?`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Runs `rtk` with the tracking database redirected into `dir`, so the suite
/// never writes to the developer's real one.
fn rtk_in(dir: &Path, args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .env("RTK_DB_PATH", dir.join("rtk-test.db"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn rtk");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code(),
    )
}

fn rtk_in_with_stdin(dir: &Path, args: &[&str], input: &str) -> (String, Option<i32>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .env("RTK_DB_PATH", dir.join("rtk-test.db"))
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rtk");
    // Tolerate a broken pipe: if rtk exits before draining stdin, the exit-code
    // assertion below is the useful failure, not a panic in the harness.
    let mut pipe = child.stdin.take().expect("stdin");
    match pipe.write_all(input.as_bytes()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => panic!("write stdin: {e}"),
    }
    drop(pipe);
    let out = child.wait_with_output().expect("wait rtk");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code(),
    )
}

fn write(dir: &Path, name: &str, contents: &str) -> String {
    std::fs::write(dir.join(name), contents).expect("write fixture");
    name.to_string()
}

#[test]
fn identical_files_exit_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let a = write(dir.path(), "a.txt", "alpha\nbeta\ngamma\n");
    let b = write(dir.path(), "b.txt", "alpha\nbeta\ngamma\n");

    let (_, code) = rtk_in(dir.path(), &["diff", &a, &b]);

    assert_eq!(code, Some(0), "identical files must exit 0");
}

#[test]
fn differing_files_exit_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let a = write(dir.path(), "a.txt", "alpha\nbeta\ngamma\n");
    let b = write(dir.path(), "b.txt", "alpha\nBETA\ngamma\n");

    let (_, code) = rtk_in(dir.path(), &["diff", &a, &b]);

    assert_eq!(code, Some(1), "differing files must exit 1");
}

/// The never-worse guard picks between rtk's rendering and the classic-diff
/// baseline by token count, so the same verdict reaches the user in two
/// different shapes. The exit code must not vary with that choice — a caller
/// branching on `$?` cannot see which branch was taken.
///
/// Which branch wins turns on the *shape* of the edit, not its size. The
/// classic form amortises one `NcN` header over a run of consecutive changes;
/// the condensed form pays a flat per-line cost under a two-line file header.
/// So a single contiguous run picks classic, and enough scattered one-line
/// changes pick condensed. The `assert_ne!` below pins that both are exercised,
/// so this cannot decay into testing one branch twice.
#[test]
fn exit_code_does_not_depend_on_which_output_the_guard_picks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base: String = (0..400)
        .map(|i| format!("line {i} some representative content here\n"))
        .collect();

    // One contiguous run of 20 changes → classic wins.
    let mut contiguous = base.clone();
    for i in 50..70 {
        contiguous = contiguous.replace(&format!("line {i} some"), &format!("line {i} EDITED"));
    }
    let cont_a = write(dir.path(), "cont_a.txt", &base);
    let cont_b = write(dir.path(), "cont_b.txt", &contiguous);
    let (cont_out, cont_code) = rtk_in(dir.path(), &["diff", &cont_a, &cont_b]);

    // 20 isolated changes, each its own run → condensed wins.
    let mut scattered = base.clone();
    for k in 0..20 {
        let i = k * 13;
        scattered = scattered.replace(&format!("line {i} some"), &format!("line {i} EDITED"));
    }
    let scat_a = write(dir.path(), "scat_a.txt", &base);
    let scat_b = write(dir.path(), "scat_b.txt", &scattered);
    let (scat_out, scat_code) = rtk_in(dir.path(), &["diff", &scat_a, &scat_b]);

    assert_ne!(
        cont_out.contains('→'),
        scat_out.contains('→'),
        "fixtures must exercise both guard branches, got contiguous={cont_out:?} scattered={scat_out:?}"
    );
    assert_eq!(cont_code, Some(1), "classic branch must still exit 1");
    assert_eq!(scat_code, Some(1), "condensed branch must still exit 1");
}

/// `diff - expected` is a routine shell idiom, and the hook rewrites it to
/// `rtk diff - expected`, so `-` has to mean stdin on both operands.
///
/// The dangerous direction is identical input: reading `-` as a path named "-"
/// fails with ENOENT and exits 1 where `diff` exits 0, which silently inverts
/// `cmd | diff - expected && <on-success>`.
#[test]
fn dash_reads_stdin_as_the_first_operand() {
    let dir = tempfile::tempdir().expect("tempdir");
    let b = write(dir.path(), "b.txt", "alpha\nbeta\n");

    let (_, same) = rtk_in_with_stdin(dir.path(), &["diff", "-", &b], "alpha\nbeta\n");
    assert_eq!(same, Some(0), "stdin identical to the file must exit 0");

    let (_, differ) = rtk_in_with_stdin(dir.path(), &["diff", "-", &b], "alpha\nBETA\n");
    assert_eq!(differ, Some(1), "stdin differing from the file must exit 1");
}

#[test]
fn dash_reads_stdin_as_the_second_operand() {
    let dir = tempfile::tempdir().expect("tempdir");
    let a = write(dir.path(), "a.txt", "alpha\nbeta\n");

    let (_, same) = rtk_in_with_stdin(dir.path(), &["diff", &a, "-"], "alpha\nbeta\n");
    assert_eq!(same, Some(0), "file identical to stdin must exit 0");

    let (_, differ) = rtk_in_with_stdin(dir.path(), &["diff", &a, "-"], "alpha\nBETA\n");
    assert_eq!(differ, Some(1), "file differing from stdin must exit 1");
}

/// Stdin can only be drained once. `diff - -` compares the input with itself
/// and exits 0; reading it twice would yield an empty second operand and a
/// spurious difference.
#[test]
fn dash_on_both_operands_compares_stdin_with_itself() {
    let dir = tempfile::tempdir().expect("tempdir");

    let (_, code) = rtk_in_with_stdin(dir.path(), &["diff", "-", "-"], "alpha\nbeta\n");

    assert_eq!(code, Some(0), "`diff - -` must exit 0, as diff does");
}

/// `rtk log -` used to look for a file named "-" and exit 1 with ENOENT.
/// Routing it to the stdin entry point also keeps `rtk log -` and `rtk log`
/// on the same newline handling, so a CRLF log dedups identically either way.
#[test]
fn log_reads_stdin_for_dash() {
    let dir = tempfile::tempdir().expect("tempdir");

    let (piped, code) = rtk_in_with_stdin(dir.path(), &["log", "-"], "ERROR boom\nINFO ok\n");
    let (bare, _) = rtk_in_with_stdin(dir.path(), &["log"], "ERROR boom\nINFO ok\n");

    assert_eq!(
        code,
        Some(0),
        "`rtk log -` must read stdin, not a file named -"
    );
    assert_eq!(piped, bare, "`rtk log -` and `rtk log` must agree");
}
