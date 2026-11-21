use std::fs;
use std::path::Path;
use std::process::Output;

mod common;

const DIFF_SUBCOMMAND: &str = "diff";
const LF_FILE: &str = "lf.txt";
const CRLF_FILE: &str = "crlf.txt";
const LF_CONTENT: &str = "alpha\nbeta\n";
const CRLF_CONTENT: &str = "alpha\r\nbeta\r\n";
const ONE_LINE_LF_CONTENT: &str = "x\n";
const ONE_LINE_CRLF_CONTENT: &str = "x\r\n";
const BOTH_FILES_SEPARATOR: &str = "\n---\n";
const WHITESPACE_ONLY_MESSAGE: &str = "whitespace or line endings";
const IDENTICAL_MESSAGE: &str = "[ok] Files are identical";
const DIFF_EXIT_CODE: i32 = 1;

fn run_rtk_diff(file1: &Path, file2: &Path) -> Output {
    let file1 = file1.display().to_string();
    let file2 = file2.display().to_string();

    common::rtk_command()
        .args([DIFF_SUBCOMMAND, &file1, &file2])
        .output()
        .expect("run rtk diff")
}

fn assert_explains_invisible_difference(lf_content: &str, crlf_content: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let lf = dir.path().join(LF_FILE);
    let crlf = dir.path().join(CRLF_FILE);

    fs::write(&lf, lf_content).expect("write LF fixture");
    fs::write(&crlf, crlf_content).expect("write CRLF fixture");

    let output = run_rtk_diff(&lf, &crlf);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");

    assert_eq!(output.status.code(), Some(DIFF_EXIT_CODE), "{stdout}");
    assert!(
        stdout.contains(WHITESPACE_ONLY_MESSAGE),
        "CRLF-vs-LF diff should explain the byte-only difference:\n{stdout}"
    );
    assert!(
        !stdout.contains(BOTH_FILES_SEPARATOR),
        "the two indistinguishable blobs must not replace the explanation:\n{stdout}"
    );
    assert!(
        !stdout.contains(IDENTICAL_MESSAGE),
        "byte-different files must not be reported identical:\n{stdout}"
    );
}

#[test]
fn small_crlf_vs_lf_diff_prints_whitespace_message() {
    assert_explains_invisible_difference(LF_CONTENT, CRLF_CONTENT);
}

#[test]
fn one_line_crlf_vs_lf_diff_prints_whitespace_message() {
    // The message is ~20 tokens and a one-line pair ~2, so any fixed allowance
    // above raw drops it here. It is shown regardless of size.
    assert_explains_invisible_difference(ONE_LINE_LF_CONTENT, ONE_LINE_CRLF_CONTENT);
}

/// What `rtk diff` prints for two files holding exactly these bytes, and its
/// exit code.
fn diff_of(content1: &str, content2: &str) -> (String, Option<i32>) {
    let dir = common::tempdir();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");

    fs::write(&first, content1).expect("write first fixture");
    fs::write(&second, content2).expect("write second fixture");

    let output = run_rtk_diff(&first, &second);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    (stdout, output.status.code())
}

// Two lines similar enough to list as one rewritten `~` line, so the condensed
// listing is the smaller output and the guard keeps it.
const SIMILAR_BEFORE: &str = "let x = 1;";
const SIMILAR_AFTER: &str = "let x = 2;";
const SIMILAR_LISTING: &str = "~   2 let x = 1; → let x = 2;\n";

// Two lines too different to pair: the listing is a `-`, a `+` and a legend,
// which costs more than classic `diff`, so the guard hands back the classic form.
const DISSIMILAR_CLASSIC: &str = "2c2\n< beta\n---\n> gamma\n";

const LINE_ENDINGS_NOTE: &str =
    "line endings differ on 1 unchanged line, first at line 1 of file 1";
const TRAILING_NEWLINE_NOTE: &str = "trailing newline: present vs absent";

#[test]
fn content_change_beside_a_crlf_line_reports_the_line_endings() {
    // Line 1 is absent from the listing, so a reader takes it for
    // byte-identical, but its terminator differs. The note says so.
    let (stdout, code) = diff_of(
        &format!("alpha\n{SIMILAR_BEFORE}\n"),
        &format!("alpha\r\n{SIMILAR_AFTER}\n"),
    );

    assert_eq!(code, Some(DIFF_EXIT_CODE), "{stdout}");
    assert_eq!(
        stdout,
        format!("{SIMILAR_LISTING}   ({LINE_ENDINGS_NOTE})\n")
    );
}

#[test]
fn content_change_beside_a_missing_trailing_newline_reports_it() {
    // Only the final newline differs besides the changed line, so no
    // line-ending claim may appear.
    let (stdout, code) = diff_of(
        &format!("alpha\n{SIMILAR_BEFORE}\n"),
        &format!("alpha\n{SIMILAR_AFTER}"),
    );

    assert_eq!(code, Some(DIFF_EXIT_CODE), "{stdout}");
    assert_eq!(
        stdout,
        format!("{SIMILAR_LISTING}   ({TRAILING_NEWLINE_NOTE})\n")
    );
}

#[test]
fn content_change_beside_both_invisible_differences_reports_both() {
    let (stdout, code) = diff_of(
        &format!("alpha\n{SIMILAR_BEFORE}\n"),
        &format!("alpha\r\n{SIMILAR_AFTER}"),
    );

    assert_eq!(code, Some(DIFF_EXIT_CODE), "{stdout}");
    assert_eq!(
        stdout,
        format!("{SIMILAR_LISTING}   ({LINE_ENDINGS_NOTE}; {TRAILING_NEWLINE_NOTE})\n")
    );
}

