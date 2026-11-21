use std::fs;
use std::io::{ErrorKind, Write};
use std::process::{Output, Stdio};
// The producers these tests spawn (`sh` and `sleep`, `cat`) are Unix only.
#[cfg(unix)]
use std::process::Command;

mod common;

fn read_stdin(input: &[u8], args: &[&str]) -> Output {
    let mut child = common::rtk_command()
        .args(["read", "-"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rtk read from stdin");
    // Stdin is written while the output is read: rtk writes a head window as it reads it, so
    // a window larger than a pipe buffer would block it on stdout before stdin was taken whole.
    let mut stdin = child.stdin.take().expect("piped stdin");
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || match stdin.write_all(input) {
            Ok(()) => {}
            // A head window can end before the input does, and rtk exits without reading the
            // rest, possibly before the write lands.
            Err(error) if error.kind() == ErrorKind::BrokenPipe => {}
            Err(error) => panic!("write stdin: {error}"),
        });
        let output = child.wait_with_output().expect("wait for rtk read");
        writer.join().expect("write stdin");
        output
    })
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
        let output = common::rtk_command()
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
            let from_file = common::rtk_command()
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
fn plain_reads_pass_non_utf8_bytes_through_for_files_and_stdin() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("latin1.log");
    for input in [
        b"line one\nlatin-1 byte: \xe9\nline three\n".as_slice(),
        b"\xff\r\nvalid\n\0\xfe".as_slice(),
    ] {
        fs::write(&file, input).expect("write non-UTF-8 file");
        let from_file = common::rtk_command()
            .arg("read")
            .arg(&file)
            .output()
            .expect("run rtk read");
        let from_stdin = read_stdin(input, &[]);
        for output in [from_file, from_stdin] {
            assert!(output.status.success(), "{:?}", output.stderr);
            assert_eq!(output.stdout, input);
        }
    }
}

#[test]
fn text_options_still_report_non_utf8_input() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("latin1.log");
    let input = b"line one\nlatin-1 byte: \xe9\nline three\n";
    fs::write(&file, input).expect("write non-UTF-8 file");
    let valid_file = dir.path().join("valid.log");
    let valid = b"line one\nascii\nline three\n";
    fs::write(&valid_file, valid).expect("write valid file");
    for args in [
        ["-l", "minimal"].as_slice(),
        ["--level", "aggressive"].as_slice(),
        ["--line-numbers"].as_slice(),
        ["--max-lines", "2"].as_slice(),
    ] {
        let from_file = common::rtk_command()
            .arg("read")
            .arg(&file)
            .args(args)
            .output()
            .expect("run rtk read");
        let from_stdin = read_stdin(input, args);
        for (source, output) in [
            (file.display().to_string(), from_file),
            ("-".into(), from_stdin),
        ] {
            assert_eq!(output.status.code(), Some(1), "{args:?} on {source}");
            assert!(output.stdout.is_empty(), "{args:?}: {:?}", output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains(&format!("cat: {source}: ")),
                "{args:?}: {stderr}"
            );
        }
        // The same options accept valid input, so the failure above is the decode.
        let from_valid_file = common::rtk_command()
            .arg("read")
            .arg(&valid_file)
            .args(args)
            .output()
            .expect("run rtk read");
        for output in [from_valid_file, read_stdin(valid, args)] {
            assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
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
            "1\t// comment\n",
        ),
        (
            ["--tail-lines", "1", "--line-numbers"].as_slice(),
            "1\tfn gamma() {}\n",
        ),
    ] {
        let output = read_stdin(input, args);
        assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        assert_eq!(output.stdout, expected.as_bytes(), "{args:?}");
    }
}

