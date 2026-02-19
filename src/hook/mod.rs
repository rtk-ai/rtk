mod cargo;
mod containers;
mod files;
mod git;
mod go;
pub mod helpers;
mod js_ts;
mod python;

use lazy_static::lazy_static;
use regex::Regex;
use std::io::Read;

lazy_static! {
    /// Matches leading env var assignments: FOO=bar BAZ=qux
    static ref ENV_PREFIX_RE: Regex =
        Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*=[^ ]* +)+").unwrap();
}

/// Entry point for `rtk hook-rewrite`.
/// Reads JSON from stdin, rewrites command if it matches known patterns.
/// Never returns — always exits with code 0.
pub fn run() -> ! {
    // Read all stdin
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        std::process::exit(0);
    }

    // Parse JSON
    let root: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => std::process::exit(0),
    };

    // Extract command
    let cmd = match root
        .get("tool_input")
        .and_then(|ti| ti.get("command"))
        .and_then(|c| c.as_str())
    {
        Some(c) if !c.is_empty() => c,
        _ => std::process::exit(0),
    };

    // Skip heredocs
    if cmd.contains("<<") {
        std::process::exit(0);
    }

    // Rewrite each segment of the command chain independently
    let rewritten = match rewrite_chain(cmd) {
        Some(r) => r,
        None => std::process::exit(0),
    };

    // Build output JSON: preserve all original tool_input fields, override command
    let mut updated_input = match root.get("tool_input").cloned() {
        Some(v) => v,
        None => std::process::exit(0),
    };
    if let Some(obj) = updated_input.as_object_mut() {
        obj.insert("command".to_string(), serde_json::Value::String(rewritten));
    }

    let output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": updated_input
        }
    });

    println!("{}", serde_json::to_string(&output).unwrap_or_default());
    std::process::exit(0);
}

/// Rewrite a command chain: split on " && " / " ; ", rewrite each segment.
/// Returns Some(rewritten) if at least one segment was rewritten, None otherwise.
pub(crate) fn rewrite_chain(cmd: &str) -> Option<String> {
    let segments = split_chain_segments(cmd);

    let mut any_rewritten = false;
    let mut result = String::with_capacity(cmd.len() + 32);

    for (i, (segment, _separator)) in segments.iter().enumerate() {
        if i > 0 {
            if let Some(sep) = &segments[i - 1].1 {
                result.push_str(sep);
            }
        }

        match rewrite_segment(segment) {
            Some(rewritten) => {
                result.push_str(&rewritten);
                any_rewritten = true;
            }
            None => {
                result.push_str(segment);
            }
        }
    }

    if any_rewritten {
        Some(result)
    } else {
        None
    }
}

/// Try to rewrite a single command segment (with env prefix handling).
fn rewrite_segment(segment: &str) -> Option<String> {
    let trimmed = segment.trim();
    if trimmed.is_empty() || trimmed.starts_with("rtk ") || trimmed.contains("/rtk ") {
        return None;
    }

    let (env_prefix, match_cmd, cmd_body) = strip_env_prefix(trimmed);
    try_rewrite(match_cmd, cmd_body).map(|r| {
        let leading_ws = &segment[..segment.len() - segment.trim_start().len()];
        format!("{}{}{}", leading_ws, env_prefix, r)
    })
}