#[test]
fn issue_3901_repro_keeps_its_note_when_the_guard_picks_classic_diff() {
    // The files from the issue. The condensed listing is larger than classic
    // `diff` here, so the guard returns the classic form; the note has to
    // survive that choice or the fix never reaches this input.
    let (stdout, code) = diff_of("alpha\nbeta\n", "alpha\r\ngamma");

    assert_eq!(code, Some(DIFF_EXIT_CODE), "{stdout}");
    assert_eq!(
        stdout,
        format!("{DISSIMILAR_CLASSIC}   ({LINE_ENDINGS_NOTE}; {TRAILING_NEWLINE_NOTE})\n")
    );
}

#[test]
fn pure_content_change_prints_exactly_what_it_did_before() {
    // Bytes agree everywhere but the changed line: no note, on either path.
    let (condensed, code) = diff_of(
        &format!("alpha\n{SIMILAR_BEFORE}\n"),
        &format!("alpha\n{SIMILAR_AFTER}\n"),
    );
    assert_eq!(code, Some(DIFF_EXIT_CODE), "{condensed}");
    assert_eq!(condensed, SIMILAR_LISTING);

    let (classic, code) = diff_of("alpha\nbeta\n", "alpha\ngamma\n");
    assert_eq!(code, Some(DIFF_EXIT_CODE), "{classic}");
    assert_eq!(classic, DISSIMILAR_CLASSIC);
}

#[test]
fn crlf_files_with_an_inserted_line_report_no_line_ending_difference() {
    // Every line of both files ends in CRLF. The insertion changes the CRLF
    // count and shifts the line numbers, which is no difference in endings.
    let (stdout, code) = diff_of("a\r\nb\r\n", "a\r\nx\r\nb\r\n");

    assert_eq!(code, Some(DIFF_EXIT_CODE), "{stdout}");
    assert_eq!(stdout, "+   2 x\n");
}

#[test]
fn shifted_lines_are_compared_with_the_line_they_pair_with() {
    // `b` ends in CRLF in both files, at line 2 and then at line 3. Comparing
    // the CRLF line numbers instead of the paired lines would call that a
    // difference.
    let (same, code) = diff_of("a\nb\r\n", "x\na\nb\r\n");
    assert_eq!(code, Some(DIFF_EXIT_CODE), "{same}");
    assert_eq!(same, "+   1 x\n");

    // `a` ends in LF in file 1 and in CRLF in file 2, past the same insertion.
    let (differs, code) = diff_of("a\nb\n", "x\na\r\nb\n");
    assert_eq!(code, Some(DIFF_EXIT_CODE), "{differs}");
    assert_eq!(differs, format!("+   1 x\n   ({LINE_ENDINGS_NOTE})\n"));
}

#[test]
fn missing_operand_exits_two_and_names_the_failed_path() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("existing.txt");
    let missing = dir.path().join("missing.txt");
    fs::write(&existing, "content\n").unwrap();

    for (left, right) in [(&missing, &existing), (&existing, &missing)] {
        let output = run_rtk_diff(left, right);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{stderr}");
        assert!(output.stdout.is_empty(), "read errors belong on stderr");
        assert!(stderr.contains(&*missing.to_string_lossy()), "{stderr}");
    }
}

#[test]
fn non_utf8_files_are_compared_instead_of_reported_as_io_errors() {
    let dir = tempfile::tempdir().unwrap();
    let left = dir.path().join("left.bin");
    let right = dir.path().join("right.bin");
    fs::write(&left, b"\xff").unwrap();

    // Reading succeeds even though UTF-8 conversion fails.
    for (content, expected) in [(b"\xff", 0), (b"\xfe", 1)] {
        fs::write(&right, content).unwrap();
        let output = run_rtk_diff(&left, &right);
        assert_eq!(output.status.code(), Some(expected));
        assert!(output.stderr.is_empty());
        if expected == 1 {
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(stdout.contains("Binary files"), "{stdout}");
            assert!(stdout.contains("differ"), "{stdout}");
        }
    }
}

#[test]
fn one_file_operand_is_a_usage_error_not_a_stdin_condense() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("o.txt");
    fs::write(&file, "a\n").unwrap();

    let output = common::rtk_command()
        .args([DIFF_SUBCOMMAND, &file.display().to_string()])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run rtk diff with one operand");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "a single file operand is a usage error, not a diff to condense: {stderr}"
    );
    assert!(
        stderr.contains("missing operand after"),
        "the usage error must name the form, like diff does: {stderr}"
    );
}

#[test]
fn one_missing_operand_is_a_usage_error_before_any_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let piped = dir.path().join("piped.diff");
    fs::write(&piped, "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-x\n+y\n").expect("write piped diff");
    let missing = dir.path().join("missing.txt");

    let output = common::rtk_command()
        .args([DIFF_SUBCOMMAND, &missing.display().to_string()])
        .stdin(fs::File::open(&piped).expect("open piped diff"))
        .output()
        .expect("run rtk diff with one missing operand");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("missing operand after"),
        "the usage error comes before the operand is opened: {stderr}"
    );
    assert!(
        !stderr.contains("rtk diff:"),
        "the missing operand is not read: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "stdin is not condensed on a usage error: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn explicit_stdin_dash_still_condenses() {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = common::rtk_command()
        .args([DIFF_SUBCOMMAND, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk diff -");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(b"--- a/f\n+++ b/f\n@@ -1 +1 @@\n-x\n+y\n")
        .expect("write diff to stdin");
    let output = child.wait_with_output().expect("wait for rtk diff -");

    assert_eq!(
        output.status.code(),
        Some(0),
        "the `-` form is the stdin mode"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("-x") || stdout.contains("+y"), "{stdout}");
}
