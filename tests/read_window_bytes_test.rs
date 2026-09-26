use std::fs;
use std::io::{ErrorKind, Write};
use std::process::{Command, Output, Stdio};

fn read_stdin(input: &[u8], args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["read", "-"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rtk read from stdin");
    let mut stdin = child.stdin.take().expect("piped stdin");
    match stdin.write_all(input) {
        Ok(()) => {}
        // A window that needs no input at all can exit before the write lands.
        Err(error) if error.kind() == ErrorKind::BrokenPipe => {}
        Err(error) => panic!("write stdin: {error}"),
    }
    drop(stdin);
    child.wait_with_output().expect("wait for rtk read")
}

#[test]
fn read_windows_preserve_non_utf8_files() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("binary.log");
    fs::write(&file, b"\xff\xfe bad\nline2\nline3\n").expect("write binary file");

    for (flag, expected) in [
        ("--head-lines", b"\xff\xfe bad\nline2\n".as_slice()),
        ("--tail-lines", b"line2\nline3\n".as_slice()),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .arg("read")
            .arg(&file)
            .args([flag, "2"])
            .output()
            .expect("run rtk read");

        assert!(output.status.success(), "{flag}: {:?}", output.stderr);
        assert_eq!(output.stdout, expected, "{flag}");
    }
}

#[test]
fn read_windows_preserve_binary_boundaries_for_files_and_stdin() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("binary.log");
    for input in [
        b"\xff\r\nvalid\n\0\xfe".as_slice(),
        b"\xff\r\nvalid\n\0\xfe\n".as_slice(),
    ] {
        fs::write(&file, input).expect("write binary file");
        let last = if input.ends_with(b"\n") {
            b"\0\xfe\n".as_slice()
        } else {
            b"\0\xfe".as_slice()
        };
        for (flag, count, expected) in [
            ("--head-lines", "0", b"".as_slice()),
            ("--tail-lines", "0", b"".as_slice()),
            ("--head-lines", "1", b"\xff\r\n".as_slice()),
            ("--tail-lines", "1", last),
            ("--head-lines", "99", input),
            ("--tail-lines", "99", input),
        ] {
            let from_file = Command::new(env!("CARGO_BIN_EXE_rtk"))
                .arg("read")
                .arg(&file)
                .args([flag, count])
                .output()
                .expect("run rtk read");
            let from_stdin = read_stdin(input, &[flag, count]);
            for output in [from_file, from_stdin] {
                assert!(
                    output.status.success(),
                    "{flag} {count}: {:?}",
                    output.stderr
                );
                assert_eq!(output.stdout, expected, "{flag} {count}");
            }
        }
    }
}

#[test]
fn read_windows_accept_invalid_bytes_outside_the_selected_window() {
    for (input, flag) in [
        (b"valid\n\xff\n".as_slice(), "--head-lines"),
        (b"\xff\nvalid\n".as_slice(), "--tail-lines"),
    ] {
        let output = read_stdin(input, &[flag, "1"]);
        assert!(output.status.success(), "{flag}: {:?}", output.stderr);
        assert_eq!(output.stdout, b"valid\n");
    }
}

#[test]
fn read_windows_still_apply_text_filtering_and_line_numbers() {
    let input = b"// comment\nfn alpha() {}\nfn beta() {}\nfn gamma() {}\n";
    for (args, expected) in [
        (
            ["--head-lines", "1", "--level", "minimal"].as_slice(),
            "fn alpha() {}\n",
        ),
        (
            ["--tail-lines", "1", "--level", "minimal"].as_slice(),
            "fn gamma() {}",
        ),
        (
            ["--head-lines", "1", "--line-numbers"].as_slice(),
            "1 │ // comment\n",
        ),
        (
            ["--tail-lines", "1", "--line-numbers"].as_slice(),
            "1 │ fn gamma() {}\n",
        ),
    ] {
        let output = read_stdin(input, args);
        assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        assert_eq!(output.stdout, expected.as_bytes(), "{args:?}");
    }
}

