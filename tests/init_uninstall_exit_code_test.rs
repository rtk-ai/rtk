// Unix only: on Windows the home and configuration directories are known folders, which
// the environment cannot redirect.
#![cfg(unix)]
//! An install then uninstall reports a removal and exits 0 for each agent listed below,
//! global Codex included. Project Codex with a refused hook exits 2, see `init_codex_uninstall_test.rs`.

use std::path::Path;
use std::process::{Command, Stdio};

mod common;

fn rtk(project: &Path, home: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.current_dir(project)
        .env("HOME", home)
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    cmd
}

#[test]
fn each_agent_uninstall_exits_0() {
    // (agent flags, extra install flags)
    let agents: &[(&[&str], &[&str])] = &[
        (&["-g"], &["--auto-patch"]),
        (&["-g", "--gemini"], &["--auto-patch"]),
        (&["-g", "--codex"], &[]),
        (&["-g", "--agent", "cursor"], &["--auto-patch"]),
        (&["-g", "--agent", "pi"], &["--auto-patch"]),
        (&["-g", "--agent", "omp"], &["--auto-patch"]),
        (&["--agent", "pi"], &["--auto-patch"]),
        (&["--agent", "omp"], &["--auto-patch"]),
        (&["-g", "--agent", "vibe"], &["--auto-patch"]),
        (&["--agent", "trae"], &[]),
        (&["--agent", "antigravity"], &[]),
        (&["--agent", "droid"], &[]),
        (&["--agent", "hermes"], &[]),
    ];
    let mut failures = Vec::new();
    for (agent, install_flags) in agents {
        let project = common::tempdir();
        let home = common::tempdir();
        let install = rtk(project.path(), home.path())
            .arg("init")
            .args(*agent)
            .args(*install_flags)
            .output()
            .expect("run rtk init");
        if install.status.code() != Some(0) {
            failures.push(format!("install {agent:?}: {install:?}"));
            continue;
        }

        let uninstall = rtk(project.path(), home.path())
            .arg("init")
            .args(*agent)
            .arg("--uninstall")
            .output()
            .expect("run rtk init --uninstall");
        let removed = String::from_utf8_lossy(&uninstall.stdout).contains("RTK uninstalled");
        if uninstall.status.code() != Some(0) || !removed {
            failures.push(format!("uninstall {agent:?}: {uninstall:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
