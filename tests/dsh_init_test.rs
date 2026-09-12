//! DSH instruction installation through the public CLI in isolated directories.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run(root: &Path, home: Option<&str>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rtk"));
    command.current_dir(root).env("HOME", root.join("home"));
    command.env_remove("DSH_HOME");
    if let Some(home) = home {
        command.env("DSH_HOME", home);
    }
    command.args(["init", "--agent", "dsh"]).args(args);
    command.output().expect("run RTK")
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn dsh_preserves_user_and_other_agent_instructions_through_lifecycle() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("AGENTS.md");
    let original = "# Project\n\nKeep this.\n<!-- rtk-instructions -->\nOther agent\n<!-- /rtk-instructions -->\n";
    fs::write(&path, original).unwrap();
    success(run(root.path(), None, &[]));
    let installed = fs::read_to_string(&path).unwrap();
    assert!(installed.starts_with(original));
    assert!(installed.contains("rtk git status"));
    assert!(!installed.contains("@RTK.md"));
    assert_eq!(
        fs::read_to_string(root.path().join("AGENTS.md.bak")).unwrap(),
        original
    );
    success(run(root.path(), None, &[]));
    assert_eq!(fs::read_to_string(&path).unwrap(), installed);
    assert_eq!(
        fs::read_to_string(root.path().join("AGENTS.md.bak")).unwrap(),
        original
    );
    assert!(success(run(root.path(), None, &["--show"])).contains("rtk git status"));
    success(run(root.path(), None, &["--uninstall", "--dry-run"]));
    assert_eq!(fs::read_to_string(&path).unwrap(), installed);
    success(run(root.path(), None, &["--uninstall"]));
    let removed = fs::read_to_string(&path).unwrap();
    assert!(removed.starts_with(original));
    assert!(!removed.contains("rtk-dsh-instructions"));
    success(run(root.path(), None, &["--uninstall"]));
    assert_eq!(fs::read_to_string(&path).unwrap(), removed);
    assert!(!root.path().join("RTK.md").exists());
    assert!(!root.path().join(".claude").exists());
}

#[test]
fn dsh_dry_run_and_absent_uninstall_do_not_create_files() {
    let root = tempfile::tempdir().unwrap();
    for args in [
        &["--dry-run"][..],
        &["--uninstall"],
        &["--global", "--dry-run"],
        &["--global", "--uninstall"],
        &["--show"],
    ] {
        success(run(root.path(), Some("absent/dsh"), args));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn dsh_updates_only_its_existing_block() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("AGENTS.md");
    fs::write(
        &path,
        "Before\n<!-- rtk-dsh-instructions -->\nOld\n<!-- /rtk-dsh-instructions -->\nAfter\n",
    )
    .unwrap();
    success(run(root.path(), None, &[]));
    let content = fs::read_to_string(&path).unwrap();
    assert!(content.starts_with("Before\n"));
    assert!(content.ends_with("\nAfter\n"));
    assert!(!content.contains("Old"));
    assert_eq!(content.matches("<!-- rtk-dsh-instructions -->").count(), 1);
}

#[test]
fn dsh_rejects_malformed_and_duplicate_blocks_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("AGENTS.md");
    for content in [
        "<!-- rtk-dsh-instructions -->\nKeep",
        "<!-- /rtk-dsh-instructions -->",
        "<!-- /rtk-dsh-instructions --><!-- rtk-dsh-instructions -->",
        "<!-- rtk-dsh-instructions --><!-- rtk-dsh-instructions --><!-- /rtk-dsh-instructions -->",
    ] {
        fs::write(&path, content).unwrap();
        for args in [&[][..], &["--uninstall"]] {
            assert!(!run(root.path(), None, args).status.success());
            assert_eq!(fs::read_to_string(&path).unwrap(), content);
            assert!(!root.path().join("AGENTS.md.bak").exists());
        }
    }
}

#[test]
fn dsh_rejects_unrelated_flags_before_any_write() {
    let root = tempfile::tempdir().unwrap();
    for flag in [
        "--codex",
        "--opencode",
        "--gemini",
        "--copilot",
        "--hook-only",
        "--claude-md",
        "--auto-patch",
        "--no-patch",
        "--trust-filters",
        "--no-trust-filters",
    ] {
        assert!(!run(root.path(), None, &[flag]).status.success());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn dsh_global_honors_relative_and_absolute_home() {
    for absolute in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("custom dsh");
        let home = if absolute {
            target.to_str().unwrap()
        } else {
            "custom dsh"
        };
        success(run(root.path(), Some(home), &["--global"]));
        assert!(target.join("AGENTS.md").exists());
        assert!(!root.path().join("AGENTS.md").exists());
        success(run(root.path(), Some(home), &["--global", "--uninstall"]));
        assert!(!fs::read_to_string(target.join("AGENTS.md"))
            .unwrap()
            .contains("rtk-dsh-instructions"));
    }
}

#[cfg(unix)]
#[test]
fn dsh_global_home_defaults_and_tilde_match_dsh() {
    for (home, expected) in [
        (None, "home/.dsh"),
        (Some("  "), "home/.dsh"),
        (Some("~/custom"), "home/custom"),
        (Some("~\\custom"), "home/custom"),
        (Some("~"), "home"),
    ] {
        let root = tempfile::tempdir().unwrap();
        success(run(root.path(), home, &["--global"]));
        assert!(root.path().join(expected).join("AGENTS.md").exists());
    }
}

#[cfg(unix)]
#[test]
fn dsh_preserves_instruction_symlink() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("instructions.md");
    let path = root.path().join("AGENTS.md");
    fs::write(&target, "Keep\n").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    success(run(root.path(), None, &[]));
    success(run(root.path(), None, &["--uninstall"]));
    assert!(path.is_symlink());
    assert!(fs::read_to_string(target).unwrap().starts_with("Keep\n"));
}
