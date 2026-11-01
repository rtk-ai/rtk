use super::audit_log;
use crate::core::pipeline_config::{self, ShellHookDecision};
use crate::hooks::decision::{self, HookDecision};
use crate::hooks::permissions::{self, Host};
use rtk_engine::block::Subject;
use rtk_engine::harness::HarnessProfile;
use serde_json::{Value, json};

pub(super) struct ToolCall<'a> {
    pub profile: &'static HarnessProfile,
    pub tool_name: &'a str,
    pub input: Value,
    pub event: &'a Value,
}

pub(super) enum Gate {
    Stop {
        message: String,
        rule: String,
        action: &'static str,
    },
    Leave {
        rule: Option<String>,
    },
    Swap {
        command: String,
        rule: String,
        ask: bool,
    },
    Continue,
}

pub(super) enum Handled {
    Emit(String),
    Silent,
}

const COMMAND_KEYS: &[&str] = &["command", "CommandLine", "cmd", "Command"];
const PATH_KEYS: &[&str] = &[
    "file_path",
    "path",
    "TargetFile",
    "AbsolutePath",
    "target_file",
    "filePath",
    "file",
];
const URL_KEYS: &[&str] = &["url", "Url", "URL"];
const PATTERN_KEYS: &[&str] = &["pattern", "Query", "query", "SearchPattern"];
const RANGE_KEYS: &[&str] = &[
    "offset",
    "limit",
    "StartLine",
    "EndLine",
    "start_line",
    "end_line",
];

pub(super) fn host_for(profile: &HarnessProfile) -> Option<Host> {
    match profile.id {
        "claude-code" => Some(Host::Claude),
        "codex" => Some(Host::Codex),
        "trae" => Some(Host::Trae),
        "cursor" => Some(Host::Cursor),
        "gemini" => Some(Host::Gemini),
        "droid" => Some(Host::Droid),
        "vibe" => Some(Host::Vibe),
        "antigravity" => Some(Host::Antigravity),
        "opencode" => Some(Host::OpenCode),
        _ => None,
    }
}

fn first_string<'a>(input: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        input
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    })
}

pub(super) fn normalise_input(subject: Subject, input: &Value) -> Value {
    let mut out = input.clone();
    let Some(obj) = out.as_object_mut() else {
        return json!({});
    };
    let canonical: &[(&str, &[&str])] = match subject {
        Subject::Shell => &[("command", COMMAND_KEYS)],
        Subject::Read | Subject::Write | Subject::Edit => &[("file_path", PATH_KEYS)],
        Subject::Web => &[("url", URL_KEYS)],
        Subject::Search => &[("pattern", PATTERN_KEYS), ("path", PATH_KEYS)],
        _ => &[],
    };
    for (key, aliases) in canonical {
        if obj.get(*key).and_then(Value::as_str).is_some() {
            continue;
        }
        if let Some(value) = first_string(input, aliases) {
            obj.insert((*key).to_string(), Value::String(value.to_string()));
        }
    }
    out
}

pub(super) fn is_ranged_read(input: &Value) -> bool {
    RANGE_KEYS
        .iter()
        .any(|key| input.get(key).is_some_and(|v| !v.is_null()))
}

pub(super) fn session_of(event: &Value) -> Option<String> {
    ["session_id", "conversation_id", "conversationId"]
        .iter()
        .find_map(|key| event.get(key).and_then(Value::as_str))
        .map(str::to_string)
}

pub(super) fn prepare(profile: &HarnessProfile, event: &Value, input: &Value) {
    crate::hooks::set_hook_agent(profile.id);
    if let Some(purpose) = input.get("description").and_then(Value::as_str) {
        pipeline_config::set_call_purpose(purpose);
    }
    let transcript = event
        .get("transcript_path")
        .or_else(|| event.get("transcriptPath"))
        .cloned()
        .unwrap_or(Value::Null);
    let source = json!({
        "transcript_path": transcript,
        "agent_message": event.get("agent_message").cloned().unwrap_or(Value::Null),
        "tool_input": { "description": input.get("description").cloned().unwrap_or(Value::Null) },
    });
    crate::core::call_context::set_source(&source);
}

