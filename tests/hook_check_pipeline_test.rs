// Windows known-folder APIs cannot redirect the trust store through the
// environment. Exercise trusted project files only where the profile is isolated.
#![cfg(all(unix, feature = "engine"))]

use std::collections::BTreeMap;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

mod common;

const HEADER: &str =
    "schema_version = 1\n[pipeline]\nname = 'hook-check-test'\nextends = 'rtk:default'\n";
const STATIC_RULES: &str = r#"
[[hook.before_tool.shell]]
name = "deny-status"
when = { command_matches = '^git\s+status\b' }
action = "deny"
message = "status is denied by the project pipeline"
[[hook.before_tool.shell]]
name = "keep-log"
when = { command_matches = '^git\s+log\b' }
action = "passthrough"
"#;

struct Fixture {
    project: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl Fixture {
    fn new(rules: Option<&str>, trusted: bool) -> Self {
        let fixture = Self {
            project: common::temp_git_repo(),
            home: common::tempdir(),
        };
        for path in [
            fixture.project.path().join(".rtk"),
            fixture.project.path().join(".claude"),
            fixture.home.path().join(".claude"),
            fixture.home.path().join("config/rtk"),
            fixture.home.path().join("data"),
        ] {
            std::fs::create_dir_all(path).expect("create isolated profile");
        }
        std::fs::write(
            fixture.project.path().join(".claude/settings.json"),
            json!({ "permissions": {
                "allow": ["Bash(git status)"],
                "deny": ["Bash(git push:*)"]
            } })
            .to_string(),
        )
        .expect("write permission controls");
        if let Some(rules) = rules {
            std::fs::write(
                fixture.project.path().join(".rtk/pipeline.toml"),
                format!("{HEADER}{rules}"),
            )
            .expect("write project pipeline");
            if trusted {
                let output = fixture.output(&["trust", "--yes"]);
                assert!(output.status.success(), "{}", stderr(&output));
                let validation = fixture.output(&["pipeline", "validate"]);
                assert!(
                    validation.status.success(),
                    "stdout: {}\nstderr: {}",
                    String::from_utf8_lossy(&validation.stdout),
                    stderr(&validation)
                );
            }
        }
        fixture
    }

    fn command(&self) -> Command {
        let mut command = common::rtk_command();
        command
            .current_dir(self.project.path())
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("RTK_CONFIG_DIR", self.home.path().join("config/rtk"))
            .env("RTK_DB_PATH", self.home.path().join("data/rtk/history.db"))
            .env("RTK_RECALL_DB", self.home.path().join("data/rtk/recall.db"))
            .env("RTK_TEE_DIR", self.home.path().join("data/rtk/tee"))
            .env("CLAUDE_CONFIG_DIR", self.home.path().join(".claude"))
            .env("RTK_TEST_MODEL_KEY", "synthetic-test-key");
        command
    }

    fn output(&self, args: &[&str]) -> Output {
        self.command()
            .args(args)
            .output()
            .expect("run isolated rtk")
    }

    fn check(&self, agent: &str, command: &str) -> Output {
        self.output(&["hook", "check", "--agent", agent, "--", command])
    }

    fn hook(&self, command: &str) -> Option<Value> {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "cwd": self.project.path(),
            "tool_input": { "command": command }
        });
        let mut child = self
            .command()
            .args(["hook", "claude"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start hook");
        child
            .stdin
            .take()
            .expect("hook stdin")
            .write_all(payload.to_string().as_bytes())
            .expect("write hook input");
        let output = child.wait_with_output().expect("finish hook");
        assert!(output.status.success(), "{}", stderr(&output));
        (!output.stdout.is_empty())
            .then(|| serde_json::from_slice(&output.stdout).expect("hook JSON"))
    }

    fn files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        collect_files(self.home.path(), &mut files);
        collect_files(self.project.path(), &mut files);
        files
    }
}

fn collect_files(directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in std::fs::read_dir(directory).expect("read fixture directory") {
        let path = entry.expect("fixture entry").path();
        if path.is_dir() {
            collect_files(&path, files);
        } else {
            files.insert(
                path.clone(),
                std::fs::read(path).expect("read fixture file"),
            );
        }
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn server() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve local model endpoint");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let url = format!(
        "http://{}/v1",
        listener.local_addr().expect("model address")
    );
    (listener, url)
}

fn assert_no_request(listener: &TcpListener) {
    assert_eq!(
        listener
            .accept()
            .expect_err("dry-run must not contact a model")
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
}

fn routed(mode: Option<&str>, url: &str) -> String {
    let mode = mode
        .map(|mode| format!("mode = '{mode}'\n"))
        .unwrap_or_default();
    format!(
        r#"
[upstream.test-model]
provider = "openai"
model = "test-model"
base_url = "{url}"
api_key_env = "RTK_TEST_MODEL_KEY"
timeout_secs = 1
[task.preview-worker]
kind = "digest"
upstream = "test-model"
[[hook.before_tool.shell]]
name = "route-preview"
action = "route"
task = "preview-worker"
{mode}
"#
    )
}

#[test]
fn pipeline_deny_precedes_a_permission_allow() {
    let fixture = Fixture::new(Some(STATIC_RULES), true);
    let actual = fixture.hook("git status").expect("native pipeline denial");
    assert_eq!(actual["hookSpecificOutput"]["permissionDecision"], "deny");
    let check = fixture.check("claude", "git status");
    assert_eq!(check.status.code(), Some(2));
    assert!(check.stdout.is_empty());
    assert!(stderr(&check).contains("deny-status"));
    assert!(stderr(&check).contains("status is denied by the project pipeline"));
}

#[test]
fn pipeline_passthrough_agrees_with_the_native_hook() {
    let fixture = Fixture::new(Some(STATIC_RULES), true);
    assert!(fixture.hook("git log -n 1").is_none());
    let check = fixture.check("claude", "git log -n 1");
    assert_eq!(check.status.code(), Some(1));
    assert!(check.stdout.is_empty());
    assert!(stderr(&check).contains("keep-log"));
    assert!(stderr(&check).contains("Passthrough"));
}

#[test]
fn permission_denial_is_distinct_from_no_rewrite_and_rewrite() {
    let fixture = Fixture::new(None, false);
    let rewrite = fixture.check("claude", "git status");
    assert_eq!(rewrite.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&rewrite.stdout).trim(),
        "rtk git status"
    );
    assert_eq!(fixture.check("claude", "echo hi").status.code(), Some(1));
    let denied = fixture.check("claude", "git push");
    assert_eq!(denied.status.code(), Some(2));
    assert!(stderr(&denied).contains("Denied by a permission rule"));
}

