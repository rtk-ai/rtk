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
            let rewrite =
                decision::decide_for_agent(cmd, verdict, permissions::DenyRules::Unattested);
            let unfiltered = matches!(rewrite, HookDecision::Defer);
            if !keeps_the_agents_check(verdict, unfiltered)
                || !swappable(
                    cmd,
                    verdict,
                    rewrite,
                    &excluded,
                    crate::core::utils::tool_exists,
                )
            {
                return false;
            }
            let ask_rule = matches!(verdict, permissions::PermissionVerdict::Ask)
                && permissions::ask_rule_applies(cmd, permissions::Host::Claude, &transparent);
            let decision = swap_decision(verdict, ask_rule, unfiltered);
            let output = swap_output(v, &command, &rule, decision);
            let _ = writeln!(io::stdout(), "{output}");
            audit_log("rewrite", cmd, &command);
            log(v, "shell", cmd, &rule, "swap");
            true
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

fn swap_output(v: &Value, command: &str, rule: &str, decision: Option<&str>) -> Value {
    let mut output = pre_tool_use_rewrite_output(v, command, decision);
    if decision == Some("ask") {
        output["hookSpecificOutput"]["permissionDecisionReason"] = Value::String(format!(
            "RTK pipeline rule `{rule}` runs this command through a worker task, and it needs approval"
        ));
    }
    output
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
        HookDecision::AllowRewrite(_) | HookDecision::AskRewrite(..) => !holds_compound_syntax(cmd),
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

/// Whether the line holds a subshell or a compound command.
/// A swap runs a line in a child `sh`, which may read those otherwise or not at all,
/// and the checks here read words, not that grammar: such a line takes the ordinary
/// rewrite. `time`, `!` and braces do not count: the commands behind them read as usual.
fn holds_compound_syntax(cmd: &str) -> bool {
    let input = cmd.trim();
    crate::discover::lexer::split_for_classify(input)
        .iter()
        .any(|segment| {
            let first = segment.text.split_whitespace().next().unwrap_or_default();
            matches!(
                input[..segment.start].trim_end().bytes().last(),
                Some(b'(' | b')')
            ) || matches!(
                first,
                "[[" | "case"
                    | "coproc"
                    | "do"
                    | "done"
                    | "elif"
                    | "else"
                    | "esac"
                    | "fi"
                    | "for"
                    | "function"
                    | "if"
                    | "in"
                    | "select"
                    | "then"
                    | "until"
                    | "while"
            )
        })
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
    crate::discover::lexer::split_on_operators(cmd)
        .iter()
        .any(|segment| {
            segment
                .split_whitespace()
                .any(|word| matches!(word, "cd" | "pushd" | "popd"))
        })
}

fn sh_can_run(cmd: &str, program_exists: impl Fn(&str) -> bool) -> bool {
    crate::discover::lexer::split_on_operators(cmd)
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
    fn a_line_with_compound_syntax_is_not_swapped() {
        for cmd in [
            "(cd sub && git log -3)",
            "(ls)",
            "(ls)&cd sub",
            "(ls) && mydeploy",
            "ls; (cd sub)",
            "time (ls)",
            "if mydeploy; then ls; fi",
            "if true;then ls;fi&cd sub",
            "while mydeploy; do ls; done",
            "for a in b; do git status&cd sub; done",
            "case b in a)ls;;b)cd sub;;esac",
            "select a in b; do ls; done",
            "function f { ls; }",
            "[[ \"$b\" =~ ^(a|b)$ ]] && git log -3",
            "[[ -f f ]] && git status",
            "(ls",
            " (ls) && git status",
            "time for a in b; do ls; done",
            "! if a; then ls; fi",
            "{ for a in b; do ls; done; }",
            "git status; coproc ls",
            "f() { ls; }",
        ] {
            assert!(holds_compound_syntax(cmd), "{cmd}");
        }
        for cmd in [
            "git log -3",
            "ls; mydeploy",
            "time git log -3",
            "! git status",
            "{ git log -3; }",
            "git log --format=\"(%h)\"",
            "git log $(git rev-parse HEAD)",
            "git log if",
            "FOO=1 git status",
            "MSG=\"a=(b)\" git commit -m x",
            "PYTHONPATH=a+=b ls",
            "./build+=x/run.sh && git log",
            "make CFLAGS+=-O2",
            "git commit -m \"fix\n\ncount+=1 overflow\"",
            "RUSTFLAGS=\"-C a=b c+=d\" cargo build",
        ] {
            assert!(!holds_compound_syntax(cmd), "{cmd}");
        }
        for verdict in [PermissionVerdict::Allow, PermissionVerdict::Default] {
            assert!(!decide("(ls) && git status", verdict, &[]));
            assert!(decide("ls && git status", verdict, &[]));
        }
    }

    #[test]
    fn a_line_is_cut_at_operators_and_nothing_else() {
        use crate::discover::lexer::split_on_operators;
        assert_eq!(
            split_on_operators(" a && b; c | d || e "),
            ["a", "b", "c", "d", "e"]
        );
        for line in ["a & b", "(a) 2>&1", "a 'b;c'", "a \\; b", "x\ny"] {
            assert_eq!(split_on_operators(line), [line]);
        }
        assert_eq!(
            split_on_operators("a |& b ;; c ;& d ;;& e"),
            ["a", "b", "c", "d", "e"]
        );
        assert!(split_on_operators(" ; ").is_empty());
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
            "# note\ncd sub && git status",
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
            "./run-suite.sh @(a|b)",
            "${ git status; }",
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
            "./run-suite.sh & wait",
            "./run-suite.sh & # note",
            "(./run-suite.sh) 2>&1",
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
    fn an_ask_rule_still_asks_before_a_swap_runs() {
        let input = json!({ "tool_input": { "command": "./run-suite.sh" } });
        let asked = swap_output(&input, "rtk t -- x", "sw", Some("ask"));
        let hook = &asked["hookSpecificOutput"];
        assert_eq!(hook["permissionDecision"], "ask");
        assert_eq!(hook["updatedInput"]["command"], "rtk t -- x");
        assert!(
            hook["permissionDecisionReason"]
                .as_str()
                .is_some_and(|reason| reason.contains("`sw`"))
        );
    }

    #[test]
    fn a_swap_with_no_decision_leaves_the_check_to_the_agent() {
        let input = json!({ "tool_input": { "command": "./run-suite.sh" } });
        let output = swap_output(&input, "rtk t -- x", "sw", None);
        let hook = &output["hookSpecificOutput"];
        assert!(hook.get("permissionDecision").is_none());
        assert_eq!(hook["permissionDecisionReason"], "RTK auto-rewrite");
        assert_eq!(hook["updatedInput"]["command"], "rtk t -- x");
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
}
