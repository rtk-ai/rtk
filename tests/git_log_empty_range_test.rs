mod common;

#[test]
fn empty_log_ranges_emit_no_bytes() {
    let repo = common::temp_git_repo();
    for args in [
        &["HEAD..HEAD"][..],
        &["--oneline", "HEAD..HEAD"],
        &["--graph", "HEAD..HEAD"],
        &["--pretty=format:%h %s", "HEAD..HEAD"],
        &["--max-count=0"],
    ] {
        let mut native = common::native_command("git");
        common::isolate_git(&mut native);
        let native = native
            .current_dir(repo.path())
            .arg("log")
            .args(args)
            .output()
            .expect("run native Git");
        let filtered = common::rtk_command()
            .current_dir(repo.path())
            .args(["git", "log"])
            .args(args)
            .output()
            .expect("run RTK Git");
        assert!(native.status.success(), "{args:?}: {native:?}");
        assert!(filtered.status.success(), "{args:?}: {filtered:?}");
        assert!(native.stdout.is_empty(), "native Git: {args:?}");
        assert_eq!(filtered.stdout, native.stdout, "{args:?}");
    }
}

#[test]
fn a_nonempty_oneline_log_keeps_its_line_terminator() {
    let repo = common::temp_git_repo();
    let output = common::rtk_command()
        .current_dir(repo.path())
        .args(["git", "log", "--oneline", "-1"])
        .output()
        .expect("run RTK Git");
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.ends_with(b"\n"));
    let text = String::from_utf8(output.stdout).expect("UTF-8 log");
    assert_eq!(text.lines().count(), 1);
    assert!(!text.trim().is_empty());
}
