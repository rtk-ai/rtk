//! End-to-end exit-code contract for `rtk diff`.
//!
//! The in-process tests assert on what `diff_cmd::run` returns. That is not the
//! same claim as "the shell sees it": the value still has to survive `run_cli`
//! and `std::process::exit`. These spawn the real binary and read `$?`.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

mod common;

/// `rtk <args>` run in `dir`, with the child's data redirected away from the
/// developer's own by `common::rtk_command`. The locale is pinned because the
/// unreadable-operand cases assert on the operating system's error text.
fn rtk_cmd(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = common::rtk_command();
    cmd.env("LC_ALL", "C").args(args).current_dir(dir);
    cmd
}

/// Runs `rtk` with a null stdin, so a regression that reads it sees
/// end-of-file instead of hanging on a terminal.
fn rtk_in(dir: &Path, args: &[&str]) -> (String, Option<i32>) {
    let out = rtk_cmd(dir, args)
        .stdin(Stdio::null())
        .output()
        .expect("spawn rtk");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code(),
    )
}

fn rtk_in_with_stdin(dir: &Path, args: &[&str], input: &str) -> (String, Option<i32>) {
    let out = rtk_piped(dir, args, input.as_bytes());
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code(),
    )
}

/// Runs `rtk` with `input` on stdin and returns the whole output, stderr
/// included, for the cases that assert on the error message.
fn rtk_piped(dir: &Path, args: &[&str], input: &[u8]) -> Output {
    let mut child = rtk_cmd(dir, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk");
    // Written from its own thread while this one drains stdout and stderr, so
    // neither side can fill a pipe buffer and block the other.
    let mut pipe = child.stdin.take().expect("stdin");
    let input = input.to_vec();
    let writer = std::thread::spawn(move || write_stdin(&mut pipe, &input));
    let out = child.wait_with_output().expect("wait rtk");
    writer.join().expect("stdin writer");
    out
}

/// Writes `input` to the child's stdin. A broken pipe is not a failure: rtk
/// may exit before reading all of it, or without reading at all, and the
/// assertions on its output and exit code are then the useful ones. Windows
/// reports that case as `BrokenPipe` too.
fn write_stdin(pipe: &mut ChildStdin, input: &[u8]) {
    match pipe.write_all(input) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => panic!("write stdin: {e}"),
    }
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

/// `-` names stdin on either operand. It reaches rtk two ways: the hook
/// rewrites a bare `diff - expected` that inherits its stdin (a piped or
/// redirected `diff` is left alone), and callers run `rtk diff - expected`
/// directly, piped or not.
///
/// The dangerous direction is identical input: reading `-` as a path named "-"
/// fails with ENOENT and exits non-zero where `diff` exits 0, which silently
/// inverts `cmd | rtk diff - expected && <on-success>`.
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

/// Runs `rtk` with `input` in a stdin pipe that stays open, and returns how
/// it ended with its stdout and stderr. An rtk that reads stdin blocks on the open pipe, so it misses
/// the deadline instead of passing on an early end-of-file. The input is in
/// the pipe before rtk starts, so no write lands while it inspects stdin.
fn rtk_with_open_stdin(dir: &Path, args: &[&str], input: &[u8]) -> (Ended, String, String) {
    let (reader, mut pipe) = std::io::pipe().expect("pipe");
    pipe.write_all(input).expect("fill stdin");
    let child = rtk_cmd(dir, args)
        .stdin(reader)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk");

    let result = wait_with_deadline(child);
    drop(pipe);
    result
}

/// How a run bounded by [`wait_with_deadline`] ended.
#[derive(Debug, PartialEq)]
enum Ended {
    /// Exited with this code.
    Code(i32),
    /// Killed by this signal.
    #[cfg_attr(not(unix), allow(dead_code))]
    Signal(i32),
    /// Still running at the deadline, and killed by the harness.
    TimedOut,
}

/// Waits up to ten seconds for `child`, killing it past that, and returns
/// how it ended with its stdout and stderr. Both are drained while it runs,
/// so a child that writes more than a pipe holds cannot stall on it.
fn wait_with_deadline(mut child: Child) -> (Ended, String, String) {
    fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).expect("read rtk output");
            String::from_utf8_lossy(&bytes).into_owned()
        })
    }
    let stdout = drain(child.stdout.take().expect("piped stdout"));
    let stderr = drain(child.stderr.take().expect("piped stderr"));

    let deadline = Instant::now() + Duration::from_secs(10);
    let ended = loop {
        if let Some(status) = child.try_wait().expect("poll rtk") {
            break ended_by(status);
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill rtk");
            child.wait().expect("reap rtk");
            break Ended::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout.join().expect("stdout reader");
    let stderr = stderr.join().expect("stderr reader");
    (ended, stdout, stderr)
}

fn ended_by(status: ExitStatus) -> Ended {
    if let Some(code) = status.code() {
        return Ended::Code(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return Ended::Signal(signal);
        }
    }
    panic!("an exit status carries a code or a signal: {status:?}")
}

/// Runs `rtk` with stdin open on `dir/stdin_file`, moved `offset` bytes in.
/// The child inherits the descriptor with its offset, as it would from
/// `{ head -c N >/dev/null; rtk ...; } < file`.
fn rtk_with_file_stdin(dir: &Path, args: &[&str], stdin_file: &str, offset: u64) -> Output {
    use std::io::{Seek, SeekFrom};

    let mut stdin = std::fs::File::open(dir.join(stdin_file)).expect("open fixture");
    stdin.seek(SeekFrom::Start(offset)).expect("seek fixture");
    rtk_cmd(dir, args).stdin(stdin).output().expect("spawn rtk")
}

/// `diff - -` names one stream twice, so the sides are identical by
/// construction: `diff` answers without reading stdin, and so must rtk.
#[test]
fn dash_on_both_operands_is_identical_without_reading_stdin() {
    let dir = tempfile::tempdir().expect("tempdir");

    let (ended, out, err) = rtk_with_open_stdin(dir.path(), &["diff", "-", "-"], b"alpha\nbeta\n");

    assert_eq!(
        ended,
        Ended::Code(0),
        "`diff - -` must exit 0 without reading stdin: {err}"
    );
    assert_eq!(out, "[ok] Files are identical\n", "{err}");
}

/// `-` next to another name for the stdin pipe is the one stream. Reading
/// both would drain it twice and compare the input with nothing; on Linux
/// `/dev/stdin` stats to the pipe itself, so `diff`'s same-file rule holds
/// and rtk answers without reading, as `diff` does.
///
/// Linux only. On macOS `/dev/fd/0` is a devfs `fdesc` node whose `stat`
/// masks the pipe's mode with the descriptor's access bits (a read-only fd
/// loses the write bits), while `fstat(0)` returns the pipe's own mode. The
/// modes differ, so the rule, which compares modes, does not match there:
/// rtk reads both, as `diff`'s `same_file` would decide on the same stat
/// results.
#[cfg(target_os = "linux")]
#[test]
fn dash_next_to_another_name_for_the_stdin_pipe_is_identical_without_reading_it() {
    let dir = tempfile::tempdir().expect("tempdir");

    for args in [["diff", "-", "/dev/stdin"], ["diff", "/dev/stdin", "-"]] {
        let (ended, out, err) = rtk_with_open_stdin(dir.path(), &args, b"alpha\nbeta\n");

        assert_eq!(
            ended,
            Ended::Code(0),
            "{args:?} must exit 0 without reading stdin: {err}"
        );
        assert_eq!(out, "[ok] Files are identical\n", "{args:?}: {err}");
    }
}

/// Two `/dev` names for the stdin pipe are one file, answered without
/// reading. `/dev/stdin /dev/stdin` exercises the rule for a name given
/// twice; `/dev/fd/0 /dev/stdin` exercises the stat rule, since both names
/// resolve to the same descriptor node. On Linux both stat to the pipe. On
/// macOS both go through devfs `fdesc`, which copies `pipe_stat`'s fields and
/// masks the mode by the same descriptor's flags, so the two lookups agree;
/// that half is reasoned from XNU's `fdesc_attr` and `pipe_stat`.
#[cfg(unix)]
#[test]
fn two_dev_names_for_the_stdin_pipe_are_identical_without_reading_it() {
    let dir = tempfile::tempdir().expect("tempdir");

    for args in [
        ["diff", "/dev/stdin", "/dev/stdin"],
        ["diff", "/dev/fd/0", "/dev/stdin"],
    ] {
        let (ended, out, err) = rtk_with_open_stdin(dir.path(), &args, b"alpha\nbeta\n");

        assert_eq!(
            ended,
            Ended::Code(0),
            "{args:?} must exit 0 without reading stdin: {err}"
        );
        assert_eq!(out, "[ok] Files are identical\n", "{args:?}: {err}");
    }
}

/// Two names that reach one file are identical without reading it, as in
/// `diff`: the same path twice, a hard link, a symlink to the other operand.
/// A file nobody may read pins "without reading": opening it fails, so only
/// the answer from metadata exits 0, given twice or through a hard link, as
/// `diff` answers it.
#[cfg(unix)]
#[test]
fn two_names_for_one_file_are_identical_without_reading_it() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let file = write(dir.path(), "f.txt", "alpha\nbeta\n");
    std::fs::hard_link(dir.path().join(&file), dir.path().join("hard.txt")).expect("hard link");
    std::os::unix::fs::symlink(&file, dir.path().join("sym.txt")).expect("symlink");
    let locked = dir.path().join(write(dir.path(), "locked.txt", "alpha\n"));
    std::fs::hard_link(&locked, dir.path().join("locked-hard.txt")).expect("hard link");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    // Root, or any process allowed to override file permissions, reads it
    // anyway, and the locked rows would prove nothing.
    let locked_rows = std::fs::File::open(&locked).is_err();

    let mut cases = vec![
        ["diff", "f.txt", "f.txt"],
        ["diff", "f.txt", "hard.txt"],
        ["diff", "sym.txt", "f.txt"],
    ];
    if locked_rows {
        cases.push(["diff", "locked.txt", "locked.txt"]);
        cases.push(["diff", "locked.txt", "locked-hard.txt"]);
    }
    for args in cases {
        let (ended, out, err) = rtk_with_open_stdin(dir.path(), &args, b"");

        assert_eq!(
            ended,
            Ended::Code(0),
            "{args:?} must exit 0 without reading: {err}"
        );
        assert_eq!(out, "[ok] Files are identical\n", "{args:?}: {err}");
    }
}