/// Split a command on " && " and " ; " separators, respecting quotes.
fn split_chain_segments(cmd: &str) -> Vec<(&str, Option<&str>)> {
    let mut segments = Vec::new();
    let bytes = cmd.as_bytes();
    let mut start = 0;
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double_quote => in_single_quote = !in_single_quote,
            b'"' if !in_single_quote => in_double_quote = !in_double_quote,
            b' ' if !in_single_quote && !in_double_quote => {
                let rest = &cmd[i..];
                if rest.starts_with(" && ") {
                    segments.push((&cmd[start..i], Some(" && ")));
                    i += 4;
                    start = i;
                    continue;
                }
                if rest.starts_with(" ; ") {
                    segments.push((&cmd[start..i], Some(" ; ")));
                    i += 3;
                    start = i;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }

    segments.push((&cmd[start..], None));
    segments
}

/// Strip leading env var assignments (e.g. "FOO=bar BAZ=1 cmd args")
fn strip_env_prefix(cmd: &str) -> (&str, &str, &str) {
    if let Some(m) = ENV_PREFIX_RE.find(cmd) {
        let prefix = &cmd[..m.end()];
        let rest = &cmd[m.end()..];
        (prefix, rest, rest)
    } else {
        ("", cmd, cmd)
    }
}

/// Attempt to rewrite a command. Returns Some(rewritten_body) or None.
fn try_rewrite(match_cmd: &str, cmd_body: &str) -> Option<String> {
    // --- Git ---
    if match_cmd.starts_with("git ") || match_cmd == "git" {
        return git::try_rewrite_git(match_cmd, cmd_body);
    }

    // --- GitHub CLI ---
    if match_cmd.starts_with("gh ") {
        return git::try_rewrite_gh(match_cmd, cmd_body);
    }

    // --- Cargo ---
    if match_cmd.starts_with("cargo ") {
        return cargo::try_rewrite_cargo(match_cmd, cmd_body);
    }

    // --- File operations + network ---
    if let Some(r) = files::try_rewrite_file_cmd(match_cmd, cmd_body) {
        return Some(r);
    }

    // --- JS/TS tooling ---
    if let Some(r) = js_ts::try_rewrite_js_ts(match_cmd, cmd_body) {
        return Some(r);
    }

    // --- Containers ---
    if match_cmd.starts_with("docker ") {
        return containers::try_rewrite_docker(match_cmd, cmd_body);
    }
    if match_cmd.starts_with("kubectl ") {
        return containers::try_rewrite_kubectl(match_cmd, cmd_body);
    }

    // --- pnpm package management ---
    if let Some(r) = js_ts::try_rewrite_pnpm_pkg(match_cmd, cmd_body) {
        return Some(r);
    }

    // --- Python ---
    if let Some(r) = python::try_rewrite_python(match_cmd, cmd_body) {
        return Some(r);
    }

    // --- Go ---
    if let Some(r) = go::try_rewrite_go(match_cmd, cmd_body) {
        return Some(r);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(cmd: &str) -> Option<String> {
        if cmd.contains("<<") {
            return None;
        }
        rewrite_chain(cmd)
    }

    // --- Edge cases ---
    #[test]
    fn test_already_rtk() {
        assert_eq!(rewrite("rtk git status"), None);
    }

    #[test]
    fn test_heredoc_skip() {
        assert_eq!(rewrite("cat <<EOF\nhello\nEOF"), None);
    }

    #[test]
    fn test_no_match() {
        assert_eq!(rewrite("terraform plan"), None);
    }

    #[test]
    fn test_env_prefix_preserved() {
        assert_eq!(
            rewrite("TEST_SESSION_ID=2 npx playwright test"),
            Some("TEST_SESSION_ID=2 rtk playwright test".into())
        );
    }

    #[test]
    fn test_env_prefix_git() {
        assert_eq!(
            rewrite("GIT_AUTHOR_NAME=test git commit -m \"msg\""),
            Some("GIT_AUTHOR_NAME=test rtk git commit -m \"msg\"".into())
        );
    }

    #[test]
    fn test_empty_command() {
        assert_eq!(rewrite(""), None);
    }

    // --- Command chains ---
    #[test]
    fn test_cd_and_git_status() {
        assert_eq!(
            rewrite("cd /c/Users/Ayoub/project && git status"),
            Some("cd /c/Users/Ayoub/project && rtk git status".into())
        );
    }

    #[test]
    fn test_cd_quoted_path() {
        assert_eq!(
            rewrite("cd \"/path with spaces\" && git diff"),
            Some("cd \"/path with spaces\" && rtk git diff".into())
        );
    }

    #[test]
    fn test_cd_semicolon() {
        assert_eq!(
            rewrite("cd /tmp ; git log --oneline"),
            Some("cd /tmp ; rtk git log --oneline".into())
        );
    }

    #[test]
    fn test_cd_no_match() {
        assert_eq!(rewrite("cd /tmp && terraform plan"), None);
    }

    #[test]
    fn test_cd_alone() {
        assert_eq!(rewrite("cd /some/path"), None);
    }

    #[test]
    fn test_chain_cargo_quality_gate() {
        assert_eq!(
            rewrite("cargo fmt --all && cargo clippy --all-targets && cargo test --all"),
            Some(
                "rtk cargo fmt --all && rtk cargo clippy --all-targets && rtk cargo test --all"
                    .into()
            )
        );
    }

    #[test]
    fn test_chain_git_add_commit_push() {
        assert_eq!(
            rewrite("git add . && git commit -m \"msg\" && git push"),
            Some("rtk git add . && rtk git commit -m \"msg\" && rtk git push".into())
        );
    }

    #[test]
    fn test_chain_cd_then_multi_commands() {
        assert_eq!(
            rewrite("cd /project && cargo fmt && cargo test"),
            Some("cd /project && rtk cargo fmt && rtk cargo test".into())
        );
    }

    #[test]
    fn test_chain_mixed_match_no_match() {
        assert_eq!(
            rewrite("mkdir -p build && cargo test"),
            Some("mkdir -p build && rtk cargo test".into())
        );
    }

    #[test]
    fn test_chain_all_no_match() {
        assert_eq!(rewrite("mkdir -p build && cd build && cmake .."), None);
    }

    #[test]
    fn test_chain_with_env_prefix() {
        assert_eq!(
            rewrite("cd /project && NODE_ENV=test npm test"),
            Some("cd /project && NODE_ENV=test rtk npm test".into())
        );
    }

    #[test]
    fn test_chain_semicolon_multi() {
        assert_eq!(
            rewrite("git add . ; git status"),
            Some("rtk git add . ; rtk git status".into())
        );
    }

    // --- JSON roundtrip test ---
    #[test]
    fn test_json_roundtrip() {
        let input = r#"{"tool_input":{"command":"git status","timeout":30000}}"#;
        let root: serde_json::Value = serde_json::from_str(input).unwrap();
        let cmd = root["tool_input"]["command"].as_str().unwrap();
        assert_eq!(cmd, "git status");

        let rewritten = rewrite(cmd).unwrap();
        assert_eq!(rewritten, "rtk git status");

        let mut updated_input = root["tool_input"].clone();
        updated_input["command"] = serde_json::Value::String(rewritten);

        assert_eq!(updated_input["timeout"], 30000);
    }
}
