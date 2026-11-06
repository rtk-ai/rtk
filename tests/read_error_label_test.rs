//! `rtk read` mirrors head/tail's error label and `tail -n 0` behavior.

mod common;

#[test]
fn read_missing_file_reports_head_or_tail_label() {
    let missing = "rtk-definitely-missing-file-xyz";

    let head = common::rtk_command()
        .args(["read", missing, "--head-lines", "1"])
        .output()
        .expect("run rtk read --head-lines");
    assert_eq!(head.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&head.stderr).contains("head:"),
        "stderr: {}",
        String::from_utf8_lossy(&head.stderr)
    );

    let tail = common::rtk_command()
        .args(["read", missing, "--tail-lines", "1"])
        .output()
        .expect("run rtk read --tail-lines");
    assert_eq!(tail.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&tail.stderr).contains("tail:"),
        "stderr: {}",
        String::from_utf8_lossy(&tail.stderr)
    );
}

#[test]
fn read_tail_zero_missing_file_succeeds() {
    let status = common::rtk_command()
        .args(["read", "rtk-definitely-missing-file-xyz", "--tail-lines", "0"])
        .status()
        .expect("run rtk read --tail-lines 0");
    assert_eq!(status.code(), Some(0));
}
