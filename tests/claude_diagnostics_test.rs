//! Read-only diagnostics for native and legacy Claude hook installations.
#![cfg(unix)]

use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Output};

const WRAPPER: &str = "#!/bin/sh\nexec rtk hook claude\n";

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("RTK_DB_PATH", home.join("data/tracking.db"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .current_dir(home)
        .output()
        .expect("run isolated diagnostic")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

type FileSnapshot = (std::path::PathBuf, Vec<u8>, u32, i64, i64);

fn snapshot(dir: &Path) -> Vec<FileSnapshot> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("read fixture directory") {
        let path = entry.expect("read fixture entry").path();
        if path.is_dir() {
            files.extend(snapshot(&path));
        } else {
            let meta = fs::metadata(&path).expect("fixture metadata");
            files.push((
                path.clone(),
                fs::read(path).expect("fixture bytes"),
                meta.mode(),
                meta.mtime(),
                meta.mtime_nsec(),
            ));
        }
    }
    files.sort();
    files
}

#[test]
fn legacy_wrapper_diagnostics_expose_integrity_without_writes() {
    for baseline in [None, Some(WRAPPER), Some("different contents")] {
        let home = tempfile::tempdir().expect("fixture home");
        let claude = home.path().join(".claude");
        let hooks = claude.join("hooks");
        fs::create_dir_all(&hooks).expect("hooks directory");
        let hook = hooks.join("rtk-rewrite.sh");
        fs::write(&hook, WRAPPER).expect("write wrapper");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("executable wrapper");
        let settings = serde_json::json!({"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":hook.to_str().expect("hook path")}]}]}});
        fs::write(claude.join("settings.json"), settings.to_string()).expect("settings");
        if let Some(contents) = baseline {
            fs::write(
                hooks.join(".rtk-hook.sha256"),
                format!("{:x}  rtk-rewrite.sh\n", Sha256::digest(contents)),
            )
            .expect("baseline");
        }
        let before = snapshot(&claude);
        let show = run(home.path(), &["init", "-g", "--show"]);
        let verify = run(home.path(), &["verify"]);
        let dry = run(home.path(), &["init", "-g", "--dry-run", "--no-patch"]);
        assert!(show.status.success(), "{}", text(&show));
        assert!(dry.status.success(), "{}", text(&dry));
        assert!(
            !text(&show).contains("native binary command"),
            "{}",
            text(&show)
        );
        match baseline {
            None => {
                assert!(text(&show).contains("no baseline hash"), "{}", text(&show));
                assert!(
                    text(&verify).contains("no baseline hash"),
                    "{}",
                    text(&verify)
                );
                assert!(text(&verify).contains("--dry-run"), "{}", text(&verify));
            }
            Some(WRAPPER) => {
                assert!(
                    text(&show).contains("hook hash verified"),
                    "{}",
                    text(&show)
                );
                assert!(
                    text(&verify).contains("PASS  hook integrity verified"),
                    "{}",
                    text(&verify)
                );
            }
            Some(_) => {
                assert!(text(&show).contains("[FAIL] Integrity"), "{}", text(&show));
                assert!(
                    text(&verify).contains("FAIL  hook integrity check FAILED"),
                    "{}",
                    text(&verify)
                );
                assert!(!verify.status.success());
            }
        }
        assert!(
            text(&dry).contains("would migrate legacy hook script"),
            "{}",
            text(&dry)
        );
        assert!(text(&show).contains("--dry-run"), "{}", text(&show));
        assert_eq!(
            before,
            snapshot(&claude),
            "diagnostics modified configuration"
        );
    }
}

#[test]
fn native_registration_keeps_leftover_script_integrity_separate() {
    for (command, leftover, mismatch) in [
        ("rtk hook claude", false, false),
        ("/opt/homebrew/bin/rtk hook claude", false, false),
        ("rtk hook claude", true, false),
        ("rtk hook claude", true, true),
    ] {
        let home = tempfile::tempdir().expect("fixture home");
        let claude = home.path().join(".claude");
        fs::create_dir_all(claude.join("hooks")).expect("hooks directory");
        let settings = serde_json::json!({"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":command}]}]}});
        fs::write(claude.join("settings.json"), settings.to_string()).expect("settings");
        if leftover {
            fs::write(claude.join("hooks/rtk-rewrite.sh"), WRAPPER).expect("leftover wrapper");
        }
        if mismatch {
            fs::write(
                claude.join("hooks/.rtk-hook.sha256"),
                format!(
                    "{:x}  rtk-rewrite.sh\n",
                    Sha256::digest("different contents")
                ),
            )
            .expect("mismatched baseline");
        }
        let before = snapshot(&claude);
        let show = run(home.path(), &["init", "-g", "--show"]);
        assert!(show.status.success(), "{}", text(&show));
        assert!(
            text(&show).contains("[ok] Hook: rtk hook claude (native binary command)"),
            "{}",
            text(&show)
        );
        if mismatch {
            assert!(
                text(&show).contains("[FAIL] Legacy script integrity:"),
                "{}",
                text(&show)
            );
            assert!(
                !text(&show).contains("[FAIL] Integrity:"),
                "{}",
                text(&show)
            );
        } else if leftover {
            assert!(
                text(&show).contains("Legacy script integrity: no baseline hash"),
                "{}",
                text(&show)
            );
        } else {
            assert!(!text(&show).contains("[warn]"), "{}", text(&show));
        }
        assert_eq!(before, snapshot(&claude));
    }
}