#[test]
fn read_line_numbers_align_multi_digit_widths_with_a_tab() {
    let input = b"a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n";
    let output = read_stdin(input, &["--line-numbers"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        output.stdout,
        b" 1\ta\n 2\tb\n 3\tc\n 4\td\n 5\te\n 6\tf\n 7\tg\n 8\th\n 9\ti\n10\tj\n11\tk\n12\tl\n"
    );
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
        let rewrite = common::rtk_command()
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
        let actual = common::rtk_command()
            .args(
                rewritten
                    .trim()
                    .strip_prefix("rtk ")
                    .expect("rtk rewrite prefix")
                    .split_whitespace(),
            )
            .output()
            .expect("execute rewritten head");
        let native = common::native_command("head")
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

    // Three lines, then the pipe stays open and silent: a read that waits for the end of its
    // input blocks without growing.
    let mut producer = Command::new("sh")
        .args(["-c", "printf 'line\\nline\\nline\\n'; exec sleep 60"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn endless producer");
    let pipe = producer.stdout.take().expect("producer stdout");
    let dir = tempfile::tempdir().expect("create test directory");
    let mut child = common::rtk_command()
        .current_dir(dir.path())
        .args(["read", "-", "--head-lines", "3"])
        .stdin(Stdio::from(pipe))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rtk read from an endless stdin");

    let deadline = Instant::now() + Duration::from_secs(20);
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

/// Runs `rtk read` in `db`'s directory with tracking pointed at `db`, and checks it succeeded.
fn run_tracked(db: &std::path::Path, args: &[&str], stdin: Stdio) -> Output {
    let output = common::rtk_command()
        .current_dir(db.parent().expect("database directory"))
        .env("RTK_DB_PATH", db)
        .arg("read")
        .args(args)
        .stdin(stdin)
        .output()
        .expect("run rtk read");
    assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
    output
}

/// `path` opened as a child's stdin, the way `< path` redirects it.
fn stdin_from(path: &std::path::Path) -> Stdio {
    Stdio::from(fs::File::open(path).expect("open file"))
}

/// A path relative to the directory [`spaced_file`] returns, with a space in it, and how a
/// row has to name it: quoted as one word.
const SPACED_PATH: &str = "sub dir/big.txt";
const SPACED_LABEL: &str = "'sub dir/big.txt'";

/// A fresh directory holding a 4000-line file at [`SPACED_PATH`], with that file's path and
/// body, and where its tracking database goes.
fn spaced_file() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    String,
    std::path::PathBuf,
) {
    let dir = tempfile::tempdir().expect("create test directory");
    fs::create_dir(dir.path().join("sub dir")).expect("create test subdirectory");
    let file = dir.path().join("sub dir").join("big.txt");
    let body: String = (0..4000).map(|i| format!("line {i}\n")).collect();
    fs::write(&file, &body).expect("write file");
    let db = dir.path().join("rtk.db");
    (dir, file, body, db)
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
/// only place any of this shows. The file sits at a relative path with a space in it, so the
/// label has to quote it as one word.
#[test]
fn read_windows_are_measured_against_the_command_they_stand_in_for() {
    let (_dir, file, body, db) = spaced_file();

    // A window well inside the file, and one that outruns it; from a named file and from
    // stdin, both redirected from the file and piped, since a named file and stdin take
    // different routes through `read`.
    // What `head`/`tail` would print, measured as tracking measures it.
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let window_tokens = |tool: &str, count: &str| {
        let n: usize = count.parse().expect("window size");
        let window = if tool == "head" {
            &lines[..n.min(lines.len())]
        } else {
            &lines[lines.len().saturating_sub(n)..]
        };
        window
            .iter()
            .map(|line| line.len())
            .sum::<usize>()
            .div_ceil(4) as u64
    };

    let mut expected = Vec::new();
    for (tool, flag, count) in [
        ("head", "--head-lines", "5"),
        ("head", "--head-lines", "99999"),
        ("tail", "--tail-lines", "5"),
        ("tail", "--tail-lines", "99999"),
    ] {
        let tokens = window_tokens(tool, count);
        run_tracked(&db, &[SPACED_PATH, flag, count], Stdio::null());
        expected.push((format!("{tool} -n {count} {SPACED_LABEL}"), tokens));

        run_tracked(&db, &["-", flag, count], stdin_from(&file));
        expected.push((format!("{tool} -n {count} -"), tokens));

        // The piped producer is `cat`, so this one case is Unix only.
        #[cfg(unix)]
        {
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
            expected.push((format!("{tool} -n {count} -"), tokens));
        }
    }

    let rows = tracked_rows(&db);
    assert_eq!(rows.len(), expected.len(), "{rows:?}");
    for (row, (label, tokens)) in rows.iter().zip(&expected) {
        assert_eq!(&row.0, label, "the row names the command it stands in for");
        assert_eq!(row.1, *tokens, "{label}: the baseline is the window");
        assert_eq!(
            row.2, *tokens,
            "{label}: the window printed whole is worth nothing"
        );
    }
}

/// `--line-numbers` renders the window the user asked for rather than printing it, but the
/// window is still all they asked RTK to do, so the row stands in for `head`/`tail` and books
/// nothing. The path has a space in it, so the label has to quote it as one word.
#[test]
fn read_windows_with_line_numbers_are_measured_like_any_other_window() {
    let (_dir, _file, _body, db) = spaced_file();

    run_tracked(
        &db,
        &[SPACED_PATH, "--head-lines", "5", "-n"],
        Stdio::null(),
    );
    run_tracked(
        &db,
        &[SPACED_PATH, "--tail-lines", "5", "-n"],
        Stdio::null(),
    );

    let rows = tracked_rows(&db);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].0, format!("head -n 5 {SPACED_LABEL}"));
    assert_eq!(rows[1].0, format!("tail -n 5 {SPACED_LABEL}"));
    for row in &rows {
        assert_eq!(
            row.1, row.2,
            "a numbered window is no saving either: {row:?}"
        );
        assert!(row.1 > 0, "{row:?}");
    }
}

/// Only a window the *user* asked for stands in for `head` or `tail`. A filter level cuts the
/// input down itself, so the read stands in for `cat` and goes on booking the saving it earns.
#[test]
fn read_filter_levels_stand_in_for_cat() {
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
    // RTK is still cutting the input down.
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
        assert!(row.1 > row.2, "filtering earns a saving: {row:?}");
    }

    // A read of stdin that is not a window is booked as `cat - (stdin)`.
    let stdin_db = dir.path().join("stdin.db");
    run_tracked(
        &stdin_db,
        &["-", "--level", "aggressive"],
        stdin_from(&file),
    );
    let stdin_rows = tracked_rows(&stdin_db);
    assert_eq!(stdin_rows[0].0, "cat - (stdin)", "{stdin_rows:?}");
}

/// A plain read of stdin is written out as it came, without the filter pipeline, and is
/// still booked as `cat - (stdin)`, like the filtered one.
#[test]
fn a_plain_stdin_read_stands_in_for_cat() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("sample.txt");
    fs::write(&file, "alpha\nbravo\n").expect("write file");
    let db = dir.path().join("rtk.db");

    run_tracked(&db, &["-"], stdin_from(&file));

    let rows = tracked_rows(&db);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "cat - (stdin)", "{rows:?}");
}

