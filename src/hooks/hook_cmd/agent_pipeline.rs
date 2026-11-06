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
        decision: Option<&'static str>,
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
            let (excluded, transparent) = crate::core::config::hook_rewrite_params();
            let rewrite =
                decision::decide_for_agent(cmd, verdict, permissions::DenyRules::Unattested);
            let unfiltered = matches!(rewrite, HookDecision::Defer);
            let ask = matches!(rewrite, HookDecision::AskRewrite(..));
            if (unfiltered && host != Host::Claude)
                || !keeps_the_agents_check(verdict, unfiltered)
                || !swappable(
                    cmd,
                    verdict,
                    rewrite,
                    &excluded,
                    crate::core::utils::tool_exists,
                )
            {
                return Gate::Continue;
            }
            let ask_rule = matches!(verdict, permissions::PermissionVerdict::Ask)
                && permissions::ask_rule_applies(cmd, host, &transparent);
            Gate::Swap {
                command,
                rule,
                ask,
                decision: swap_decision(verdict, ask_rule, unfiltered),
            }
        }
    }
}

fn keeps_the_agents_check(verdict: permissions::PermissionVerdict, unfiltered: bool) -> bool {
    !unfiltered || !matches!(verdict, permissions::PermissionVerdict::Ask)
}

fn swap_decision(
    verdict: permissions::PermissionVerdict,
    ask_rule: bool,
    unfiltered: bool,
) -> Option<&'static str> {
    if ask_rule {
        Some("ask")
    } else if unfiltered && matches!(verdict, permissions::PermissionVerdict::Allow) {
        Some("allow")
    } else {
        None
    }
}

fn swappable(
    cmd: &str,
    verdict: permissions::PermissionVerdict,
    rewrite: HookDecision,
    excluded: &[String],
    program_exists: impl Fn(&str) -> bool,
) -> bool {
    use crate::discover::registry;
    use permissions::PermissionVerdict;
    if changes_directory(cmd) {
        return false;
    }
    match rewrite {
        HookDecision::AllowRewrite(_) | HookDecision::AskRewrite(..) => true,
        HookDecision::Deny => false,
        HookDecision::Defer => {
            matches!(
                verdict,
                PermissionVerdict::Allow | PermissionVerdict::Ask | PermissionVerdict::Default
            ) && !crate::discover::lexer::contains_unattestable_construct(cmd)
                && !crate::discover::fish_script::is_unambiguous_fish(cmd)
                && !runs_rtk(cmd)
                && !cmd.contains(['\n', '\r', '\\'])
                && !registry::chain_mentions_excluded(cmd, excluded)
                && sh_can_run(cmd, program_exists)
        }
    }
}

fn runs_rtk(cmd: &str) -> bool {
    cmd.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '\'' | '"' | '`' | '(' | ')' | '{' | '}' | ';' | '|' | '&'
            )
    })
    .any(|word| word == "rtk" || word.ends_with("/rtk"))
}

fn changes_directory(cmd: &str) -> bool {
    crate::discover::lexer::split_on_operators(cmd, false)
        .iter()
        .any(|segment| {
            segment
                .split_whitespace()
                .any(|word| matches!(word, "cd" | "pushd" | "popd"))
        })
}

