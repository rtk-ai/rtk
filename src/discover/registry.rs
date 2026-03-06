use lazy_static::lazy_static;
use regex::{Regex, RegexSet};

use super::rules::{IGNORED_EXACT, IGNORED_PREFIXES, PATTERNS, RULES};

/// Result of classifying a command.
#[derive(Debug, PartialEq)]
pub enum Classification {
    Supported {
        rtk_equivalent: &'static str,
        category: &'static str,
        estimated_savings_pct: f64,
        status: super::report::RtkStatus,
    },
    Unsupported {
        base_command: String,
    },
    Ignored,
}

/// Average token counts per category for estimation when no output_len available.
pub fn category_avg_tokens(category: &str, subcmd: &str) -> usize {
    match category {
        "Git" => match subcmd {
            "log" | "diff" | "show" => 200,
            _ => 40,
        },
        "Cargo" => match subcmd {
            "test" => 500,
            _ => 150,
        },
        "Tests" => 800,
        "Files" => 100,
        "Build" => 300,
        "Infra" => 120,
        "Network" => 150,
        "GitHub" => 200,
        "PackageManager" => 150,
        _ => 150,
    }
}

lazy_static! {
    static ref REGEX_SET: RegexSet = RegexSet::new(PATTERNS).expect("invalid regex patterns");
    static ref COMPILED: Vec<Regex> = PATTERNS
        .iter()
        .map(|p| Regex::new(p).expect("invalid regex"))
        .collect();
    static ref ENV_PREFIX: Regex =
        Regex::new(r"^(?:sudo\s+|env\s+|[A-Z_][A-Z0-9_]*=[^\s]*\s+)+").unwrap();
}

/// Classify a single (already-split) command.
pub fn classify_command(cmd: &str) -> Classification {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return Classification::Ignored;
    }

    // Check ignored
    for exact in IGNORED_EXACT {
        if trimmed == *exact {
            return Classification::Ignored;
        }
    }
    for prefix in IGNORED_PREFIXES {
        if trimmed.starts_with(prefix) {
            return Classification::Ignored;
        }
    }

    // Strip env prefixes (sudo, env VAR=val, VAR=val)
    let stripped = ENV_PREFIX.replace(trimmed, "");
    let cmd_clean = stripped.trim();
    if cmd_clean.is_empty() {
        return Classification::Ignored;
    }

    // Fast check with RegexSet — take the last (most specific) match
    let matches: Vec<usize> = REGEX_SET.matches(cmd_clean).into_iter().collect();
    if let Some(&idx) = matches.last() {
        let rule = &RULES[idx];

        // Extract subcommand for savings override and status detection
        let (savings, status) = if let Some(caps) = COMPILED[idx].captures(cmd_clean) {
            if let Some(sub) = caps.get(1) {
                let subcmd = sub.as_str();
                // Check if this subcommand has a special status
                let status = rule
                    .subcmd_status
                    .iter()
                    .find(|(s, _)| *s == subcmd)
                    .map(|(_, st)| *st)
                    .unwrap_or(super::report::RtkStatus::Existing);

                // Check if this subcommand has custom savings
                let savings = rule
                    .subcmd_savings
                    .iter()
                    .find(|(s, _)| *s == subcmd)
                    .map(|(_, pct)| *pct)
                    .unwrap_or(rule.savings_pct);

                (savings, status)
            } else {
                (rule.savings_pct, super::report::RtkStatus::Existing)
            }
        } else {
            (rule.savings_pct, super::report::RtkStatus::Existing)
        };

        Classification::Supported {
            rtk_equivalent: rule.rtk_cmd,
            category: rule.category,
            estimated_savings_pct: savings,
            status,
        }
    } else {
        // Extract base command for unsupported
        let base = extract_base_command(cmd_clean);
        if base.is_empty() {
            Classification::Ignored
        } else {
            Classification::Unsupported {
                base_command: base.to_string(),
            }
        }
    }
}

/// Extract the base command (first word, or first two if it looks like a subcommand pattern).
fn extract_base_command(cmd: &str) -> &str {
    let parts: Vec<&str> = cmd.splitn(3, char::is_whitespace).collect();
    match parts.len() {
        0 => "",
        1 => parts[0],
        _ => {
            let second = parts[1];
            // If the second token looks like a subcommand (no leading -)
            if !second.starts_with('-') && !second.contains('/') && !second.contains('.') {
                // Return "cmd subcmd"
                let end = cmd
                    .find(char::is_whitespace)
                    .and_then(|i| {
                        let rest = &cmd[i..];
                        let trimmed = rest.trim_start();
                        trimmed
                            .find(char::is_whitespace)
                            .map(|j| i + (rest.len() - trimmed.len()) + j)
                    })
                    .unwrap_or(cmd.len());
                &cmd[..end]
            } else {
                parts[0]
            }
        }
    }
}

