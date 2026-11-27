//! What delegation to the agent's own sub-agents saves. A pipeline rule that
//! routes a call (`action = "route"`, suggest mode) refuses it and tells the
//! agent to hand the work to a sub-agent; rtk counts each such call under the
//! rule's name. While a session has routed calls, every tool output a
//! sub-agent reads (a hook call carrying `agent_id`) adds to that rule's
//! reading, and each report a sub-agent hands back (`SubagentStop`) is
//! subtracted: the difference is context kept out of the main agent's
//! window. Claude Code reports sub-agent calls to hooks; other agents show
//! the routed count only. Everything is written to `engine_attribution`
//! (site `hook`, kind `route`), next to the other per-rule figures.

use crate::core::tracking::{Tracker, estimate_tokens};
use rtk_engine::pipeline::{Resolved, SiteKind};
use serde_json::Value;

use crate::core::tracking::ATTRIBUTION_ROUTE as ROUTE;
const SESSION: &str = "route-session";
const SEPARATOR: char = '\u{1f}';

pub fn routes_to_subagents(resolved: &Resolved) -> bool {
    resolved
        .rules
        .iter()
        .filter(|(position, _)| position.site == SiteKind::Hook)
        .flat_map(|(_, rules)| rules)
        .any(|rule| {
            rule.def.enabled
                && !rule.def.pro_inert
                && rule.def.action.as_deref() == Some(ROUTE)
                && matches!(rule.def.mode.as_deref(), None | Some("suggest"))
        })
}

pub fn record_route(event: &Value, rule: &str) {
    if let Ok(tracker) = Tracker::new() {
        record_route_in(&tracker, event, rule);
    }
}

pub fn observe(event: &Value) {
    if subagent_event(event)
        && let Ok(tracker) = Tracker::new()
    {
        observe_in(&tracker, event);
    }
}

fn subagent_event(event: &Value) -> bool {
    event
        .get("agent_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
}

fn record_route_in(tracker: &Tracker, event: &Value, rule: &str) {
    let _ = tracker.record_attribution_counted("hook", ROUTE, rule, 1, 0, 0);
    if let Some(session) = session_of(event) {
        let marker = format!("{session}{SEPARATOR}{rule}");
        let _ = tracker.record_attribution_counted("hook", SESSION, &marker, 1, 0, 0);
    }
}

fn observe_in(tracker: &Tracker, event: &Value) {
    if !subagent_event(event) {
        return;
    }
    let (read, report) = match event.get("hook_event_name").and_then(Value::as_str) {
        Some("PostToolUse") => (text_tokens(event.get("tool_response")), 0),
        Some("SubagentStop") => (0, text_tokens(event.get("last_assistant_message"))),
        _ => return,
    };
    if read == 0 && report == 0 {
        return;
    }
    let Some(session) = session_of(event) else {
        return;
    };
    let prefix = format!("{session}{SEPARATOR}");
    let Ok(Some(marker)) = tracker.attribution_name_with_prefix("hook", SESSION, &prefix) else {
        return;
    };
    let rule = &marker[prefix.len()..];
    let _ = tracker.record_attribution_counted("hook", ROUTE, rule, 0, read, report);
}

fn session_of(event: &Value) -> Option<&str> {
    event
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && !s.contains(SEPARATOR))
}

fn text_tokens(value: Option<&Value>) -> usize {
    fn collect(value: &Value, out: &mut String) {
        match value {
            Value::String(s) if !s.is_empty() => {
                out.push_str(s);
                out.push('\n');
            }
            Value::Array(items) => items.iter().for_each(|v| collect(v, out)),
            Value::Object(map) => map.values().for_each(|v| collect(v, out)),
            _ => {}
        }
    }
    let mut text = String::new();
    if let Some(value) = value {
        collect(value, &mut text);
    }
    if text.trim().is_empty() {
        0
    } else {
        estimate_tokens(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pipeline_config::load_candidate;
    use serde_json::json;

    #[test]
    fn only_suggest_routes_count_as_delegation_to_subagents() {
        let pipeline = |rule: &str| {
            format!(
                "schema_version = 1\n[pipeline]\nname = \"p\"\n[task.t]\nkind = \"digest\"\nupstream = \"u\"\n[upstream.u]\nprovider = \"anthropic\"\nmodel = \"m\"\n[[hook.before_tool.shell]]\nname = \"r\"\naction = \"route\"\ntask = \"t\"\n{rule}"
            )
        };
        let routed = |text: &str| routes_to_subagents(&load_candidate(text, "p").resolved);
        assert!(routed(&pipeline("")));
        assert!(routed(&pipeline("mode = \"suggest\"\n")));
        assert!(!routed(&pipeline("mode = \"answer\"\n")));
        assert!(!routed(&pipeline("enabled = false\n")));
        assert!(!routed(
            "schema_version = 1\n[pipeline]\nname = \"q\"\nextends = [\"rtk:default\"]\n"
        ));
    }

    #[test]
    fn subagent_reading_after_a_route_is_counted_minus_the_report() {
        let tracker = Tracker::new_in_memory().expect("tracker");
        let session = "sess-delegation-test";
        observe_in(
            &tracker,
            &json!({"session_id": "other-session", "hook_event_name": "PostToolUse",
                "agent_id": "a1", "tool_response": {"stdout": "x".repeat(4000)}}),
        );
        record_route_in(
            &tracker,
            &json!({"session_id": session}),
            "delegate-investigation",
        );
        record_route_in(
            &tracker,
            &json!({"session_id": session}),
            "delegate-investigation",
        );
        observe_in(
            &tracker,
            &json!({"session_id": session, "hook_event_name": "PostToolUse",
                "tool_response": {"stdout": "y".repeat(4000)}}),
        );
        observe_in(
            &tracker,
            &json!({"session_id": session, "hook_event_name": "PostToolUse",
                "agent_id": "a1", "tool_response": {"stdout": "z".repeat(4000), "stderr": ""}}),
        );
        observe_in(
            &tracker,
            &json!({"session_id": session, "hook_event_name": "SubagentStop",
                "agent_id": "a1", "last_assistant_message": "w".repeat(400)}),
        );
        let rows = tracker.attribution_site("hook", ROUTE, 30).expect("rows");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].name, "delegate-investigation");
        assert_eq!(rows[0].calls, 2);
        assert_eq!(
            rows[0].tokens_in,
            estimate_tokens(&format!("{}\n", "z".repeat(4000))) as i64
        );
        assert_eq!(
            rows[0].tokens_out,
            estimate_tokens(&format!("{}\n", "w".repeat(400))) as i64
        );
    }
}
