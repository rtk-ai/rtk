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

#[test]
fn gemini_uninstall_preserves_artifacts_when_settings_cannot_be_parsed() {
    for original in ["", "{", "{\"hooks\": {},}"] {
        for dry_run in [false, true] {
            let project = common::tempdir();
            let home = common::tempdir();
            let install = rtk(project.path(), home.path())
                .args(["init", "-g", "--gemini", "--auto-patch"])
                .output()
                .unwrap();
            assert!(install.status.success(), "{install:?}");

            let gemini = home.path().join(".gemini");
            let settings = gemini.join("settings.json");
            let hook = gemini.join("hooks/rtk-hook-gemini.sh");
            let awareness = gemini.join("GEMINI.md");
            let hook_before = std::fs::read(&hook).unwrap();
            let awareness_before = std::fs::read(&awareness).unwrap();
            std::fs::write(&settings, original).unwrap();

            let mut command = rtk(project.path(), home.path());
            command.args(["init", "-g", "--gemini", "--uninstall"]);
            if dry_run {
                command.arg("--dry-run");
            }
            let uninstall = command.output().unwrap();
            assert!(!uninstall.status.success(), "{uninstall:?}");
            let error = String::from_utf8_lossy(&uninstall.stderr);
            assert!(error.contains("settings.json"), "{error}");
            assert!(error.contains("parse"), "{error}");
            assert_eq!(std::fs::read_to_string(&settings).unwrap(), original);
            assert_eq!(std::fs::read(&hook).unwrap(), hook_before);
            assert_eq!(std::fs::read(&awareness).unwrap(), awareness_before);
        }
    }
}