/// Split a command chain on `&&`, `||`, `;` outside quotes.
/// For pipes `|`, only keep the first command.
/// Lines with `<<` (heredoc) or `$((` are returned whole.
pub fn split_command_chain(cmd: &str) -> Vec<&str> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    // Heredoc or arithmetic expansion: treat as single command
    if trimmed.contains("<<") || trimmed.contains("$((") {
        return vec![trimmed];
    }

    let mut results = Vec::new();
    let mut start = 0;
    let bytes = trimmed.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut pipe_seen = false;

    while i < len {
        let b = bytes[i];
        match b {
            b'\'' if !in_double => {
                in_single = !in_single;
                i += 1;
            }
            b'"' if !in_single => {
                in_double = !in_double;
                i += 1;
            }
            b'|' if !in_single && !in_double => {
                if i + 1 < len && bytes[i + 1] == b'|' {
                    // ||
                    let segment = trimmed[start..i].trim();
                    if !segment.is_empty() {
                        results.push(segment);
                    }
                    i += 2;
                    start = i;
                } else {
                    // pipe: keep only first command
                    let segment = trimmed[start..i].trim();
                    if !segment.is_empty() {
                        results.push(segment);
                    }
                    pipe_seen = true;
                    break;
                }
            }
            b'&' if !in_single && !in_double && i + 1 < len && bytes[i + 1] == b'&' => {
                let segment = trimmed[start..i].trim();
                if !segment.is_empty() {
                    results.push(segment);
                }
                i += 2;
                start = i;
            }
            b';' if !in_single && !in_double => {
                let segment = trimmed[start..i].trim();
                if !segment.is_empty() {
                    results.push(segment);
                }
                i += 1;
                start = i;
            }
            _ => {
                i += 1;
            }
        }
    }

    if !pipe_seen && start < len {
        let segment = trimmed[start..].trim();
        if !segment.is_empty() {
            results.push(segment);
        }
    }

    results
}

/// Rewrite a raw command to its RTK equivalent.
///
/// Returns `Some(rewritten)` if the command has an RTK equivalent or is already RTK.
/// Returns `None` if the command is unsupported or ignored (hook should pass through).
///
/// Handles compound commands (`&&`, `||`, `;`) by rewriting each segment independently.
/// For pipes (`|`), only rewrites the first command (the filter stays raw).
pub fn rewrite_command(cmd: &str, excluded: &[String]) -> Option<String> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Heredoc or arithmetic expansion — unsafe to split/rewrite
    if trimmed.contains("<<") || trimmed.contains("$((") {
        return None;
    }

    // Simple (non-compound) already-RTK command — return as-is.
    // For compound commands that start with "rtk" (e.g. "rtk git add . && cargo test"),
    // fall through to rewrite_compound so the remaining segments get rewritten.
    let has_compound = trimmed.contains("&&")
        || trimmed.contains("||")
        || trimmed.contains(';')
        || trimmed.contains('|')
        || trimmed.contains(" & ");
    if !has_compound && (trimmed.starts_with("rtk ") || trimmed == "rtk") {
        return Some(trimmed.to_string());
    }

    rewrite_compound(trimmed, excluded)
}

/// Rewrite a compound command (with `&&`, `||`, `;`, `|`) by rewriting each segment.
fn rewrite_compound(cmd: &str, excluded: &[String]) -> Option<String> {
    let bytes = cmd.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len + 32);
    let mut any_changed = false;
    let mut seg_start = 0;
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;

    while i < len {
        let b = bytes[i];
        match b {
            b'\'' if !in_double => {
                in_single = !in_single;
                i += 1;
            }
            b'"' if !in_single => {
                in_double = !in_double;
                i += 1;
            }
            b'|' if !in_single && !in_double => {
                if i + 1 < len && bytes[i + 1] == b'|' {
                    // `||` operator — rewrite left, continue
                    let seg = cmd[seg_start..i].trim();
                    let rewritten =
                        rewrite_segment(seg, excluded).unwrap_or_else(|| seg.to_string());
                    if rewritten != seg {
                        any_changed = true;
                    }
                    result.push_str(&rewritten);
                    result.push_str(" || ");
                    i += 2;
                    while i < len && bytes[i] == b' ' {
                        i += 1;
                    }
                    seg_start = i;
                } else {
                    // `|` pipe — rewrite first segment only, pass through the rest unchanged
                    let seg = cmd[seg_start..i].trim();
                    let rewritten =
                        rewrite_segment(seg, excluded).unwrap_or_else(|| seg.to_string());
                    if rewritten != seg {
                        any_changed = true;
                    }
                    result.push_str(&rewritten);
                    // Preserve the space before the pipe that was lost by trim()
                    result.push(' ');
                    result.push_str(cmd[i..].trim_start());
                    return if any_changed { Some(result) } else { None };
                }
            }
            b'&' if !in_single && !in_double && i + 1 < len && bytes[i + 1] == b'&' => {
                // `&&` operator — rewrite left, continue
                let seg = cmd[seg_start..i].trim();
                let rewritten = rewrite_segment(seg, excluded).unwrap_or_else(|| seg.to_string());
                if rewritten != seg {
                    any_changed = true;
                }
                result.push_str(&rewritten);
                result.push_str(" && ");
                i += 2;
                while i < len && bytes[i] == b' ' {
                    i += 1;
                }
                seg_start = i;
            }
            b'&' if !in_single && !in_double => {
                // #346: redirect detection — 2>&1 / >&2 (> before &) or &>file / &>>file (> after &)
                let is_redirect =
                    (i > 0 && bytes[i - 1] == b'>') || (i + 1 < len && bytes[i + 1] == b'>');
                if is_redirect {
                    i += 1;
                } else {
                    // single `&` background execution operator
                    let seg = cmd[seg_start..i].trim();
                    let rewritten =
                        rewrite_segment(seg, excluded).unwrap_or_else(|| seg.to_string());
                    if rewritten != seg {
                        any_changed = true;
                    }
                    result.push_str(&rewritten);
                    result.push_str(" & ");
                    i += 1;
                    while i < len && bytes[i] == b' ' {
                        i += 1;
                    }
                    seg_start = i;
                }
            }
            b';' if !in_single && !in_double => {
                // `;` separator
                let seg = cmd[seg_start..i].trim();
                let rewritten = rewrite_segment(seg, excluded).unwrap_or_else(|| seg.to_string());
                if rewritten != seg {
                    any_changed = true;
                }
                result.push_str(&rewritten);
                result.push(';');
                i += 1;
                while i < len && bytes[i] == b' ' {
                    i += 1;
                }
                if i < len {
                    result.push(' ');
                }
                seg_start = i;
            }
            _ => {
                i += 1;
            }
        }
    }

    // Last (or only) segment
    let seg = cmd[seg_start..len].trim();
    let rewritten = rewrite_segment(seg, excluded).unwrap_or_else(|| seg.to_string());
    if rewritten != seg {
        any_changed = true;
    }
    result.push_str(&rewritten);

    if any_changed {
        Some(result)
    } else {
        None
    }
}

