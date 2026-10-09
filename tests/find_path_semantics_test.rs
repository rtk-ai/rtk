//! Bare find paths stay paths when rtk runs the real binary.
//!
//! A file root, several roots, or a missing name must agree with the platform
//! `find` on exit status, visited paths, and what `-delete` removes. These
//! comparisons need GNU or BSD find, so they are Unix-only. Argv dispatch is
//! covered on every platform by the `find_cmd` unit tests. Nothing here deletes
//! outside its own temporary directory, and the test process cwd is left alone.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARKER: &str = "and-reached";

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

fn sandbox(build: fn(&Path)) -> Sandbox {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("work");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&root).expect("work dir");
    std::fs::create_dir_all(&home).expect("home dir");
    build(&root);
    Sandbox {
        _dir: dir,
        root,
        home,
    }
}

fn write_file(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, bytes).expect("write fixture");
}

/// Pins home, config, and tracking so a developer machine cannot leak into the run.
fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", home.join("claude-absent"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("RTK_TEE_DIR", home.join("tee"))
        .env("LC_ALL", "C")
        .env_remove("RTK_SUPPRESS_HOOK_WARNING");
}

fn run(sb: &Sandbox, program: &str, args: &[&str]) -> Output {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(&sb.root);
    isolate(&mut cmd, &sb.home);
    cmd.output()
        .unwrap_or_else(|err| panic!("spawn {program}: {err}"))
}

fn rtk_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut all = Vec::with_capacity(args.len() + 1);
    all.push("find");
    all.extend_from_slice(args);
    all
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("process exited via signal")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn snapshot(root: &Path) -> Vec<String> {
    let mut names = Vec::new();
    collect(root, root, &mut names);
    names.sort();
    names
}

fn collect(root: &Path, dir: &Path, names: &mut Vec<String>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|err| panic!("read {}: {err}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .expect("entry stays under the fixture")
            .to_string_lossy()
            .replace('\\', "/");
        if rel == MARKER {
            continue;
        }
        let file_type = entry.file_type().expect("file type");
        if file_type.is_dir() {
            names.push(format!("{rel}/"));
            collect(root, &path, names);
        } else {
            names.push(rel);
        }
    }
}

fn assert_same_as_find(build: fn(&Path), args: &[&str]) {
    let find_sb = sandbox(build);
    let rtk_sb = sandbox(build);
    let find_out = run(&find_sb, "find", args);
    let rtk_argv = rtk_args(args);
    let rtk_out = run(&rtk_sb, env!("CARGO_BIN_EXE_rtk"), &rtk_argv);
    assert_eq!(
        code(&rtk_out),
        code(&find_out),
        "exit for {args:?}\nrtk stderr:\n{}\nfind stderr:\n{}",
        text(&rtk_out.stderr),
        text(&find_out.stderr)
    );
    assert_eq!(
        rtk_out.stdout,
        find_out.stdout,
        "stdout for {args:?}\nrtk stderr:\n{}",
        text(&rtk_out.stderr)
    );
    assert_eq!(
        snapshot(&rtk_sb.root),
        snapshot(&find_sb.root),
        "tree for {args:?}"
    );
}

