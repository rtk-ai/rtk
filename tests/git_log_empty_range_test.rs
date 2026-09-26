use std::process::Command;

fn init_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@t.t"][..],
        &["config", "user.name", "t"][..],
        &["commit", "-q", "--allow-empty", "-m", "init"][..],
    ] {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git setup failed: {args:?}");
    }
    dir
}

// #3365: an empty range must print nothing, or `| wc -l` counts 1 instead of 0.
#[test]
fn git_log_empty_range_produces_no_output() {
    let dir = init_git_repo();

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["git", "log", "--oneline", "HEAD..HEAD"])
        .current_dir(dir.path())
        .output()
        .expect("spawn rtk");

    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "expected empty stdout for an empty commit range, got {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}
