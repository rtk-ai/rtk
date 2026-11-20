//! Regression coverage for Kilo Code dispatch after the CLI moved to lib.rs.

use std::path::Path;
use std::process::{Command, Output, Stdio};

mod common;

fn rtk(root: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.current_dir(root)
        .env("KILO_CONFIG_DIR", root.join("kilo"))
        .env("XDG_CONFIG_HOME", root.join("xdg"))
        .stdin(Stdio::null());
    cmd
}

fn init(root: &Path, extra: &[&str]) -> Output {
    let output = rtk(root)
        .args(["init", "-g", "--agent", "kilocode"])
        .args(extra)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Kilo Code init {extra:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn kilocode_install_is_idempotent_and_uninstall_preserves_other_plugins() {
    let root = common::tempdir();
    let plugin_dir = root.path().join("kilo/plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let other = plugin_dir.join("other.ts");
    std::fs::write(&other, "export default {}\n").unwrap();
    let plugin = plugin_dir.join("rtk.ts");

    init(root.path(), &[]);
    let expected = include_str!("../hooks/kilocode/rtk.ts");
    assert_eq!(std::fs::read_to_string(&plugin).unwrap(), expected);
    let output = init(root.path(), &[]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("already current"));
    assert_eq!(std::fs::read_to_string(&plugin).unwrap(), expected);
    assert!(!root.path().join(".kilocode").exists());
    assert!(!root.path().join("xdg/kilo").exists());

    init(root.path(), &["--uninstall"]);
    assert!(!plugin.exists());
    assert_eq!(
        std::fs::read_to_string(&other).unwrap(),
        "export default {}\n"
    );
    init(root.path(), &["--uninstall"]);
}

#[test]
fn kilocode_dry_run_does_not_install_or_remove_plugin() {
    let root = common::tempdir();
    let plugin = root.path().join("kilo/plugin/rtk.ts");
    let output = init(root.path(), &["--dry-run"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("rtk.ts"));
    assert!(!plugin.exists());

    init(root.path(), &[]);
    let before = std::fs::read(&plugin).unwrap();
    init(root.path(), &["--uninstall", "--dry-run"]);
    assert_eq!(std::fs::read(&plugin).unwrap(), before);
}

#[test]
fn kilocode_project_scope_is_rejected_without_writes() {
    let root = common::tempdir();
    for extra in [&[][..], &["--uninstall"][..]] {
        let output = rtk(root.path())
            .args(["init", "--agent", "kilocode"])
            .args(extra)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("user-scoped"));
        assert!(!root.path().join("kilo").exists());
        assert!(!root.path().join(".kilocode").exists());
    }
}

#[test]
fn kilocode_uses_xdg_config_when_kilo_override_is_empty_or_unset() {
    let root = common::tempdir();
    let plugin = root.path().join("xdg/kilo/plugin/rtk.ts");
    for value in [None, Some("")] {
        let mut cmd = rtk(root.path());
        if let Some(value) = value {
            cmd.env("KILO_CONFIG_DIR", value);
        } else {
            cmd.env_remove("KILO_CONFIG_DIR");
        }
        let output = cmd
            .args(["init", "-g", "--agent", "kilocode"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            std::fs::read_to_string(&plugin).unwrap(),
            include_str!("../hooks/kilocode/rtk.ts")
        );
    }
}
