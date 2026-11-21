//! #5107: when the recall store cannot be opened (a sandbox that cannot write the
//! data folder), a truncated result loses its `rtk recall` hint. The agent must be
//! told why, not left with a cut it cannot recover.

mod common;

fn rtk_grep_with_recall_db(
    dir: &std::path::Path,
    recall_db: &std::path::Path,
) -> std::process::Output {
    let big = dir.join("big.txt");
    let lines: String = (1..=500).map(|i| format!("{i}\n")).collect();
    std::fs::write(&big, lines).expect("write big.txt");
    let mut cmd = common::rtk_command();
    cmd.env("LC_ALL", "C")
        .env("RTK_RECALL_DB", recall_db)
        .args(["grep", "-n", "1", big.to_str().expect("utf-8 path")]);
    cmd.output().expect("run rtk grep")
}

#[test]
fn a_cut_the_store_cannot_hold_says_why_on_stderr() {
    let dir = common::tempdir();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "a file where the store's directory should be").expect("write");
    let out = rtk_grep_with_recall_db(dir.path(), &blocker.join("recall.db"));

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("more in"),
        "expected a capped listing: {stdout:?}"
    );
    assert!(!stdout.contains("rtk recall"), "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr.matches("[rtk] recall store unavailable").count(),
        1,
        "one notice per run: {stderr:?}"
    );
    assert!(stderr.contains("rtk bypass"), "{stderr:?}");
}

#[test]
fn a_cut_the_store_holds_prints_no_notice() {
    let dir = common::tempdir();
    let out = rtk_grep_with_recall_db(dir.path(), &dir.path().join("recall.db"));

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hidden: rtk recall"), "{stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("recall store unavailable"), "{stderr:?}");
}
