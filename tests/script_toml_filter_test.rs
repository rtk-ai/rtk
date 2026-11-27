#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn command(root: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root)
        .env("ARGV_FILE", root.join("argv"));
    cmd
}

#[test]
fn trusted_script_filters_agree_in_hooks_and_execution() {
    let dir = common::tempdir();
    let root = dir.path();
    fs::create_dir(root.join("scripts")).expect("script directory");
    fs::create_dir(root.join(".rtk")).expect("project filter directory");
    for name in ["probe.sh", "plainprobe"] {
        let script = root.join("scripts").join(name);
        fs::write(&script, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGV_FILE\"\ni=0\nwhile [ \"$i\" -lt 40 ]; do printf 'noise progress\\n'; i=$((i+1)); done\nprintf 'keep result\\n'\nprintf 'script diagnostic\\n' >&2\nexit \"${SCRIPT_EXIT:-0}\"\n").expect("write script");
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).expect("executable script");
    }
    let filters = root.join(".rtk/filters.toml");
    let contents = r#"schema_version = 1
[filters.shellprobe]
match_command = '^(?:bash|sh)\b'
strip_lines_matching = ['^noise']
[filters.scriptprobe]
match_command = '^(?:\./)?scripts/probe\.sh\b'
strip_lines_matching = ['^noise']
[filters.basenameprobe]
match_command = '^plainprobe\b'
strip_lines_matching = ['^noise']
[filters.ignoredprobe]
match_command = '^(?:cd|echo|export)\b'
strip_lines_matching = ['^noise']
"#;
    fs::write(&filters, contents).expect("write filters");
    let untrusted = command(root)
        .args(["rewrite", "bash scripts/probe.sh"])
        .output()
        .expect("untrusted rewrite");
    assert_eq!(untrusted.status.code(), Some(1));
    assert!(untrusted.stdout.is_empty());
    assert!(
        command(root)
            .args(["trust", "--yes"])
            .output()
            .expect("trust filters")
            .status
            .success()
    );

    for args in [
        vec!["bash", "scripts/probe.sh"],
        vec!["sh", "scripts/probe.sh"],
        vec!["scripts/probe.sh"],
        vec!["./scripts/probe.sh"],
        vec!["scripts/plainprobe"],
    ] {
        let original = format!("{} 'a b'", args.join(" "));
        let rewritten = command(root)
            .args(["rewrite", &original])
            .output()
            .expect("rewrite script");
        assert_eq!(rewritten.status.code(), Some(3), "{original}");
        assert_eq!(
            String::from_utf8_lossy(&rewritten.stdout),
            format!("rtk {original}")
        );
        let checked = command(root)
            .args(["hook", "check", &original])
            .output()
            .expect("check hook");
        assert!(String::from_utf8_lossy(&checked.stdout).contains(&format!("rtk {original}")));
        for code in [0, 7] {
            let output = command(root)
                .args(&args)
                .arg("a b")
                .env("SCRIPT_EXIT", code.to_string())
                .output()
                .expect("execute script");
            assert_eq!(output.status.code(), Some(code), "{original}");
            let shown = String::from_utf8_lossy(&output.stdout);
            assert!(shown.contains("keep result"), "{shown}");
            assert!(!shown.contains("noise progress"), "{shown}");
            assert_eq!(output.stderr, b"script diagnostic\n");
            assert_eq!(
                fs::read_to_string(root.join("argv")).expect("arguments"),
                "a b\n"
            );
        }
        let disabled = command(root)
            .args(["rewrite", &original])
            .env("RTK_NO_TOML", "1")
            .output()
            .expect("disabled filter");
        assert_eq!(disabled.status.code(), Some(1));
        assert!(disabled.stdout.is_empty());
    }
    for original in [
        "cd scripts",
        "echo hello",
        "export X=1",
        "verify custom",
        "bash",
        "bash -c 'echo hello'",
        "bash -lc 'git status'",
        "sh -s",
        "scripts/probe.sh | cat",
        "echo hello | scripts/probe.sh",
        "RTK_DISABLED=1 bash scripts/probe.sh",
    ] {
        let output = command(root)
            .args(["rewrite", original])
            .output()
            .expect("preserve ignored command");
        assert_eq!(output.status.code(), Some(1), "{original}");
        assert!(output.stdout.is_empty(), "{original}");
    }
    let wrapper = command(root)
        .args(["rewrite", "bash -c 'git status'"])
        .output()
        .expect("rewrite shell wrapper");
    assert_eq!(
        String::from_utf8_lossy(&wrapper.stdout),
        "bash -c 'rtk git status'"
    );

    for config in [
        root.join("rtk"),
        root.join("Library/Application Support/rtk"),
    ] {
        fs::create_dir_all(&config).expect("config directory");
        fs::write(config.join("config.toml"), "[hooks]\nexclude_commands = ['^bash scripts/probe\\.sh', '^scripts/probe\\.sh', '^plainprobe\\b']\n").expect("write exclusions");
    }
    for original in [
        "bash scripts/probe.sh",
        "scripts/probe.sh",
        "scripts/plainprobe",
    ] {
        let excluded = command(root)
            .args(["rewrite", original])
            .output()
            .expect("excluded script");
        assert_eq!(excluded.status.code(), Some(1), "{original}");
        assert!(excluded.stdout.is_empty());
    }
    fs::write(&filters, format!("{contents}\n# changed\n")).expect("invalidate trust");
    let changed = command(root)
        .args(["rewrite", "sh scripts/probe.sh"])
        .output()
        .expect("changed filter");
    assert_eq!(changed.status.code(), Some(1));
    assert!(changed.stdout.is_empty());
}
