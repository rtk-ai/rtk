//! End-to-end tests for the Copilot legacy hook config self-heal.
//!
//! Runs the real `rtk hook copilot` binary against crafted configs in
//! sandboxed HOME/COPILOT_HOME and asserts the hook protocol is never
//! broken: correct responses, exit 0, silent stderr, and configs only
//! ever modified when they contain rtk's own stale camelCase entry.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

const LEGACY_STOCK: &str = r#"{
  "version": 1,
  "hooks": {
    "PreToolUse": [
      { "type": "command", "command": "rtk hook copilot", "cwd": ".", "timeout": 5 }
    ],
    "preToolUse": [
      { "type": "command", "bash": "rtk hook copilot", "powershell": "rtk hook copilot", "cwd": ".", "timeoutSec": 5 }
    ]
  }
}
"#;

const CURRENT_STOCK: &str = r#"{
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

const LEGACY_PAYLOAD: &str = r#"{"toolName":"bash","toolArgs":"{\"command\":\"git status\"}"}"#;
const LEGACY_PAYLOAD_PS: &str =
    r#"{"toolName":"powershell","toolArgs":"{\"command\":\"git status\"}"}"#;
const JETBRAINS_PAYLOAD: &str =
    r#"{"toolName":"run_in_terminal","toolArgs":"{\"command\":\"git status\"}"}"#;
const PASCAL_PAYLOAD: &str = r#"{"tool_name":"Bash","tool_input":{"command":"git status"}}"#;
const UNKNOWN_TOOL_PAYLOAD: &str = r#"{"tool_name":"Edit","tool_input":{"command":"git status"}}"#;

struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    copilot_home: PathBuf,
    project: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let home = root.path().join("home");
        let copilot_home = root.path().join("copilot-home");
        let project = root.path().join("project");
        std::fs::create_dir_all(&home).expect("mkdir home");
        std::fs::create_dir_all(copilot_home.join("hooks")).expect("mkdir copilot hooks");
        std::fs::create_dir_all(project.join(".github/hooks")).expect("mkdir project hooks");
        Self {
            _root: root,
            home,
            copilot_home,
            project,
        }
    }

    fn project_config(&self) -> PathBuf {
        self.project.join(".github/hooks/rtk-rewrite.json")
    }

    fn global_config(&self) -> PathBuf {
        self.copilot_home.join("hooks/rtk-rewrite.json")
    }

    fn write_project(&self, content: &str) {
        std::fs::write(self.project_config(), content).expect("write project config");
    }

    fn write_global(&self, content: &str) {
        std::fs::write(self.global_config(), content).expect("write global config");
    }

    fn run_hook(&self, payload: &str) -> (String, String, Option<i32>) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["hook", "copilot"])
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("COPILOT_HOME", &self.copilot_home)
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rtk");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(payload.as_bytes())
            .expect("write payload");
        let out = child.wait_with_output().expect("wait rtk");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code(),
        )
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read config")
}

fn has_camel(path: &Path) -> bool {
    read(path).contains("\"preToolUse\"")
}

fn assert_hook_ok(payload: &str, stdout: &str, stderr: &str, code: Option<i32>) {
    assert_eq!(code, Some(0), "hook must exit 0 for payload {payload}");
    assert!(
        stderr.is_empty(),
        "stderr must stay silent (protocol safety) for payload {payload}, got: {stderr}"
    );
    if !stdout.trim().is_empty() {
        serde_json::from_str::<serde_json::Value>(stdout.trim())
            .unwrap_or_else(|e| panic!("stdout must be valid JSON for {payload}: {e}\n{stdout}"));
    }
}

// ── Heal correctness ─────────────────────────────────────────

#[test]
fn legacy_invocation_heals_project_and_global_to_current_stock() {
    let sb = Sandbox::new();
    sb.write_project(LEGACY_STOCK);
    sb.write_global(LEGACY_STOCK);

    let (stdout, stderr, code) = sb.run_hook(LEGACY_PAYLOAD);

    assert_hook_ok(LEGACY_PAYLOAD, &stdout, &stderr, code);
    assert!(
        stdout.contains("rtk git status"),
        "rewrite must still work during heal: {stdout}"
    );
    assert_eq!(read(&sb.project_config()), CURRENT_STOCK);
    assert_eq!(read(&sb.global_config()), CURRENT_STOCK);
}

#[test]
fn powershell_legacy_invocation_also_heals() {
    let sb = Sandbox::new();
    sb.write_project(LEGACY_STOCK);

    let (stdout, stderr, code) = sb.run_hook(LEGACY_PAYLOAD_PS);

    assert_hook_ok(LEGACY_PAYLOAD_PS, &stdout, &stderr, code);
    assert!(!has_camel(&sb.project_config()));
}