/// Rewrite `head -N file` → `rtk read file --max-lines N`.
/// Returns `None` if the command doesn't match this pattern (fall through to generic logic).
fn rewrite_head_numeric(cmd: &str) -> Option<String> {
    // Match: head -<digits> <file>  (with optional env prefix)
    lazy_static! {
        static ref HEAD_N: Regex = Regex::new(r"^head\s+-(\d+)\s+(.+)$").expect("valid regex");
        static ref HEAD_LINES: Regex =
            Regex::new(r"^head\s+--lines=(\d+)\s+(.+)$").expect("valid regex");
    }
    if let Some(caps) = HEAD_N.captures(cmd) {
        let n = caps.get(1)?.as_str();
        let file = caps.get(2)?.as_str();
        return Some(format!("rtk read {} --max-lines {}", file, n));
    }
    if let Some(caps) = HEAD_LINES.captures(cmd) {
        let n = caps.get(1)?.as_str();
        let file = caps.get(2)?.as_str();
        return Some(format!("rtk read {} --max-lines {}", file, n));
    }
    // head with any other flag (e.g. -c, -q): skip rewriting to avoid clap errors
    if cmd.starts_with("head -") {
        return None;
    }
    None
}

/// Rewrite a single (non-compound) command segment.
/// Returns `Some(rewritten)` if matched (including already-RTK pass-through).
/// Returns `None` if no match (caller uses original segment).
fn rewrite_segment(seg: &str, excluded: &[String]) -> Option<String> {
    let trimmed = seg.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Already RTK — pass through unchanged
    if trimmed.starts_with("rtk ") || trimmed == "rtk" {
        return Some(trimmed.to_string());
    }

    // Special case: `head -N file` / `head --lines=N file` → `rtk read file --max-lines N`
    // Must intercept before generic prefix replacement, which would produce `rtk read -20 file`.
    // Only intercept when head has a flag (-N, --lines=N, -c, etc.); plain `head file` falls
    // through to the generic rewrite below and produces `rtk read file` as expected.
    if trimmed.starts_with("head -") {
        return rewrite_head_numeric(trimmed);
    }

    // Use classify_command for correct ignore/prefix handling
    let rtk_equivalent = match classify_command(trimmed) {
        Classification::Supported { rtk_equivalent, .. } => {
            // Check if the base command is excluded from rewriting (#243)
            let base = trimmed.split_whitespace().next().unwrap_or("");
            if excluded.iter().any(|e| e == base) {
                return None;
            }
            rtk_equivalent
        }
        _ => return None,
    };

    // Find the matching rule (rtk_cmd values are unique across all rules)
    let rule = RULES.iter().find(|r| r.rtk_cmd == rtk_equivalent)?;

    // Extract env prefix (sudo, env VAR=val, etc.)
    let stripped_cow = ENV_PREFIX.replace(trimmed, "");
    let env_prefix_len = trimmed.len() - stripped_cow.len();
    let env_prefix = &trimmed[..env_prefix_len];
    let cmd_clean = stripped_cow.trim();

    // #345: RTK_DISABLED=1 in env prefix → skip rewrite entirely
    if env_prefix.contains("RTK_DISABLED=") {
        return None;
    }

    // #196: gh with --json/--jq/--template produces structured output that
    // rtk gh would corrupt — skip rewrite so the caller gets raw JSON.
    if rule.rtk_cmd == "rtk gh" {
        let args_lower = cmd_clean.to_lowercase();
        if args_lower.contains("--json")
            || args_lower.contains("--jq")
            || args_lower.contains("--template")
        {
            return None;
        }
    }

    // Try each rewrite prefix (longest first) with word-boundary check
    for &prefix in rule.rewrite_prefixes {
        if let Some(rest) = strip_word_prefix(cmd_clean, prefix) {
            let rewritten = if rest.is_empty() {
                format!("{}{}", env_prefix, rule.rtk_cmd)
            } else {
                format!("{}{} {}", env_prefix, rule.rtk_cmd, rest)
            };
            return Some(rewritten);
        }
    }

    None
}

