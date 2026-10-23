use super::{STDIN_CAP, audit_log, pre_tool_use_rewrite_output};
use crate::core::pipeline_config::{self, ShellHookDecision};
use crate::hooks::constants::{POST_TOOL_USE_KEY, PRE_TOOL_USE_KEY, SESSION_START_KEY};
use crate::hooks::decision::{self, HookDecision};
use crate::hooks::permissions;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{self, Read, Write};

const STDIN_CAP_AFTER_TOOL: usize = 64 * 1_048_576;

pub(super) fn read_payload() -> Result<String> {
    let mut input = String::new();
    io::stdin()
        .take((STDIN_CAP_AFTER_TOOL + 1) as u64)
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;
    if input.len() > STDIN_CAP_AFTER_TOOL {
        anyhow::bail!("hook stdin exceeds {} byte limit", STDIN_CAP_AFTER_TOOL);
    }
    Ok(input)
}

pub(super) fn handle(v: &Value, raw_len: usize) -> Result<bool> {
    crate::hooks::set_hook_agent("claude-code");
    if let Some(purpose) = v
        .pointer("/tool_input/description")
        .and_then(|d| d.as_str())
    {
        pipeline_config::set_call_purpose(purpose);
    }
    crate::core::call_context::set_source(v);

    match v.get("hook_event_name").and_then(|e| e.as_str()) {
        Some(POST_TOOL_USE_KEY) => {
            crate::hooks::posthook::run(v);
            return Ok(true);
        }
        Some(SESSION_START_KEY) => {
            crate::hooks::session_start::run();
            return Ok(true);
        }
        Some(PRE_TOOL_USE_KEY) | None => {
            if raw_len > STDIN_CAP {
                anyhow::bail!("hook stdin exceeds {} byte limit", STDIN_CAP);
            }
        }
        Some(_) => return Ok(true),
    }

    if !pipeline_config::any_user_pipeline_files() {
        return Ok(false);
    }
    if read_gate(v) || tool_gate(v) {
        return Ok(true);
    }
    Ok(shell_gate(v))
}

fn deny(message: &str) {
    let response = json!({
        "hookSpecificOutput": {
            "hookEventName": PRE_TOOL_USE_KEY,
            "permissionDecision": "deny",
            "permissionDecisionReason": message,
        }
    });
    let _ = writeln!(io::stdout(), "{response}");
}

fn read_gate(v: &Value) -> bool {
    let Some(tool) = v.get("tool_name").and_then(|t| t.as_str()) else {
        return false;
    };
    if !matches!(tool, "Read" | "NotebookRead") {
        return false;
    }
    let Some(path) = v.pointer("/tool_input/file_path").and_then(|p| p.as_str()) else {
        return false;
    };
    let ranged = ["offset", "limit"].iter().any(|key| {
        v.pointer("/tool_input")
            .and_then(|input| input.get(key))
            .is_some_and(|value| !value.is_null())
    });
    let path = std::path::PathBuf::from(path);
    let Some(decision) = pipeline_config::before_read_decision(&path, !ranged) else {
        return false;
    };
    deny(&decision.message);
    let target = path.display().to_string();
    audit_log(
        &format!("skip:pipeline_{}", decision.action),
        &target,
        &decision.rule,
    );
    log(v, "read", &target, &decision.rule, decision.action);
    true
}

fn tool_gate(v: &Value) -> bool {
    use rtk_engine::block::Subject;
    let Some(name) = v.get("tool_name").and_then(|t| t.as_str()) else {
        return false;
    };
    let Some(profile) = rtk_engine::harness::profile("claude-code") else {
        return false;
    };
    let subject = profile.map_tool(name);
    if matches!(subject, Subject::Shell | Subject::Read) {
        return false;
    }
    let input = v.get("tool_input").unwrap_or(&Value::Null);
    let facts = pipeline_config::subject_facts(subject, name, input);
    match pipeline_config::before_tool_decision(subject, &facts) {
        ShellHookDecision::Deny { message, rule }
        | ShellHookDecision::Route { message, rule }
        | ShellHookDecision::Answer { message, rule } => {
            deny(&message);
            audit_log("skip:pipeline_stop", name, &rule);
            log(v, subject.as_str(), name, &rule, "deny");
            true
        }
        ShellHookDecision::Passthrough { rule } => {
            log(v, subject.as_str(), name, &rule, "passthrough");
            true
        }
        ShellHookDecision::NoRewrite => true,
        ShellHookDecision::Rewrite | ShellHookDecision::Swap { .. } => false,
    }
}