/// `sh -c` so `&&` is the shell's, with each argument passed as its own argv.
/// `args` is the full argv after the program (`find …` or `rtk find …`).
fn shell_and(sb: &Sandbox, program: &str, args: &[&str]) -> i32 {
    let marker = sb.root.join(MARKER);
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(r#"bin=$1; marker=$2; shift 2; "$bin" "$@" && touch "$marker""#)
        .arg("sh")
        .arg(program)
        .arg(&marker)
        .args(args)
        .current_dir(&sb.root);
    isolate(&mut cmd, &sb.home);
    let out = cmd.output().expect("spawn sh");
    out.status.code().expect("sh exited via signal")
}

fn assert_and(build: fn(&Path), args: &[&str], reached: bool) {
    let find_sb = sandbox(build);
    let rtk_sb = sandbox(build);
    let find_code = shell_and(&find_sb, "find", args);
    let rtk_code = shell_and(&rtk_sb, env!("CARGO_BIN_EXE_rtk"), &rtk_args(args));
    assert_eq!(rtk_code, find_code, "&& exit for {args:?}");
    assert_eq!(
        find_sb.root.join(MARKER).exists(),
        reached,
        "platform find && for {args:?}"
    );
    assert_eq!(
        rtk_sb.root.join(MARKER).exists(),
        reached,
        "rtk && for {args:?}"
    );
    assert_eq!(
        snapshot(&rtk_sb.root),
        snapshot(&find_sb.root),
        "tree after && for {args:?}"
    );
}

fn same_name_files(root: &Path) {
    write_file(root, "stale.log", b"root");
    write_file(root, "nested/stale.log", b"nested");
    write_file(root, "keep.txt", b"keep");
}

fn two_roots(root: &Path) {
    write_file(root, "x.tmp", b"x");
    write_file(root, "y.tmp", b"y");
    write_file(root, "nested/x.tmp", b"nested");
    write_file(root, "keep.txt", b"keep");
}

fn missing_with_decoy(root: &Path) {
    write_file(root, "nested/nodir", b"decoy");
    write_file(root, "keep.txt", b"keep");
}

fn spaced_names(root: &Path) {
    write_file(root, "my file.log", b"root");
    write_file(root, "nested/my file.log", b"nested");
    write_file(root, "keep.txt", b"keep");
}

fn glob_chars_in_name(root: &Path) {
    write_file(root, "file*.log", b"literal");
    write_file(root, "fileX.log", b"other");
    write_file(root, "nested/file*.log", b"nested");
    write_file(root, "nested/fileX.log", b"nested-other");
}

fn explicit_directory(root: &Path) {
    write_file(root, "stale.log", b"root");
    write_file(root, "sub/stale.log", b"sub");
    write_file(root, "sub/nested/stale.log", b"nested");
    write_file(root, "keep.txt", b"keep");
}

#[test]
fn bare_file_delete_removes_only_that_root() {
    assert_same_as_find(same_name_files, &["stale.log", "-delete"]);
    assert_and(same_name_files, &["stale.log", "-delete"], true);
    let sb = sandbox(same_name_files);
    let _ = run(
        &sb,
        env!("CARGO_BIN_EXE_rtk"),
        &rtk_args(&["stale.log", "-delete"]),
    );
    assert!(!sb.root.join("stale.log").exists());
    assert!(sb.root.join("nested/stale.log").exists());
    assert!(sb.root.join("keep.txt").exists());
}

#[test]
fn multiple_file_roots_delete_only_those_files() {
    assert_same_as_find(two_roots, &["x.tmp", "y.tmp", "-delete"]);
    let sb = sandbox(two_roots);
    let out = run(
        &sb,
        env!("CARGO_BIN_EXE_rtk"),
        &rtk_args(&["x.tmp", "y.tmp", "-delete"]),
    );
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(!sb.root.join("x.tmp").exists());
    assert!(!sb.root.join("y.tmp").exists());
    assert!(sb.root.join("nested/x.tmp").exists());
    assert!(sb.root.join("keep.txt").exists());
}

#[test]
fn exec_echo_visits_only_the_named_root() {
    assert_same_as_find(same_name_files, &["stale.log", "-exec", "echo", "{}", ";"]);
    let sb = sandbox(same_name_files);
    let out = run(
        &sb,
        env!("CARGO_BIN_EXE_rtk"),
        &rtk_args(&["stale.log", "-exec", "echo", "{}", ";"]),
    );
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "stale.log\n");
    assert!(sb.root.join("stale.log").exists());
    assert!(sb.root.join("nested/stale.log").exists());
}