#[cfg(unix)]
#[test]
fn rewritten_head_spellings_match_native_on_non_utf8_files() {
    let dir = tempfile::tempdir().expect("create test directory");
    let claude_dir = dir.path().join(".claude");
    fs::create_dir(&claude_dir).expect("create isolated Claude config directory");
    let config_dir = dir.path().join("config");
    fs::create_dir(&config_dir).expect("create isolated rtk config directory");
    let file = dir.path().join("binary.log");
    fs::write(&file, b"\xff\xfe bad\nline2\nline3\n").expect("write binary file");
    for flags in ["-2", "-n 2", "--lines 2", "--lines=2", ""] {
        let command = format!("head {flags} {}", file.display());
        let rewrite = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .current_dir(dir.path())
            .env("CLAUDE_CONFIG_DIR", &claude_dir)
            .env("XDG_CONFIG_HOME", &config_dir)
            .args(["rewrite", &command])
            .output()
            .expect("rewrite head command");
        assert_eq!(
            rewrite.status.code(),
            Some(3),
            "{command}: {:?}",
            rewrite.stderr
        );
        let rewritten = String::from_utf8(rewrite.stdout).expect("UTF-8 command");
        let count = if flags.is_empty() { "10" } else { "2" };
        assert_eq!(
            rewritten.trim(),
            format!("rtk read {} --head-lines {count}", file.display())
        );
        let actual = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(
                rewritten
                    .trim()
                    .strip_prefix("rtk ")
                    .expect("rtk rewrite prefix")
                    .split_whitespace(),
            )
            .output()
            .expect("execute rewritten head");
        let native = Command::new("head")
            .args(flags.split_whitespace())
            .arg(&file)
            .output()
            .expect("run native head");
        assert!(native.status.success(), "{command}: {:?}", native.stderr);
        assert_eq!(actual.status.code(), native.status.code(), "{command}");
        assert_eq!(actual.stdout, native.stdout, "{command}");
    }
}

/// A producer that never closes its end of the pipe: `rtk read - --head-lines N` has to stop at
/// the Nth line, as `head` does, instead of waiting for input that never ends.
#[cfg(unix)]
#[test]
fn read_stdin_head_stops_without_draining_an_endless_producer() {
    use std::time::{Duration, Instant};

    let mut producer = Command::new("yes")
        .arg("line")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn endless producer");
    let pipe = producer.stdout.take().expect("producer stdout");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["read", "-", "--head-lines", "3"])
        .stdin(Stdio::from(pipe))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rtk read from an endless stdin");

    let deadline = Instant::now() + Duration::from_secs(30);
    let timed_out = loop {
        match child.try_wait().expect("poll rtk read") {
            Some(_) => break false,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                break true;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let output = child.wait_with_output().expect("wait for rtk read");
    let _ = producer.kill();
    let _ = producer.wait();

    assert!(
        !timed_out,
        "rtk read - --head-lines drained an endless producer"
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"line\nline\nline\n");
}

/// Runs `rtk read` with tracking pointed at `db`, and returns nothing: these tests read the
/// rows, not the output.
fn run_tracked(db: &std::path::Path, args: &[&str], stdin: Stdio) {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("RTK_DB_PATH", db)
        .arg("read")
        .args(args)
        .stdin(stdin)
        .output()
        .expect("run rtk read");
    assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
}

/// Every recorded row, oldest first: the command it stands in for, and its token counts.
fn tracked_rows(db: &std::path::Path) -> Vec<(String, u64, u64)> {
    rusqlite::Connection::open(db)
        .expect("open tracking database")
        .prepare("SELECT original_cmd, input_tokens, output_tokens FROM commands ORDER BY id")
        .expect("prepare query")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query rows")
        .collect::<Result<_, _>>()
        .expect("read rows")
}

/// A window the user asked for is measured against the command it stands in for, whatever
/// spelling produced it: `head -n N` and `tail -n N` print the same window RTK prints, so the
/// row books no saving, and the row names that command rather than `cat`. The rows are the
/// only place any of this shows — the bytes written are what they always were.
#[cfg(unix)]
#[test]
fn read_windows_are_measured_against_the_command_they_stand_in_for() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("big.txt");
    let body: String = (0..4000).map(|i| format!("line {i}\n")).collect();
    fs::write(&file, &body).expect("write file");
    let db = dir.path().join("rtk.db");
    let path = file.to_string_lossy().into_owned();

    // A window well inside the file, and one that outruns it; from a named file, from a
    // redirected file and from a pipe, since each takes a different route through `read`.
    let mut expected_labels = Vec::new();
    for (flag, count) in [
        ("--head-lines", "5"),
        ("--head-lines", "99999"),
        ("--tail-lines", "5"),
        ("--tail-lines", "99999"),
    ] {
        let tool = if flag == "--head-lines" {
            "head"
        } else {
            "tail"
        };
        run_tracked(&db, &[&path, flag, count], Stdio::null());
        expected_labels.push(format!("{tool} -n {count} {path}"));

        run_tracked(
            &db,
            &["-", flag, count],
            Stdio::from(fs::File::open(&file).expect("open file")),
        );
        expected_labels.push(format!("{tool} -n {count} -"));

        let mut producer = Command::new("cat")
            .arg(&file)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn producer");
        run_tracked(
            &db,
            &["-", flag, count],
            Stdio::from(producer.stdout.take().expect("producer stdout")),
        );
        let _ = producer.wait();
        expected_labels.push(format!("{tool} -n {count} -"));
    }

    let rows = tracked_rows(&db);
    assert_eq!(rows.len(), expected_labels.len(), "{rows:?}");
    for (row, label) in rows.iter().zip(&expected_labels) {
        assert_eq!(&row.0, label, "the row names the command it stands in for");
        assert_eq!(
            row.1, row.2,
            "{label}: the window printed whole is worth nothing"
        );
        assert!(row.1 > 0, "{label}: the window is not empty");
    }
}