#[test]
fn other_agents_keep_their_existing_pipeline_scope() {
    let fixture = Fixture::new(Some(STATIC_RULES), true);
    let check = fixture.check("codex", "git status");
    assert_eq!(check.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&check.stdout).trim(),
        "rtk git status"
    );
    assert!(!stderr(&check).contains("deny-status"));
}

#[test]
fn untrusted_pipeline_is_not_applied() {
    let fixture = Fixture::new(Some(STATIC_RULES), false);
    let check = fixture.check("claude", "git status");
    assert_eq!(check.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&check.stdout).trim(),
        "rtk git status"
    );
    assert!(stderr(&check).contains("untrusted"));
}

#[test]
fn disabling_the_pipeline_rewrite_does_not_report_a_rewrite() {
    let fixture = Fixture::new(
        Some(
            r#"
[override.rtk-rewrite]
enabled = false
"#,
        ),
        true,
    );
    assert!(fixture.hook("git status").is_none());
    let check = fixture.check("claude", "git status");
    assert_eq!(check.status.code(), Some(1));
    assert!(check.stdout.is_empty());
    assert!(stderr(&check).contains("no hook.before_tool.shell pipeline rule matches"));
}

#[test]
fn route_names_the_task_and_matches_the_native_stop() {
    let (listener, url) = server();
    let fixture = Fixture::new(Some(&routed(None, &url)), true);
    let actual = fixture.hook("git status").expect("native route stop");
    assert_eq!(actual["hookSpecificOutput"]["permissionDecision"], "deny");
    let check = fixture.check("claude", "git status");
    assert_eq!(check.status.code(), Some(2));
    assert!(check.stdout.is_empty());
    assert!(stderr(&check).contains("route-preview"));
    assert!(stderr(&check).contains("preview-worker"));
    assert_no_request(&listener);
}

#[test]
fn swap_uses_the_native_replacement_and_preserves_the_permission_gate() {
    let (listener, url) = server();
    let fixture = Fixture::new(Some(&routed(Some("swap"), &url)), true);
    let actual = fixture.hook("git status").expect("native swap");
    let check = fixture.check("claude", "git status");
    assert_eq!(check.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&check.stdout).trim(),
        actual["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .expect("native replacement")
    );
    assert!(fixture.hook("git push").is_none());
    assert_eq!(fixture.check("claude", "git push").status.code(), Some(2));
    assert_no_request(&listener);
}

#[test]
fn invalid_answer_rule_keeps_the_fallback_without_executing_anything() {
    let (listener, url) = server();
    // The schema reserves answer mode for read/search/web. A shell diagnostic
    // must retain that validation and its normal fallback, not run the task.
    let fixture = Fixture::new(Some(&routed(Some("answer"), &url)), false);
    let trust = fixture.output(&["trust", "--yes"]);
    assert!(trust.status.success(), "{}", stderr(&trust));
    let validation = fixture.output(&["pipeline", "validate"]);
    assert!(!validation.status.success());
    assert!(String::from_utf8_lossy(&validation.stdout).contains("does not apply to"));
    let before = fixture.files();
    assert_eq!(fixture.check("claude", "").status.code(), Some(1));
    let check = fixture.check("claude", "echo should-not-run > preview-marker.txt");
    assert_eq!(check.status.code(), Some(1));
    assert!(check.stdout.is_empty());
    assert!(stderr(&check).contains("does not apply to"));
    assert_eq!(
        fixture.files(),
        before,
        "dry-run must not write status or run the command"
    );
    assert_no_request(&listener);
}

#[test]
fn classifier_preview_reports_missing_runtime_data_without_calling_the_judge() {
    let (listener, url) = server();
    let rules = format!(
        r#"
[upstream.test-judge]
provider = "laya"
model = "test-judge"
base_url = "{url}/systemone"
api_key_env = "RTK_TEST_MODEL_KEY"
timeout_secs = 1
[classifier.preview]
upstream = "test-judge"
question = "noul"
instructions = "Return true for this synthetic test."
[[hook.before_tool.shell]]
name = "judged-passthrough"
when = {{ "noul.preview_gte" = 0.8 }}
action = "passthrough"
"#
    );
    let fixture = Fixture::new(Some(&rules), true);
    let before = fixture.files();
    let check = fixture.check("claude", "git status");
    assert_eq!(check.status.code(), Some(3));
    assert!(check.stdout.is_empty());
    assert!(stderr(&check).contains("runtime classifier results"));
    assert_eq!(
        fixture.files(),
        before,
        "dry-run must not write classifier or status data"
    );
    assert_no_request(&listener);
}