#[test]
fn heal_preserves_user_hooks_and_key_order() {
    let extended = r#"{
  "version": 1,
  "customTopLevel": { "keep": true },
  "hooks": {
    "sessionStart": [
      { "type": "command", "command": "echo hi" }
    ],
    "PreToolUse": [
      { "type": "command", "command": "rtk hook copilot", "cwd": ".", "timeout": 5 }
    ],
    "preToolUse": [
      { "type": "command", "bash": "rtk hook copilot", "powershell": "rtk hook copilot", "cwd": ".", "timeoutSec": 5 }
    ]
  }
}
"#;
    let sb = Sandbox::new();
    sb.write_project(extended);

    let (stdout, stderr, code) = sb.run_hook(LEGACY_PAYLOAD);

    assert_hook_ok(LEGACY_PAYLOAD, &stdout, &stderr, code);
    let healed: serde_json::Value =
        serde_json::from_str(&read(&sb.project_config())).expect("healed config valid JSON");
    assert!(healed["hooks"].get("preToolUse").is_none());
    assert_eq!(healed["customTopLevel"]["keep"], true);
    assert_eq!(healed["hooks"]["sessionStart"][0]["command"], "echo hi");
    assert_eq!(
        healed["hooks"]["PreToolUse"][0]["command"],
        "rtk hook copilot"
    );
    let keys: Vec<&str> = healed["hooks"]
        .as_object()
        .expect("hooks object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["sessionStart", "PreToolUse"], "key order preserved");
}

#[test]
fn heal_is_idempotent_across_invocations() {
    let sb = Sandbox::new();
    sb.write_project(LEGACY_STOCK);
    sb.write_global(LEGACY_STOCK);

    sb.run_hook(LEGACY_PAYLOAD);
    let project_after = read(&sb.project_config());
    let global_after = read(&sb.global_config());

    for _ in 0..3 {
        let (stdout, stderr, code) = sb.run_hook(LEGACY_PAYLOAD);
        assert_hook_ok(LEGACY_PAYLOAD, &stdout, &stderr, code);
    }
    assert_eq!(read(&sb.project_config()), project_after);
    assert_eq!(read(&sb.global_config()), global_after);
}

// ── Response integrity: heal never changes hook behavior ─────

#[test]
fn response_is_byte_identical_before_and_after_heal() {
    let sb = Sandbox::new();
    sb.write_project(LEGACY_STOCK);

    let (before, _, _) = sb.run_hook(LEGACY_PAYLOAD);
    assert!(!has_camel(&sb.project_config()), "first run must heal");
    let (after, _, _) = sb.run_hook(LEGACY_PAYLOAD);

    assert_eq!(before, after, "heal must not alter the hook response");
}

#[test]
fn pascalcase_invocation_works_and_never_touches_configs() {
    let sb = Sandbox::new();
    sb.write_project(LEGACY_STOCK);
    sb.write_global(LEGACY_STOCK);

    let (stdout, stderr, code) = sb.run_hook(PASCAL_PAYLOAD);

    assert_hook_ok(PASCAL_PAYLOAD, &stdout, &stderr, code);
    assert!(
        stdout.contains("rtk git status"),
        "PascalCase rewrite must work: {stdout}"
    );
    assert_eq!(read(&sb.project_config()), LEGACY_STOCK);
    assert_eq!(read(&sb.global_config()), LEGACY_STOCK);
}

#[test]
fn jetbrains_invocation_works_and_never_touches_configs() {
    let sb = Sandbox::new();
    sb.write_project(LEGACY_STOCK);

    let (stdout, stderr, code) = sb.run_hook(JETBRAINS_PAYLOAD);

    assert_hook_ok(JETBRAINS_PAYLOAD, &stdout, &stderr, code);
    assert!(
        stdout.contains("rtk git status"),
        "JetBrains deny-with-suggestion must carry the rewrite: {stdout}"
    );
    assert_eq!(read(&sb.project_config()), LEGACY_STOCK);
}

#[test]
fn non_shell_tool_passes_through_and_never_touches_configs() {
    let sb = Sandbox::new();
    sb.write_project(LEGACY_STOCK);

    let (stdout, stderr, code) = sb.run_hook(UNKNOWN_TOOL_PAYLOAD);

    assert_hook_ok(UNKNOWN_TOOL_PAYLOAD, &stdout, &stderr, code);
    assert!(stdout.trim().is_empty(), "pass-through must stay silent");
    assert_eq!(read(&sb.project_config()), LEGACY_STOCK);
}

#[test]
fn garbage_and_empty_stdin_exit_zero_without_touching_configs() {
    let sb = Sandbox::new();
    for payload in ["", "not json at all", "{\"toolName\":\"bash\"}"] {
        sb.write_project(LEGACY_STOCK);
        let (_, _, code) = sb.run_hook(payload);
        assert_eq!(code, Some(0), "payload {payload:?} must exit 0");
        assert_eq!(
            read(&sb.project_config()),
            LEGACY_STOCK,
            "payload {payload:?} must not modify configs"
        );
    }
}

