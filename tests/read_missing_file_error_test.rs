use std::process::Output;

mod common;

fn read_file(file: &str, args: &[&str]) -> Output {
    common::rtk_command()
        .arg("read")
        .arg(file)
        .args(args)
        .output()
        .expect("run rtk read")
}

#[test]
fn read_missing_file_error_names_head_or_tail_not_cat() {
    // Issue #4818: a rewritten `head -n N f` / `tail -n N f` should report
    // the original tool, not `cat`, and without the `(os error N)` suffix.
    for (flag, prog) in [("--head-lines", "head"), ("--tail-lines", "tail")] {
        let output = read_file("/tmp/rtk-test-missing-file", &[flag, "5"]);
        assert_eq!(output.status.code(), Some(1), "{prog}: {:?}", output.stderr);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr,
            format!(
                "{prog}: cannot open '/tmp/rtk-test-missing-file' for reading: No such file or directory\n"
            )
        );
    }
}

#[test]
fn read_missing_file_cat_format_kept() {
    // Plain `rtk read f` (a rewritten `cat f`) keeps the cat-style message,
    // but without the Rust `(os error N)` suffix.
    let output = read_file("/tmp/rtk-test-missing-file", &[]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr,
        "cat: /tmp/rtk-test-missing-file: No such file or directory\n"
    );
}

#[test]
fn read_tail_zero_missing_file_exits_zero() {
    // GNU `tail -n 0 missing` does not open the file and exits 0.
    let output = read_file("/tmp/rtk-test-missing-file", &["--tail-lines", "0"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