/// An endless device given twice is one file, answered without reading. A
/// regressed rtk would read `/dev/zero` forever, so the child's address space
/// is capped and it fails its allocation within a second rather than at the
/// deadline. Linux only: Darwin accepts `RLIMIT_AS` and does not enforce it,
/// so there the cap would not bite. The rule for a name given twice stays
/// covered on every Unix by `/dev/stdin /dev/stdin`.
#[cfg(target_os = "linux")]
#[test]
fn an_endless_device_given_twice_is_identical_without_reading_it() {
    use std::os::unix::process::CommandExt;

    let dir = tempfile::tempdir().expect("tempdir");

    let mut cmd = common::rtk_command();
    cmd.args(["diff", "/dev/zero", "/dev/zero"])
        .env("LC_ALL", "C")
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: the closure runs between fork and exec, where only
    // async-signal-safe work is allowed. It calls one syscall and builds the
    // argument on its own stack: no allocation, no lock, no inherited mutex.
    #[allow(unsafe_code)]
    // nosemgrep: unsafe-block — libc::setrlimit on this process's own child, as scratch.rs does for atexit; test-only
    unsafe {
        cmd.pre_exec(|| {
            let bytes = 1 << 30;
            let limit = libc::rlimit {
                rlim_cur: bytes,
                rlim_max: bytes,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let child = cmd.spawn().expect("spawn rtk");

    let (ended, out, err) = wait_with_deadline(child);
    assert_eq!(ended, Ended::Code(0), "{err}");
    assert_eq!(out, "[ok] Files are identical\n", "{err}");
}

/// A directory given twice is reported as unreadable, as a directory next to
/// any other operand is, rather than as identical.
#[test]
fn a_directory_given_twice_is_trouble() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("d")).expect("mkdir");

    let (_, code) = rtk_in(dir.path(), &["diff", "d", "d"]);

    assert_eq!(code, Some(2));
}

/// `rtk diff - f < f` compares a file with itself, as `diff` does.
#[test]
fn dash_redirected_from_the_other_operand_is_identical() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = write(dir.path(), "f.txt", "alpha\nbeta\n");

    let out = rtk_with_file_stdin(dir.path(), &["diff", "-", &file], &file, 0);

    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "[ok] Files are identical\n"
    );
}

