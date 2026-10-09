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