fn sh_can_run(cmd: &str, program_exists: impl Fn(&str) -> bool) -> bool {
    crate::discover::lexer::split_on_operators(cmd, false)
        .iter()
        .all(|segment| {
            segment
                .split_whitespace()
                .find(|word| !is_assignment(word))
                .is_some_and(|program| program.contains('/') || program_exists(program))
        })
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
    })
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
    use permissions::PermissionVerdict;

    fn decide(cmd: &str, verdict: PermissionVerdict, excluded: &[String]) -> bool {
        let rewrite = decision::decide_with_params(
            cmd,
            verdict,
            excluded,
            &[],
            permissions::DenyRules::Unattested,
        );
        let rewrite = decision::suppress_identity(cmd, rewrite);
        swappable(cmd, verdict, rewrite, excluded, |_| true)
    }

    #[test]
    fn a_command_rtk_has_no_filter_for_is_swapped() {
        assert!(decide("./run-suite.sh", PermissionVerdict::Default, &[]));
        assert!(decide("python3 -c 1", PermissionVerdict::Allow, &[]));
        assert!(decide(
            "./run-suite.sh --fast && ./lint.sh",
            PermissionVerdict::Default,
            &[]
        ));
    }

    #[test]
    fn a_filtered_command_is_swapped_as_before() {
        assert!(decide("git log -3", PermissionVerdict::Default, &[]));
        assert!(decide("git log -3", PermissionVerdict::Allow, &[]));
        assert!(decide("bash -c 'ls'", PermissionVerdict::Ask, &[]));
    }

    #[test]
    fn an_ask_rule_swaps_like_any_other() {
        assert!(decide("./run-suite.sh", PermissionVerdict::Ask, &[]));
        assert!(decide("git log -3", PermissionVerdict::Ask, &[]));
    }

    #[test]
    fn a_command_that_moves_the_shell_is_not_swapped() {
        for cmd in [
            "cd sub && ./run-suite.sh",
            "cd sub && git status",
            "./run-suite.sh; cd sub",
            "pushd sub && ./run-suite.sh",
            "popd || ./run-suite.sh",
            "./run-suite.sh | tee out; cd sub",
        ] {
            assert!(!decide(cmd, PermissionVerdict::Allow, &[]), "{cmd}");
        }
    }

    #[test]
    fn what_a_child_sh_cannot_run_is_not_swapped() {
        let only_git = |program: &str| program == "git";
        for cmd in [
            "mydeploy",
            "mydeploy --fast",
            "./run-suite.sh && mydeploy",
            "./run-suite.sh | mydeploy",
            "FOO=1 mydeploy",
            "[[ -f x ]] && ./run-suite.sh",
            "if true; then ./run-suite.sh; fi",
            "FOO=1",
            "=x git status",
            "a-b=1 git status",
        ] {
            assert!(!sh_can_run(cmd, only_git), "{cmd}");
        }
        for cmd in [
            "git status",
            "./run-suite.sh --fast",
            "bin/run-suite",
            "FOO=1 ./run-suite.sh",
            "FOO=1 BAR=2 git status",
            "git status | ./run-suite.sh",
        ] {
            assert!(sh_can_run(cmd, only_git), "{cmd}");
        }
    }

    #[test]
    fn a_program_missing_from_path_stays_out_of_the_swap_only_when_unfiltered() {
        let swap = |cmd: &str| {
            let rewrite = decision::suppress_identity(
                cmd,
                decision::decide_with_params(
                    cmd,
                    PermissionVerdict::Allow,
                    &[],
                    &[],
                    permissions::DenyRules::Unattested,
                ),
            );
            swappable(cmd, PermissionVerdict::Allow, rewrite, &[], |_| false)
        };
        assert!(!swap("mydeploy"));
        assert!(swap("git log -3"));
    }

    #[test]
    fn an_unfiltered_command_stays_as_typed_when_the_swap_would_hide_it_from_a_rule() {
        use PermissionVerdict::{Allow, Ask, Default};
        assert!(!keeps_the_agents_check(Ask, true));
        assert!(keeps_the_agents_check(Default, true));
        assert!(keeps_the_agents_check(Allow, true));
    }

    #[test]
    fn a_filtered_command_swaps_as_before_whatever_the_rules() {
        use PermissionVerdict::{Allow, Ask, Default};
        for verdict in [Allow, Ask, Default] {
            assert!(keeps_the_agents_check(verdict, false));
        }
    }

    #[test]
    fn an_ask_rule_on_a_filtered_command_asks() {
        assert_eq!(
            swap_decision(PermissionVerdict::Ask, true, false),
            Some("ask")
        );
    }

    #[test]
    fn an_allow_rule_on_an_unfiltered_command_still_allows_its_swap() {
        assert_eq!(
            swap_decision(PermissionVerdict::Allow, false, true),
            Some("allow")
        );
    }

    #[test]
    fn otherwise_the_agent_checks_the_swap_itself() {
        use PermissionVerdict::{Allow, Ask, Default};
        assert_eq!(swap_decision(Default, false, true), None);
        assert_eq!(swap_decision(Default, false, false), None);
        assert_eq!(swap_decision(Allow, false, false), None);
        assert_eq!(swap_decision(Ask, false, false), None);
    }

    #[test]
    fn deny_and_defer_rules_stop_the_swap() {
        assert!(!decide("./run-suite.sh", PermissionVerdict::Deny, &[]));
        assert!(!decide("git log -3", PermissionVerdict::Deny, &[]));
        assert!(!decide("./run-suite.sh", PermissionVerdict::Defer, &[]));
    }

    #[test]
    fn commands_the_gate_cannot_attest_are_not_swapped() {
        for cmd in [
            "./run-suite.sh $(cat flags)",
            "./run-suite.sh `cat flags`",
            "cat <<EOF\nx\nEOF",
            "./run-suite.sh <<EOF",
            "echo $((1 + 2))",
            "./run-suite.sh \\\n --fast",
            "./a.sh\n./b.sh",
            "./run-suite.sh 'a\\nb'",
        ] {
            assert!(!decide(cmd, PermissionVerdict::Allow, &[]), "{cmd}");
        }
    }

    #[test]
    fn a_fish_script_is_never_swapped() {
        for cmd in [
            "test -d src; and git status",
            "test -d src; and ./run-suite.sh",
            "if test -d src\n  ./run-suite.sh\nend",
        ] {
            assert!(!decide(cmd, PermissionVerdict::Allow, &[]), "{cmd}");
        }
    }

    #[test]
    fn rtk_commands_are_not_swapped_again() {
        assert!(!decide("rtk git log -3", PermissionVerdict::Allow, &[]));
        assert!(!decide(
            "rtk check --question q -- ./run-suite.sh",
            PermissionVerdict::Default,
            &[]
        ));
    }

    #[test]
    fn excluded_commands_are_not_swapped() {
        let excluded = vec!["./run-suite.sh".to_string(), "curl".to_string()];
        assert!(!decide(
            "./run-suite.sh",
            PermissionVerdict::Allow,
            &excluded
        ));
        assert!(!decide(
            "cd x && curl -s a",
            PermissionVerdict::Allow,
            &excluded
        ));
        assert!(!decide(
            "python3 -c 1 | curl -s a",
            PermissionVerdict::Allow,
            &excluded
        ));
        assert!(!decide(
            "python3 -c 1 && curl -s a",
            PermissionVerdict::Allow,
            &excluded
        ));
        assert!(decide("python3 -c 1", PermissionVerdict::Allow, &excluded));
    }

    #[test]
    fn an_excluded_command_behind_a_wrapper_is_not_swapped() {
        let excluded = vec!["make".to_string()];
        for cmd in ["sh -c 'make'", "env FOO=1 make", "time make", "make test"] {
            assert!(!decide(cmd, PermissionVerdict::Allow, &excluded), "{cmd}");
            assert!(decide(cmd, PermissionVerdict::Allow, &[]), "{cmd}");
        }
    }

    #[test]
    fn an_excluded_command_with_no_filter_is_not_swapped_behind_a_wrapper_or_group() {
        let excluded = vec!["./run-suite.sh".to_string()];
        for cmd in [
            "sh -c './run-suite.sh'",
            "bash -c \"./run-suite.sh --fast\"",
            "env X=1 ./run-suite.sh",
            "X=1 ./run-suite.sh",
            "time ./run-suite.sh",
            "nice ./run-suite.sh",
            "nohup ./run-suite.sh",
            "command ./run-suite.sh",
            "xargs ./run-suite.sh",
            "( ./run-suite.sh )",
            "(./run-suite.sh)",
            "{ ./run-suite.sh; }",
            "./lint.sh | xargs ./run-suite.sh",
            "./lint.sh && time ./run-suite.sh --fast",
        ] {
            assert!(!decide(cmd, PermissionVerdict::Default, &excluded), "{cmd}");
            assert!(decide(cmd, PermissionVerdict::Default, &[]), "{cmd}");
        }
    }

    #[test]
    fn an_excluded_filtered_command_behind_xargs_is_not_swapped() {
        let excluded = vec!["git".to_string()];
        assert!(!decide(
            "xargs git status",
            PermissionVerdict::Default,
            &excluded
        ));
        assert!(decide("xargs git status", PermissionVerdict::Default, &[]));
    }

    #[test]
    fn an_excluded_command_called_by_path_is_not_swapped() {
        let excluded = vec!["curl".to_string(), "terraform".to_string()];
        for cmd in [
            "/usr/bin/curl -s a",
            "./node_modules/.bin/terraform plan",
            "python3 -c 1 | /usr/bin/curl -s a",
            "env X=1 /usr/bin/curl -s a",
        ] {
            assert!(!decide(cmd, PermissionVerdict::Default, &excluded), "{cmd}");
            assert!(decide(cmd, PermissionVerdict::Default, &[]), "{cmd}");
        }
        for cmd in ["python3 tools/curl/run.py", "/usr/bin/curlie -s a"] {
            assert!(decide(cmd, PermissionVerdict::Default, &excluded), "{cmd}");
        }
    }

    #[test]
    fn an_rtk_command_anywhere_in_the_line_is_not_swapped() {
        for cmd in [
            "rtk bypass ./run-suite.sh | ./lint.sh",
            "rtk frobnicate && ./run-suite.sh",
            "./lint.sh && rtk bypass ./run-suite.sh",
            "./lint.sh | rtk bypass cat",
            "./lint.sh;rtk bypass cat",
            "time rtk bypass ./run-suite.sh",
            "X=1 rtk bypass ./run-suite.sh",
            "/usr/local/bin/rtk bypass ./run-suite.sh",
        ] {
            assert!(!decide(cmd, PermissionVerdict::Default, &[]), "{cmd}");
        }
        assert!(decide(
            "./lint.sh && ./run-suite.sh",
            PermissionVerdict::Default,
            &[]
        ));
    }

    #[test]
    fn a_here_string_or_heredoc_is_not_swapped() {
        for cmd in [
            "./run-suite.sh <<<hello",
            "./run-suite.sh <<< 'abc'",
            "grep x <<<\"$d\"",
            "./run-suite.sh <</dev/null",
            "./run-suite.sh <<EOF\nx\nEOF",
            "./run-suite.sh <<-'EOF'\n\tx\nEOF",
        ] {
            assert!(!decide(cmd, PermissionVerdict::Default, &[]), "{cmd}");
        }
    }

    #[test]
    fn an_arithmetic_expansion_is_not_swapped_but_a_quoted_literal_is() {
        assert!(!decide(
            "./run-suite.sh $((1+2))",
            PermissionVerdict::Default,
            &[]
        ));
        assert!(decide(
            "./run-suite.sh '$((x))'",
            PermissionVerdict::Default,
            &[]
        ));
    }

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

    const SWAP: &str = "[upstream.u]\nprovider = \"anthropic\"\nmodel = \"m\"\n\n[task.t]\nkind = \"digest\"\nupstream = \"u\"\n\n[[hook.before_tool.shell]]\nname = \"sw\"\nwhen = { command_matches = \"^(./run|git log|cd )\" }\naction = \"route\"\ntask = \"t\"\nmode = \"swap\"\n";

    fn swap_gate(agent: &str, tool: &str, cmd: &str) -> Gate {
        let profile = rtk_engine::harness::profile(agent).expect("profile");
        let event = json!({ "tool_input": { "command": cmd } });
        with_pipeline(SWAP, || {
            gate_call(profile, &event, tool, json!({ "command": cmd }))
        })
    }

    const AGENTS: &[(&str, &str)] = &[
        ("claude-code", "Bash"),
        ("cursor", "Shell"),
        ("codex", "Bash"),
    ];

    #[test]
    fn a_filtered_command_swaps_on_every_agent() {
        for (agent, tool) in AGENTS {
            assert!(
                matches!(swap_gate(agent, tool, "git log -3"), Gate::Swap { .. }),
                "{agent}"
            );
        }
    }

    #[test]
    fn only_claude_swaps_a_command_rtk_has_no_filter_for() {
        assert!(matches!(
            swap_gate("claude-code", "Bash", "./run.sh"),
            Gate::Swap { decision: None, .. }
        ));
        assert!(matches!(
            swap_gate("cursor", "Shell", "./run.sh"),
            Gate::Continue
        ));
        assert!(matches!(
            swap_gate("codex", "Bash", "./run.sh"),
            Gate::Continue
        ));
    }

    #[test]
    fn no_agent_swaps_a_line_that_moves_the_shell() {
        for (agent, tool) in AGENTS {
            assert!(
                matches!(
                    swap_gate(agent, tool, "cd sub && git log -3"),
                    Gate::Continue
                ),
                "{agent}"
            );
        }
    }
}
