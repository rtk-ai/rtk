#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

mod common;

fn failing_uv(dir: &Path, lines: usize) -> Command {
    let uv = dir.join("uv");
    fs::write(
        &uv,
        format!(
            "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt {lines} ]; do\n  printf 'partial result: %s\\n' \"$i\"\n  i=$((i + 1))\ndone\nprintf 'warning: stopped before completion\\n' >&2\nexit 3\n"
        ),
    )
    .expect("write uv fixture");
    fs::set_permissions(&uv, fs::Permissions::from_mode(0o755)).expect("chmod uv fixture");
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut cmd = common::rtk_command();
    cmd.args(["uv", "run", "--no-project", "script.py"])
        .env("PATH", path);
    cmd
}

#[test]
fn failed_uv_run_keeps_partial_stdout_and_exit_code() {
    let dir = common::tempdir();
    let out = failing_uv(dir.path(), 40)
        .env("RTK_RECALL", "0")
        .output()
        .expect("run rtk uv");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(out.status.code(), Some(3));
    assert!(stdout.contains("partial result: 39"), "{stdout}");
    assert!(
        stdout.contains("warning: stopped before completion"),
        "{stdout}"
    );
    assert_eq!(stdout.matches("partial result: 39").count(), 1);
}

#[test]
fn capped_failed_uv_output_has_working_recovery_hints() {
    for mode in ["sqlite", "tee"] {
        let dir = common::tempdir();
        let config_dir = dir.path().join("config");
        let config_root = if cfg!(target_os = "macos") {
            dir.path().join("Library/Application Support")
        } else {
            config_dir.clone()
        };
        fs::create_dir_all(config_root.join("rtk")).expect("create config dir");
        let config = if mode == "tee" {
            "[tee]\nenabled = true\nmode = \"always\"\n"
        } else {
            "[retriever]\nmode = \"sqlite\"\n"
        };
        fs::write(config_root.join("rtk/config.toml"), config).expect("write config");
        let recall_db = dir.path().join("recall.db");
        let out = failing_uv(dir.path(), 120)
            .env("HOME", dir.path())
            .env("XDG_CONFIG_HOME", &config_dir)
            .env("RTK_TEE_DIR", dir.path().join("tee"))
            .env("RTK_RECALL_DB", &recall_db)
            .output()
            .expect("run rtk uv");
        let stdout = String::from_utf8_lossy(&out.stdout);

        assert_eq!(out.status.code(), Some(3));
        assert!(stdout.contains("partial result: 0"), "{mode}: {stdout}");
        assert!(stdout.contains("partial result: 119"), "{mode}: {stdout}");
        assert!(stdout.contains("lines omitted"), "{mode}: {stdout}");
        assert!(
            stdout.contains("warning: stopped before completion"),
            "{mode}: {stdout}"
        );
        let hint = stdout
            .lines()
            .find_map(|line| line.strip_prefix("[full output: ")?.strip_suffix(']'))
            .expect("full merged output recovery hint");
        let recovered = if mode == "tee" {
            let path = hint
                .strip_prefix("~/")
                .map(|p| dir.path().join(p))
                .unwrap_or_else(|| hint.into());
            fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("tee hint {hint:?}, path {path:?}: {e}; {stdout}"))
        } else {
            let hash = hint.strip_prefix("rtk recall ").expect("recall command");
            let recall = common::rtk_command()
                .args(["recall", hash, "--full"])
                .env("RTK_RECALL_DB", &recall_db)
                .output()
                .expect("recall full output");
            assert!(recall.status.success());
            String::from_utf8(recall.stdout).expect("UTF-8 fixture output")
        };
        for i in 0..120 {
            assert!(
                recovered.contains(&format!("partial result: {i}\n")),
                "{mode}: {i}"
            );
        }
        assert!(recovered.contains("warning: stopped before completion"));
    }
}