/// Stdin redirected from the other operand but already read into holds only
/// the rest of the file, so the two differ: `diff` compares from stdin's
/// current offset, and `{ head -c 6 >/dev/null; diff - f; } < f` exits 1.
#[test]
fn dash_redirected_from_the_other_operand_but_partly_read_differs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = write(dir.path(), "f.txt", "alpha\nbeta\n");

    let out = rtk_with_file_stdin(dir.path(), &["diff", "-", &file], &file, 6);

    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("alpha"),
        "the unread line is the difference: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A refusal to list with a stdin operand says stdin has to be supplied
/// again: this run consumed it, so `rtk proxy diff` alone would get nothing.
#[test]
fn refusal_with_a_stdin_operand_asks_for_stdin_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = write(dir.path(), "f.txt", "SHARED\n");
    let piped: String = (0..60000)
        .map(|i| {
            if i == 30000 {
                "SHARED\n".to_string()
            } else {
                format!("x{i}\n")
            }
        })
        .collect();

    let (out, code) = rtk_in_with_stdin(dir.path(), &["diff", &file, "-"], &piped);

    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("changed in f.txt"), "{out}");
    assert!(
        out.contains("stdin was consumed, so feed it again to `rtk proxy diff`"),
        "{out}"
    );
}

/// An unreadable file operand is trouble (exit 2), not a difference, even when
/// the other operand is stdin, and the message names the file rather than `-`.
#[test]
fn dash_with_a_missing_file_exits_two_and_names_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");

    for args in [["diff", "-", "missing.txt"], ["diff", "missing.txt", "-"]] {
        let out = rtk_piped(dir.path(), &args, b"alpha\n");
        let stderr = String::from_utf8_lossy(&out.stderr);

        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(
            out.stdout.is_empty(),
            "{args:?}: read errors belong on stderr"
        );
        assert!(stderr.contains("missing.txt"), "{args:?}: {stderr}");
        assert!(!stderr.contains("rtk diff: -:"), "{args:?}: {stderr}");
    }
}

/// Stdin is read as bytes, like a file operand, so non-UTF-8 input is compared
/// rather than reported as a read failure.
#[test]
fn dash_compares_non_utf8_stdin_byte_for_byte() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("latin1.txt"), b"caf\xe9\n").expect("write fixture");

    let same = rtk_piped(dir.path(), &["diff", "-", "latin1.txt"], b"caf\xe9\n");
    assert_eq!(
        same.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&same.stderr)
    );

    let differ = rtk_piped(dir.path(), &["diff", "-", "latin1.txt"], b"caf\xe8\n");
    assert_eq!(
        differ.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&differ.stderr)
    );
}

/// `rtk log -` reads stdin, not a file named "-". It takes the same entry point
/// as `rtk log`, so both share one newline handling and a CRLF log dedups
/// identically either way.
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
