#![cfg(unix)]
//! #5199: the verbatim re-run must fire only for BusyBox-style parse-aid
//! rejection, never for a genuine engine error that occurs after matches were
//! already found (otherwise the whole search runs twice and filtered output
//! is replaced by raw).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

mod common;

fn rtk_with_fake_grep(bin_dir: &std::path::Path) -> Command {
    let orig_path = std::env::var("PATH").unwrap_or_default();
    let mut cmd = common::rtk_command();
    cmd.env("LC_ALL", "C")
        .env("PATH", format!("{}:{orig_path}", bin_dir.display()));
    cmd
}

/// Emits one match line, then exits 2 with a genuine "Permission denied"
/// error — no unsupported-option signature.
fn install_late_error_grep(bin_dir: &std::path::Path) {
    let script = "#!/bin/sh\n\
                  printf '%s\\n' 'a.txt:1:hello world'\n\
                  echo 'grep: a.txt: Permission denied' >&2\n\
                  exit 2\n";
    let grep_path = bin_dir.join("grep");
    std::fs::write(&grep_path, script).unwrap();
    let mut perms = std::fs::metadata(&grep_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&grep_path, perms).unwrap();
}

#[test]
fn genuine_late_error_is_not_re_run() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    install_late_error_grep(&bin_dir);

    let out = rtk_with_fake_grep(&bin_dir)
        .args(["grep", "hello", "a.txt"])
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(2),
        "genuine error must surface as-is"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.matches("hello world").count(),
        1,
        "search must not run twice: stdout={stdout:?}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Permission denied"),
        "original engine error must remain visible"
    );
}

#[test]
fn unsupported_parse_flag_still_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();

    // Reject RTK's parse aids, then behave like grep on the verbatim argv.
    let script = "#!/bin/sh\n\
                  for arg in \"$@\"; do\n\
                    case \"$arg\" in\n\
                      --null|-I|--line-buffered)\n\
                        echo \"/bin/grep: unrecognized option: ${arg#--}\" >&2\n\
                        exit 2\n\
                        ;;\n\
                    esac\n\
                  done\n\
                  printf '%s\\n' 'a.txt:1:hello world'\n\
                  exit 0\n";
    let grep_path = bin_dir.join("grep");
    std::fs::write(&grep_path, script).unwrap();
    let mut perms = std::fs::metadata(&grep_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&grep_path, perms).unwrap();

    let out = rtk_with_fake_grep(&bin_dir)
        .args(["grep", "hello", "a.txt"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "fallback must succeed, got {:?}, stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("hello world"),
        "fallback must show the match"
    );
}

/// Piped-stdin variants: `-o` routes straight to passthrough, where the
/// `--line-buffered` retry must fire only for a flag rejection.
fn install_stdin_grep(bin_dir: &std::path::Path, script: &str) {
    let grep_path = bin_dir.join("grep");
    std::fs::write(&grep_path, script).unwrap();
    let mut perms = std::fs::metadata(&grep_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&grep_path, perms).unwrap();
}

fn run_piped(bin_dir: &std::path::Path, needle: &str) -> std::process::Output {
    let mut child = rtk_with_fake_grep(bin_dir)
        .args(["grep", "-o", needle, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn rtk");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{needle}\n").as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn stdin_flag_rejection_retries_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    install_stdin_grep(
        &bin_dir,
        "#!/bin/sh\n\
         for arg in \"$@\"; do\n\
           case \"$arg\" in\n\
             --line-buffered)\n\
               echo '/bin/grep: unrecognized option: line-buffered' >&2\n\
               exit 2\n\
               ;;\n\
           esac\n\
         done\n\
         cat >/dev/null\n\
         printf 'needle\\n'\n\
         exit 0\n",
    );

    let out = run_piped(&bin_dir, "needle");
    assert!(
        out.status.success(),
        "retry without --line-buffered must succeed: {:?}, stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.matches("needle").count(),
        1,
        "match must be printed once: {stdout:?}"
    );
}

#[test]
fn stdin_genuine_late_error_is_not_re_run() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    install_stdin_grep(
        &bin_dir,
        "#!/bin/sh\n\
         cat >/dev/null\n\
         printf 'needle\\n'\n\
         echo 'grep: missing.txt: No such file or directory' >&2\n\
         exit 2\n",
    );

    let out = run_piped(&bin_dir, "needle");
    assert_eq!(
        out.status.code(),
        Some(2),
        "genuine error must surface as-is"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.matches("needle").count(),
        1,
        "stdin search must not run twice: {stdout:?}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("No such file or directory"),
        "original engine error must remain visible"
    );
}
