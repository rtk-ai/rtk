//! End-to-end regression tests for hook status detection of the GitHub
//! Copilot integration.
//!
//! Regression: a valid user-global Copilot hook
//! (`$COPILOT_HOME/hooks/rtk-rewrite.json`) must suppress the
//! "No hook installed" warning in `rtk gain`, and `rtk init --show` must
//! report the Copilot hook status — even when `~/.claude` exists but has
//! no RTK hook configured.

use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

const COPILOT_STOCK: &str = r#"{
  "version": 1,
  "hooks": {
    "PreToolUse": [
      {
        "type": "command",
        "command": "rtk hook copilot",
        "cwd": ".",
        "timeout": 5
      }
    ]
  }
}
"#;

struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    claude_dir: PathBuf,
    copilot_home: PathBuf,
    project: PathBuf,
}

impl Sandbox {
    /// Fresh sandbox: `.claude`-equivalent dir EXISTS but is unconfigured
    /// (the exact condition that produced the false warning).
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let home = root.path().join("home");
        let claude_dir = root.path().join("claude");
        let copilot_home = root.path().join("copilot-home");
        let project = root.path().join("project");
        std::fs::create_dir_all(&home).expect("mkdir home");
        std::fs::create_dir_all(&claude_dir).expect("mkdir claude");
        std::fs::create_dir_all(&copilot_home).expect("mkdir copilot");
        std::fs::create_dir_all(&project).expect("mkdir project");
        Self {
            _root: root,
            home,
            claude_dir,
            copilot_home,
            project,
        }
    }

    fn install_copilot_hook(&self, content: &str) {
        let hooks_dir = self.copilot_home.join("hooks");
        std::fs::create_dir_all(&hooks_dir).expect("mkdir copilot hooks");
        std::fs::write(hooks_dir.join("rtk-rewrite.json"), content).expect("write copilot hook");
    }

    fn rtk(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(args)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("CLAUDE_CONFIG_DIR", &self.claude_dir)
            .env("COPILOT_HOME", &self.copilot_home)
            .env("NO_COLOR", "1")
            .env("LC_ALL", "C")
            .output()
            .expect("spawn rtk")
    }

    /// Seed the tracking database so `rtk gain` reaches its summary view
    /// (and thus its hook-status warning) instead of "No tracking data yet".
    fn seed_tracking_data(&self) {
        let out = self.rtk(&["proxy", "echo", "ok"]);
        assert!(out.status.success(), "seeding via rtk proxy must succeed");
    }

    fn gain(&self) -> (String, String) {
        let out = self.rtk(&["gain"]);
        assert!(out.status.success(), "rtk gain must exit 0");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            !stdout.contains("No tracking data yet"),
            "test setup failed: tracking data was not seeded"
        );
        (stdout, stderr)
    }

    fn init_show(&self) -> String {
        let out = self.rtk(&["init", "--show"]);
        assert!(out.status.success(), "rtk init --show must exit 0");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

// ── rtk gain warning ─────────────────────────────────────────

#[test]
fn gain_does_not_warn_when_only_copilot_hook_installed() {
    let sb = Sandbox::new();
    sb.install_copilot_hook(COPILOT_STOCK);
    sb.seed_tracking_data();

    let (_, stderr) = sb.gain();

    assert!(
        !stderr.contains("No hook installed"),
        "valid Copilot hook must suppress the missing-hook warning, got: {stderr}"
    );
}

#[test]
fn gain_does_not_warn_with_absolute_rtk_path_in_copilot_hook() {
    let sb = Sandbox::new();
    sb.install_copilot_hook(
        &COPILOT_STOCK.replace("rtk hook copilot", "/opt/homebrew/bin/rtk hook copilot"),
    );
    sb.seed_tracking_data();

    let (_, stderr) = sb.gain();

    assert!(
        !stderr.contains("No hook installed"),
        "absolute rtk path must count as a valid Copilot hook, got: {stderr}"
    );
}

#[test]
fn gain_warns_when_no_integration_installed() {
    let sb = Sandbox::new();
    sb.seed_tracking_data();

    let (_, stderr) = sb.gain();

    assert!(
        stderr.contains("No hook installed"),
        "warning must remain when no integration is installed, got: {stderr}"
    );
}

#[test]
fn gain_warns_when_copilot_hook_is_malformed() {
    let sb = Sandbox::new();
    sb.install_copilot_hook("{ not json");
    sb.seed_tracking_data();

    let (_, stderr) = sb.gain();

    assert!(
        stderr.contains("No hook installed"),
        "malformed Copilot hook must not count as installed, got: {stderr}"
    );
}

#[test]
fn gain_warns_when_copilot_hook_has_wrong_command() {
    let sb = Sandbox::new();
    sb.install_copilot_hook(&COPILOT_STOCK.replace("rtk hook copilot", "other-tool --hook"));
    sb.seed_tracking_data();

    let (_, stderr) = sb.gain();

    assert!(
        stderr.contains("No hook installed"),
        "foreign PreToolUse command must not count as installed, got: {stderr}"
    );
}

// ── rtk init --show ──────────────────────────────────────────

#[test]
fn init_show_reports_copilot_hook_registered() {
    let sb = Sandbox::new();
    sb.install_copilot_hook(COPILOT_STOCK);

    let stdout = sb.init_show();

    assert!(
        stdout.contains("[ok] GitHub Copilot hook: registered"),
        "init --show must report the registered Copilot hook, got: {stdout}"
    );
}

#[test]
fn init_show_reports_copilot_hook_not_found() {
    let sb = Sandbox::new();

    let stdout = sb.init_show();

    assert!(
        stdout.contains("[--] GitHub Copilot hook: not found"),
        "init --show must report a missing Copilot hook, got: {stdout}"
    );
}

#[test]
fn init_show_reports_invalid_copilot_hook() {
    let sb = Sandbox::new();
    sb.install_copilot_hook(r#"{ "version": 1, "hooks": { "PreToolUse": [] } }"#);

    let stdout = sb.init_show();

    assert!(
        stdout.contains("[warn] GitHub Copilot hook:"),
        "init --show must flag an invalid Copilot hook, got: {stdout}"
    );
}
