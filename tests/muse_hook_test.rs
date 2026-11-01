//! Muse protocol and lifecycle tests. No Muse binary or model call is required.
mod common;
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Output, Stdio};

struct MuseHome {
    root: tempfile::TempDir,
}
impl MuseHome {
    fn new() -> Self {
        let root = common::tempdir();
        std::fs::create_dir_all(root.path().join(".config/muse")).expect(".config");
        let result = Self { root };
        result.settings(json!({"schema_version":1,"permissions":{"default_profile":":ask-me"}}));
        result
    }
    fn settings(&self, value: Value) {
        std::fs::write(
            self.root.path().join(".config/muse/settings.json"),
            value.to_string(),
        )
        .expect("settings");
    }
    fn command(&self) -> std::process::Command {
        let mut cmd = common::rtk_command();
        cmd.current_dir(self.root.path())
            .env("HOME", self.root.path())
            .env("XDG_CONFIG_HOME", self.root.path().join(".config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"));
        cmd
    }
    fn hook(&self, payload: &str) -> Output {
        let mut child = self
            .command()
            .args(["hook", "muse"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("hook");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(payload.as_bytes())
            .expect("payload");
        let output = child.wait_with_output().expect("hook output");
        assert!(output.status.success(), "{:?}", output);
        output
    }
    fn payload(&self, command: &str) -> String {
        json!({"tool_name":"Bash","tool_input":{"command":command,"description":"keep me"},"cwd":self.root.path()}).to_string()
    }
}

#[test]
fn muse_rewrites_without_asserting_approval_and_preserves_metadata() {
    let home = MuseHome::new();
    let output = home.hook(&home.payload("git status"));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).expect("json"),
        json!({
            "hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{
                "command":"rtk git status","description":"keep me"
            }}
        })
    );
    let checked = home
        .command()
        .args(["hook", "check", "--agent", "muse", "git status"])
        .output()
        .expect("check");
    assert!(checked.status.success());
    assert_eq!(
        String::from_utf8_lossy(&checked.stdout).trim(),
        "rtk git status"
    );
    for command in [
        "echo hello",
        "rtk git status",
        "git status > output",
        "git status $(whoami)",
    ] {
        assert!(
            home.hook(&home.payload(command)).stdout.is_empty(),
            "{command}"
        );
    }
    for input in [
        "",
        "not json",
        "{}",
        r#"{"tool_name":"Read","tool_input":{"command":"git status"}}"#,
    ] {
        assert!(home.hook(input).stdout.is_empty());
    }
}

#[test]
fn muse_native_lowercase_tool_name_preserves_the_real_envelope() {
    let home = MuseHome::new();
    let mut payload: Value = serde_json::from_str(&home.payload("git status")).expect("payload");
    payload["tool_name"] = json!("bash");
    payload["hook_event_name"] = json!("PreToolUse");
    payload["permission_mode"] = json!("default");
    payload["tool_input"]["workdir"] = json!(home.root.path());
    let output = home.hook(&payload.to_string());
    assert_eq!(output.status.code(), Some(0));
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).expect("native rewrite");
    assert_eq!(
        result["hookSpecificOutput"]["updatedInput"]["command"],
        "rtk git status"
    );
    assert_eq!(
        result["hookSpecificOutput"]["updatedInput"]["workdir"],
        payload["tool_input"]["workdir"]
    );
}

