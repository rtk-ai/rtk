//! Human-facing `rtk hook check`: preview policy without executing a tool or a task.

use crate::hooks::decision::{AgentPath, HookDecision};

pub(crate) fn run(agent: &str, command: &str) -> i32 {
    let Some(path) = AgentPath::from_agent(agent) else {
        return 2;
    };

    #[cfg(feature = "engine")]
    if !command.is_empty()
        && matches!(&path, AgentPath::ClaudeHook)
        && let Some(code) = check_pipeline(command)
    {
        return code;
    }

    match path.decide(command) {
        HookDecision::AllowRewrite(rewritten) | HookDecision::AskRewrite(rewritten) => {
            println!("{rewritten}");
            0
        }
        HookDecision::Deny => {
            eprintln!("Denied by a permission rule: {command}");
            2
        }
        HookDecision::Defer => {
            eprintln!("No rewrite for: {command}");
            1
        }
    }
}

/// The runtime's loader, facts and rule matcher, without its status writes,
/// classifier calls or task execution. `None` continues to the permission gate.
#[cfg(feature = "engine")]
fn check_pipeline(command: &str) -> Option<i32> {
    use super::{pipeline_config, task_cmd};
    use crate::hooks::{decision, permissions};
    use rtk_engine::block::{Event, Subject};
    use rtk_engine::pipeline::{
        Action, DecisionKind, Position, evaluate, interpolate, route_message,
    };

    if !pipeline_config::any_user_pipeline_files() {
        return None;
    }
    let loaded = pipeline_config::load();
    for diagnostic in &loaded.diagnostics {
        eprintln!(
            "Pipeline {}: {}",
            diagnostic.code.as_str(),
            diagnostic.message
        );
    }
    if loaded.fell_back {
        return None;
    }

    let position = Position::hook(Event::BeforeTool, Subject::Shell);
    // The runtime populates these facts by calling its configured classifiers
    // before matching rules. Missing results must not look like a known false.
    if !loaded.resolved.classifiers_referenced(position).is_empty() {
        eprintln!(
            "Cannot determine the pipeline decision without runtime classifier results; \
             dry-run does not call classifiers."
        );
        return Some(3);
    }
    let facts = pipeline_config::shell_facts(command);
    let decisions = evaluate(&loaded.resolved, position, &facts);
    let Some(first) = decisions.first() else {
        eprintln!("No rewrite: no hook.before_tool.shell pipeline rule matches {command}");
        return Some(1);
    };
    let DecisionKind::Act(action) = &first.kind else {
        return None;
    };
    let rule = &first.rule.name;
    eprintln!(
        "Pipeline rule `{rule}` [{}]: {}",
        first.rule.origin,
        action.as_str()
    );
    match action {
        Action::Rewrite => None,
        Action::Passthrough => {
            eprintln!("Passthrough: {command}");
            Some(1)
        }
        Action::Deny => {
            let message = first
                .rule
                .def
                .message
                .as_deref()
                .filter(|message| !message.trim().is_empty())
                .unwrap_or("blocked by an rtk pipeline rule");
            eprintln!("Denied: {}", interpolate(message, &facts));
            Some(2)
        }
        Action::Route => {
            let task = first.rule.def.task.as_deref().unwrap_or("digest");
            if first.rule.def.mode.as_deref() == Some("swap")
                && let Some(swapped) = task_cmd::swap_command(task, command)
            {
                let verdict = permissions::check_command_for(command, permissions::Host::Claude);
                // Match the runtime swap gate, which does not offer a
                // fish wrap. A refused swap resumes the ordinary hook.
                return match decision::decide_for_agent(
                    command,
                    verdict,
                    permissions::DenyRules::Unattested,
                ) {
                    HookDecision::AllowRewrite(_) | HookDecision::AskRewrite(_) => {
                        println!("{swapped}");
                        Some(0)
                    }
                    HookDecision::Deny | HookDecision::Defer => None,
                };
            }
            eprintln!(
                "Routed: {}",
                route_message(task, &facts, first.rule.def.message.as_deref())
            );
            Some(2)
        }
    }
}
