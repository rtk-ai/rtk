//! What `rtk read` books in the savings history, read back from the row it
//! wrote: the raw side (`input_tokens`) must describe the same span as the
//! printed output, so a window the user asked for is never counted as a saving
//! and a filter's real saving is never erased.

use rusqlite::Connection;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use tempfile::TempDir;

mod common;

/// The one row `rtk read` wrote.
#[derive(Debug, Clone, Copy)]
struct Row {
    input: i64,
    output: i64,
}

/// A home of the test's own, so every read starts from an empty history.
fn home() -> TempDir {
    common::tempdir()
}

/// Run `rtk read <args>`, feeding `stdin` when given, and return what it
/// printed with the row it booked. Every row must be non-negative.
fn read(home: &Path, args: &[&str], stdin: Option<&str>) -> (String, Row) {
    let db = home.join("rtk.db");
    let mut cmd = common::rtk_command();
    cmd.arg("read")
        .args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("RTK_DB_PATH", &db)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = cmd.spawn().expect("spawn rtk read");
    if let Some(text) = stdin {
        let mut pipe = child.stdin.take().expect("stdin pipe");
        pipe.write_all(text.as_bytes()).expect("feed stdin");
    }
    let out = child.wait_with_output().expect("wait rtk read");
    assert!(
        out.status.success(),
        "rtk read {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Connection::open(&db).expect("open tracking db");
    let (count, input, output): (i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), MAX(input_tokens), MAX(output_tokens) FROM commands",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("read the commands row");
    assert_eq!(
        count, 1,
        "rtk read {args:?} booked {count} rows, expected one"
    );
    let row = Row { input, output };
    assert!(
        row.input >= row.output,
        "rtk read {args:?} booked a negative saving: {row:?}"
    );
    (String::from_utf8_lossy(&out.stdout).into_owned(), row)
}

/// rtk's token estimate of `text`, so a row can be compared with a span.
fn tokens(text: &str) -> i64 {
    text.len().div_ceil(4) as i64
}

/// Fifty distinct lines, long enough that a window is a small part of it.
fn plain_text() -> String {
    (1..=50)
        .map(|i| format!("line {i:02} with some padding so a window is a small share\n"))
        .collect()
}

/// A Rust file whose comments `--level minimal` strips, with a comment on
/// every other line to the very end, so any window of the raw text holds some.
fn commented_rust() -> String {
    (1..=20)
        .map(|i| {
            format!(
                "// comment {i} that minimal filtering strips from what is shown to the reader\nlet value_{i} = compute_{i}();\n"
            )
        })
        .collect()
}

fn write(home: &Path, name: &str, text: &str) -> String {
    let path = home.join(name);
    fs::write(&path, text).expect("write fixture");
    path.to_string_lossy().into_owned()
}

#[test]
fn file_windows_book_the_window_on_both_sides() {
    let text = plain_text();
    let whole = tokens(&text);
    for window in [
        &["--head-lines", "3"][..],
        &["--tail-lines", "3"],
        &["--max-lines", "3"],
        &["-n", "--tail-lines", "3"],
    ] {
        let home = home();
        let path = write(home.path(), "plain.txt", &text);
        let mut args = vec![path.as_str()];
        args.extend_from_slice(window);
        let (_, row) = read(home.path(), &args, None);
        assert_eq!(
            row.input, row.output,
            "{window:?}: a window the user asked for is not a saving: {row:?}"
        );
        assert!(
            row.input * 4 < whole,
            "{window:?}: the raw side must be the window, not the file: {row:?}, file {whole}"
        );
    }
}

#[test]
fn stdin_windows_book_the_window_on_both_sides() {
    let text = plain_text();
    let whole = tokens(&text);
    for window in [
        &["--tail-lines", "3"][..],
        &["--max-lines", "3"],
        &["-n", "--tail-lines", "3"],
    ] {
        let home = home();
        let mut args = vec!["-"];
        args.extend_from_slice(window);
        let (_, row) = read(home.path(), &args, Some(&text));
        assert_eq!(
            row.input, row.output,
            "stdin {window:?}: a window the user asked for is not a saving: {row:?}"
        );
        assert!(
            row.input * 4 < whole,
            "stdin {window:?}: the raw side must be the window, not the input: {row:?}, input {whole}"
        );
    }
}

#[test]
fn filtered_window_books_only_the_filter_saving() {
    let text = commented_rust();
    let whole = tokens(&text);
    let home = home();
    let path = write(home.path(), "commented.rs", &text);
    let (stdout, row) = read(
        home.path(),
        &[&path, "--level", "minimal", "--tail-lines", "4"],
        None,
    );
    assert!(
        !stdout.contains("// comment"),
        "minimal filtering strips the comments: {stdout:?}"
    );
    assert!(
        row.input > row.output,
        "the comments the filter removed from the window are a saving: {row:?}"
    );
    assert!(
        row.input * 4 < whole,
        "the raw side is the window of the file, not the file: {row:?}, file {whole}"
    );
}

#[test]
fn max_lines_shown_as_cat_books_no_saving() {
    // Three lines: the truncation marker makes the cut version longer than
    // the file, so never_worse prints the file as cat would.
    let text = "one\ntwo\nthree\n";
    let home = home();
    let path = write(home.path(), "short.txt", text);
    let (stdout, row) = read(home.path(), &[&path, "--max-lines", "2"], None);
    assert_eq!(stdout, text, "never_worse prints the whole file");
    assert_eq!(
        row.input, row.output,
        "what cat prints is no saving: {row:?}"
    );
    assert_eq!(row.input, tokens(text), "the row is the file: {row:?}");
}

#[test]
fn max_lines_that_cuts_nothing_books_the_filter_saving() {
    // The filter leaves two lines, which fit the window: the window removed
    // nothing, so the saving is the filter's, against the whole file.
    let text = "// first comment stripped by minimal\n// second comment stripped by minimal\nfn alpha() {}\nfn bravo() {}\n";
    let home = home();
    let path = write(home.path(), "tiny.rs", text);
    let (stdout, row) = read(
        home.path(),
        &[&path, "--level", "minimal", "--max-lines", "2"],
        None,
    );
    assert!(
        !stdout.contains("comment"),
        "comments are stripped: {stdout:?}"
    );
    assert_eq!(
        row.input,
        tokens(text),
        "the raw side is the whole file: {row:?}"
    );
    assert_eq!(
        row.output,
        tokens(&stdout),
        "the output side is what was printed: {row:?}"
    );
    assert!(
        row.input > row.output,
        "the filter's saving is kept: {row:?}"
    );
}

#[test]
fn stdin_max_lines_that_cuts_nothing_books_the_filter_saving() {
    // Minimal trims the blank lines whatever the language, leaving exactly as
    // many lines as the window holds: the window removed nothing, so the
    // saving is the filter's, against the whole input.
    let text = "\n\n\n\nfn alpha() {}\nfn bravo() {}\n\n\n\n";
    let home = home();
    let (stdout, row) = read(
        home.path(),
        &["-", "--level", "minimal", "--max-lines", "2"],
        Some(text),
    );
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        ["fn alpha() {}", "fn bravo() {}"],
        "the blank lines are trimmed"
    );
    assert_eq!(
        row.input,
        tokens(text),
        "the raw side is the whole input: {row:?}"
    );
    assert_eq!(
        row.output,
        tokens(&stdout),
        "the output side is what was printed: {row:?}"
    );
    assert!(
        row.input > row.output,
        "the filter's saving is kept: {row:?}"
    );
}

