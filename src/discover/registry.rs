use lazy_static::lazy_static;
use regex::{Regex, RegexSet};

/// A rule mapping a shell command pattern to its RTK equivalent.
struct RtkRule {
    rtk_cmd: &'static str,
    category: &'static str,
    savings_pct: f64,
    subcmd_savings: &'static [(&'static str, f64)],
}

/// Result of classifying a command.
#[derive(Debug, PartialEq)]
pub enum Classification {
    Supported {
        rtk_equivalent: &'static str,
        category: &'static str,
        estimated_savings_pct: f64,
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

// Patterns ordered to match RTK_RULES indices exactly.
const PATTERNS: &[&str] = &[
    r"^git\s+(status|log|diff|show|add|commit|push|pull|branch|fetch|stash|worktree)",
    r"^gh\s+(pr|issue|run|repo|api)",
    r"^cargo\s+(build|test|clippy|fmt)",
    r"^pnpm\s+(list|ls|outdated|install)",
    r"^npm\s+(run|exec)",
    r"^npx\s+",
    r"^(cat|head|tail)\s+",
    r"^(rg|grep)\s+",
    r"^ls(\s|$)",
    r"^find\s+",
    r"^(npx\s+|pnpm\s+)?tsc(\s|$)",
    r"^(npx\s+|pnpm\s+)?(eslint|biome|lint)(\s|$)",
    r"^(npx\s+|pnpm\s+)?prettier",
    r"^(npx\s+|pnpm\s+)?next\s+build",
    r"^(pnpm\s+|npx\s+)?(vitest|jest|test)(\s|$)",
    r"^(npx\s+|pnpm\s+)?playwright",
    r"^(npx\s+|pnpm\s+)?prisma",
    r"^docker\s+(ps|images|logs)",
    r"^kubectl\s+(get|logs)",
    r"^curl\s+",
    r"^wget\s+",
];

const RULES: &[RtkRule] = &[
    RtkRule {
        rtk_cmd: "rtk git",
        category: "Git",
        savings_pct: 70.0,
        subcmd_savings: &[
            ("diff", 80.0),
            ("show", 80.0),
            ("add", 59.0),
            ("commit", 59.0),
        ],
    },
    RtkRule {
        rtk_cmd: "rtk gh",
        category: "GitHub",
        savings_pct: 82.0,
        subcmd_savings: &[("pr", 87.0), ("run", 82.0), ("issue", 80.0)],
    },
    RtkRule {
        rtk_cmd: "rtk cargo",
        category: "Cargo",
        savings_pct: 80.0,
        subcmd_savings: &[("test", 90.0)],
    },
    RtkRule {
        rtk_cmd: "rtk pnpm",
        category: "PackageManager",
        savings_pct: 80.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk npm",
        category: "PackageManager",
        savings_pct: 70.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk npx",
        category: "PackageManager",
        savings_pct: 70.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk read",
        category: "Files",
        savings_pct: 60.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk grep",
        category: "Files",
        savings_pct: 75.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk ls",
        category: "Files",
        savings_pct: 65.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk find",
        category: "Files",
        savings_pct: 70.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk tsc",
        category: "Build",
        savings_pct: 83.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk lint",
        category: "Build",
        savings_pct: 84.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk prettier",
        category: "Build",
        savings_pct: 70.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk next",
        category: "Build",
        savings_pct: 87.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk vitest",
        category: "Tests",
        savings_pct: 99.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk playwright",
        category: "Tests",
        savings_pct: 94.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk prisma",
        category: "Build",
        savings_pct: 88.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk docker",
        category: "Infra",
        savings_pct: 85.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk kubectl",
        category: "Infra",
        savings_pct: 85.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk curl",
        category: "Network",
        savings_pct: 70.0,
        subcmd_savings: &[],
    },
    RtkRule {
        rtk_cmd: "rtk wget",
        category: "Network",
        savings_pct: 65.0,
        subcmd_savings: &[],
    },
];

/// Commands to ignore (shell builtins, trivial, already rtk).
const IGNORED_PREFIXES: &[&str] = &[
    "cd ",
    "cd\t",
    "echo ",
    "printf ",
    "export ",
    "source ",
    "mkdir ",
    "rm ",
    "mv ",
    "cp ",
    "chmod ",
    "chown ",
    "touch ",
    "which ",
    "type ",
    "command ",
    "test ",
    "true",
    "false",
    "sleep ",
    "wait",
    "kill ",
    "set ",
    "unset ",
    "wc ",
    "sort ",
    "uniq ",
    "tr ",
    "cut ",
    "awk ",
    "sed ",
    "python3 -c",
    "python -c",
    "node -e",
    "ruby -e",
    "rtk ",
    "pwd",
    "bash ",
    "sh ",
    "then\n",
    "then ",
    "else\n",
    "else ",
    "fi",
    "do\n",
    "do ",
    "done",
    "for ",
    "while ",
    "if ",
    "case ",
];

const IGNORED_EXACT: &[&str] = &["cd", "echo", "true", "false", "wait", "pwd", "bash", "sh"];

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

        // Extract subcommand for savings override
        let savings = if !rule.subcmd_savings.is_empty() {
            if let Some(caps) = COMPILED[idx].captures(cmd_clean) {
                if let Some(sub) = caps.get(1) {
                    rule.subcmd_savings
                        .iter()
                        .find(|(s, _)| *s == sub.as_str())
                        .map(|(_, pct)| *pct)
                        .unwrap_or(rule.savings_pct)
                } else {
                    rule.savings_pct
                }
            } else {
                rule.savings_pct
            }
        } else {
            rule.savings_pct
        };

        Classification::Supported {
            rtk_equivalent: rule.rtk_cmd,
            category: rule.category,
            estimated_savings_pct: savings,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_git_status() {
        assert_eq!(
            classify_command("git status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
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
            }
        );
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
}
