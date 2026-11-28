//! Messages for the user that a hook call collects while it runs, such as rtk
//! starting the local decision model a pipeline asks. The agent never reads
//! them. Claude Code shows them through the `systemMessage` field of the hook
//! answer; agents with no such field leave them to `rtk pipeline status`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ANSWERED: AtomicBool = AtomicBool::new(false);

pub fn push(message: String) {
    if let Ok(mut pending) = PENDING.lock() {
        pending.push(message);
    }
}

fn take() -> Option<String> {
    let mut pending = PENDING.lock().ok()?;
    if pending.is_empty() {
        return None;
    }
    Some(std::mem::take(&mut *pending).join("\n"))
}

pub fn claude_answer(answer: &str) -> String {
    ANSWERED.store(true, Ordering::Relaxed);
    let Some(message) = take() else {
        return answer.to_string();
    };
    with_system_message(answer, &message).unwrap_or_else(|| answer.to_string())
}

pub fn claude_leftover() -> Option<String> {
    if ANSWERED.load(Ordering::Relaxed) {
        return None;
    }
    let message = take()?;
    Some(serde_json::json!({ "systemMessage": message }).to_string())
}

fn with_system_message(answer: &str, message: &str) -> Option<String> {
    let serde_json::Value::Object(mut map) = serde_json::from_str(answer).ok()? else {
        return None;
    };
    let text = match map.get("systemMessage").and_then(|m| m.as_str()) {
        Some(existing) if !existing.is_empty() => format!("{existing}\n{message}"),
        _ => message.to_string(),
    };
    map.insert("systemMessage".into(), serde_json::Value::String(text));
    Some(serde_json::Value::Object(map).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_joins_the_answer_claude_reads() {
        let answer =
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#;
        let merged: serde_json::Value =
            serde_json::from_str(&with_system_message(answer, "rtk started Laya").expect("object"))
                .expect("json");
        assert_eq!(merged["systemMessage"], "rtk started Laya");
        assert_eq!(merged["hookSpecificOutput"]["permissionDecision"], "allow");
    }

    #[test]
    fn an_existing_message_is_kept_first() {
        let merged: serde_json::Value = serde_json::from_str(
            &with_system_message(r#"{"systemMessage":"proxy restarted"}"#, "rtk started Laya")
                .expect("object"),
        )
        .expect("json");
        assert_eq!(merged["systemMessage"], "proxy restarted\nrtk started Laya");
    }

    #[test]
    fn an_answer_that_is_not_an_object_is_left_alone() {
        assert_eq!(with_system_message("not json", "m"), None);
        assert_eq!(with_system_message("[1,2]", "m"), None);
    }
}
