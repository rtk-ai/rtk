use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn run_rtk(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("LC_ALL", "C")
        .args(args)
        .current_dir(home)
        .output()
        .expect("spawn rtk")
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn opencode_global_install_also_registers_claude_hook() {
    let home = TempDir::new().expect("create temporary home");
    std::fs::create_dir_all(home.path().join(".claude")).expect("create Claude config directory");

    let additive = run_rtk(home.path(), &["init", "-g", "--opencode", "--auto-patch"]);
    assert!(
        additive.status.success(),
        "additive install failed:\n{}",
        output_text(&additive)
    );
    let additive_output = output_text(&additive);
    assert!(additive_output.contains("RTK hook registered (global)"));
    assert!(additive_output.contains("OpenCode:"));
    assert!(home.path().join(".claude/settings.json").exists());
    assert!(home.path().join(".config/opencode/plugins/rtk.ts").exists());

    let claude_only = run_rtk(home.path(), &["init", "-g", "--auto-patch"]);
    assert!(
        claude_only.status.success(),
        "Claude-only install failed:\n{}",
        output_text(&claude_only)
    );
    assert!(output_text(&claude_only).contains("RTK hook registered (global)"));
    assert!(!output_text(&claude_only).contains("OpenCode:"));

    let local_opencode = run_rtk(home.path(), &["init", "--opencode"]);
    assert!(!local_opencode.status.success());
    assert!(output_text(&local_opencode).contains("OpenCode plugin is global-only"));
}
