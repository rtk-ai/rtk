use serde_json::Value;
use std::path::PathBuf;

/// Verdict from checking a command against Claude Code's permission rules.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PermissionVerdict {
    /// No deny/ask rules matched — safe to auto-allow.
    Allow,
    /// A deny rule matched — pass through to Claude Code's native deny handling.
    Deny,
    /// An ask rule matched — rewrite the command but let Claude Code prompt the user.
    Ask,
}

/// Check `cmd` against Claude Code's deny/ask permission rules.
///
/// Returns `Allow` when no rules match (preserves existing behavior),
/// `Deny` when a deny rule matches, or `Ask` when an ask rule matches.
/// Deny takes priority over Ask if both match the same command.
pub fn check_command(cmd: &str) -> PermissionVerdict {
    let (deny_rules, ask_rules) = load_deny_ask_rules();
    check_command_with_rules(cmd, &deny_rules, &ask_rules)
}

/// Internal implementation allowing tests to inject rules without file I/O.
pub(crate) fn check_command_with_rules(
    cmd: &str,
    deny_rules: &[String],
    ask_rules: &[String],
) -> PermissionVerdict {
    let segments = split_compound_command(cmd);
    let mut any_ask = false;

    for segment in &segments {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }

        for pattern in deny_rules {
            if command_matches_pattern(segment, pattern) {
                return PermissionVerdict::Deny;
            }
        }

        if !any_ask {
            for pattern in ask_rules {
                if command_matches_pattern(segment, pattern) {
                    any_ask = true;
                    break;
                }
            }
        }
    }

    if any_ask {
        PermissionVerdict::Ask
    } else {
        PermissionVerdict::Allow
    }
}

/// Load deny and ask Bash rules from all Claude Code settings files.
///
/// Files read (in order, later files do not override earlier ones — all are merged):
/// 1. `$PROJECT_ROOT/.claude/settings.json`
/// 2. `$PROJECT_ROOT/.claude/settings.local.json`
/// 3. `~/.claude/settings.json`
/// 4. `~/.claude/settings.local.json`
///
/// Missing files and malformed JSON are silently skipped.
fn load_deny_ask_rules() -> (Vec<String>, Vec<String>) {
    let mut deny_rules = Vec::new();
    let mut ask_rules = Vec::new();

    for path in get_settings_paths() {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<Value>(&content) else {
            continue;
        };
        let Some(permissions) = json.get("permissions") else {
            continue;
        };

        append_bash_rules(permissions.get("deny"), &mut deny_rules);
        append_bash_rules(permissions.get("ask"), &mut ask_rules);
    }

    (deny_rules, ask_rules)
}

/// Extract Bash-scoped patterns from a JSON array and append them to `target`.
///
/// Only rules with a `Bash(...)` prefix are kept. Non-Bash rules (e.g. `Read(...)`) are ignored.
fn append_bash_rules(rules_value: Option<&Value>, target: &mut Vec<String>) {
    let Some(arr) = rules_value.and_then(|v| v.as_array()) else {
        return;
    };
    for rule in arr {
        if let Some(s) = rule.as_str() {
            if s.starts_with("Bash(") {
                target.push(extract_bash_pattern(s).to_string());
            }
        }
    }
}

/// Return the ordered list of Claude Code settings file paths to check.
fn get_settings_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(root) = find_project_root() {
        paths.push(root.join(".claude").join("settings.json"));
        paths.push(root.join(".claude").join("settings.local.json"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".claude").join("settings.json"));
        paths.push(home.join(".claude").join("settings.local.json"));
    }

    paths
}

/// Locate the project root by walking up from CWD looking for `.claude/`.
///
/// Falls back to `git rev-parse --show-toplevel` if not found via directory walk.
fn find_project_root() -> Option<PathBuf> {
    // Fast path: walk up CWD looking for .claude/ — no subprocess needed.
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".claude").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }

    // Fallback: git (spawns a subprocess, slower but handles monorepo layouts).
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;

    if output.status.success() {
        let path = String::from_utf8(output.stdout).ok()?;
        return Some(PathBuf::from(path.trim()));
    }

    None
}

/// Extract the pattern string from inside `Bash(pattern)`.
///
/// Returns the original string unchanged if it does not match the expected format.
pub(crate) fn extract_bash_pattern(rule: &str) -> &str {
    if let Some(inner) = rule.strip_prefix("Bash(") {
        if let Some(pattern) = inner.strip_suffix(')') {
            return pattern;
        }
    }
    rule
}

/// Check if `cmd` matches a Claude Code permission pattern.
///
/// Pattern forms:
/// - `*` → matches everything
/// - `prefix:*` or `prefix *` (trailing `*`, no other wildcards) → prefix match with word boundary
/// - `* suffix`, `pre * suf` → glob matching where `*` matches any sequence of characters
/// - `pattern` → exact match or prefix match (cmd must equal pattern or start with `{pattern} `)
pub(crate) fn command_matches_pattern(cmd: &str, pattern: &str) -> bool {
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
            // Middle segment: find next occurrence
            match cmd[search_from..].find(*part) {
                Some(pos) => search_from += pos + part.len(),
                None => return false,
            }
        }
    }

    true
}

/// Split a compound shell command into individual segments.
///
/// Splits on `&&`, `||`, `|`, and `;`. Not a full shell parser — handles common cases.
fn split_compound_command(cmd: &str) -> Vec<&str> {
    cmd.split("&&")
        .flat_map(|s| s.split("||"))
        .flat_map(|s| s.split(['|', ';']))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
            check_command_with_rules("git push --force", &deny, &ask),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_non_bash_rules_ignored() {
        // Non-Bash rules (e.g. Read, Write) must not match Bash commands.
        // In load_deny_ask_rules, only Bash( rules are kept — we verify that
        // extract_bash_pattern returns the original string for non-Bash rules.
        assert_eq!(extract_bash_pattern("Read(**/.env*)"), "Read(**/.env*)");

        // With empty rule sets (what you get after filtering out non-Bash rules),
        // verdict is always Allow.
        assert_eq!(
            check_command_with_rules("cat .env", &[], &[]),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn test_empty_permissions() {
        assert_eq!(
            check_command_with_rules("git push --force", &[], &[]),
            PermissionVerdict::Allow
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
    fn test_compound_command_deny() {
        let deny = vec!["git push --force".to_string()];
        assert_eq!(
            check_command_with_rules("git status && git push --force", &deny, &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_compound_command_ask() {
        let ask = vec!["git push".to_string()];
        assert_eq!(
            check_command_with_rules("git status && git push origin main", &[], &ask),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn test_compound_command_deny_overrides_ask() {
        let deny = vec!["git push --force".to_string()];
        let ask = vec!["git status".to_string()];
        // deny in compound cmd takes priority even if ask also matches a segment
        assert_eq!(
            check_command_with_rules("git status && git push --force", &deny, &ask),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_ask_verdict() {
        let ask = vec!["git push".to_string()];
        assert_eq!(
            check_command_with_rules("git push origin main", &[], &ask),
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
            check_command_with_rules("git push --force", &deny, &[]),
            PermissionVerdict::Deny
        );
        assert_eq!(
            check_command_with_rules("git push", &deny, &[]),
            PermissionVerdict::Allow
        );
    }

    // Integration: deny *:* blocks everything
    #[test]
    fn test_deny_star_colon_star() {
        let deny = vec!["*:*".to_string()];
        assert_eq!(
            check_command_with_rules("rm -rf /", &deny, &[]),
            PermissionVerdict::Deny
        );
    }
}
