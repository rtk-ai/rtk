//! `rtk git ls-tree` folds a plain path list (`--name-only`, `--name-status`) into one line per
//! directory run and hands every other form through byte-identical to git.

use std::path::Path;
use std::process::{Command, Output, Stdio};

mod common;

const FILES: &[&str] = &[
    "Cargo.toml",
    "README.md",
    "my file.txt",
    "dir with space/a.rs",
    "quote\"name.txt",
    "src/\u{e9}.rs",
    "src/lib.rs",
    "src/main.rs",
    "src/cmds/alpha.rs",
    "src/cmds/beta.rs",
    "src/cmds/gamma.rs",
    "tests/one.rs",
    "tests/two.rs",
];

fn git(repo: &Path) -> Command {
    let mut cmd = common::native_command("git");
    common::isolate_git(&mut cmd);
    cmd.current_dir(repo);
    cmd
}

fn repo_with_awkward_paths() -> tempfile::TempDir {
    let repo = common::temp_git_repo();
    let blob = git(repo.path())
        .args(["hash-object", "-w", "--stdin"])
        .stdin(Stdio::null())
        .output()
        .expect("git hash-object");
    assert!(blob.status.success(), "{blob:?}");
    let oid = String::from_utf8(blob.stdout).expect("object id");
    for file in FILES {
        // Git for Windows validates index paths too; these names never reach the filesystem.
        let add = git(repo.path())
            .args([
                "-c",
                "core.protectNTFS=false",
                "update-index",
                "--add",
                "--cacheinfo",
                "100644",
                oid.trim(),
                file,
            ])
            .output()
            .expect("git update-index");
        assert!(add.status.success(), "{file:?}: {add:?}");
    }
    let commit = git(repo.path())
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "files",
        ])
        .output()
        .expect("git commit");
    assert!(commit.status.success(), "{commit:?}");
    repo
}

fn rtk_ls_tree(repo: &Path, args: &[&str]) -> Output {
    let mut cmd = common::rtk_command();
    cmd.env("RTK_NO_TRACK", "1").current_dir(repo).arg("git");
    cmd.args(args).output().expect("rtk git ls-tree")
}

fn native_ls_tree(repo: &Path, args: &[&str]) -> Output {
    git(repo).args(args).output().expect("git ls-tree")
}

/// Reads a folded listing back into git's lines: `<dir>/ name…` (`./` for no directory part), or
/// a quoted path on its own line, verbatim when it holds a `\` escape and unwrapped otherwise.
fn unfold(folded: &str) -> String {
    let mut paths = String::new();
    for line in folded.lines() {
        if let Some(inner) = line.strip_prefix('"') {
            if line.contains('\\') {
                paths.push_str(line);
            } else {
                paths.push_str(inner.strip_suffix('"').expect("closing quote"));
            }
            paths.push('\n');
            continue;
        }
        let mut words = line.split(' ');
        let dir = words.next().expect("directory");
        assert!(dir.ends_with('/'), "{line:?}");
        let dir = if dir == "./" { "" } else { dir };
        for name in words {
            paths.push_str(dir);
            paths.push_str(name);
            paths.push('\n');
        }
    }
    paths
}

#[test]
fn name_only_listing_folds_and_every_path_rebuilds() {
    let repo = repo_with_awkward_paths();
    for args in [
        &["ls-tree", "-r", "--name-only", "HEAD"][..],
        &["ls-tree", "-r", "--name-status", "HEAD"],
        &[
            "-c",
            "core.quotePath=false",
            "ls-tree",
            "-r",
            "--name-only",
            "HEAD",
        ],
    ] {
        let native = native_ls_tree(repo.path(), args);
        let rtk = rtk_ls_tree(repo.path(), args);
        assert_eq!(rtk.status.code(), Some(0), "{args:?}: {rtk:?}");
        let native = String::from_utf8(native.stdout).expect("utf-8 listing");
        let folded = String::from_utf8(rtk.stdout).expect("utf-8 fold");
        assert_ne!(folded, native, "{args:?} should fold");
        assert!(folded.len() < native.len(), "{args:?}:\n{folded}");
        assert_eq!(unfold(&folded), native, "{args:?}:\n{folded}");
        assert!(
            folded.contains("src/cmds/ alpha.rs beta.rs gamma.rs\n"),
            "{args:?}:\n{folded}"
        );
        assert!(folded.contains("\"my file.txt\"\n"), "{args:?}:\n{folded}");
        assert!(
            folded.contains("\"dir with space/a.rs\"\n"),
            "{args:?}:\n{folded}"
        );
    }
}

#[test]
fn current_directory_tree_entry_is_byte_identical_to_git() {
    let repo = repo_with_awkward_paths();
    let subdir = repo.path().join("src");
    std::fs::create_dir(&subdir).expect("create working subdirectory");
    for args in [
        &["ls-tree", "-t", "-r", "--name-only", "HEAD"][..],
        &["ls-tree", "-d", "-r", "--name-only", "HEAD"],
        &["ls-tree", "-t", "-r", "--name-only", "HEAD", "."],
    ] {
        let native = native_ls_tree(&subdir, args);
        assert!(native.status.success(), "{args:?}: {native:?}");
        assert!(native.stdout.starts_with(b"./\n"), "{args:?}: {native:?}");
        let rtk = rtk_ls_tree(&subdir, args);
        assert_eq!(rtk.status.code(), native.status.code(), "{args:?}");
        assert_eq!(rtk.stdout, native.stdout, "{args:?}");
        assert_eq!(rtk.stderr, native.stderr, "{args:?}");
    }
}

#[test]
fn other_forms_and_lookups_are_byte_identical_to_git() {
    let repo = repo_with_awkward_paths();
    for args in [
        &["ls-tree", "HEAD"][..],
        &["ls-tree", "-r", "HEAD"],
        &["ls-tree", "-r", "-l", "HEAD"],
        &["ls-tree", "-r", "--long", "--name-only", "HEAD"],
        &["ls-tree", "-r", "-z", "--name-only", "HEAD"],
        &["ls-tree", "-r", "--format=%(objectname) %(path)", "HEAD"],
        &["ls-tree", "-r", "--object-only", "HEAD"],
        &["ls-tree", "-r", "--name-o", "HEAD"],
        &["ls-tree", "--name-only", "HEAD", "Cargo.toml"],
        &["ls-tree", "--name-only", "HEAD", "src/main.rs"],
        &["ls-tree", "--name-only", "HEAD", "absent.rs"],
        &["ls-tree", "--name-only", "HEAD", "--", "src/cmds/alpha.rs"],
        &["ls-tree", "-r", "--name-only", "no-such-ref"],
    ] {
        let native = native_ls_tree(repo.path(), args);
        let rtk = rtk_ls_tree(repo.path(), args);
        assert_eq!(rtk.status.code(), native.status.code(), "{args:?}");
        assert_eq!(
            rtk.stdout,
            native.stdout,
            "{args:?}:\nrtk:\n{}\ngit:\n{}",
            String::from_utf8_lossy(&rtk.stdout),
            String::from_utf8_lossy(&native.stdout)
        );
        assert_eq!(
            rtk.stderr.is_empty(),
            native.stderr.is_empty(),
            "{args:?}: stderr reaches the caller"
        );
    }
}