/// `--line-numbers` renders the window the user asked for rather than printing it, but the
/// window is still all they asked RTK to do, so the row stands in for `head`/`tail` and books
/// nothing — where before it booked the whole file against a five-line window.
#[test]
fn read_windows_with_line_numbers_are_measured_like_any_other_window() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("big.txt");
    let body: String = (0..4000).map(|i| format!("line {i}\n")).collect();
    fs::write(&file, &body).expect("write file");
    let db = dir.path().join("rtk.db");
    let path = file.to_string_lossy().into_owned();

    run_tracked(&db, &[&path, "--head-lines", "5", "-n"], Stdio::null());
    run_tracked(&db, &[&path, "--tail-lines", "5", "-n"], Stdio::null());

    let rows = tracked_rows(&db);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].0, format!("head -n 5 {path}"));
    assert_eq!(rows[1].0, format!("tail -n 5 {path}"));
    for row in &rows {
        assert_eq!(
            row.1, row.2,
            "a numbered window is no saving either: {row:?}"
        );
        assert!(row.1 > 0, "{row:?}");
    }
}

/// Only a window the *user* asked for is measured against itself. A filter level truncates
/// output the user asked for in full, so it keeps the whole input as its baseline and goes on
/// booking the saving it earns.
#[test]
fn read_filter_levels_keep_the_whole_input_as_their_baseline() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("sample.rs");
    let body: String = (0..200)
        .map(|i| format!("// a comment worth dropping, number {i}\nfn item{i}() {{}}\n"))
        .collect();
    fs::write(&file, &body).expect("write file");
    let db = dir.path().join("rtk.db");

    let path = file.to_string_lossy().into_owned();
    run_tracked(&db, &[&path, "--level", "aggressive"], Stdio::null());
    // Asking for a window on top of a filter level does not turn the read into a window:
    // RTK is still cutting the input down, so the baseline is still the whole of it.
    run_tracked(
        &db,
        &[&path, "--level", "aggressive", "--head-lines", "3"],
        Stdio::null(),
    );

    let rows = tracked_rows(&db);
    assert_eq!(rows.len(), 2, "{rows:?}");
    for row in &rows {
        assert!(
            row.0.starts_with("cat "),
            "a filtered read still stands in for `cat`: {row:?}"
        );
        assert!(
            row.1 > row.2,
            "filtering earns a saving against the whole input: {row:?}"
        );
    }
    assert_eq!(
        rows[0].1, rows[1].1,
        "both are measured against the same whole input: {rows:?}"
    );

    // A read of stdin that is not a window keeps the name its rows have always had.
    let stdin_db = dir.path().join("stdin.db");
    run_tracked(
        &stdin_db,
        &["-", "--level", "aggressive"],
        Stdio::from(fs::File::open(&file).expect("open file")),
    );
    let stdin_rows = tracked_rows(&stdin_db);
    assert_eq!(stdin_rows[0].0, "cat - (stdin)", "{stdin_rows:?}");
}

/// A size on disk can never be the baseline for a window, and a pseudo-file is where that
/// would show: a sysfs attribute reports a page and prints one line, so measuring against it
/// would book a saving RTK never made. The assertion is skipped where no such file exists, so
/// it pins the behaviour on Linux and nowhere else.
#[cfg(target_os = "linux")]
#[test]
fn read_head_never_measures_a_window_against_a_size_on_disk() {
    let lying = [
        "/sys/kernel/mm/transparent_hugepage/enabled",
        "/sys/devices/system/cpu/online",
        "/sys/kernel/profiling",
    ]
    .into_iter()
    .map(std::path::Path::new)
    .find(|path| match (fs::metadata(path), fs::read(path)) {
        (Ok(meta), Ok(body)) => {
            meta.is_file() && !body.is_empty() && meta.len() > body.len() as u64
        }
        _ => false,
    });
    let Some(lying) = lying else {
        return;
    };
    let produced = fs::read(lying).expect("read the attribute").len();

    let dir = tempfile::tempdir().expect("create test directory");
    let db = dir.path().join("rtk.db");
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("RTK_DB_PATH", &db)
        .args(["read", &lying.to_string_lossy(), "--head-lines", "200"])
        .output()
        .expect("run rtk read");
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        output.stdout.len(),
        produced,
        "the whole attribute is shown"
    );

    let (input_tokens, output_tokens): (u64, u64) = rusqlite::Connection::open(&db)
        .expect("open tracking database")
        .query_row(
            "SELECT input_tokens, output_tokens FROM commands",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read the row");
    assert_eq!(
        input_tokens,
        output_tokens,
        "{} produced {produced} bytes and claims {} on disk; the window is the whole of it",
        lying.display(),
        fs::metadata(lying).expect("stat the attribute").len()
    );
}