#[test]
fn max_lines_that_cuts_books_the_windowed_file() {
    let text = plain_text();
    let home = home();
    let path = write(home.path(), "plain.txt", &text);
    let (stdout, row) = read(home.path(), &[&path, "--max-lines", "5"], None);
    assert!(stdout.contains("more lines"), "the window cut: {stdout:?}");
    assert_eq!(
        row.input, row.output,
        "a cut the user asked for is no saving: {row:?}"
    );
}

#[test]
fn max_lines_prints_the_filtered_content_not_a_raw_window() {
    // The tracking baseline is a windowed copy of the raw content. It is only
    // ever recorded: what is printed stays the filtered content.
    let text = "short\n// a comment that minimal filtering should strip\na_very_long_real_code_line_that_should_still_print();\n";
    let home = home();
    let path = write(home.path(), "leak.rs", text);
    let (stdout, _) = read(
        home.path(),
        &[&path, "--level", "minimal", "--max-lines", "2"],
        None,
    );
    assert!(
        !stdout.contains("// a comment"),
        "the comment must be filtered out, not leak in via the raw baseline: {stdout:?}"
    );
    assert!(
        stdout.contains("a_very_long_real_code_line_that_should_still_print();"),
        "the filtered code line is printed: {stdout:?}"
    );
}
