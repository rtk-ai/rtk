//! Muse 1.4.14 policy overlay: rewrite only commands untouched by persisted rules.

use crate::core::user_dirs;
use crate::discover::lexer::split_for_permissions;
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn config_dir() -> Option<PathBuf> {
    user_dirs::env_path("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| user_dirs::home().map(|home| home.join(".config")))
        .map(|base| base.join("muse"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema_version: u64,
    rules: Vec<Rule>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    #[serde(rename = "effect")]
    _effect: Effect,
    #[serde(rename = "durability")]
    _durability: String,
    rule: Matcher,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Effect {
    Allow,
    Prompt,
    Deny,
}
#[derive(Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Matcher {
    #[serde(rename = "shell_command_argv_prefix")]
    Prefix {
        argv_prefix: Vec<String>,
        #[serde(rename = "workspace_root")]
        _workspace_root: String,
    },
    #[serde(rename = "shell_command_exact_raw")]
    Exact { command: String },
}

pub(crate) fn preserves_policy(original: &str, rewritten: &str, cwd: Option<&str>) -> bool {
    // Muse strips XDG from hook subprocesses. A shell dry-run with a custom
    // root cannot attest to the policy the live hook will read.
    if let Some(xdg) = user_dirs::env_path("XDG_CONFIG_HOME")
        && !user_dirs::home().is_some_and(|home| Path::new(&xdg) == home.join(".config"))
    {
        return false;
    }
    config_dir().is_some_and(|dir| preserves_at(&dir, original, rewritten, cwd))
}

fn preserves_at(dir: &Path, original: &str, rewritten: &str, cwd: Option<&str>) -> bool {
    // CR is a shell word character, not the whitespace the rewriter assumes.
    if original.contains('\r') || rewritten.contains('\r') {
        return false;
    }
    let Ok(raw) = fs::read_to_string(dir.join("settings.json")) else {
        return false;
    };
    let Ok(settings) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    if settings.get("schema_version").and_then(Value::as_u64) != Some(1)
        || settings
            .pointer("/permissions/default_profile")
            .and_then(Value::as_str)
            != Some(":ask-me")
        || settings
            .pointer("/permissions/schema_version")
            .is_some_and(|v| v.as_u64() != Some(1))
    {
        return false;
    }
    let raw = match fs::read_to_string(dir.join("approval-policy.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
        Err(_) => return false,
        Ok(raw) => raw,
    };
    let Ok(policy) = serde_json::from_str::<Policy>(&raw) else {
        return false;
    };
    policy.schema_version == 2 && parity(&policy.rules, original, rewritten, cwd)
}

fn parity(rules: &[Rule], original: &str, rewritten: &str, _cwd: Option<&str>) -> bool {
    if rules.is_empty() {
        return true;
    }
    let (Some(before), Some(after)) = (stages(original), stages(rewritten)) else {
        return false;
    };
    // Scope and effect never exclude a rule: hook cwd need not be the session
    // root. Step aside if any persisted rule names either candidate.
    !rules.iter().any(|rule| match &rule.rule {
        Matcher::Exact { command } => {
            [original, original.trim(), rewritten, rewritten.trim()].contains(&command.as_str())
                || before.iter().chain(&after).any(|stage| *stage == command)
                || [original, rewritten]
                    .into_iter()
                    .chain(before.iter().chain(&after).copied())
                    .any(|candidate| {
                        candidate.split_whitespace().collect::<Vec<_>>().join(" ")
                            == command.split_whitespace().collect::<Vec<_>>().join(" ")
                    })
        }
        Matcher::Prefix { argv_prefix, .. } => {
            let prefix: Vec<_> = argv_prefix.iter().map(String::as_str).collect();
            before.iter().chain(&after).any(|stage| {
                stage
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .starts_with(&prefix)
            })
        }
    })
}

fn stages(command: &str) -> Option<Vec<&str>> {
    // Only plain words and the existing splitters' operators are attested.
    if command.replace("&&", "").contains('&') {
        return None;
    }
    if !command
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b" _-.,:/@%+\t\n&|;".contains(&c))
    {
        return None;
    }
    let stages = split_for_permissions(command);
    if stages.is_empty()
        || stages.iter().any(|stage| {
            let words: Vec<_> = stage.split_whitespace().collect();
            words.is_empty()
                || words[0].contains('/')
                // See discover::registry PROCESS_WRAPPERS and MatchingReadings:
                // Muse's wrapper peeling is unknown, so defer on that peel set.
                || matches!(
                    words[0],
                    "env"
                        | "sudo"
                        | "command"
                        | "exec"
                        | "nice"
                        | "nohup"
                        | "time"
                        | "timeout"
                        | "stdbuf"
                        | "builtin"
                        | "noglob"
                        | "xargs"
                        | "nocorrect"
                        | "bash"
                        | "sh"
                        | "zsh"
                        | "fish"
                        | "dash"
                )
                || stage.contains(['&', '|', ';'])
        })
    {
        return None;
    }
    Some(stages)
}

#[cfg(test)]
#[path = "permissions_muse_tests.rs"]
mod tests;