/// Strip a command prefix with word-boundary check.
/// Returns the remainder of the command after the prefix, or `None` if no match.
fn strip_word_prefix<'a>(cmd: &'a str, prefix: &str) -> Option<&'a str> {
    if cmd == prefix {
        Some("")
    } else if cmd.len() > prefix.len()
        && cmd.starts_with(prefix)
        && cmd.as_bytes()[prefix.len()] == b' '
    {
        Some(cmd[prefix.len() + 1..].trim_start())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::report::RtkStatus;
    use super::*;

    #[test]
    fn test_classify_git_status() {
        assert_eq!(
            classify_command("git status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_git_diff_cached() {
        assert_eq!(
            classify_command("git diff --cached"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cargo_test_filter() {
        assert_eq!(
            classify_command("cargo test filter::"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 90.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_npx_tsc() {
        assert_eq!(
            classify_command("npx tsc --noEmit"),
            Classification::Supported {
                rtk_equivalent: "rtk tsc",
                category: "Build",
                estimated_savings_pct: 83.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cat_file() {
        assert_eq!(
            classify_command("cat src/main.rs"),
            Classification::Supported {
                rtk_equivalent: "rtk read",
                category: "Files",
                estimated_savings_pct: 60.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cd_ignored() {
        assert_eq!(classify_command("cd /tmp"), Classification::Ignored);
    }

    #[test]
    fn test_classify_rtk_already() {
        assert_eq!(classify_command("rtk git status"), Classification::Ignored);
    }

    #[test]
    fn test_classify_echo_ignored() {
        assert_eq!(
            classify_command("echo hello world"),
            Classification::Ignored
        );
    }

    #[test]
    fn test_classify_terraform_unsupported() {
        match classify_command("terraform plan -var-file=prod.tfvars") {
            Classification::Unsupported { base_command } => {
                assert_eq!(base_command, "terraform plan");
            }
            other => panic!("expected Unsupported, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_env_prefix_stripped() {
        assert_eq!(
            classify_command("GIT_SSH_COMMAND=ssh git push"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_sudo_stripped() {
        assert_eq!(
            classify_command("sudo docker ps"),
            Classification::Supported {
                rtk_equivalent: "rtk docker",
                category: "Infra",
                estimated_savings_pct: 85.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cargo_check() {
        assert_eq!(
            classify_command("cargo check"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cargo_check_all_targets() {
        assert_eq!(
            classify_command("cargo check --all-targets"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cargo_fmt_passthrough() {
        assert_eq!(
            classify_command("cargo fmt"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Passthrough,
            }
        );
    }

    #[test]
    fn test_classify_cargo_clippy_savings() {
        assert_eq!(
            classify_command("cargo clippy --all-targets"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_patterns_rules_length_match() {
        assert_eq!(
            PATTERNS.len(),
            RULES.len(),
            "PATTERNS and RULES must be aligned"
        );
    }

    #[test]
    fn test_registry_covers_all_cargo_subcommands() {
        // Verify that every CargoCommand variant (Build, Test, Clippy, Check, Fmt)
        // except Other has a matching pattern in the registry
        for subcmd in ["build", "test", "clippy", "check", "fmt"] {
            let cmd = format!("cargo {subcmd}");
            match classify_command(&cmd) {
                Classification::Supported { .. } => {}
                other => panic!("cargo {subcmd} should be Supported, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_registry_covers_all_git_subcommands() {
        // Verify that every GitCommand subcommand has a matching pattern
        for subcmd in [
            "status", "log", "diff", "show", "add", "commit", "push", "pull", "branch", "fetch",
            "stash", "worktree",
        ] {
            let cmd = format!("git {subcmd}");
            match classify_command(&cmd) {
                Classification::Supported { .. } => {}
                other => panic!("git {subcmd} should be Supported, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_classify_find_not_blocked_by_fi() {
        // Regression: "fi" in IGNORED_PREFIXES used to shadow "find" commands
        // because "find".starts_with("fi") is true. "fi" should only match exactly.
        assert_eq!(
            classify_command("find . -name foo"),
            Classification::Supported {
                rtk_equivalent: "rtk find",
                category: "Files",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_fi_still_ignored_exact() {
        // Bare "fi" (shell keyword) should still be ignored
        assert_eq!(classify_command("fi"), Classification::Ignored);
    }

    #[test]
    fn test_done_still_ignored_exact() {
        // Bare "done" (shell keyword) should still be ignored
        assert_eq!(classify_command("done"), Classification::Ignored);
    }

    #[test]
    fn test_split_chain_and() {
        assert_eq!(split_command_chain("a && b"), vec!["a", "b"]);
    }

    #[test]
    fn test_split_chain_semicolon() {
        assert_eq!(split_command_chain("a ; b"), vec!["a", "b"]);
    }

    #[test]
    fn test_split_pipe_first_only() {
        assert_eq!(split_command_chain("a | b"), vec!["a"]);
    }

    #[test]
    fn test_split_single() {
        assert_eq!(split_command_chain("git status"), vec!["git status"]);
    }

    #[test]
    fn test_split_quoted_and() {
        assert_eq!(
            split_command_chain(r#"echo "a && b""#),
            vec![r#"echo "a && b""#]
        );
    }

    #[test]
    fn test_split_heredoc_no_split() {
        let cmd = "cat <<'EOF'\nhello && world\nEOF";
        assert_eq!(split_command_chain(cmd), vec![cmd]);
    }

    #[test]
    fn test_classify_mypy() {
        assert_eq!(
            classify_command("mypy src/"),
            Classification::Supported {
                rtk_equivalent: "rtk mypy",
                category: "Build",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_python_m_mypy() {
        assert_eq!(
            classify_command("python3 -m mypy --strict"),
            Classification::Supported {
                rtk_equivalent: "rtk mypy",
                category: "Build",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    // --- rewrite_command tests ---

    #[test]
    fn test_rewrite_git_status() {
        assert_eq!(
            rewrite_command("git status", &[]),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_git_log() {
        assert_eq!(
            rewrite_command("git log -10", &[]),
            Some("rtk git log -10".into())
        );
    }

    #[test]
    fn test_rewrite_cargo_test() {
        assert_eq!(
            rewrite_command("cargo test", &[]),
            Some("rtk cargo test".into())
        );
    }

    #[test]
    fn test_rewrite_compound_and() {
        assert_eq!(
            rewrite_command("git add . && cargo test", &[]),
            Some("rtk git add . && rtk cargo test".into())
        );
    }

    #[test]
    fn test_rewrite_compound_three_segments() {
        assert_eq!(
            rewrite_command(
                "cargo fmt --all && cargo clippy --all-targets && cargo test",
                &[]
            ),
            Some("rtk cargo fmt --all && rtk cargo clippy --all-targets && rtk cargo test".into())
        );
    }

    #[test]
    fn test_rewrite_already_rtk() {
        assert_eq!(
            rewrite_command("rtk git status", &[]),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_background_single_amp() {
        assert_eq!(
            rewrite_command("cargo test & git status", &[]),
            Some("rtk cargo test & rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_background_unsupported_right() {
        assert_eq!(
            rewrite_command("cargo test & terraform plan", &[]),
            Some("rtk cargo test & terraform plan".into())
        );
    }

    #[test]
    fn test_rewrite_background_does_not_affect_double_amp() {
        // `&&` must still work after adding `&` support
        assert_eq!(
            rewrite_command("cargo test && git status", &[]),
            Some("rtk cargo test && rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_unsupported_returns_none() {
        assert_eq!(rewrite_command("terraform plan", &[]), None);
    }

    #[test]
    fn test_rewrite_ignored_cd() {
        assert_eq!(rewrite_command("cd /tmp", &[]), None);
    }

    #[test]
    fn test_rewrite_with_env_prefix() {
        assert_eq!(
            rewrite_command("GIT_SSH_COMMAND=ssh git push", &[]),
            Some("GIT_SSH_COMMAND=ssh rtk git push".into())
        );
    }

    #[test]
    fn test_rewrite_npx_tsc() {
        assert_eq!(
            rewrite_command("npx tsc --noEmit", &[]),
            Some("rtk tsc --noEmit".into())
        );
    }

    #[test]
    fn test_rewrite_pnpm_tsc() {
        assert_eq!(
            rewrite_command("pnpm tsc --noEmit", &[]),
            Some("rtk tsc --noEmit".into())
        );
    }

    #[test]
    fn test_rewrite_cat_file() {
        assert_eq!(
            rewrite_command("cat src/main.rs", &[]),
            Some("rtk read src/main.rs".into())
        );
    }

    #[test]
    fn test_rewrite_rg_pattern() {
        assert_eq!(
            rewrite_command("rg \"fn main\"", &[]),
            Some("rtk grep \"fn main\"".into())
        );
    }

    #[test]
    fn test_rewrite_npx_playwright() {
        assert_eq!(
            rewrite_command("npx playwright test", &[]),
            Some("rtk playwright test".into())
        );
    }

    #[test]
    fn test_rewrite_next_build() {
        assert_eq!(
            rewrite_command("next build --turbo", &[]),
            Some("rtk next --turbo".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_first_only() {
        // After a pipe, the filter command stays raw
        assert_eq!(
            rewrite_command("git log -10 | grep feat", &[]),
            Some("rtk git log -10 | grep feat".into())
        );
    }

    #[test]
    fn test_rewrite_heredoc_returns_none() {
        assert_eq!(rewrite_command("cat <<'EOF'\nfoo\nEOF", &[]), None);
    }

    #[test]
    fn test_rewrite_empty_returns_none() {
        assert_eq!(rewrite_command("", &[]), None);
        assert_eq!(rewrite_command("   ", &[]), None);
    }

    #[test]
    fn test_rewrite_mixed_compound_partial() {
        // First segment already RTK, second gets rewritten
        assert_eq!(
            rewrite_command("rtk git add . && cargo test", &[]),
            Some("rtk git add . && rtk cargo test".into())
        );
    }

    // --- #345: RTK_DISABLED ---

    #[test]
    fn test_rewrite_rtk_disabled_curl() {
        assert_eq!(
            rewrite_command("RTK_DISABLED=1 curl https://example.com", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_rtk_disabled_git_status() {
        assert_eq!(rewrite_command("RTK_DISABLED=1 git status", &[]), None);
    }

    #[test]
    fn test_rewrite_rtk_disabled_multi_env() {
        assert_eq!(
            rewrite_command("FOO=1 RTK_DISABLED=1 git status", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_non_rtk_disabled_env_still_rewrites() {
        assert_eq!(
            rewrite_command("SOME_VAR=1 git status", &[]),
            Some("SOME_VAR=1 rtk git status".into())
        );
    }

    // --- #346: 2>&1 and &> redirect detection ---

    #[test]
    fn test_rewrite_redirect_2_gt_amp_1_with_pipe() {
        assert_eq!(
            rewrite_command("cargo test 2>&1 | head", &[]),
            Some("rtk cargo test 2>&1 | head".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_2_gt_amp_1_trailing() {
        assert_eq!(
            rewrite_command("cargo test 2>&1", &[]),
            Some("rtk cargo test 2>&1".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_plain_2_devnull() {
        // 2>/dev/null has no `&`, never broken — non-regression
        assert_eq!(
            rewrite_command("git status 2>/dev/null", &[]),
            Some("rtk git status 2>/dev/null".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_2_gt_amp_1_with_and() {
        assert_eq!(
            rewrite_command("cargo test 2>&1 && echo done", &[]),
            Some("rtk cargo test 2>&1 && echo done".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_amp_gt_devnull() {
        assert_eq!(
            rewrite_command("cargo test &>/dev/null", &[]),
            Some("rtk cargo test &>/dev/null".into())
        );
    }

    #[test]
    fn test_rewrite_background_amp_non_regression() {
        // background `&` must still work after redirect fix
        assert_eq!(
            rewrite_command("cargo test & git status", &[]),
            Some("rtk cargo test & rtk git status".into())
        );
    }

    // --- P0.2: head -N rewrite ---

    #[test]
    fn test_rewrite_head_numeric_flag() {
        // head -20 file → rtk read file --max-lines 20 (not rtk read -20 file)
        assert_eq!(
            rewrite_command("head -20 src/main.rs", &[]),
            Some("rtk read src/main.rs --max-lines 20".into())
        );
    }

    #[test]
    fn test_rewrite_head_lines_long_flag() {
        assert_eq!(
            rewrite_command("head --lines=50 src/lib.rs", &[]),
            Some("rtk read src/lib.rs --max-lines 50".into())
        );
    }

    #[test]
    fn test_rewrite_head_no_flag_still_rewrites() {
        // plain `head file` → `rtk read file` (no numeric flag)
        assert_eq!(
            rewrite_command("head src/main.rs", &[]),
            Some("rtk read src/main.rs".into())
        );
    }

    #[test]
    fn test_rewrite_head_other_flag_skipped() {
        // head -c 100 file: unsupported flag, skip rewriting
        assert_eq!(rewrite_command("head -c 100 src/main.rs", &[]), None);
    }

    // --- New registry entries ---

    #[test]
    fn test_classify_gh_release() {
        assert!(matches!(
            classify_command("gh release list"),
            Classification::Supported {
                rtk_equivalent: "rtk gh",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_cargo_install() {
        assert!(matches!(
            classify_command("cargo install rtk"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_docker_run() {
        assert!(matches!(
            classify_command("docker run --rm ubuntu bash"),
            Classification::Supported {
                rtk_equivalent: "rtk docker",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_docker_exec() {
        assert!(matches!(
            classify_command("docker exec -it mycontainer bash"),
            Classification::Supported {
                rtk_equivalent: "rtk docker",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_docker_build() {
        assert!(matches!(
            classify_command("docker build -t myimage ."),
            Classification::Supported {
                rtk_equivalent: "rtk docker",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_kubectl_describe() {
        assert!(matches!(
            classify_command("kubectl describe pod mypod"),
            Classification::Supported {
                rtk_equivalent: "rtk kubectl",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_kubectl_apply() {
        assert!(matches!(
            classify_command("kubectl apply -f deploy.yaml"),
            Classification::Supported {
                rtk_equivalent: "rtk kubectl",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_tree() {
        assert!(matches!(
            classify_command("tree src/"),
            Classification::Supported {
                rtk_equivalent: "rtk tree",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_diff() {
        assert!(matches!(
            classify_command("diff file1.txt file2.txt"),
            Classification::Supported {
                rtk_equivalent: "rtk diff",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_tree() {
        assert_eq!(
            rewrite_command("tree src/", &[]),
            Some("rtk tree src/".into())
        );
    }

    #[test]
    fn test_rewrite_diff() {
        assert_eq!(
            rewrite_command("diff file1.txt file2.txt", &[]),
            Some("rtk diff file1.txt file2.txt".into())
        );
    }

    #[test]
    fn test_rewrite_gh_release() {
        assert_eq!(
            rewrite_command("gh release list", &[]),
            Some("rtk gh release list".into())
        );
    }

    #[test]
    fn test_rewrite_cargo_install() {
        assert_eq!(
            rewrite_command("cargo install rtk", &[]),
            Some("rtk cargo install rtk".into())
        );
    }

    #[test]
    fn test_rewrite_kubectl_describe() {
        assert_eq!(
            rewrite_command("kubectl describe pod mypod", &[]),
            Some("rtk kubectl describe pod mypod".into())
        );
    }

    #[test]
    fn test_rewrite_docker_run() {
        assert_eq!(
            rewrite_command("docker run --rm ubuntu bash", &[]),
            Some("rtk docker run --rm ubuntu bash".into())
        );
    }

    // --- #336: docker compose supported subcommands rewritten, unsupported skipped ---

    #[test]
    fn test_rewrite_docker_compose_ps() {
        assert_eq!(
            rewrite_command("docker compose ps", &[]),
            Some("rtk docker compose ps".into())
        );
    }

    #[test]
    fn test_rewrite_docker_compose_logs() {
        assert_eq!(
            rewrite_command("docker compose logs web", &[]),
            Some("rtk docker compose logs web".into())
        );
    }

    #[test]
    fn test_rewrite_docker_compose_build() {
        assert_eq!(
            rewrite_command("docker compose build", &[]),
            Some("rtk docker compose build".into())
        );
    }

    #[test]
    fn test_rewrite_docker_compose_up_skipped() {
        assert_eq!(rewrite_command("docker compose up -d", &[]), None);
    }

    #[test]
    fn test_rewrite_docker_compose_down_skipped() {
        assert_eq!(rewrite_command("docker compose down", &[]), None);
    }

    #[test]
    fn test_rewrite_docker_compose_config_skipped() {
        assert_eq!(
            rewrite_command("docker compose -f foo.yaml config --services", &[]),
            None
        );
    }

    // --- AWS / psql (PR #216) ---

    #[test]
    fn test_classify_aws() {
        assert!(matches!(
            classify_command("aws s3 ls"),
            Classification::Supported {
                rtk_equivalent: "rtk aws",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_aws_ec2() {
        assert!(matches!(
            classify_command("aws ec2 describe-instances"),
            Classification::Supported {
                rtk_equivalent: "rtk aws",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_psql() {
        assert!(matches!(
            classify_command("psql -U postgres"),
            Classification::Supported {
                rtk_equivalent: "rtk psql",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_psql_url() {
        assert!(matches!(
            classify_command("psql postgres://localhost/mydb"),
            Classification::Supported {
                rtk_equivalent: "rtk psql",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_aws() {
        assert_eq!(
            rewrite_command("aws s3 ls", &[]),
            Some("rtk aws s3 ls".into())
        );
    }

    #[test]
    fn test_rewrite_aws_ec2() {
        assert_eq!(
            rewrite_command("aws ec2 describe-instances --region us-east-1", &[]),
            Some("rtk aws ec2 describe-instances --region us-east-1".into())
        );
    }

    #[test]
    fn test_rewrite_psql() {
        assert_eq!(
            rewrite_command("psql -U postgres -d mydb", &[]),
            Some("rtk psql -U postgres -d mydb".into())
        );
    }

    // --- Python tooling ---

    #[test]
    fn test_classify_ruff_check() {
        assert!(matches!(
            classify_command("ruff check ."),
            Classification::Supported {
                rtk_equivalent: "rtk ruff",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_ruff_format() {
        assert!(matches!(
            classify_command("ruff format src/"),
            Classification::Supported {
                rtk_equivalent: "rtk ruff",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_pytest() {
        assert!(matches!(
            classify_command("pytest tests/"),
            Classification::Supported {
                rtk_equivalent: "rtk pytest",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_python_m_pytest() {
        assert!(matches!(
            classify_command("python -m pytest tests/"),
            Classification::Supported {
                rtk_equivalent: "rtk pytest",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_pip_list() {
        assert!(matches!(
            classify_command("pip list"),
            Classification::Supported {
                rtk_equivalent: "rtk pip",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_uv_pip_list() {
        assert!(matches!(
            classify_command("uv pip list"),
            Classification::Supported {
                rtk_equivalent: "rtk pip",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_ruff_check() {
        assert_eq!(
            rewrite_command("ruff check .", &[]),
            Some("rtk ruff check .".into())
        );
    }

    #[test]
    fn test_rewrite_ruff_format() {
        assert_eq!(
            rewrite_command("ruff format src/", &[]),
            Some("rtk ruff format src/".into())
        );
    }

    #[test]
    fn test_rewrite_pytest() {
        assert_eq!(
            rewrite_command("pytest tests/", &[]),
            Some("rtk pytest tests/".into())
        );
    }

    #[test]
    fn test_rewrite_python_m_pytest() {
        assert_eq!(
            rewrite_command("python -m pytest -x tests/", &[]),
            Some("rtk pytest -x tests/".into())
        );
    }

    #[test]
    fn test_rewrite_pip_list() {
        assert_eq!(
            rewrite_command("pip list", &[]),
            Some("rtk pip list".into())
        );
    }

    #[test]
    fn test_rewrite_pip_outdated() {
        assert_eq!(
            rewrite_command("pip outdated", &[]),
            Some("rtk pip outdated".into())
        );
    }

    #[test]
    fn test_rewrite_uv_pip_list() {
        assert_eq!(
            rewrite_command("uv pip list", &[]),
            Some("rtk pip list".into())
        );
    }

    // --- Go tooling ---

    #[test]
    fn test_classify_go_test() {
        assert!(matches!(
            classify_command("go test ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk go",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_go_build() {
        assert!(matches!(
            classify_command("go build ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk go",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_go_vet() {
        assert!(matches!(
            classify_command("go vet ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk go",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint() {
        assert!(matches!(
            classify_command("golangci-lint run"),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_go_test() {
        assert_eq!(
            rewrite_command("go test ./...", &[]),
            Some("rtk go test ./...".into())
        );
    }

    #[test]
    fn test_rewrite_go_build() {
        assert_eq!(
            rewrite_command("go build ./...", &[]),
            Some("rtk go build ./...".into())
        );
    }

    #[test]
    fn test_rewrite_go_vet() {
        assert_eq!(
            rewrite_command("go vet ./...", &[]),
            Some("rtk go vet ./...".into())
        );
    }

    #[test]
    fn test_rewrite_golangci_lint() {
        assert_eq!(
            rewrite_command("golangci-lint run ./...", &[]),
            Some("rtk golangci-lint run ./...".into())
        );
    }

    // --- JS/TS tooling ---

    #[test]
    fn test_classify_vitest() {
        assert!(matches!(
            classify_command("vitest run"),
            Classification::Supported {
                rtk_equivalent: "rtk vitest",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_vitest() {
        assert_eq!(
            rewrite_command("vitest run", &[]),
            Some("rtk vitest run".into())
        );
    }

    #[test]
    fn test_rewrite_pnpm_vitest() {
        assert_eq!(
            rewrite_command("pnpm vitest run", &[]),
            Some("rtk vitest run".into())
        );
    }

    #[test]
    fn test_classify_prisma() {
        assert!(matches!(
            classify_command("npx prisma migrate dev"),
            Classification::Supported {
                rtk_equivalent: "rtk prisma",
                ..
            }
        ));
    }

    #[test]
    fn test_rewrite_prisma() {
        assert_eq!(
            rewrite_command("npx prisma migrate dev", &[]),
            Some("rtk prisma migrate dev".into())
        );
    }

    #[test]
    fn test_rewrite_prettier() {
        assert_eq!(
            rewrite_command("npx prettier --check src/", &[]),
            Some("rtk prettier --check src/".into())
        );
    }

    #[test]
    fn test_rewrite_pnpm_list() {
        assert_eq!(
            rewrite_command("pnpm list", &[]),
            Some("rtk pnpm list".into())
        );
    }

    // --- Compound operator edge cases ---

    #[test]
    fn test_rewrite_compound_or() {
        // `||` fallback: left rewritten, right rewritten
        assert_eq!(
            rewrite_command("cargo test || cargo build", &[]),
            Some("rtk cargo test || rtk cargo build".into())
        );
    }

    #[test]
    fn test_rewrite_compound_semicolon() {
        assert_eq!(
            rewrite_command("git status; cargo test", &[]),
            Some("rtk git status; rtk cargo test".into())
        );
    }

    #[test]
    fn test_rewrite_compound_pipe_raw_filter() {
        // Pipe: rewrite first segment only, pass through rest unchanged
        assert_eq!(
            rewrite_command("cargo test | grep FAILED", &[]),
            Some("rtk cargo test | grep FAILED".into())
        );
    }

    #[test]
    fn test_rewrite_compound_pipe_git_grep() {
        assert_eq!(
            rewrite_command("git log -10 | grep feat", &[]),
            Some("rtk git log -10 | grep feat".into())
        );
    }

    #[test]
    fn test_rewrite_compound_four_segments() {
        assert_eq!(
            rewrite_command(
                "cargo fmt --all && cargo clippy && cargo test && git status",
                &[]
            ),
            Some(
                "rtk cargo fmt --all && rtk cargo clippy && rtk cargo test && rtk git status"
                    .into()
            )
        );
    }

    #[test]
    fn test_rewrite_compound_mixed_supported_unsupported() {
        // unsupported segments stay raw
        assert_eq!(
            rewrite_command("cargo test && terraform plan", &[]),
            Some("rtk cargo test && terraform plan".into())
        );
    }

    #[test]
    fn test_rewrite_compound_all_unsupported_returns_none() {
        // No rewrite at all: returns None
        assert_eq!(
            rewrite_command("terraform plan && terraform apply", &[]),
            None
        );
    }

    // --- sudo / env prefix + rewrite ---

    #[test]
    fn test_rewrite_sudo_docker() {
        assert_eq!(
            rewrite_command("sudo docker ps", &[]),
            Some("sudo rtk docker ps".into())
        );
    }

    #[test]
    fn test_rewrite_env_var_prefix() {
        assert_eq!(
            rewrite_command("GIT_SSH_COMMAND=ssh git push origin main", &[]),
            Some("GIT_SSH_COMMAND=ssh rtk git push origin main".into())
        );
    }

    // --- find with native flags ---

    #[test]
    fn test_rewrite_find_with_flags() {
        assert_eq!(
            rewrite_command("find . -name '*.rs' -type f", &[]),
            Some("rtk find . -name '*.rs' -type f".into())
        );
    }

    // --- Ensure PATTERNS and RULES stay aligned after modifications ---

    #[test]
    fn test_patterns_rules_aligned_after_aws_psql() {
        // If this fails, someone added a PATTERN without a matching RULE (or vice versa)
        assert_eq!(
            PATTERNS.len(),
            RULES.len(),
            "PATTERNS[{}] != RULES[{}] — they must stay 1:1",
            PATTERNS.len(),
            RULES.len()
        );
    }

    // --- All RULES have non-empty rtk_cmd and at least one rewrite_prefix ---

    #[test]
    fn test_all_rules_have_valid_rtk_cmd() {
        for rule in RULES {
            assert!(!rule.rtk_cmd.is_empty(), "Rule with empty rtk_cmd found");
            assert!(
                rule.rtk_cmd.starts_with("rtk "),
                "rtk_cmd '{}' must start with 'rtk '",
                rule.rtk_cmd
            );
            assert!(
                !rule.rewrite_prefixes.is_empty(),
                "Rule '{}' has no rewrite_prefixes",
                rule.rtk_cmd
            );
        }
    }

    // --- exclude_commands (#243) ---

    #[test]
    fn test_rewrite_excludes_curl() {
        let excluded = vec!["curl".to_string()];
        assert_eq!(
            rewrite_command("curl https://api.example.com/health", &excluded),
            None
        );
    }

    #[test]
    fn test_rewrite_exclude_does_not_affect_other_commands() {
        let excluded = vec!["curl".to_string()];
        assert_eq!(
            rewrite_command("git status", &excluded),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_empty_excludes_rewrites_curl() {
        let excluded: Vec<String> = vec![];
        assert!(rewrite_command("curl https://api.example.com", &excluded).is_some());
    }

    #[test]
    fn test_rewrite_compound_partial_exclude() {
        // curl excluded but git still rewrites
        let excluded = vec!["curl".to_string()];
        assert_eq!(
            rewrite_command("git status && curl https://api.example.com", &excluded),
            Some("rtk git status && curl https://api.example.com".into())
        );
    }

    // --- Every PATTERN compiles to a valid Regex ---

    #[test]
    fn test_all_patterns_are_valid_regex() {
        use regex::Regex;
        for (i, pattern) in PATTERNS.iter().enumerate() {
            assert!(
                Regex::new(pattern).is_ok(),
                "PATTERNS[{i}] = '{pattern}' is not a valid regex"
            );
        }
    }

    // --- #196: gh --json/--jq/--template passthrough ---

    #[test]
    fn test_rewrite_gh_json_skipped() {
        assert_eq!(rewrite_command("gh pr list --json number,title", &[]), None);
    }

    #[test]
    fn test_rewrite_gh_jq_skipped() {
        assert_eq!(
            rewrite_command("gh pr list --json number --jq '.[].number'", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_gh_template_skipped() {
        assert_eq!(
            rewrite_command("gh pr view 42 --template '{{.title}}'", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_gh_api_json_skipped() {
        assert_eq!(
            rewrite_command("gh api repos/owner/repo --jq '.name'", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_gh_without_json_still_works() {
        assert_eq!(
            rewrite_command("gh pr list", &[]),
            Some("rtk gh pr list".into())
        );
    }
}
