//! Integration test: `rtk pipeline share` finds the project's command rows
//! when the working directory is not spelled as the tracker records it. The
//! tracker stores the canonical path; on Windows `current_dir()` never is one
//! (`canonicalize` adds `\\?\`), and on Unix a symlinked directory is not.

use std::path::{Path, PathBuf};

mod common;

/// Pin every directory the binary resolves from the environment to `home`.
fn isolating_env(home: &Path) -> Vec<(String, std::ffi::OsString)> {
    vec![
        ("HOME".into(), home.as_os_str().to_owned()),
        ("RTK_DB_PATH".into(), home.join("rtk.db").into_os_string()),
        (
            "XDG_CONFIG_HOME".into(),
            home.join(".config").into_os_string(),
        ),
        (
            "XDG_DATA_HOME".into(),
            home.join(".local").join("share").into_os_string(),
        ),
    ]
}

/// Run rtk with `args` in `cwd`, with rtk's state under `home`.
fn rtk(home: &Path, cwd: &Path, args: &[&std::ffi::OsStr]) -> String {
    let output = common::rtk_command()
        .args(args)
        .current_dir(cwd)
        .envs(isolating_env(home))
        .env("LC_ALL", "C")
        .output()
        .expect("spawn rtk");
    assert!(
        output.status.success(),
        "rtk {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A project directory reached through a path that is not its canonical one.
#[cfg(unix)]
fn non_canonical_project(home: &Path) -> PathBuf {
    let real = home.join("project");
    std::fs::create_dir(&real).expect("create project dir");
    let link = home.join("project-link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink project dir");
    link
}

#[cfg(windows)]
fn non_canonical_project(home: &Path) -> PathBuf {
    let dir = home.join("project");
    std::fs::create_dir(&dir).expect("create project dir");
    dir
}

#[test]
fn share_finds_the_rows_recorded_under_the_canonical_project_path() {
    let home = common::tempdir();
    let project = non_canonical_project(home.path());
    assert_ne!(
        project.canonicalize().expect("canonical project path"),
        project,
        "the test needs a working directory that is not canonical"
    );

    let mut content = String::new();
    for i in 0..10 {
        content.push_str(&format!("line {i:04} lorem ipsum dolor sit amet\n"));
    }
    std::fs::write(project.join("read.txt"), content).expect("write fixture file");
    rtk(
        home.path(),
        &project,
        &["read".as_ref(), "read.txt".as_ref()],
    );

    let card = rtk(
        home.path(),
        &project,
        &["pipeline".as_ref(), "share".as_ref(), "--md".as_ref()],
    );
    assert!(!card.contains("No recorded activity yet"), "{card}");
    assert!(
        card.contains("| command filtering (rtk git/npm/…) | 1 commands |"),
        "{card}"
    );
}