#[test]
fn missing_bare_root_fails_and_mentions_the_path() {
    for args in [
        &["nodir"][..],
        &["nodir", "-delete"],
        &["nodir", "-type", "d"],
        &["nodir", "-mtime", "+0"],
        &["nodir", "-name", "*.o", "-delete"],
    ] {
        let find_sb = sandbox(missing_with_decoy);
        let rtk_sb = sandbox(missing_with_decoy);
        let find_out = run(&find_sb, "find", args);
        let rtk_out = run(&rtk_sb, env!("CARGO_BIN_EXE_rtk"), &rtk_args(args));
        assert_ne!(code(&find_out), 0, "find control succeeded for {args:?}");
        assert_eq!(
            code(&rtk_out),
            code(&find_out),
            "exit for {args:?}\nrtk stderr:\n{}\nfind stderr:\n{}",
            text(&rtk_out.stderr),
            text(&find_out.stderr)
        );
        assert!(
            text(&rtk_out.stderr).contains("nodir"),
            "rtk stderr for {args:?}:\n{}",
            text(&rtk_out.stderr)
        );
        assert!(
            text(&find_out.stderr).contains("nodir"),
            "find stderr for {args:?}:\n{}",
            text(&find_out.stderr)
        );
        assert!(
            !text(&rtk_out.stdout).contains("nested"),
            "missing root must not list the decoy:\n{}",
            text(&rtk_out.stdout)
        );
        assert!(rtk_sb.root.join("nested/nodir").exists(), "{args:?}");
        assert_eq!(snapshot(&rtk_sb.root), snapshot(&find_sb.root), "{args:?}");
    }
}

#[test]
fn missing_root_does_not_reach_shell_and() {
    assert_and(missing_with_decoy, &["nodir"], false);
    assert_and(missing_with_decoy, &["nodir", "-delete"], false);
    let sb = sandbox(missing_with_decoy);
    let _ = shell_and(
        &sb,
        env!("CARGO_BIN_EXE_rtk"),
        &rtk_args(&["nodir", "-delete"]),
    );
    assert!(!sb.root.join(MARKER).exists());
    assert!(sb.root.join("nested/nodir").exists());
}

#[test]
fn quoted_root_with_spaces_is_a_single_path() {
    assert_same_as_find(spaced_names, &["my file.log", "-exec", "echo", "{}", ";"]);
    assert_same_as_find(spaced_names, &["my file.log", "-delete"]);
    let sb = sandbox(spaced_names);
    let out = run(
        &sb,
        env!("CARGO_BIN_EXE_rtk"),
        &rtk_args(&["my file.log", "-exec", "echo", "{}", ";"]),
    );
    assert_eq!(text(&out.stdout), "my file.log\n");
    let _ = run(
        &sb,
        env!("CARGO_BIN_EXE_rtk"),
        &rtk_args(&["my file.log", "-delete"]),
    );
    assert!(!sb.root.join("my file.log").exists());
    assert!(sb.root.join("nested/my file.log").exists());
}

#[test]
fn dot_slash_keeps_glob_characters_in_the_file_name() {
    assert_same_as_find(glob_chars_in_name, &["./file*.log", "-delete"]);
    let sb = sandbox(glob_chars_in_name);
    let out = run(
        &sb,
        env!("CARGO_BIN_EXE_rtk"),
        &rtk_args(&["./file*.log", "-delete"]),
    );
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(!sb.root.join("file*.log").exists());
    assert!(sb.root.join("fileX.log").exists());
    assert!(sb.root.join("nested/file*.log").exists());
    assert!(sb.root.join("nested/fileX.log").exists());
}

#[test]
fn explicit_directory_action_stays_under_that_root() {
    assert_same_as_find(
        explicit_directory,
        &["sub", "-name", "stale.log", "-delete"],
    );
    let sb = sandbox(explicit_directory);
    let out = run(
        &sb,
        env!("CARGO_BIN_EXE_rtk"),
        &rtk_args(&["sub", "-name", "stale.log", "-delete"]),
    );
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(sb.root.join("stale.log").exists());
    assert!(!sb.root.join("sub/stale.log").exists());
    assert!(!sb.root.join("sub/nested/stale.log").exists());
    assert!(sb.root.join("keep.txt").exists());
}