pub(super) fn gate_call(
    profile: &'static HarnessProfile,
    event: &Value,
    tool_name: &str,
    input: Value,
) -> Gate {
    let subject = profile.map_tool(tool_name);
    let call = ToolCall {
        profile,
        tool_name,
        input: normalise_input(subject, &input),
        event,
    };
    gate(&call)
}

pub(super) fn gate(call: &ToolCall) -> Gate {
    if !pipeline_config::any_user_pipeline_files() {
        return Gate::Continue;
    }
    let subject = call.profile.map_tool(call.tool_name);
    let gate = match subject {
        Subject::Read => read_gate(call),
        Subject::Shell => shell_gate(call),
        Subject::Any if call.input.get("command").is_some() => shell_gate(call),
        other => tool_gate(call, other),
    };
    record(call, subject, &gate);
    gate
}

fn read_gate(call: &ToolCall) -> Gate {
    let Some(path) = call.input.get("file_path").and_then(Value::as_str) else {
        return Gate::Continue;
    };
    let whole_file = !is_ranged_read(&call.input);
    let path = std::path::PathBuf::from(path);
    match pipeline_config::before_read_decision(&path, whole_file) {
        Some(decision) => Gate::Stop {
            message: decision.message,
            rule: decision.rule,
            action: decision.action,
        },
        None => Gate::Continue,
    }
}

fn tool_gate(call: &ToolCall, subject: Subject) -> Gate {
    let mut facts = pipeline_config::subject_facts(subject, call.tool_name, &call.input);
    if let Some(server) = call.profile.mcp_server(call.tool_name) {
        facts.insert(
            "server".into(),
            rtk_engine::pipeline::Fact::Str(server.to_string()),
        );
    }
    match pipeline_config::before_tool_decision(subject, &facts) {
        ShellHookDecision::Deny { message, rule } => Gate::Stop {
            message,
            rule,
            action: "deny",
        },
        ShellHookDecision::Route { message, rule } => Gate::Stop {
            message,
            rule,
            action: "route",
        },
        ShellHookDecision::Answer { message, rule } => Gate::Stop {
            message,
            rule,
            action: "answer",
        },
        ShellHookDecision::Passthrough { rule } => Gate::Leave { rule: Some(rule) },
        ShellHookDecision::NoRewrite => Gate::Leave { rule: None },
        ShellHookDecision::Rewrite | ShellHookDecision::Swap { .. } => Gate::Continue,
    }
}

fn shell_gate(call: &ToolCall) -> Gate {
    let Some(cmd) = call
        .input
        .get("command")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
    else {
        return Gate::Continue;
    };
    match pipeline_config::before_shell_decision(cmd) {
        ShellHookDecision::Rewrite => Gate::Continue,
        ShellHookDecision::NoRewrite => Gate::Leave { rule: None },
        ShellHookDecision::Deny { message, rule } => Gate::Stop {
            message,
            rule,
            action: "deny",
        },
        ShellHookDecision::Route { message, rule } => Gate::Stop {
            message,
            rule,
            action: "route",
        },
        ShellHookDecision::Answer { message, rule } => Gate::Stop {
            message,
            rule,
            action: "answer",
        },
        ShellHookDecision::Passthrough { rule } => Gate::Leave { rule: Some(rule) },
        ShellHookDecision::Swap { command, rule } => {
            let Some(host) = host_for(call.profile) else {
                return Gate::Continue;
            };
            let verdict = permissions::check_command_for(cmd, host);
            match decision::decide_for_agent(cmd, verdict, permissions::DenyRules::Unattested) {
                HookDecision::AllowRewrite(_) => Gate::Swap {
                    command,
                    rule,
                    ask: false,
                },
                HookDecision::AskRewrite(_) => Gate::Swap {
                    command,
                    rule,
                    ask: true,
                },
                HookDecision::Deny | HookDecision::Defer => Gate::Continue,
            }
        }
    }
}

fn target_of(call: &ToolCall, subject: Subject) -> String {
    let key = match subject {
        Subject::Shell => "command",
        Subject::Read | Subject::Write | Subject::Edit => "file_path",
        Subject::Web => "url",
        _ => return call.tool_name.to_string(),
    };
    call.input
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(call.tool_name)
        .to_string()
}

