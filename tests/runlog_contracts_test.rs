use std::process::Command;

#[test]
fn runlog_worktree_porcelain_preserves_native_bytes() {
    let dir = tempfile::tempdir().expect("temporary repository");
    let init = Command::new("git")
        .args(["init", "--quiet"])
        .arg(dir.path())
        .output()
        .expect("initialize repository");
    assert!(init.status.success());

    for flags in [vec!["--porcelain"], vec!["--porcelain", "-z"]] {
        let native = Command::new("git")
            .current_dir(dir.path())
            .args(["worktree", "list"])
            .args(&flags)
            .output()
            .expect("native worktree list");
        let actual = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .current_dir(dir.path())
            .env("RTK_DB_PATH", dir.path().join("tracking.db"))
            .args(["git", "worktree", "list"])
            .args(&flags)
            .output()
            .expect("RTK worktree list");
        assert!(native.status.success());
        assert_eq!(actual.status.code(), native.status.code());
        assert_eq!(actual.stdout, native.stdout, "native format for {flags:?}");
        assert_eq!(actual.stderr, native.stderr);
    }
}

#[test]
fn runlog_read_preserves_glob_string_lines() {
    let dir = tempfile::tempdir().expect("temporary source fixture");
    let source = "export default {\n  // a removable comment\n  include: [\n    'tests/**/*.test.ts',\n    \"tests/**/*.spec.ts\",\n    `workspace/**/*.tsx`,\n  ],\n  exclude: ['**/*.d.ts'],\n};\n";
    let file = dir.path().join("vitest.config.ts");
    std::fs::write(&file, source).expect("write source");
    let actual = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("RTK_DB_PATH", dir.path().join("tracking.db"))
        .arg("read")
        .arg(file)
        .output()
        .expect("RTK read");
    assert!(actual.status.success());
    let output = String::from_utf8(actual.stdout).expect("UTF-8 output");
    for line in source.lines().filter(|line| line.contains("/*")) {
        assert!(
            output.lines().any(|actual| actual == line),
            "missing source line: {line}"
        );
    }
    assert!(!output.contains("removable comment"));
}
