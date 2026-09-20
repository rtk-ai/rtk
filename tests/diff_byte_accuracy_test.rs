use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const RTK_BIN: &str = env!("CARGO_BIN_EXE_rtk");
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

    Command::new(RTK_BIN)
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
