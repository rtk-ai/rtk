//! Exercise the wrapper through a deterministic native child process.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

mod common;

static STUB: LazyLock<PathBuf> = LazyLock::new(|| {
    let dir = common::scratch_dir().join("ansible-lint-bin");
    std::fs::create_dir_all(&dir).expect("stub directory");
    let path = dir.join(format!("ansible-lint{}", std::env::consts::EXE_SUFFIX));
    let result = Command::new("rustc")
        .args(["--edition=2024", "--crate-name=ansible_lint_stub"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ansible_lint_stub.rs"))
        .arg("-o")
        .arg(&path)
        .output()
        .expect("compile native stub");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    path
});

fn rtk() -> Command {
    let path = std::env::join_paths(
        std::iter::once(STUB.parent().expect("bin").to_path_buf()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .expect("PATH");
    let mut cmd = common::rtk_command();
    cmd.env("PATH", path);
    cmd
}

#[test]
fn ansible_lint_preserves_arguments_and_exit_code() {
    let dir = common::tempdir();
    let log = dir.path().join("argv");
    let db = dir.path().join("history.db");
    let output = rtk()
        .args([
            "ansible-lint",
            "--offline",
            "roles with spaces/",
            "playbooks/",
            "--",
            "-filename.yml",
        ])
        .env("RTK_DB_PATH", &db)
        .env("ANSIBLE_LINT_TEST_LOG", &log)
        .output()
        .expect("rtk");
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(log).expect("argv"),
        b"--offline\0roles with spaces/\0playbooks/\0--\0-filename.yml\0"
    );
    let shown = String::from_utf8_lossy(&output.stdout);
    assert!(shown.contains("name[missing]: 6"), "{shown}");
    assert!(shown.contains("Failed: 6 failure(s)"), "{shown}");
    assert!(shown.contains("… +3 more"), "{shown}");
    let hash = shown
        .split("rtk recall ")
        .nth(1)
        .expect("recovery hint")
        .split(']')
        .next()
        .expect("hash");
    let recalled = common::rtk_command()
        .args(["recall", hash])
        .output()
        .expect("recall");
    assert!(
        recalled.status.success(),
        "{}",
        String::from_utf8_lossy(&recalled.stderr)
    );
    assert!(String::from_utf8_lossy(&recalled.stdout).contains("main.yml:7"));
    let history = common::rtk_command()
        .args(["gain", "--history"])
        .env("RTK_DB_PATH", db)
        .output()
        .expect("gain");
    assert!(history.status.success());
    assert!(String::from_utf8_lossy(&history.stdout).contains("ansible-lint"));
}

#[test]
fn explicit_output_modes_preserve_both_streams_and_exit_status() {
    for args in [
        vec!["--help"],
        vec!["-h"],
        vec!["--version"],
        vec!["-L"],
        vec!["-T"],
        vec!["--format=json"],
        vec!["-f", "sarif"],
        vec!["--format", "pep8"],
    ] {
        let out = rtk()
            .arg("ansible-lint")
            .args(&args)
            .env("ANSIBLE_LINT_TEST_MODE", "passthrough")
            .output()
            .expect("rtk");
        assert_eq!(out.status.code(), Some(17), "{args:?}");
        assert_eq!(out.stdout, b"\x1b[31m{\"items\": []}\x1b[0m", "{args:?}");
        assert_eq!(out.stderr, b"native warning without newline", "{args:?}");
    }
}

#[test]
fn summaries_unknown_failures_and_silent_success() {
    for (mode, code, expected) in [
        ("empty", 0, ""),
        (
            "clean",
            0,
            "Passed: 0 failure(s), 0 warning(s) in 1 files processed of 1 encountered.\n",
        ),
        (
            "unknown",
            9,
            "ERROR custom rule could not load\n  check configuration\n",
        ),
    ] {
        let out = rtk()
            .args(["ansible-lint", "--offline", "-p"])
            .env("ANSIBLE_LINT_TEST_MODE", mode)
            .output()
            .expect("rtk");
        assert_eq!(out.status.code(), Some(code));
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
    }
}

#[test]
fn disabled_recovery_keeps_all_samples() {
    let out = rtk()
        .args(["ansible-lint", "--offline"])
        .env("RTK_RECALL", "0")
        .output()
        .expect("rtk");
    assert_eq!(out.status.code(), Some(2));
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(shown.contains("main.yml:7"));
    assert!(!shown.contains("… +3 more"));
}

#[test]
fn leading_boundary_and_native_paths_are_preserved() {
    let dir = common::tempdir();
    let log = dir.path().join("argv");
    let mut args = vec![std::ffi::OsString::from("--"), "--help".into()];
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        args.push(std::ffi::OsString::from_vec(b"role-\xff.yml".to_vec()));
    }
    let out = rtk()
        .arg("ansible-lint")
        .args(&args)
        .env("ANSIBLE_LINT_TEST_LOG", &log)
        .env("ANSIBLE_LINT_TEST_MODE", "empty")
        .output()
        .expect("rtk");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let expected: Vec<u8> = args
        .iter()
        .flat_map(|s| s.as_encoded_bytes().iter().copied().chain([0]))
        .collect();
    assert_eq!(std::fs::read(log).expect("argv"), expected);
}

#[test]
fn hook_rewrites_ansible_lint_only() {
    let out = common::rtk_command()
        .args(["rewrite", "ansible-lint --offline -p roles/"])
        .output()
        .expect("rewrite");
    assert!(matches!(out.status.code(), Some(0 | 3)));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "rtk ansible-lint --offline -p roles/"
    );
    let out = common::rtk_command()
        .args(["rewrite", "ansible-lint-extra roles/"])
        .output()
        .expect("rewrite");
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
}
