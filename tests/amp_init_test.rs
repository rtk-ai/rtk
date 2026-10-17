//! Public CLI coverage for the native Amp plugin lifecycle.

use std::fs;
use std::path::Path;
use std::process::Output;

mod common;

const PLUGIN: &str = include_str!("../hooks/amp/rtk/index.ts");

fn run(project: &Path, args: &[&str]) -> Output {
    common::rtk_command()
        .current_dir(project)
        .env("XDG_CONFIG_HOME", project.join("config"))
        .args(["init", "--agent", "amp"])
        .args(args)
        .output()
        .expect("spawn rtk init")
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "rtk init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn project_and_machine_plugin_lifecycle() {
    for global in [false, true] {
        let tmp = common::tempdir();
        let project = tmp.path();
        let scope = if global { vec!["-g"] } else { vec![] };
        let plugin = project.join(if global {
            "config/amp/plugins/rtk/index.ts"
        } else {
            ".amp/plugins/rtk/index.ts"
        });
        let mut args = scope.clone();
        args.push("--dry-run");
        assert!(success(run(project, &args)).contains("[dry-run] Nothing written."));
        assert!(!plugin.parent().unwrap().exists());

        success(run(project, &scope));
        assert_eq!(fs::read_to_string(&plugin).unwrap(), PLUGIN);
        let modified = fs::metadata(&plugin).unwrap().modified().unwrap();
        success(run(project, &scope));
        assert_eq!(fs::metadata(&plugin).unwrap().modified().unwrap(), modified);
        assert!(!project.join("AGENTS.md").exists());
        assert!(!project.join("CLAUDE.md").exists());
        assert!(!project.join(".claude").exists());

        let shown = success(run(project, &["--show"]));
        assert!(shown.contains("Configuration (Amp)"));
        let displayed = if global {
            plugin.clone()
        } else {
            Path::new(".amp/plugins/rtk/index.ts").to_path_buf()
        };
        assert!(shown.contains(&format!("{} (up to date)", displayed.display())));
        assert!(shown.contains("(not installed)"));

        // Amp accepts CRLF stock files too, and uninstall preserves support files.
        fs::write(&plugin, PLUGIN.replace('\n', "\r\n")).unwrap();
        let support = plugin.with_file_name("custom.ts");
        fs::write(&support, "// user support file\n").unwrap();
        let mut uninstall = scope.clone();
        uninstall.push("--uninstall");
        args = uninstall.clone();
        args.push("--dry-run");
        success(run(project, &args));
        assert!(plugin.exists());
        success(run(project, &uninstall));
        assert!(!plugin.exists());
        assert!(support.exists());
        success(run(project, &uninstall));
    }
}

#[cfg(unix)]
#[test]
fn global_path_falls_back_to_home_with_unset_or_empty_xdg() {
    for empty in [false, true] {
        let tmp = common::tempdir();
        let mut cmd = common::rtk_command();
        cmd.current_dir(tmp.path())
            .env("HOME", tmp.path().join("home"))
            .env_remove("XDG_CONFIG_HOME")
            .args(["init", "-g", "--agent", "amp"]);
        if empty {
            cmd.env("XDG_CONFIG_HOME", "");
        }
        success(cmd.output().unwrap());
        assert_eq!(
            fs::read_to_string(tmp.path().join("home/.config/amp/plugins/rtk/index.ts")).unwrap(),
            PLUGIN
        );
    }
}

#[test]
fn protects_modified_unrelated_and_unreadable_entries() {
    for content in [
        format!("{PLUGIN}\n// user customization\n").into_bytes(),
        b"export default () => {}\n".to_vec(),
        vec![0xff, 0xfe],
    ] {
        let tmp = common::tempdir();
        let plugin = tmp.path().join(".amp/plugins/rtk/index.ts");
        fs::create_dir_all(plugin.parent().unwrap()).unwrap();
        fs::write(&plugin, &content).unwrap();

        success(run(tmp.path(), &["--dry-run"]));
        assert!(!run(tmp.path(), &[]).status.success());
        assert!(!run(tmp.path(), &["--no-patch"]).status.success());
        assert_eq!(fs::read(&plugin).unwrap(), content);
        let uninstall = run(tmp.path(), &["--uninstall"]);
        if content.starts_with(b"export default") {
            success(uninstall);
        } else {
            assert!(!uninstall.status.success());
        }
        assert_eq!(fs::read(&plugin).unwrap(), content);

        success(run(tmp.path(), &["--auto-patch", "--dry-run"]));
        assert_eq!(fs::read(&plugin).unwrap(), content);
        success(run(tmp.path(), &["--auto-patch"]));
        assert_eq!(fs::read_to_string(&plugin).unwrap(), PLUGIN);
    }
}

#[test]
fn index_js_requires_approval_before_shadowing_and_survives_uninstall() {
    let tmp = common::tempdir();
    let plugin = tmp.path().join(".amp/plugins/rtk/index.ts");
    let javascript = plugin.with_extension("js");
    fs::create_dir_all(plugin.parent().unwrap()).unwrap();
    fs::write(&javascript, "export default () => {}\n").unwrap();
    assert!(!run(tmp.path(), &[]).status.success());
    assert!(!plugin.exists());
    success(run(tmp.path(), &["--auto-patch"]));
    assert_eq!(fs::read_to_string(&plugin).unwrap(), PLUGIN);
    success(run(tmp.path(), &["--uninstall"]));
    assert!(!plugin.exists());
    assert!(javascript.exists());
}

#[test]
fn single_file_plugins_are_not_silently_shadowed() {
    for extension in ["ts", "js"] {
        let tmp = common::tempdir();
        let existing = tmp.path().join(format!(".amp/plugins/rtk.{extension}"));
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, "export default () => {}\n").unwrap();
        let result = run(tmp.path(), &["--auto-patch"]);
        assert!(!result.status.success());
        assert!(!tmp.path().join(".amp/plugins/rtk").exists());
        assert_eq!(
            fs::read_to_string(existing).unwrap(),
            "export default () => {}\n"
        );
    }
}

#[test]
fn incompatible_flags_fail_before_installing_any_agent() {
    for flag in [
        "--gemini",
        "--copilot",
        "--codex",
        "--opencode",
        "--claude-md",
        "--hook-only",
    ] {
        let tmp = common::tempdir();
        let result = run(tmp.path(), &[flag]);
        assert!(!result.status.success(), "accepted {flag}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("--agent amp cannot be combined"));
        assert!(!tmp.path().join(".amp").exists());
        assert!(!tmp.path().join("config").exists());
    }
}