// ── Refusal matrix: configs never wrongly modified ───────────

#[test]
fn non_stock_configs_are_never_modified_by_legacy_invocations() {
    let customized = LEGACY_STOCK.replace(
        r#""bash": "rtk hook copilot""#,
        r#""bash": "my-wrapper.sh""#,
    );
    let extra_field = LEGACY_STOCK.replace(r#""timeoutSec": 5"#, r#""timeoutSec": 5, "x": 1"#);
    let two_entries = LEGACY_STOCK.replace(
        r#""preToolUse": [
      {"#,
        r#""preToolUse": [
      { "type": "command", "bash": "rtk hook copilot", "powershell": "rtk hook copilot", "cwd": ".", "timeoutSec": 5 },
      {"#,
    );
    let missing_pascal = LEGACY_STOCK.replace(
        r#""PreToolUse": [
      { "type": "command", "command": "rtk hook copilot", "cwd": ".", "timeout": 5 }
    ],
    "#,
        "",
    );
    let foreign_pascal = LEGACY_STOCK.replace(
        r#""command": "rtk hook copilot""#,
        r#""command": "other-tool --hook""#,
    );
    for (label, content) in [
        ("customized camelCase", customized.as_str()),
        ("extra field in entry", extra_field.as_str()),
        ("two camelCase entries", two_entries.as_str()),
        ("missing PascalCase", missing_pascal.as_str()),
        ("foreign PascalCase", foreign_pascal.as_str()),
        ("current stock", CURRENT_STOCK),
        ("malformed", "{ not json"),
        ("empty object", "{}"),
        ("empty file", ""),
        ("array root", "[1, 2]"),
    ] {
        let sb = Sandbox::new();
        sb.write_project(content);
        sb.write_global(content);

        let (stdout, stderr, code) = sb.run_hook(LEGACY_PAYLOAD);

        assert_hook_ok(LEGACY_PAYLOAD, &stdout, &stderr, code);
        assert!(
            stdout.contains("rtk git status"),
            "{label}: hook must keep rewriting: {stdout}"
        );
        assert_eq!(
            read(&sb.project_config()),
            content,
            "{label}: project modified"
        );
        assert_eq!(
            read(&sb.global_config()),
            content,
            "{label}: global modified"
        );
    }
}

#[test]
fn missing_configs_are_never_created() {
    let sb = Sandbox::new();

    let (stdout, stderr, code) = sb.run_hook(LEGACY_PAYLOAD);

    assert_hook_ok(LEGACY_PAYLOAD, &stdout, &stderr, code);
    assert!(!sb.project_config().exists());
    assert!(!sb.global_config().exists());
}

// ── Robustness ───────────────────────────────────────────────

#[test]
fn concurrent_legacy_invocations_leave_valid_healed_configs() {
    let sb = Sandbox::new();
    sb.write_project(LEGACY_STOCK);
    sb.write_global(LEGACY_STOCK);

    let sb_ref = &sb;
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(move || sb_ref.run_hook(LEGACY_PAYLOAD)))
            .collect();
        for handle in handles {
            let (stdout, stderr, code) = handle.join().expect("thread");
            assert_hook_ok(LEGACY_PAYLOAD, &stdout, &stderr, code);
        }
    });

    assert_eq!(read(&sb.project_config()), CURRENT_STOCK);
    assert_eq!(read(&sb.global_config()), CURRENT_STOCK);
    let stray: Vec<_> = walk(&sb.project)
        .into_iter()
        .chain(walk(&sb.copilot_home))
        .filter(|p| p.to_string_lossy().contains(".heal."))
        .collect();
    assert!(stray.is_empty(), "temp files left behind: {stray:?}");
}

#[cfg(unix)]
#[test]
fn unwritable_hooks_dir_never_breaks_the_hook() {
    use std::os::unix::fs::PermissionsExt;

    let sb = Sandbox::new();
    sb.write_project(LEGACY_STOCK);
    let hooks_dir = sb.project.join(".github/hooks");
    let mut perms = std::fs::metadata(&hooks_dir).expect("meta").permissions();
    perms.set_mode(0o555);
    std::fs::set_permissions(&hooks_dir, perms.clone()).expect("chmod");

    let (stdout, stderr, code) = sb.run_hook(LEGACY_PAYLOAD);

    perms.set_mode(0o755);
    std::fs::set_permissions(&hooks_dir, perms).expect("chmod back");

    assert_hook_ok(LEGACY_PAYLOAD, &stdout, &stderr, code);
    assert!(
        stdout.contains("rtk git status"),
        "rewrite must survive write failure: {stdout}"
    );
    assert_eq!(
        read(&sb.project_config()),
        LEGACY_STOCK,
        "config must stay intact"
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
