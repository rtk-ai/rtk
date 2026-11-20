#![cfg(unix)]
//! Regression tests for #5192: BusyBox/non-GNU grep rejecting RTK's injected
//! parse flags (`--null`, `-I`, `--line-buffered`) on both file and piped stdin paths.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

mod common;

fn install_fake_busybox_grep(bin_dir: &std::path::Path) {
    let real_grep = if std::path::Path::new("/bin/grep").exists() {
        "/bin/grep"
    } else {
        "/usr/bin/grep"
    };
    let script = format!(
        "#!/bin/sh\n\
         for arg in \"$@\"; do\n\
           case \"$arg\" in\n\
             --null|-I|--line-buffered)\n\
               echo \"/bin/grep: unrecognized option: ${{arg#--}}\" >&2\n\
               exit 2\n\
               ;;\n\
           esac\n\
         done\n\
         exec {real_grep} \"$@\"\n"
    );
    let grep_path = bin_dir.join("grep");
    std::fs::write(&grep_path, script).unwrap();
    let mut perms = std::fs::metadata(&grep_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&grep_path, perms).unwrap();
}

fn rtk_with_fake_grep(bin_dir: &std::path::Path) -> Command {
    let orig_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{orig_path}", bin_dir.display());
    let mut cmd = common::rtk_command();
    cmd.env("LC_ALL", "C").env("PATH", new_path);
    cmd
}

#[test]
fn busybox_grep_file_search_falls_back_to_passthrough() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    install_fake_busybox_grep(&bin_dir);

    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello world\nbye\n").unwrap();

    let out = rtk_with_fake_grep(&bin_dir)
        .args(["grep", "hello", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}, stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello world"), "stdout={stdout:?}");
}

#[test]
fn busybox_grep_piped_stdin_falls_back_to_passthrough() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    install_fake_busybox_grep(&bin_dir);

    let mut child = rtk_with_fake_grep(&bin_dir)
        .args(["grep", "hello"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"hello from stdin\nother line\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}, stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello from stdin"), "stdout={stdout:?}");
}

#[test]
fn busybox_grep_preserves_genuine_error_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    install_fake_busybox_grep(&bin_dir);

    let file = dir.path().join("a.txt");
    let missing = dir.path().join("missing.txt");
    std::fs::write(&file, "hello\n").unwrap();

    let out = rtk_with_fake_grep(&bin_dir)
        .args(["grep", "hello", file.to_str().unwrap(), missing.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("missing.txt"), "stderr={stderr:?}");
}