fn record(call: &ToolCall, subject: Subject, gate: &Gate) {
    let target = target_of(call, subject);
    let (rule, action) = match gate {
        Gate::Stop {
            rule,
            action,
            message: _,
        } => {
            audit_log("skip:pipeline_stop", &target, rule);
            (rule.as_str(), *action)
        }
        Gate::Leave { rule: Some(rule) } => {
            audit_log("skip:pipeline_passthrough", &target, rule);
            (rule.as_str(), "passthrough")
        }
        Gate::Swap { rule, command, .. } => {
            audit_log("rewrite", &target, command);
            (rule.as_str(), "swap")
        }
        Gate::Leave { rule: None } | Gate::Continue => return,
    };
    use crate::core::pipeline_log;
    if !pipeline_log::enabled() {
        return;
    }
    pipeline_log::append(&pipeline_log::LogEntry {
        ts: pipeline_log::now_ts(),
        session: session_of(call.event),
        site: "hook".to_string(),
        event: "before_tool".to_string(),
        subject: subject.as_str().to_string(),
        tool: Some(subject.as_str().to_string()),
        command: Some(target),
        rule: rule.to_string(),
        action: Some(action.to_string()),
        ..pipeline_log::LogEntry::default()
    });
}

pub(super) fn with_command(input: &Value, command: &str) -> Value {
    let mut updated = input.clone();
    match updated.as_object_mut() {
        Some(obj) => {
            obj.insert("command".into(), Value::String(command.to_string()));
        }
        None => updated = json!({ "command": command }),
    }
    updated
}

#[cfg(test)]
pub(super) fn with_pipeline<T>(pipeline: &str, run: impl FnOnce() -> T) -> T {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("pipeline.toml"), pipeline).expect("write pipeline");
    let root = dir.path().to_str().expect("utf-8 path");
    crate::core::user_env::with_vars(
        &[
            ("RTK_CONFIG_DIR", Some(root)),
            ("XDG_DATA_HOME", Some(root)),
            ("HOME", Some(root)),
        ],
        run,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_keys_are_added_beside_each_agent_spelling() {
        let shell = normalise_input(
            Subject::Shell,
            &json!({ "CommandLine": "git status", "Cwd": "/w" }),
        );
        assert_eq!(shell["command"], "git status");
        assert_eq!(shell["CommandLine"], "git status");

        let write = normalise_input(Subject::Write, &json!({ "TargetFile": "/w/a.rs" }));
        assert_eq!(write["file_path"], "/w/a.rs");

        let read = normalise_input(Subject::Read, &json!({ "AbsolutePath": "/w/a.rs" }));
        assert_eq!(read["file_path"], "/w/a.rs");

        let web = normalise_input(Subject::Web, &json!({ "Url": "https://x" }));
        assert_eq!(web["url"], "https://x");

        let kept = normalise_input(Subject::Shell, &json!({ "command": "ls", "cmd": "x" }));
        assert_eq!(kept["command"], "ls");

        assert_eq!(normalise_input(Subject::Shell, &json!("text")), json!({}));
    }

    #[test]
    fn ranged_reads_are_detected_in_every_spelling() {
        assert!(is_ranged_read(&json!({ "file_path": "a", "limit": 10 })));
        assert!(is_ranged_read(
            &json!({ "AbsolutePath": "a", "StartLine": 3 })
        ));
        assert!(!is_ranged_read(
            &json!({ "file_path": "a", "offset": null })
        ));
        assert!(!is_ranged_read(&json!({ "file_path": "a" })));
    }

    #[test]
    fn session_is_read_in_each_agent_spelling() {
        assert_eq!(
            session_of(&json!({ "session_id": "s" })).as_deref(),
            Some("s")
        );
        assert_eq!(
            session_of(&json!({ "conversationId": "c" })).as_deref(),
            Some("c")
        );
        assert_eq!(session_of(&json!({})), None);
    }

    #[test]
    fn with_command_keeps_the_other_arguments() {
        let updated = with_command(&json!({ "command": "ls", "timeout": 5 }), "rtk ls");
        assert_eq!(updated, json!({ "command": "rtk ls", "timeout": 5 }));
        assert_eq!(
            with_command(&Value::Null, "rtk ls"),
            json!({ "command": "rtk ls" })
        );
    }
}
