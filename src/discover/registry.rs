//! Matches shell commands against known RTK rewrite rules to decide how to handle them.

use lazy_static::lazy_static;
use regex::{Regex, RegexSet};

use super::lexer::{split_on_operators, tokenize, TokenKind};
use super::rules::{IGNORED_EXACT, IGNORED_PREFIXES, RULES};

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
    static ref REGEX_SET: RegexSet =
        RegexSet::new(RULES.iter().map(|r| r.pattern)).expect("invalid regex patterns");
    static ref COMPILED: Vec<Regex> = RULES
        .iter()
        .map(|r| Regex::new(r.pattern).expect("invalid regex"))
        .collect();
    static ref ENV_PREFIX: Regex = {
        let double_quoted = r#""(?:[^"\\]|\\.)*""#;
        let single_quoted = r#"'(?:[^'\\]|\\.)*'"#;
        let unquoted = r#"[^\s]*"#;
        let env_value = format!("(?:{}|{}|{})", double_quoted, single_quoted, unquoted);
        let env_assign = format!(r#"[A-Z_][A-Z0-9_]*={}"#, env_value);
        Regex::new(&format!(r#"^(?:sudo\s+|env\s+|{}\s+)+"#, env_assign)).unwrap()
    };
    // Git global options that appear before the subcommand: -C <path>, -c <key=val>,
    // --git-dir <dir>, --work-tree <dir>, and flag-only options (#163)
    static ref GIT_GLOBAL_OPT: Regex =
        Regex::new(r"^(?:(?:-C\s+\S+|-c\s+\S+|--git-dir(?:=\S+|\s+\S+)|--work-tree(?:=\S+|\s+\S+)|--no-pager|--no-optional-locks|--bare|--literal-pathspecs)\s+)+").unwrap();
    static ref HEAD_N: Regex = Regex::new(r"^head\s+-(\d+)\s+(.+)$").unwrap();
    static ref HEAD_LINES: Regex = Regex::new(r"^head\s+--lines=(\d+)\s+(.+)$").unwrap();
    static ref TAIL_N: Regex = Regex::new(r"^tail\s+-(\d+)\s+(.+)$").unwrap();
    static ref TAIL_N_SPACE: Regex = Regex::new(r"^tail\s+-n\s+(\d+)\s+(.+)$").unwrap();
    static ref TAIL_LINES_EQ: Regex = Regex::new(r"^tail\s+--lines=(\d+)\s+(.+)$").unwrap();
    static ref TAIL_LINES_SPACE: Regex = Regex::new(r"^tail\s+--lines\s+(\d+)\s+(.+)$").unwrap();
}

const GOLANGCI_GLOBAL_OPT_WITH_VALUE: &[&str] = &[
    "-c",
    "--color",
    "--config",
    "--cpu-profile-path",
    "--mem-profile-path",
    "--trace-path",
];

#[derive(Debug, Clone, Copy)]
struct GolangciRunParts<'a> {
    global_segment: &'a str,
    run_segment: &'a str,
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

    // Normalize absolute binary paths: /usr/bin/grep → grep (#485)
    let cmd_normalized = strip_absolute_path(cmd_clean);
    // Strip git global options: git -C /tmp status → git status (#163)
    let cmd_normalized = strip_git_global_opts(&cmd_normalized);
    // Strip golangci-lint global options before `run` so classify/rewrite stays
    // aligned with the runtime wrapper behavior.
    let cmd_normalized = strip_golangci_global_opts(&cmd_normalized);
    let cmd_clean = cmd_normalized.as_str();

    // Exclude cat/head/tail with redirect operators — these are writes, not reads (#315)
    if cmd_clean.starts_with("cat ")
        || cmd_clean.starts_with("head ")
        || cmd_clean.starts_with("tail ")
    {
        let has_redirect = cmd_clean
            .split_whitespace()
            .skip(1)
            .any(|t| t.starts_with('>') || t == "<" || t.starts_with(">>"));
        if has_redirect {
            return Classification::Unsupported {
                base_command: cmd_clean
                    .split_whitespace()
                    .next()
                    .unwrap_or("cat")
                    .to_string(),
            };
        }
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

/// Quote-aware heredoc detection — `<<` inside quotes is not a heredoc.
pub fn has_heredoc(cmd: &str) -> bool {
    tokenize(cmd)
        .iter()
        .any(|t| t.kind == TokenKind::Redirect && t.value.starts_with("<<"))
}

pub fn split_command_chain(cmd: &str) -> Vec<&str> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    // Lexer-based for `<<`; string-based for `$((` (lexer splits it across tokens).
    if has_heredoc(trimmed) || trimmed.contains("$((") {
        return vec![trimmed];
    }

    split_on_operators(trimmed, true)
}

/// Strip git global options before the subcommand (#163).
/// `git -C /tmp status` → `git status`, preserving the rest.
/// Returns the original string unchanged if not a git command.
fn strip_git_global_opts(cmd: &str) -> String {
    // Only applies to commands starting with "git "
    if !cmd.starts_with("git ") {
        return cmd.to_string();
    }
    let after_git = &cmd[4..]; // skip "git "
    let stripped = GIT_GLOBAL_OPT.replace(after_git, "");
    format!("git {}", stripped.trim())
}

/// Strip golangci-lint global options before the `run` subcommand.
/// `golangci-lint --color never run ./...` → `golangci-lint run ./...`
/// Returns the original string unchanged if this is not a supported compact `run` invocation.
fn strip_golangci_global_opts(cmd: &str) -> String {
    match parse_golangci_run_parts(cmd) {
        Some(parts) => format!("golangci-lint {}", parts.run_segment),
        None => cmd.to_string(),
    }
}

/// Parse supported golangci-lint invocations with optional global flags before `run`.
fn parse_golangci_run_parts(cmd: &str) -> Option<GolangciRunParts<'_>> {
    let tokens = split_token_spans(cmd);
    let first = tokens.first()?;
    if first.0 != "golangci-lint" && first.0 != "golangci" {
        return None;
    }

    let mut i = 1;
    while i < tokens.len() {
        let token = tokens[i].0;

        if token == "--" {
            return None;
        }

        if !token.starts_with('-') {
            if token == "run" {
                let global_segment = if i > 1 {
                    cmd[tokens[1].1..tokens[i].1].trim()
                } else {
                    ""
                };
                let run_segment = cmd[tokens[i].1..].trim();
                return Some(GolangciRunParts {
                    global_segment,
                    run_segment,
                });
            }
            return None;
        }

        if let Some(flag) = split_golangci_flag_name(token) {
            if golangci_flag_takes_separate_value(token, flag) {
                i += 1;
            }
        }

        i += 1;
    }

    None
}

fn split_golangci_flag_name(arg: &str) -> Option<&str> {
    if arg.starts_with("--") {
        return Some(arg.split_once('=').map(|(flag, _)| flag).unwrap_or(arg));
    }

    if arg.starts_with('-') {
        return Some(arg);
    }

    None
}

fn golangci_flag_takes_separate_value(arg: &str, flag: &str) -> bool {
    if !GOLANGCI_GLOBAL_OPT_WITH_VALUE.contains(&flag) {
        return false;
    }

    if arg.starts_with("--") && arg.contains('=') {
        return false;
    }

    true
}

fn split_token_spans(cmd: &str) -> Vec<(&str, usize, usize)> {
    let mut tokens = Vec::new();
    let mut start = None;

    for (idx, ch) in cmd.char_indices() {
        if ch.is_whitespace() {
            if let Some(token_start) = start.take() {
                tokens.push((&cmd[token_start..idx], token_start, idx));
            }
        } else if start.is_none() {
            start = Some(idx);
        }
    }

    if let Some(token_start) = start {
        tokens.push((&cmd[token_start..], token_start, cmd.len()));
    }

    tokens
}

/// Normalize absolute binary paths: `/usr/bin/grep -rn foo` → `grep -rn foo` (#485)
/// Only strips if the first word contains a `/` (Unix path).
fn strip_absolute_path(cmd: &str) -> String {
    let first_space = cmd.find(' ');
    let first_word = match first_space {
        Some(pos) => &cmd[..pos],
        None => cmd,
    };
    if first_word.contains('/') {
        // Extract basename
        let basename = first_word.rsplit('/').next().unwrap_or(first_word);
        if basename.is_empty() {
            return cmd.to_string();
        }
        match first_space {
            Some(pos) => format!("{}{}", basename, &cmd[pos..]),
            None => basename.to_string(),
        }
    } else {
        cmd.to_string()
    }
}

/// Check if a command has RTK_DISABLED= prefix in its env prefix portion.
pub fn has_rtk_disabled_prefix(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    let stripped = ENV_PREFIX.replace(trimmed, "");
    let prefix_len = trimmed.len() - stripped.len();
    let prefix_part = &trimmed[..prefix_len];
    prefix_part.contains("RTK_DISABLED=")
}

/// Strip RTK_DISABLED=X and other env prefixes, return the actual command.
pub fn strip_disabled_prefix(cmd: &str) -> &str {
    let trimmed = cmd.trim();
    let stripped = ENV_PREFIX.replace(trimmed, "");
    // stripped is a Cow<str> that borrows from trimmed when no replacement happens.
    // We need to return a &str into the original, so compute the offset.
    let prefix_len = trimmed.len() - stripped.len();
    trimmed[prefix_len..].trim_start()
}

fn strip_trailing_redirects(cmd: &str) -> (&str, &str) {
    let tokens = tokenize(cmd);
    if tokens.is_empty() {
        return (cmd, "");
    }

    let mut redir_boundary = tokens.len();
    let mut i = tokens.len();
    while i > 0 {
        i -= 1;
        match tokens[i].kind {
            TokenKind::Redirect => {
                redir_boundary = i;
            }
            TokenKind::Arg => {
                if i > 0 && tokens[i - 1].kind == TokenKind::Redirect {
                    redir_boundary = i - 1;
                    i -= 1;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }

    if redir_boundary >= tokens.len() {
        return (cmd, "");
    }

    let cut = tokens[redir_boundary].offset;
    let cmd_part = cmd[..cut].trim_end();
    let redir_part = &cmd[cmd_part.len()..];
    (cmd_part, redir_part)
}

/// Returns `None` if the command is unsupported or ignored (hook should pass through).
///
/// Handles compound commands (`&&`, `||`, `;`) by rewriting each segment independently.
/// For pipes (`|`), only rewrites the left-hand command (pipe targets stay raw),
/// but continues rewriting segments after subsequent `&&`/`||`/`;` operators.
pub fn rewrite_command(cmd: &str, excluded: &[String]) -> Option<String> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return None;
    }

    if has_heredoc(trimmed) || trimmed.contains("$((") {
        return None;
    }

    let compiled = compile_exclude_patterns(excluded);

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

    rewrite_compound(trimmed, &compiled)
}

/// Rewrite a compound command (with `&&`, `||`, `;`, `|`) by rewriting each segment.
fn rewrite_compound(cmd: &str, excluded: &[ExcludePattern]) -> Option<String> {
    let tokens = tokenize(cmd);
    let mut result = String::with_capacity(cmd.len() + 32);
    let mut any_changed = false;
    let mut seg_start: usize = 0;

    for tok in &tokens {
        if tok.offset < seg_start {
            continue;
        }
        match tok.kind {
            TokenKind::Operator => {
                let seg = cmd[seg_start..tok.offset].trim();
                let rewritten = rewrite_segment(seg, excluded).unwrap_or_else(|| seg.to_string());
                if rewritten != seg {
                    any_changed = true;
                }
                result.push_str(&rewritten);
                if tok.value == ";" {
                    result.push(';');
                    let after = tok.offset + tok.value.len();
                    if after < cmd.len() {
                        result.push(' ');
                    }
                } else {
                    result.push(' ');
                    result.push_str(&tok.value);
                    result.push(' ');
                }
                seg_start = tok.offset + tok.value.len();
                while seg_start < cmd.len() && cmd.as_bytes().get(seg_start) == Some(&b' ') {
                    seg_start += 1;
                }
            }
            TokenKind::Pipe => {
                let seg = cmd[seg_start..tok.offset].trim();
                let is_pipe_incompatible = seg.starts_with("find ")
                    || seg == "find"
                    || seg.starts_with("fd ")
                    || seg == "fd";
                let rewritten = if is_pipe_incompatible {
                    seg.to_string()
                } else {
                    rewrite_segment(seg, excluded).unwrap_or_else(|| seg.to_string())
                };
                if rewritten != seg {
                    any_changed = true;
                }
                result.push_str(&rewritten);

                let pipe_group_end = tokens.iter().find(|t| {
                    t.offset > tok.offset
                        && (t.kind == TokenKind::Operator
                            || (t.kind == TokenKind::Shellism && t.value == "&"))
                });

                match pipe_group_end {
                    Some(next_op) => {
                        result.push(' ');
                        result.push_str(cmd[tok.offset..next_op.offset].trim());
                        seg_start = next_op.offset;
                    }
                    None => {
                        result.push(' ');
                        result.push_str(cmd[tok.offset..].trim_start());
                        return if any_changed { Some(result) } else { None };
                    }
                }
            }
            TokenKind::Shellism if tok.value == "&" => {
                let seg = cmd[seg_start..tok.offset].trim();
                let rewritten = rewrite_segment(seg, excluded).unwrap_or_else(|| seg.to_string());
                if rewritten != seg {
                    any_changed = true;
                }
                result.push_str(&rewritten);
                result.push_str(" & ");
                seg_start = tok.offset + tok.value.len();
                while seg_start < cmd.len() && cmd.as_bytes().get(seg_start) == Some(&b' ') {
                    seg_start += 1;
                }
            }
            _ => {}
        }
    }

    let seg = cmd[seg_start..].trim();
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

fn rewrite_line_range(cmd: &str) -> Option<String> {
    for re in [&*HEAD_N, &*HEAD_LINES] {
        if let Some(caps) = re.captures(cmd) {
            let n = caps.get(1)?.as_str();
            let file = caps.get(2)?.as_str();
            return Some(format!("rtk read {} --max-lines {}", file, n));
        }
    }
    if cmd.starts_with("head -") {
        return None;
    }
    for re in [
        &*TAIL_N,
        &*TAIL_N_SPACE,
        &*TAIL_LINES_EQ,
        &*TAIL_LINES_SPACE,
    ] {
        if let Some(caps) = re.captures(cmd) {
            let n = caps.get(1)?.as_str();
            let file = caps.get(2)?.as_str();
            return Some(format!("rtk read {} --tail-lines {}", file, n));
        }
    }
    None
}

/// Shell prefix builtins that modify how the shell runs a command
/// but don't change which command runs. Strip before routing, re-prepend after.
const SHELL_PREFIX_BUILTINS: &[&str] = &["noglob", "command", "builtin", "exec", "nocorrect"];

const MAX_PREFIX_DEPTH: usize = 10;

enum ExcludePattern {
    Regex(Regex),
    Prefix(String),
}

fn compile_exclude_patterns(patterns: &[String]) -> Vec<ExcludePattern> {
    patterns
        .iter()
        .filter_map(|pattern| {
            let trimmed = pattern.trim();
            if trimmed.is_empty() || trimmed == "^" {
                eprintln!(
                    "rtk: warning: ignoring trivial exclude_commands pattern '{}'",
                    pattern
                );
                return None;
            }
            let anchored = if trimmed.starts_with('^') {
                trimmed.to_string()
            } else {
                format!(r"^{}($|\s)", regex::escape(trimmed))
            };
            Some(match Regex::new(&anchored) {
                Ok(re) => ExcludePattern::Regex(re),
                Err(e) => {
                    eprintln!(
                        "rtk: warning: invalid exclude_commands pattern '{}': {}",
                        pattern, e
                    );
                    ExcludePattern::Prefix(pattern.clone())
                }
            })
        })
        .collect()
}

fn rewrite_segment(seg: &str, excluded: &[ExcludePattern]) -> Option<String> {
    rewrite_segment_inner(seg, excluded, 0)
}

fn is_excluded(cmd: &str, excluded: &[ExcludePattern]) -> bool {
    excluded.iter().any(|pat| match pat {
        ExcludePattern::Regex(re) => re.is_match(cmd),
        ExcludePattern::Prefix(prefix) => cmd.starts_with(prefix.as_str()),
    })
}

fn rewrite_segment_inner(seg: &str, excluded: &[ExcludePattern], depth: usize) -> Option<String> {
    let trimmed = seg.trim();
    if trimmed.is_empty() {
        return None;
    }

    if depth >= MAX_PREFIX_DEPTH {
        return None;
    }

    for &prefix in SHELL_PREFIX_BUILTINS {
        if let Some(rest) = strip_word_prefix(trimmed, prefix) {
            if rest.is_empty() {
                return None;
            }
            return match rewrite_segment_inner(rest, excluded, depth + 1) {
                Some(rewritten) => Some(format!("{} {}", prefix, rewritten)),
                None => None,
            };
        }
    }

    // Strip trailing stderr/stdout redirects before matching (#530)
    // e.g. "git status 2>&1" → match "git status", re-append " 2>&1"
    let (cmd_part, redirect_suffix) = strip_trailing_redirects(trimmed);

    // Already RTK — pass through unchanged
    if cmd_part.starts_with("rtk ") || cmd_part == "rtk" {
        return Some(trimmed.to_string());
    }

    if cmd_part.starts_with("head -") || cmd_part.starts_with("tail ") {
        return rewrite_line_range(cmd_part).map(|r| format!("{}{}", r, redirect_suffix));
    }

    // Most cat flags (-v, -A, -e, -t, -s, -b, --show-all, etc.) have different
    // semantics than rtk read or no equivalent at all. Only `-n` (line numbers)
    // maps correctly to `rtk read -n`. Skip rewrite for any other flag.
    if let Some(cmd_args) = cmd_part.strip_prefix("cat ") {
        let args = cmd_args.trim_start();
        if args.starts_with('-') && !args.starts_with("-n ") && !args.starts_with("-n\t") {
            return None;
        }
    }

    // Use classify_command for correct ignore/prefix handling
    let rtk_equivalent = match classify_command(cmd_part) {
        Classification::Supported { rtk_equivalent, .. } => {
            let stripped = ENV_PREFIX.replace(cmd_part, "");
            let cmd_clean = stripped.trim();
            if is_excluded(cmd_clean, excluded) {
                return None;
            }
            rtk_equivalent
        }
        _ => return None,
    };

    // Find the matching rule (rtk_cmd values are unique across all rules)
    let rule = RULES.iter().find(|r| r.rtk_cmd == rtk_equivalent)?;

    // Extract env prefix (sudo, env VAR=val, etc.)
    let stripped_cow = ENV_PREFIX.replace(cmd_part, "");
    let env_prefix_len = cmd_part.len() - stripped_cow.len();
    let env_prefix = &cmd_part[..env_prefix_len];
    let cmd_clean = stripped_cow.trim();

    // #345: RTK_DISABLED=1 in env prefix → skip rewrite entirely
    // #508: warn on stderr so agents learn to stop overusing it
    if has_rtk_disabled_prefix(cmd_part) {
        eprintln!(
            "[rtk] RTK_DISABLED=1 detected — skipping filter for this command. \
             Remove RTK_DISABLED=1 to restore token savings."
        );
        return None;
    }

    if let Some(parts) = parse_golangci_run_parts(cmd_clean) {
        let rewritten = if parts.global_segment.is_empty() {
            format!("{}rtk golangci-lint {}", env_prefix, parts.run_segment)
        } else {
            format!(
                "{}rtk golangci-lint {} {}",
                env_prefix, parts.global_segment, parts.run_segment
            )
        };
        return Some(rewritten);
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
                format!("{}{}{}", env_prefix, rule.rtk_cmd, redirect_suffix)
            } else {
                format!("{}{} {}{}", env_prefix, rule.rtk_cmd, rest, redirect_suffix)
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
    fn test_classify_yadm_status() {
        assert_eq!(
            classify_command("yadm status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_yadm_diff() {
        assert_eq!(
            classify_command("yadm diff"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_rewrite_yadm_status() {
        assert_eq!(
            rewrite_command("yadm status", &[]),
            Some("rtk git status".to_string())
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
    fn test_classify_cat_redirect_not_supported() {
        // cat > file and cat >> file are writes, not reads — should not be classified as supported
        let write_commands = [
            "cat > /tmp/output.txt",
            "cat >> /tmp/output.txt",
            "cat file.txt > output.txt",
            "cat -n file.txt >> log.txt",
            "head -10 README.md > output.txt",
            "tail -f app.log > /dev/null",
        ];
        for cmd in &write_commands {
            if let Classification::Supported { .. } = classify_command(cmd) {
                panic!("{} should NOT be classified as Supported", cmd)
            }
            // Unsupported or Ignored is fine
        }
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
    fn test_classify_htop_unsupported() {
        match classify_command("htop -d 10") {
            Classification::Unsupported { base_command } => {
                assert_eq!(base_command, "htop");
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

    // --- git -C <path> support (#555) ---

    #[test]
    fn test_rewrite_git_dash_c_status() {
        assert_eq!(
            rewrite_command("git -C /path/to/repo status", &[]),
            Some("rtk git -C /path/to/repo status".into())
        );
    }

    #[test]
    fn test_rewrite_git_dash_c_log() {
        assert_eq!(
            rewrite_command("git -C /tmp/myrepo log --oneline -5", &[]),
            Some("rtk git -C /tmp/myrepo log --oneline -5".into())
        );
    }

    #[test]
    fn test_rewrite_git_dash_c_diff() {
        assert_eq!(
            rewrite_command("git -C /home/user/project diff --name-only", &[]),
            Some("rtk git -C /home/user/project diff --name-only".into())
        );
    }

    #[test]
    fn test_classify_git_dash_c() {
        let result = classify_command("git -C /tmp status");
        assert!(
            matches!(
                result,
                Classification::Supported {
                    rtk_equivalent: "rtk git",
                    ..
                }
            ),
            "git -C should be classified as supported, got: {:?}",
            result
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
            rewrite_command("cargo test & htop", &[]),
            Some("rtk cargo test & htop".into())
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
        assert_eq!(rewrite_command("htop", &[]), None);
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
    fn test_rewrite_tsc() {
        let commands = vec![
            "npm exec tsc",
            "npm rum tsc",
            "npm run tsc",
            "npm run-script tsc",
            "npm urn tsc",
            "npm x tsc",
            "pnpm dlx tsc",
            "pnpm exec tsc",
            "pnpm run tsc",
            "pnpm run-script tsc",
            "npm tsc",
            "npx tsc",
            "pnpm tsc",
            "pnpx tsc",
            "tsc",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command(&format!("{command} --noEmit"), &[]),
                Some("rtk tsc --noEmit".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_cat_file() {
        assert_eq!(
            rewrite_command("cat src/main.rs", &[]),
            Some("rtk read src/main.rs".into())
        );
    }

    #[test]
    fn test_rewrite_cat_with_incompatible_flags_skipped() {
        // cat flags with different semantics than rtk read — skip rewrite
        assert_eq!(rewrite_command("cat -A file.cpp", &[]), None);
        assert_eq!(rewrite_command("cat -v file.txt", &[]), None);
        assert_eq!(rewrite_command("cat -e file.txt", &[]), None);
        assert_eq!(rewrite_command("cat -t file.txt", &[]), None);
        assert_eq!(rewrite_command("cat -s file.txt", &[]), None);
        assert_eq!(rewrite_command("cat --show-all file.txt", &[]), None);
    }

    #[test]
    fn test_rewrite_cat_with_compatible_flags() {
        // cat -n (line numbers) maps to rtk read -n — allow rewrite
        assert_eq!(
            rewrite_command("cat -n file.txt", &[]),
            Some("rtk read -n file.txt".into())
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
    fn test_rewrite_playwright() {
        let commands = vec![
            "npm exec playwright",
            "npm rum playwright",
            "npm run playwright",
            "npm run-script playwright",
            "npm urn playwright",
            "npm x playwright",
            "pnpm dlx playwright",
            "pnpm exec playwright",
            "pnpm run playwright",
            "pnpm run-script playwright",
            "npm playwright",
            "npx playwright",
            "pnpm playwright",
            "pnpx playwright",
            "playwright",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command(&format!("{command} test"), &[]),
                Some("rtk playwright test".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_next_build() {
        let commands = vec![
            "npm exec next build",
            "npm rum next build",
            "npm run next build",
            "npm run-script next build",
            "npm urn next build",
            "npm x next build",
            "pnpm dlx next build",
            "pnpm exec next build",
            "pnpm run next build",
            "pnpm run-script next build",
            "npm next build",
            "npx next build",
            "pnpm next build",
            "pnpx next build",
            "next build",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command(&format!("{command} --turbo"), &[]),
                Some("rtk next --turbo".into()),
                "Failed for command: {}",
                command
            );
        }
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
    fn test_rewrite_find_pipe_skipped() {
        // find in a pipe should NOT be rewritten — rtk find output format
        // is incompatible with pipe consumers like xargs (#439)
        assert_eq!(
            rewrite_command("find . -name '*.rs' | xargs grep 'fn run'", &[]),
            None
        );
    }

    #[test]
    fn test_rewrite_find_pipe_xargs_wc() {
        assert_eq!(rewrite_command("find src -type f | wc -l", &[]), None);
    }

    #[test]
    fn test_rewrite_find_no_pipe_still_rewritten() {
        // find WITHOUT a pipe should still be rewritten
        assert_eq!(
            rewrite_command("find . -name '*.rs'", &[]),
            Some("rtk find . -name '*.rs'".into())
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
    fn test_rewrite_rtk_disabled_warns_on_stderr() {
        assert_eq!(rewrite_command("RTK_DISABLED=1 git status", &[]), None);
    }

    #[test]
    fn test_rewrite_rtk_disabled_subprocess_warns() {
        let rtk_bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("rtk");
        if !rtk_bin.exists() {
            return;
        }
        let rtk_mtime = std::fs::metadata(&rtk_bin)
            .ok()
            .and_then(|m| m.modified().ok());
        let test_mtime = std::env::current_exe()
            .ok()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        if let (Some(rtk_t), Some(test_t)) = (rtk_mtime, test_mtime) {
            if rtk_t < test_t {
                return;
            }
        }

        let output = std::process::Command::new(&rtk_bin)
            .args(["rewrite", "RTK_DISABLED=1 git status"])
            .output()
            .expect("Failed to run rtk");

        assert!(
            !output.status.success(),
            "Should exit non-zero (no rewrite)"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("RTK_DISABLED=1 detected"),
            "Should warn on stderr, got: {}",
            stderr
        );
    }

    #[test]
    fn test_rewrite_non_rtk_disabled_env_still_rewrites() {
        assert_eq!(
            rewrite_command("SOME_VAR=1 git status", &[]),
            Some("SOME_VAR=1 rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_env_quoted_value_with_spaces() {
        assert_eq!(
            rewrite_command(
                r#"GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=no" git push"#,
                &[]
            ),
            Some(r#"GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=no" rtk git push"#.into())
        );
    }

    #[test]
    fn test_rewrite_env_single_quoted_value_with_spaces() {
        assert_eq!(
            rewrite_command("EDITOR='vim -u NONE' git commit", &[]),
            Some("EDITOR='vim -u NONE' rtk git commit".into())
        );
    }

    #[test]
    fn test_rewrite_env_quoted_plus_unquoted() {
        assert_eq!(
            rewrite_command(r#"FOO="bar baz" BAR=1 git status"#, &[]),
            Some(r#"FOO="bar baz" BAR=1 rtk git status"#.into())
        );
    }

    #[test]
    fn test_rewrite_env_escaped_quotes_in_value() {
        assert_eq!(
            rewrite_command(r#"FOO="he said \"hello\"" git status"#, &[]),
            Some(r#"FOO="he said \"hello\"" rtk git status"#.into())
        );
    }

    #[test]
    fn test_classify_env_quoted_value_stripped() {
        assert_eq!(
            classify_command(r#"GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=no" git push"#),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
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
    fn test_rewrite_redirect_double() {
        // Double redirect: only last one stripped, but full command rewrites correctly
        assert_eq!(
            rewrite_command("git status 2>&1 >/dev/null", &[]),
            Some("rtk git status 2>&1 >/dev/null".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_fd_close() {
        // 2>&- (close stderr fd)
        assert_eq!(
            rewrite_command("git status 2>&-", &[]),
            Some("rtk git status 2>&-".into())
        );
    }

    #[test]
    fn test_rewrite_redirect_quotes_not_stripped() {
        // Redirect-like chars inside quotes should NOT be stripped
        // Known limitation: apostrophes cause conservative no-strip (safe fallback)
        let result = rewrite_command("git commit -m \"it's fixed\" 2>&1", &[]);
        assert!(
            result.is_some(),
            "Should still rewrite even with apostrophe"
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

    #[test]
    fn test_rewrite_tail_numeric_flag() {
        assert_eq!(
            rewrite_command("tail -20 src/main.rs", &[]),
            Some("rtk read src/main.rs --tail-lines 20".into())
        );
    }

    #[test]
    fn test_rewrite_tail_n_space_flag() {
        assert_eq!(
            rewrite_command("tail -n 12 src/lib.rs", &[]),
            Some("rtk read src/lib.rs --tail-lines 12".into())
        );
    }

    #[test]
    fn test_rewrite_tail_lines_long_flag() {
        assert_eq!(
            rewrite_command("tail --lines=7 src/lib.rs", &[]),
            Some("rtk read src/lib.rs --tail-lines 7".into())
        );
    }

    #[test]
    fn test_rewrite_tail_lines_space_flag() {
        assert_eq!(
            rewrite_command("tail --lines 7 src/lib.rs", &[]),
            Some("rtk read src/lib.rs --tail-lines 7".into())
        );
    }

    #[test]
    fn test_rewrite_tail_other_flag_skipped() {
        assert_eq!(rewrite_command("tail -c 100 src/main.rs", &[]), None);
    }

    #[test]
    fn test_rewrite_tail_plain_file_skipped() {
        assert_eq!(rewrite_command("tail src/main.rs", &[]), None);
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

    #[test]
    fn test_classify_swift_test() {
        assert!(matches!(
            classify_command("swift test"),
            Classification::Supported {
                rtk_equivalent: "rtk swift",
                category: "Build",
                estimated_savings_pct: 90.0,
                status: RtkStatus::Existing,
            }
        ));
    }

    #[test]
    fn test_rewrite_swift_test() {
        assert_eq!(
            rewrite_command("swift test --parallel", &[]),
            Some("rtk swift test --parallel".into())
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
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_with_flag_before_run() {
        assert!(matches!(
            classify_command("golangci-lint -v run ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_with_value_flag_before_run() {
        assert!(matches!(
            classify_command("golangci-lint --color never run ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_with_inline_value_flag_before_run() {
        assert!(matches!(
            classify_command("golangci-lint --color=never run ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_with_inline_config_flag_before_run() {
        assert!(matches!(
            classify_command("golangci-lint --config=foo.yml run ./..."),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_bare_is_not_compact_wrapper() {
        assert!(!matches!(
            classify_command("golangci-lint"),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
                ..
            }
        ));
    }

    #[test]
    fn test_classify_golangci_lint_other_subcommand_is_not_compact_wrapper() {
        assert!(!matches!(
            classify_command("golangci-lint version"),
            Classification::Supported {
                rtk_equivalent: "rtk golangci-lint run",
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

    #[test]
    fn test_rewrite_golangci_lint_with_flag_before_run() {
        assert_eq!(
            rewrite_command("golangci-lint -v run ./...", &[]),
            Some("rtk golangci-lint -v run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_golangci_lint_with_value_flag_before_run() {
        assert_eq!(
            rewrite_command("golangci-lint --color never run ./...", &[]),
            Some("rtk golangci-lint --color never run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_golangci_lint_with_inline_value_flag_before_run() {
        assert_eq!(
            rewrite_command("golangci-lint --color=never run ./...", &[]),
            Some("rtk golangci-lint --color=never run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_golangci_lint_with_inline_config_flag_before_run() {
        assert_eq!(
            rewrite_command("golangci-lint --config=foo.yml run ./...", &[]),
            Some("rtk golangci-lint --config=foo.yml run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_env_prefixed_golangci_lint_with_value_flag_before_run() {
        assert_eq!(
            rewrite_command("FOO=1 golangci-lint --color never run ./...", &[]),
            Some("FOO=1 rtk golangci-lint --color never run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_env_prefixed_golangci_lint_with_inline_value_flag_before_run() {
        assert_eq!(
            rewrite_command("FOO=1 golangci-lint --color=never run ./...", &[]),
            Some("FOO=1 rtk golangci-lint --color=never run ./...".into())
        );
    }

    #[test]
    fn test_rewrite_bare_golangci_lint_skips_compact_wrapper() {
        assert_eq!(rewrite_command("golangci-lint", &[]), None);
    }

    #[test]
    fn test_rewrite_other_golangci_lint_subcommand_skips_compact_wrapper() {
        assert_eq!(rewrite_command("golangci-lint version", &[]), None);
    }

    // --- JS/TS tooling ---

    #[test]
    fn test_classify_lint() {
        let commands = vec![
            "npm exec biome",
            "npm exec eslint",
            "npm rum biome",
            "npm rum eslint",
            "npm rum lint",
            "npm run biome",
            "npm run eslint",
            "npm run lint",
            "npm run-script biome",
            "npm run-script eslint",
            "npm run-script lint",
            "npm urn biome",
            "npm urn eslint",
            "npm urn lint",
            "npm x biome",
            "npm x eslint",
            "pnpm dlx biome",
            "pnpm dlx eslint",
            "pnpm exec biome",
            "pnpm exec eslint",
            "pnpm run biome",
            "pnpm run eslint",
            "pnpm run lint",
            "pnpm run-script biome",
            "pnpm run-script eslint",
            "pnpm run-script lint",
            "npm biome",
            "npm eslint",
            "npm lint",
            "npx biome",
            "npx eslint",
            "npx lint",
            "pnpm biome",
            "pnpm eslint",
            "pnpm lint",
            "pnpx biome",
            "pnpx eslint",
            "pnpx lint",
            "biome",
            "eslint",
            "lint",
        ];
        for command in commands {
            assert!(
                matches!(
                    classify_command(command),
                    Classification::Supported {
                        rtk_equivalent: "rtk lint",
                        ..
                    }
                ),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_lint() {
        let commands = vec![
            "npm exec biome",
            "npm exec eslint",
            "npm rum biome",
            "npm rum eslint",
            "npm rum lint",
            "npm run biome",
            "npm run eslint",
            "npm run lint",
            "npm run-script biome",
            "npm run-script eslint",
            "npm run-script lint",
            "npm urn biome",
            "npm urn eslint",
            "npm urn lint",
            "npm x biome",
            "npm x eslint",
            "pnpm dlx biome",
            "pnpm dlx eslint",
            "pnpm exec biome",
            "pnpm exec eslint",
            "pnpm run biome",
            "pnpm run eslint",
            "pnpm run lint",
            "pnpm run-script biome",
            "pnpm run-script eslint",
            "pnpm run-script lint",
            "npm biome",
            "npm eslint",
            "npm lint",
            "npx biome",
            "npx eslint",
            "npx lint",
            "pnpm biome",
            "pnpm eslint",
            "pnpm lint",
            "pnpx biome",
            "pnpx eslint",
            "pnpx lint",
            "biome",
            "eslint",
            "lint",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command(command, &[]),
                Some("rtk lint".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_classify_jest() {
        let commands = vec![
            "jest run",
            "jest",
            "npm exec jest run",
            "npm exec jest",
            "npm jest run",
            "npm jest",
            "npm rum jest run",
            "npm rum jest",
            "npm run jest run",
            "npm run jest",
            "npm run-script jest run",
            "npm run-script jest",
            "npm urn jest run",
            "npm urn jest",
            "npm x jest run",
            "npm x jest",
            "npx jest run",
            "npx jest",
            "pnpm dlx jest run",
            "pnpm dlx jest",
            "pnpm exec jest run",
            "pnpm exec jest",
            "pnpm jest run",
            "pnpm jest",
            "pnpm run jest run",
            "pnpm run jest",
            "pnpm run-script jest run",
            "pnpm run-script jest",
            "pnpx jest run",
            "pnpx jest",
        ];
        for command in commands {
            assert!(
                matches!(
                    classify_command(command),
                    Classification::Supported {
                        rtk_equivalent: "rtk jest",
                        ..
                    }
                ),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_jest() {
        let commands = vec![
            "jest run",
            "jest",
            "npm exec jest run",
            "npm exec jest",
            "npm jest run",
            "npm jest",
            "npm rum jest run",
            "npm rum jest",
            "npm run jest run",
            "npm run jest",
            "npm run-script jest run",
            "npm run-script jest",
            "npm urn jest run",
            "npm urn jest",
            "npm x jest run",
            "npm x jest",
            "npx jest run",
            "npx jest",
            "pnpm dlx jest run",
            "pnpm dlx jest",
            "pnpm exec jest run",
            "pnpm exec jest",
            "pnpm jest run",
            "pnpm jest",
            "pnpm run jest run",
            "pnpm run jest",
            "pnpm run-script jest run",
            "pnpm run-script jest",
            "pnpx jest run",
            "pnpx jest",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command(command, &[]),
                Some("rtk jest".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_classify_vitest() {
        let commands = vec![
            "npm exec vitest run",
            "npm exec vitest",
            "npm rum vitest run",
            "npm rum vitest",
            "npm run vitest run",
            "npm run vitest",
            "npm run-script vitest run",
            "npm run-script vitest",
            "npm urn vitest run",
            "npm urn vitest",
            "npm vitest run",
            "npm vitest",
            "npm x vitest run",
            "npm x vitest",
            "npx vitest run",
            "npx vitest",
            "pnpm dlx vitest run",
            "pnpm dlx vitest",
            "pnpm exec vitest run",
            "pnpm exec vitest",
            "pnpm run vitest run",
            "pnpm run vitest",
            "pnpm run-script vitest run",
            "pnpm run-script vitest",
            "pnpm vitest run",
            "pnpm vitest",
            "pnpx vitest run",
            "pnpx vitest",
            "vitest run",
            "vitest",
        ];
        for command in commands {
            assert!(
                matches!(
                    classify_command(command),
                    Classification::Supported {
                        rtk_equivalent: "rtk vitest",
                        ..
                    }
                ),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_vitest() {
        let commands = vec![
            "npm exec vitest run",
            "npm exec vitest",
            "npm rum vitest run",
            "npm rum vitest",
            "npm run vitest run",
            "npm run vitest",
            "npm run-script vitest run",
            "npm run-script vitest",
            "npm urn vitest run",
            "npm urn vitest",
            "npm vitest run",
            "npm vitest",
            "npm x vitest run",
            "npm x vitest",
            "npx vitest run",
            "npx vitest",
            "pnpm dlx vitest run",
            "pnpm dlx vitest",
            "pnpm exec vitest run",
            "pnpm exec vitest",
            "pnpm run vitest run",
            "pnpm run vitest",
            "pnpm run-script vitest run",
            "pnpm run-script vitest",
            "pnpm vitest run",
            "pnpm vitest",
            "pnpx vitest run",
            "pnpx vitest",
            "vitest run",
            "vitest",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command(command, &[]),
                Some("rtk vitest".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_classify_prisma() {
        let commands = vec![
            "npm exec prisma",
            "npm rum prisma",
            "npm run prisma",
            "npm run-script prisma",
            "npm urn prisma",
            "npm x prisma",
            "pnpm dlx prisma",
            "pnpm exec prisma",
            "pnpm run prisma",
            "pnpm run-script prisma",
            "npm prisma",
            "npx prisma",
            "pnpm prisma",
            "pnpx prisma",
            "prisma",
        ];
        for command in commands {
            assert!(
                matches!(
                    classify_command(format!("{command} migrate dev").as_str()),
                    Classification::Supported {
                        rtk_equivalent: "rtk prisma",
                        ..
                    }
                ),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_prisma() {
        let commands = vec![
            "npm exec prisma",
            "npm rum prisma",
            "npm run prisma",
            "npm run-script prisma",
            "npm urn prisma",
            "npm x prisma",
            "pnpm dlx prisma",
            "pnpm exec prisma",
            "pnpm run prisma",
            "pnpm run-script prisma",
            "npm prisma",
            "npx prisma",
            "pnpm prisma",
            "pnpx prisma",
            "prisma",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command(format!("{command} migrate dev").as_str(), &[]),
                Some("rtk prisma migrate dev".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_prettier() {
        let commands = vec![
            "npm exec prettier",
            "npm rum prettier",
            "npm run prettier",
            "npm run-script prettier",
            "npm urn prettier",
            "npm x prettier",
            "pnpm dlx prettier",
            "pnpm exec prettier",
            "pnpm run prettier",
            "pnpm run-script prettier",
            "npm prettier",
            "npx prettier",
            "pnpm prettier",
            "pnpx prettier",
            "prettier",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command(format!("{command} --check src/").as_str(), &[]),
                Some("rtk prettier --check src/".into()),
                "Failed for command: {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_pnpm_command() {
        let commands = vec![
            "exec",
            "i",
            "install",
            "list",
            "ls",
            "outdated",
            "run",
            "run-script",
        ];
        for command in commands {
            assert_eq!(
                rewrite_command(format!("pnpm {command}").as_str(), &[]),
                Some(format!("rtk pnpm {command}")),
                "Failed for command: pnpm {}",
                command
            );
        }
    }

    #[test]
    fn test_rewrite_npx() {
        assert_eq!(
            rewrite_command("npx svgo", &[]),
            Some("rtk npx svgo".to_string()),
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
            rewrite_command("cargo test && htop", &[]),
            Some("rtk cargo test && htop".into())
        );
    }

    #[test]
    fn test_rewrite_compound_all_unsupported_returns_none() {
        // No rewrite at all: returns None
        assert_eq!(rewrite_command("htop && top", &[]), None);
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

    #[test]
    fn test_all_rules_are_complete() {
        for rule in RULES {
            assert!(
                !rule.pattern.is_empty(),
                "Rule '{}' has empty pattern",
                rule.rtk_cmd
            );
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

    #[test]
    fn test_exclude_env_prefixed_command() {
        let excluded = vec!["psql".to_string()];
        assert_eq!(
            rewrite_command("PGPASSWORD=postgres psql -h localhost", &excluded),
            None
        );
    }

    #[test]
    fn test_exclude_subcommand_pattern() {
        let excluded = vec!["git push".to_string()];
        assert_eq!(rewrite_command("git push origin main", &excluded), None);
    }

    #[test]
    fn test_exclude_regex_pattern() {
        let excluded = vec!["^curl".to_string()];
        assert_eq!(rewrite_command("curl http://example.com", &excluded), None);
    }

    #[test]
    fn test_exclude_invalid_regex_fallback() {
        let excluded = vec!["curl[".to_string()];
        assert!(rewrite_command("curl http://example.com", &excluded).is_some());
    }

    #[test]
    fn test_exclude_does_not_substring_match() {
        let excluded = vec!["go".to_string()];
        assert!(rewrite_command("golangci-lint run ./...", &excluded).is_some());
    }

    #[test]
    fn test_exclude_does_not_match_hyphenated_command() {
        let excluded = vec!["golangci".to_string()];
        assert!(rewrite_command("golangci-lint run ./...", &excluded).is_some());
    }

    #[test]
    fn test_exclude_empty_pattern_ignored() {
        let excluded = vec!["".to_string()];
        assert!(rewrite_command("git status", &excluded).is_some());
    }

    #[test]
    fn test_exclude_bare_anchor_ignored() {
        let excluded = vec!["^".to_string()];
        assert!(rewrite_command("git status", &excluded).is_some());
    }

    #[test]
    fn test_all_patterns_are_valid_regex() {
        use regex::Regex;
        for (i, rule) in RULES.iter().enumerate() {
            assert!(
                Regex::new(rule.pattern).is_ok(),
                "RULES[{i}] ({}) has invalid pattern '{}'",
                rule.rtk_cmd,
                rule.pattern
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

    // --- #508: RTK_DISABLED detection helpers ---

    #[test]
    fn test_has_rtk_disabled_prefix() {
        assert!(has_rtk_disabled_prefix("RTK_DISABLED=1 git status"));
        assert!(has_rtk_disabled_prefix("FOO=1 RTK_DISABLED=1 cargo test"));
        assert!(has_rtk_disabled_prefix(
            "RTK_DISABLED=true git log --oneline"
        ));
        assert!(!has_rtk_disabled_prefix("git status"));
        assert!(!has_rtk_disabled_prefix("rtk git status"));
        assert!(!has_rtk_disabled_prefix("SOME_VAR=1 git status"));
    }

    #[test]
    fn test_strip_disabled_prefix() {
        assert_eq!(
            strip_disabled_prefix("RTK_DISABLED=1 git status"),
            "git status"
        );
        assert_eq!(
            strip_disabled_prefix("FOO=1 RTK_DISABLED=1 cargo test"),
            "cargo test"
        );
        assert_eq!(strip_disabled_prefix("git status"), "git status");
    }

    // --- #485: absolute path normalization ---

    #[test]
    fn test_classify_absolute_path_grep() {
        assert_eq!(
            classify_command("/usr/bin/grep -rni pattern"),
            Classification::Supported {
                rtk_equivalent: "rtk grep",
                category: "Files",
                estimated_savings_pct: 75.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_absolute_path_ls() {
        assert_eq!(
            classify_command("/bin/ls -la"),
            Classification::Supported {
                rtk_equivalent: "rtk ls",
                category: "Files",
                estimated_savings_pct: 65.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_absolute_path_git() {
        assert_eq!(
            classify_command("/usr/local/bin/git status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_absolute_path_no_args() {
        // /usr/bin/find alone → still classified
        assert_eq!(
            classify_command("/usr/bin/find ."),
            Classification::Supported {
                rtk_equivalent: "rtk find",
                category: "Files",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_strip_absolute_path_helper() {
        assert_eq!(strip_absolute_path("/usr/bin/grep -rn foo"), "grep -rn foo");
        assert_eq!(strip_absolute_path("/bin/ls -la"), "ls -la");
        assert_eq!(strip_absolute_path("grep -rn foo"), "grep -rn foo");
        assert_eq!(strip_absolute_path("/usr/local/bin/git"), "git");
    }

    // --- #163: git global options ---

    #[test]
    fn test_classify_git_with_dash_c_path() {
        assert_eq!(
            classify_command("git -C /tmp status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_git_no_pager_log() {
        assert_eq!(
            classify_command("git --no-pager log -5"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_git_git_dir() {
        assert_eq!(
            classify_command("git --git-dir /tmp/.git status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_rewrite_git_dash_c() {
        assert_eq!(
            rewrite_command("git -C /tmp status", &[]),
            Some("rtk git -C /tmp status".to_string())
        );
    }

    #[test]
    fn test_rewrite_git_no_pager() {
        assert_eq!(
            rewrite_command("git --no-pager log -5", &[]),
            Some("rtk git --no-pager log -5".to_string())
        );
    }

    #[test]
    fn test_strip_git_global_opts_helper() {
        assert_eq!(strip_git_global_opts("git -C /tmp status"), "git status");
        assert_eq!(strip_git_global_opts("git --no-pager log"), "git log");
        assert_eq!(strip_git_global_opts("git status"), "git status");
        assert_eq!(strip_git_global_opts("cargo test"), "cargo test");
    }

    #[test]
    fn test_strip_golangci_global_opts_helper() {
        assert_eq!(
            strip_golangci_global_opts("golangci-lint -v run ./..."),
            "golangci-lint run ./..."
        );
        assert_eq!(
            strip_golangci_global_opts("golangci-lint --color never run ./..."),
            "golangci-lint run ./..."
        );
        assert_eq!(
            strip_golangci_global_opts("golangci-lint --color=never run ./..."),
            "golangci-lint run ./..."
        );
        assert_eq!(
            strip_golangci_global_opts("golangci-lint --config=foo.yml run ./..."),
            "golangci-lint run ./..."
        );
        assert_eq!(
            strip_golangci_global_opts("golangci-lint version"),
            "golangci-lint version"
        );
        assert_eq!(strip_golangci_global_opts("cargo test"), "cargo test");
    }

    // --- #wc: wc filter was silently ignored by the hook ---

    #[test]
    fn test_classify_wc_supported() {
        // BUG: "wc " was in IGNORED_PREFIXES despite wc_cmd.rs having a full filter.
        // This test documents the bug: it must FAIL before the fix and PASS after.
        assert_eq!(
            classify_command("wc -l src/main.rs"),
            Classification::Supported {
                rtk_equivalent: "rtk wc",
                category: "Files",
                estimated_savings_pct: 60.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_wc_multi_file() {
        assert_eq!(
            classify_command("wc src/*.rs"),
            Classification::Supported {
                rtk_equivalent: "rtk wc",
                category: "Files",
                estimated_savings_pct: 60.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_rewrite_wc() {
        assert_eq!(
            rewrite_command("wc -l src/main.rs", &[]),
            Some("rtk wc -l src/main.rs".into())
        );
    }

    #[test]
    fn test_rewrite_wc_multi_file() {
        assert_eq!(
            rewrite_command("wc src/*.rs", &[]),
            Some("rtk wc src/*.rs".into())
        );
    }

    #[test]
    fn test_classify_command_substitution_passthrough() {
        assert_eq!(
            classify_command("git log $(git rev-parse HEAD~1)"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_rewrite_command_substitution_passthrough() {
        assert_eq!(
            rewrite_command("git log $(git rev-parse HEAD~1)", &[]),
            Some("rtk git log $(git rev-parse HEAD~1)".into())
        );
    }

    #[test]
    fn test_split_command_substitution_no_split() {
        assert_eq!(
            split_command_chain("git log $(git rev-parse HEAD~1)"),
            vec!["git log $(git rev-parse HEAD~1)"]
        );
    }

    #[test]
    fn test_shell_prefix_noglob() {
        assert_eq!(
            rewrite_command("noglob git status", &[]),
            Some("noglob rtk git status".into())
        );
    }

    #[test]
    fn test_shell_prefix_command() {
        assert_eq!(
            rewrite_command("command git status", &[]),
            Some("command rtk git status".into())
        );
    }

    #[test]
    fn test_shell_prefix_builtin_exec_nocorrect() {
        assert_eq!(
            rewrite_command("builtin git status", &[]),
            Some("builtin rtk git status".into())
        );
        assert_eq!(
            rewrite_command("exec git status", &[]),
            Some("exec rtk git status".into())
        );
        assert_eq!(
            rewrite_command("nocorrect git status", &[]),
            Some("nocorrect rtk git status".into())
        );
    }

    #[test]
    fn test_shell_prefix_unknown_inner() {
        assert_eq!(rewrite_command("noglob unknown_cmd --flag", &[]), None);
    }

    #[test]
    fn test_python3_m_pytest() {
        assert_eq!(
            rewrite_command("python3 -m pytest tests/", &[]),
            Some("rtk pytest tests/".into())
        );
    }

    #[test]
    fn test_pip_show() {
        assert_eq!(
            rewrite_command("pip show flask", &[]),
            Some("rtk pip show flask".into())
        );
    }

    #[test]
    fn test_gt_graphite() {
        assert_eq!(rewrite_command("gt log", &[]), Some("rtk gt log".into()));
    }

    #[test]
    fn test_command_no_longer_ignored() {
        assert_ne!(
            classify_command("command git status"),
            Classification::Ignored
        );
    }

    // --- Pipe + operator rewrite ---

    #[test]
    fn test_rewrite_pipe_then_and() {
        assert_eq!(
            rewrite_command("git log | head -5 && git stash", &[]),
            Some("rtk git log | head -5 && rtk git stash".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_then_semicolon() {
        assert_eq!(
            rewrite_command("cargo test | head; git status", &[]),
            Some("rtk cargo test | head; rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_then_or() {
        assert_eq!(
            rewrite_command("cargo test | grep FAIL || git stash", &[]),
            Some("rtk cargo test | grep FAIL || rtk git stash".into())
        );
    }

    #[test]
    fn test_rewrite_env_pipe_then_and() {
        assert_eq!(
            rewrite_command(
                "RUST_BACKTRACE=1 cargo test 2>&1 | grep FAILED && git stash",
                &[]
            ),
            Some("RUST_BACKTRACE=1 rtk cargo test 2>&1 | grep FAILED && rtk git stash".into())
        );
    }

    #[test]
    fn test_rewrite_and_then_pipe() {
        assert_eq!(
            rewrite_command("git status && cargo test | grep FAIL", &[]),
            Some("rtk git status && rtk cargo test | grep FAIL".into())
        );
    }

    #[test]
    fn test_rewrite_multi_pipe_then_and() {
        assert_eq!(
            rewrite_command("git log | head | tail && git status", &[]),
            Some("rtk git log | head | tail && rtk git status".into())
        );
    }
}
