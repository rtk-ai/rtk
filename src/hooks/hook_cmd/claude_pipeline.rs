use super::agent_pipeline::{self, Gate};
use super::{STDIN_CAP, pre_tool_use_rewrite_output};
use crate::hooks::constants::{POST_TOOL_USE_KEY, PRE_TOOL_USE_KEY, SESSION_START_KEY};
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
    let Some(profile) = rtk_engine::harness::profile("claude-code") else {
        return Ok(false);
    };
    let input = v.get("tool_input").cloned().unwrap_or(Value::Null);
    agent_pipeline::prepare(profile, v, &input);

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

    let Some(tool_name) = v.get("tool_name").and_then(Value::as_str) else {
        return Ok(false);
    };
    Ok(
        match agent_pipeline::gate_call(profile, v, tool_name, input) {
            Gate::Continue => false,
            Gate::Leave { .. } => true,
            Gate::Stop { message, .. } => {
                deny(&message);
                true
            }
            Gate::Swap { command, .. } => {
                let output = pre_tool_use_rewrite_output(v, &command, None);
                let _ = writeln!(io::stdout(), "{output}");
                true
            }
        },
    )
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