#[test]
fn muse_defers_every_unqualified_policy_and_profile() {
    let home = MuseHome::new();
    let policy = home.root.path().join(".config/muse/approval-policy.json");
    for rule in [
        json!({"kind":"shell_command_argv_prefix","argv_prefix":["git","status"],"workspace_root":home.root.path()}),
        json!({"kind":"shell_command_argv_prefix","argv_prefix":["git","status"],"workspace_root":"/other"}),
        json!({"kind":"shell_command_exact_raw","command":"git status"}),
        json!({"kind":"unknown"}),
    ] {
        std::fs::write(
            &policy,
            json!({"schema_version":2,"rules":[{"effect":"allow","durability":"local_persistent","rule":rule}]}).to_string(),
        )
        .expect("policy");
        for command in ["git status", "git status && git diff", "git status | cat"] {
            assert!(home.hook(&home.payload(command)).stdout.is_empty());
        }
        let checked = home
            .command()
            .args(["hook", "check", "--agent", "muse", "git status"])
            .output()
            .expect("check");
        assert_eq!(checked.status.code(), Some(1));
        assert!(checked.stdout.is_empty());
    }
    std::fs::write(&policy, "broken").expect("policy");
    assert!(home.hook(&home.payload("git status")).stdout.is_empty());
    std::fs::remove_file(&policy).expect("remove fixture");
    home.settings(json!({"schema_version":1,"permissions":{"default_profile":":unrestricted"}}));
    assert!(home.hook(&home.payload("git status")).stdout.is_empty());
}

#[test]
fn muse_bundle_lifecycle_is_explicit_idempotent_and_scoped() {
    let home = MuseHome::new();
    for global in [false, true] {
        let mut args = vec!["init", "--agent", "muse"];
        if global {
            args.push("-g");
        }
        let bundle = home.root.path().join(if global {
            ".config/rtk/plugins/muse"
        } else {
            ".rtk/muse-plugin"
        });
        let dry = home
            .command()
            .args(&args)
            .arg("--dry-run")
            .output()
            .expect("dry run");
        assert!(dry.status.success());
        assert!(!bundle.exists());
        for _ in 0..2 {
            let output = home.command().args(&args).output().expect("init");
            assert!(output.status.success(), "{:?}", output);
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("muse plugins approve rtk"));
            assert!(stdout.contains(if global {
                "--scope user"
            } else {
                "--scope project"
            }));
            let hooks: Value = serde_json::from_str(
                &std::fs::read_to_string(bundle.join("hooks/hooks.json")).expect("hooks"),
            )
            .expect("json");
            assert_eq!(
                hooks["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
                "rtk hook muse"
            );
            assert!(
                !home
                    .root
                    .path()
                    .join("data/muse/plugins/installed.json")
                    .exists()
            );
        }
        let show = home
            .command()
            .args(&args)
            .arg("--show")
            .output()
            .expect("show");
        assert!(show.status.success());
        assert!(String::from_utf8_lossy(&show.stdout).contains("installed: false"));
        std::fs::write(bundle.join("custom.txt"), "preserve").expect("custom file");
        let uninstall = home
            .command()
            .args(&args)
            .arg("--uninstall")
            .output()
            .expect("uninstall");
        assert!(uninstall.status.success());
        assert!(!bundle.join("hooks/hooks.json").exists());
        assert!(bundle.join("custom.txt").exists());
    }
}

#[test]
fn muse_check_defers_custom_xdg_and_accepts_both_settings_shapes() {
    let home = MuseHome::new();
    for permissions in [
        json!({"default_profile":":ask-me"}),
        json!({"schema_version":1,"default_profile":":ask-me"}),
    ] {
        home.settings(json!({"schema_version":1,"permissions":permissions}));
        let normal = home
            .command()
            .args(["hook", "check", "--agent", "muse", "git status"])
            .output()
            .expect("normal check");
        assert!(normal.status.success());
        let custom = home.root.path().join("custom");
        std::fs::create_dir_all(custom.join("muse")).expect("custom config");
        std::fs::copy(
            home.root.path().join(".config/muse/settings.json"),
            custom.join("muse/settings.json"),
        )
        .expect("settings copy");
        let output = home
            .command()
            .env("XDG_CONFIG_HOME", &custom)
            .args(["hook", "check", "--agent", "muse", "git status"])
            .output()
            .expect("custom check");
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("policy is unknown"));
        assert!(home.hook(&home.payload("git status\r")).stdout.is_empty());
    }
}
