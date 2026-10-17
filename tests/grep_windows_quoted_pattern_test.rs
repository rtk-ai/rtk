#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::process::{Command, Output};

fn system_grep() -> Option<std::path::PathBuf> {
    which::which("grep").ok()
}

fn raw_grep(grep: &std::path::Path, cwd: &std::path::Path) -> Output {
    let mut command = Command::new(grep);
    command.current_dir(cwd);
    command.raw_arg("-o");
    command.raw_arg(r#""path=\"[^\"]*\"""#);
    command.raw_arg("input.txt");
    command.output().expect("run system grep")
}

#[test]
fn only_matching_quoted_regex_matches_system_grep() {
    let Some(grep) = system_grep() else {
        return;
    };
    let dir = tempfile::tempdir().expect("create temp directory");
    std::fs::write(
        dir.path().join("input.txt"),
        "path=\"/\"\npath=\"/login\"\nno match\npath=\"/register\"\n",
    )
    .expect("write grep input");

    // MSYS2 grep accepts this argv spelling from a Windows shell. `raw_arg`
    // preserves that independent baseline instead of applying Rust's C argv
    // quoting a second time.
    let expected = raw_grep(&grep, dir.path());
    assert!(
        expected.status.success(),
        "system grep failed: {expected:?}"
    );
    assert!(!expected.stdout.is_empty(), "system grep found no matches");

    let actual = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .current_dir(dir.path())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .args(["grep", "-o", r#"path="[^"]*""#, "input.txt"])
        .output()
        .expect("run rtk grep");

    assert_eq!(actual.stdout, expected.stdout, "stdout differs from grep");
    assert_eq!(
        actual.status.code(),
        expected.status.code(),
        "exit differs from grep"
    );
}

#[test]
fn quote_metacharacters_do_not_escape_batch_argument() {
    let dir = tempfile::tempdir().expect("create temp directory");
    std::fs::write(dir.path().join("grep.cmd"), "@echo off\r\n@exit /b 0\r\n")
        .expect("write grep wrapper");
    std::fs::write(dir.path().join("input.txt"), "irrelevant\n").expect("write grep input");

    let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("construct PATH");
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .current_dir(dir.path())
        .env("PATH", path)
        .args([
            "grep",
            "-o",
            r#"x"& echo INJECTED>sentinel.txt & rem ""#,
            "input.txt",
        ])
        .output()
        .expect("run rtk grep through batch wrapper");

    assert!(output.status.success(), "rtk grep failed: {output:?}");
    assert!(
        !dir.path().join("sentinel.txt").exists(),
        "quote-containing grep argument escaped into cmd.exe syntax"
    );
}
