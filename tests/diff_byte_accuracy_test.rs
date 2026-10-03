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
const BINARY_SNIFF_BYTES: usize = 4 * 1024;

fn run_rtk_diff(file1: &Path, file2: &Path) -> Output {
    let tracking_db = file1.with_extension("rtk-test.sqlite");
    let file1 = file1.display().to_string();
    let file2 = file2.display().to_string();

    common::rtk_command()
        .args([DIFF_SUBCOMMAND, &file1, &file2])
        .env("RTK_DB_PATH", tracking_db)
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
            assert!(!stdout.contains("Binary files"), "{stdout}");
            assert!(!stdout.contains(IDENTICAL_MESSAGE), "{stdout}");
        }
    }
}

#[test]
fn nul_free_legacy_text_is_rendered_as_a_text_diff() {
    let dir = tempfile::tempdir().expect("tempdir");
    let left = dir.path().join("left.txt");
    let right = dir.path().join("right.txt");
    fs::write(&left, b"bonjour caf\xe9\nligne deux\n").expect("write left fixture");
    fs::write(&right, b"bonsoir caf\xe9\nligne deux\n").expect("write right fixture");

    let output = run_rtk_diff(&left, &right);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");

    assert_eq!(output.status.code(), Some(DIFF_EXIT_CODE), "{stdout}");
    assert!(!stdout.contains("Binary files"), "{stdout}");
    assert!(stdout.contains("bonjour caf"), "{stdout}");
    assert!(stdout.contains("bonsoir caf"), "{stdout}");
    assert!(
        stdout.contains('\u{fffd}'),
        "legacy bytes should be rendered lossily: {stdout}"
    );
}

#[test]
fn nul_bytes_are_reported_as_binary_even_when_valid_utf8() {
    let dir = tempfile::tempdir().expect("tempdir");
    let left = dir.path().join("left.bin");
    let right = dir.path().join("right.bin");
    fs::write(&left, b"before\0after\n").expect("write left fixture");
    fs::write(&right, b"before\0changed\n").expect("write right fixture");

    let output = run_rtk_diff(&left, &right);
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");

    assert_eq!(output.status.code(), Some(DIFF_EXIT_CODE), "{stdout}");
    assert!(stdout.contains("Binary files"), "{stdout}");
    assert!(
        !stdout.contains("before"),
        "binary contents must not be rendered: {stdout}"
    );
}

#[test]
fn binary_sniffing_is_limited_to_the_initial_block() {
    let dir = tempfile::tempdir().expect("tempdir");
    let left = dir.path().join("left.dat");
    let right = dir.path().join("right.dat");

    let mut inside_left = vec![b'a'; BINARY_SNIFF_BYTES + 1];
    inside_left[BINARY_SNIFF_BYTES - 1] = 0;
    let mut inside_right = inside_left.clone();
    inside_right[BINARY_SNIFF_BYTES] = b'b';
    fs::write(&left, inside_left).expect("write inside-prefix left fixture");
    fs::write(&right, inside_right).expect("write inside-prefix right fixture");

    let binary_output = run_rtk_diff(&left, &right);
    let binary_stdout = String::from_utf8(binary_output.stdout).expect("binary stdout utf8");
    assert_eq!(
        binary_output.status.code(),
        Some(DIFF_EXIT_CODE),
        "{binary_stdout}"
    );
    assert!(binary_stdout.contains("Binary files"), "{binary_stdout}");

    let mut outside_left = vec![b'a'; BINARY_SNIFF_BYTES + 2];
    outside_left[BINARY_SNIFF_BYTES] = 0;
    let mut outside_right = outside_left.clone();
    outside_right[BINARY_SNIFF_BYTES + 1] = b'b';
    fs::write(&left, outside_left).expect("write outside-prefix left fixture");
    fs::write(&right, outside_right).expect("write outside-prefix right fixture");

    let text_output = run_rtk_diff(&left, &right);
    let text_stdout = String::from_utf8(text_output.stdout).expect("text stdout utf8");
    assert_eq!(
        text_output.status.code(),
        Some(DIFF_EXIT_CODE),
        "{text_stdout}"
    );
    assert!(!text_stdout.contains("Binary files"), "{text_stdout}");
}
