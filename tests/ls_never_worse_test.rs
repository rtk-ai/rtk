#![cfg(unix)]

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

mod common;

fn tokens(text: &[u8]) -> usize {
    text.len().div_ceil(4)
}

fn native_ls(dir: &Path, args: &[&str]) -> Output {
    common::native_command("ls")
        .env("LC_ALL", "C")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("ls")
}

fn rtk_ls(dir: &Path, args: &[&str]) -> Output {
    common::rtk_command()
        .env("LC_ALL", "C")
        .arg("ls")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("rtk ls")
}

fn compare(dir: &Path, args: &[&str]) -> (String, String) {
    let native = native_ls(dir, args);
    let rtk = rtk_ls(dir, args);
    assert_eq!(
        rtk.status.code(),
        native.status.code(),
        "exit code for ls {args:?}"
    );
    assert!(
        tokens(&rtk.stdout) <= tokens(&native.stdout),
        "rtk ls {args:?} is larger than native ls:\n--- rtk\n{}--- ls\n{}",
        String::from_utf8_lossy(&rtk.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
    (
        String::from_utf8_lossy(&rtk.stdout).into_owned(),
        String::from_utf8_lossy(&native.stdout).into_owned(),
    )
}

fn file(dir: &Path, name: &str, len: u64) {
    File::create(dir.join(name))
        .and_then(|f| f.set_len(len))
        .expect("create file");
}

fn home_like(dir: &Path) {
    for name in ["Applications", "Desktop", "Documents", "WORKSPACES", "go"] {
        fs::create_dir(dir.join(name)).expect("create dir");
    }
    file(dir, "kubectl", 60_000_000);
    file(dir, "rtk-bench-gka.json", 2_300);
    file(dir, "rtk-registry-backup.yaml", 557);
    file(dir, "empty", 0);
    std::os::unix::fs::symlink("kubectl", dir.join("kube-link")).expect("symlink");
}

#[test]
fn bare_ls_is_native_ls_when_compacting_would_cost_more() {
    let dir = common::tempdir();
    home_like(dir.path());
    for args in [&[][..], &["."], &["-t"], &["-r"], &["-S"]] {
        let (rtk, native) = compare(dir.path(), args);
        assert_eq!(rtk, native, "rtk ls {args:?}");
    }
}

#[test]
fn bare_ls_never_prints_sizes() {
    let dir = common::tempdir();
    home_like(dir.path());
    fs::create_dir(dir.path().join("node_modules")).expect("create dir");
    for n in 0..60 {
        file(
            dir.path(),
            &format!("module_with_a_long_name_{n:02}.rs"),
            1_234,
        );
    }
    let (rtk, _) = compare(dir.path(), &[]);
    assert!(!rtk.contains("1.2K"), "{rtk}");
    assert!(!rtk.contains("57.2M"), "{rtk}");
    assert!(
        rtk.contains("more"),
        "the cap must report what it hid: {rtk}"
    );
}

#[test]
fn ls_variants_are_never_larger_than_native() {
    let dir = common::tempdir();
    home_like(dir.path());
    fs::create_dir(dir.path().join("target")).expect("create dir");
    fs::create_dir(dir.path().join(".git")).expect("create dir");
    file(dir.path(), ".env", 10);
    for args in [
        &[][..],
        &["-a"],
        &["-A"],
        &["-1"],
        &["-F"],
        &["-p"],
        &["-d", "go"],
        &["go"],
        &["go", "Desktop"],
        &["-R"],
        &["-l"],
        &["-la"],
        &["-lh"],
        &["-ltr"],
    ] {
        compare(dir.path(), args);
    }
}

#[test]
fn failed_bare_ls_prints_what_ls_printed() {
    let dir = common::tempdir();
    home_like(dir.path());
    for args in [&["go", "missing-entry"][..], &["missing-entry", "."]] {
        let native = native_ls(dir.path(), args);
        let rtk = rtk_ls(dir.path(), args);
        assert_ne!(native.status.code(), Some(0), "ls {args:?} must fail");
        assert_eq!(rtk.status.code(), native.status.code(), "ls {args:?}");
        assert_eq!(rtk.stdout, native.stdout, "ls {args:?}");
        assert!(!rtk.stderr.is_empty(), "ls {args:?} must keep its error");
    }
}

#[test]
fn an_unreadable_directory_prints_what_ls_printed() {
    let dir = common::tempdir();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).expect("create dir");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod");
    for args in [&["locked"][..], &["-A", "locked"], &["-l", "locked"]] {
        let native = native_ls(dir.path(), args);
        if native.status.success() {
            compare(dir.path(), args);
            continue;
        }
        let rtk = rtk_ls(dir.path(), args);
        assert_eq!(rtk.status.code(), native.status.code(), "ls {args:?}");
        assert_eq!(rtk.stdout, native.stdout, "ls {args:?}");
    }
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[test]
fn a_flag_after_an_operand_is_held_to_what_ls_listed() {
    let dir = common::tempdir();
    home_like(dir.path());
    for args in [&[".", "-l"][..], &[".", "-la"], &[".", "-a"]] {
        let native = native_ls(dir.path(), args);
        let rtk = rtk_ls(dir.path(), args);
        assert!(
            tokens(&rtk.stdout) <= tokens(&native.stdout),
            "rtk ls {args:?} is larger than native ls:\n--- rtk\n{}--- ls\n{}",
            String::from_utf8_lossy(&rtk.stdout),
            String::from_utf8_lossy(&native.stdout)
        );
    }
}

#[test]
fn options_ls_rejects_never_turn_into_an_empty_success() {
    let dir = common::tempdir();
    home_like(dir.path());
    for args in [
        &["--all"][..],
        &["--all", "."],
        &["-l", "--all"],
        &["-l", "--human-readable"],
    ] {
        let native = native_ls(dir.path(), args);
        if native.status.success() {
            compare(dir.path(), args);
            continue;
        }
        let rtk = rtk_ls(dir.path(), args);
        assert!(
            !rtk.status.success() || !rtk.stdout.is_empty(),
            "rtk ls {args:?} succeeded with no output where ls failed"
        );
    }
}

#[test]
fn bare_ls_of_an_empty_directory_prints_nothing_like_ls() {
    let dir = common::tempdir();
    let (rtk, native) = compare(dir.path(), &[]);
    assert_eq!(rtk, native);
}