/// A row counts the bytes RTK wrote. Bytes that are not valid UTF-8 are written as they came,
/// so decoding them to count them would book each invalid byte as the replacement character's
/// three, more than was printed.
#[test]
fn a_non_utf8_read_books_the_bytes_it_printed() {
    let dir = tempfile::tempdir().expect("create test directory");
    let file = dir.path().join("latin1.txt");
    // `café` in Latin-1: the `é` is one byte that is not valid UTF-8 on its own.
    let body: Vec<u8> = (0..40)
        .flat_map(|i| [b"caf\xe9 ".as_slice(), i.to_string().as_bytes(), b"\n"].concat())
        .collect();
    fs::write(&file, &body).expect("write file");
    let db = dir.path().join("rtk.db");
    let lines: Vec<&[u8]> = body.split_inclusive(|&byte| byte == b'\n').collect();
    let tokens = |window: &[&[u8]]| window.concat().len().div_ceil(4) as u64;

    let cases: [(&[&str], u64); 6] = [
        (&["latin1.txt"], tokens(&lines)),
        (&["latin1.txt", "--head-lines", "5"], tokens(&lines[..5])),
        (&["latin1.txt", "--tail-lines", "5"], tokens(&lines[35..])),
        (&["-"], tokens(&lines)),
        (&["-", "--head-lines", "5"], tokens(&lines[..5])),
        (&["-", "--tail-lines", "5"], tokens(&lines[35..])),
    ];
    for (args, _) in &cases {
        run_tracked(&db, args, stdin_from(&file));
    }

    let rows = tracked_rows(&db);
    assert_eq!(rows.len(), cases.len(), "{rows:?}");
    for (row, (args, expected)) in rows.iter().zip(&cases) {
        assert_eq!((row.1, row.2), (*expected, *expected), "{args:?}: {row:?}");
    }
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
    let output = run_tracked(
        &db,
        &[&lying.to_string_lossy(), "--head-lines", "200"],
        Stdio::null(),
    );
    assert_eq!(
        output.stdout.len(),
        produced,
        "the whole attribute is shown"
    );

    let rows = tracked_rows(&db);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let (_, input_tokens, output_tokens) = &rows[0];
    assert_eq!(
        input_tokens,
        output_tokens,
        "{} produced {produced} bytes and claims {} on disk; the window is the whole of it",
        lying.display(),
        fs::metadata(lying).expect("stat the attribute").len()
    );
}
