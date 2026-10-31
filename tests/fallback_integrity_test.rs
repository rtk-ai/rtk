#![cfg(unix)]
//! The fallback dispatcher must enforce the same legacy-hook gate as typed commands.
mod common;

use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

fn run(root: &Path, args: &[&str]) -> Output {
    common::rtk_command()
        .current_dir(root)
        .env("HOME", root)
        .env("CLAUDE_CONFIG_DIR", root.join(".claude"))
        .env("PATH", root.join("bin"))
        .env("CALLED", root.join("called"))
        .args(args)
        .output()
        .expect("run rtk")
}

fn sandbox(state: &str) -> tempfile::TempDir {
    let dir = common::tempdir();
    let root = dir.path();
    std::fs::create_dir(root.join("bin")).expect("write isolated fixture");
    for tool in [
        "make",
        "terraform",
        "yadm",
        "docker",
        "prisma",
        "aws",
        "unknown-tool",
    ] {
        let path = root.join("bin").join(tool);
        std::fs::write(
            &path,
            "#!/bin/sh\nprintf 'called\\n' >> \"$CALLED\"\nprintf 'stub output\\n'\nexit 7\n",
        )
        .expect("write isolated fixture");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("write isolated fixture");
    }
    let hooks = root.join(".claude/hooks");
    std::fs::create_dir_all(&hooks).expect("write isolated fixture");
    let original = b"#!/bin/sh\n# original fixture\n";
    if state != "missing" {
        let contents: &[u8] = if state == "tampered" {
            b"#!/bin/sh\n# modified fixture\n"
        } else {
            original
        };
        std::fs::write(hooks.join("rtk-rewrite.sh"), contents).expect("write isolated fixture");
    }
    if matches!(state, "verified" | "tampered") {
        std::fs::write(
            hooks.join(".rtk-hook.sha256"),
            format!("{:x}  rtk-rewrite.sh\n", Sha256::digest(original)),
        )
        .expect("write isolated fixture");
    }
    dir
}

#[test]
fn fallback_checks_integrity_before_dispatch() {
    let cases: &[&[&str]] = &[
        &["make", "all"],
        &["terraform", "plan"],
        &["yadm", "x"],
        &["docker", "ps", "x"],
        &["prisma", "x"],
        &["aws", "--profile", "prod", "s3", "ls"],
        &["aws", "--version"],
        &["unknown-tool", "arg"],
    ];
    for state in ["missing", "no-baseline", "verified", "tampered"] {
        for args in cases {
            let dir = sandbox(state);
            let out = run(dir.path(), args);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if state == "tampered" {
                assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
                assert!(
                    stderr.contains("hook integrity check FAILED"),
                    "{args:?}: {stderr}"
                );
                assert!(!dir.path().join("called").exists(), "{args:?} reached tool");
            } else {
                assert_eq!(out.status.code(), Some(7), "{state} {args:?}: {stderr}");
                assert!(dir.path().join("called").exists(), "{state} {args:?}");
                assert!(!stderr.contains("hook integrity check FAILED"));
            }
        }
    }
}

#[test]
fn fallback_preserves_meta_diagnostics_and_root_help() {
    let dir = sandbox("tampered");
    for args in [&["--help"][..], &["--version"], &["init", "--help"]] {
        let out = run(dir.path(), args);
        assert!(out.status.success(), "{args:?}");
        assert!(!String::from_utf8_lossy(&out.stderr).contains("hook integrity check FAILED"));
    }
    for args in [&["gain", "--badtypo"][..], &["test", "--shell"]] {
        let out = run(dir.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(!String::from_utf8_lossy(&out.stderr).contains("hook integrity check FAILED"));
    }
    assert!(!dir.path().join("called").exists());
}