fn shell_gate(v: &Value) -> bool {
    let Some(cmd) = v
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
    else {
        return false;
    };
    match pipeline_config::before_shell_decision(cmd) {
        ShellHookDecision::Rewrite => false,
        ShellHookDecision::NoRewrite => true,
        ShellHookDecision::Deny { message, rule } => stop(v, cmd, &message, &rule, "deny"),
        ShellHookDecision::Route { message, rule } => stop(v, cmd, &message, &rule, "route"),
        ShellHookDecision::Answer { message, rule } => stop(v, cmd, &message, &rule, "answer"),
        ShellHookDecision::Passthrough { rule } => {
            audit_log("skip:pipeline_passthrough", cmd, &rule);
            log(v, "shell", cmd, &rule, "passthrough");
            true
        }
        ShellHookDecision::Swap { command, rule } => {
            let verdict = permissions::check_command_for(cmd, permissions::Host::Claude);
            let (excluded, transparent) = crate::core::config::hook_rewrite_params();
            if !swappable(
                cmd,
                verdict,
                decision::decide_for_agent(cmd, verdict),
                &excluded,
                &transparent,
            ) {
                return false;
            }
            let output = pre_tool_use_rewrite_output(v, &command, None);
            let _ = writeln!(io::stdout(), "{output}");
            audit_log("rewrite", cmd, &command);
            log(v, "shell", cmd, &rule, "swap");
            true
        }
    }
}

fn swappable(
    cmd: &str,
    verdict: permissions::PermissionVerdict,
    rewrite: HookDecision,
    excluded: &[String],
    transparent: &[String],
) -> bool {
    use crate::discover::registry;
    use permissions::PermissionVerdict;
    match rewrite {
        HookDecision::AllowRewrite(_) | HookDecision::AskRewrite(_) => true,
        HookDecision::Deny => false,
        HookDecision::Defer => {
            matches!(
                verdict,
                PermissionVerdict::Allow | PermissionVerdict::Ask | PermissionVerdict::Default
            ) && !crate::discover::lexer::contains_unattestable_construct(cmd)
                && cmd.split_whitespace().next() != Some("rtk")
                && !cmd.contains(['\n', '\r', '\\'])
                && !registry::has_heredoc(cmd)
                && !cmd.contains("$((")
                && !registry::chain_mentions_excluded(cmd, excluded)
                && registry::rewrite_command(cmd, &[], transparent).is_none()
        }
    }
}

fn stop(v: &Value, cmd: &str, message: &str, rule: &str, action: &str) -> bool {
    deny(message);
    audit_log("skip:pipeline_stop", cmd, rule);
    log(v, "shell", cmd, rule, action);
    true
}

fn log(v: &Value, subject: &str, target: &str, rule: &str, action: &str) {
    use crate::core::pipeline_log;
    if !pipeline_log::enabled() {
        return;
    }
    pipeline_log::append(&pipeline_log::LogEntry {
        ts: pipeline_log::now_ts(),
        session: v
            .get("session_id")
            .and_then(|s| s.as_str())
            .map(str::to_string),
        site: "hook".to_string(),
        event: "before_tool".to_string(),
        subject: subject.to_string(),
        tool: Some(subject.to_string()),
        command: Some(target.to_string()),
        rule: rule.to_string(),
        action: Some(action.to_string()),
        ..pipeline_log::LogEntry::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use permissions::PermissionVerdict;

    fn decide(cmd: &str, verdict: PermissionVerdict, excluded: &[String]) -> bool {
        let rewrite = decision::decide_with_params(cmd, verdict, excluded, &[]);
        let rewrite = decision::suppress_identity(cmd, rewrite);
        swappable(cmd, verdict, rewrite, excluded, &[])
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
            "echo $((1 + 2))",
            "./run-suite.sh \\\n --fast",
            "./a.sh\n./b.sh",
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
}
