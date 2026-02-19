use lazy_static::lazy_static;
use regex::{Regex, RegexSet};
use std::collections::HashMap;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Hook routing table — used by `cmd::hook` for O(1) command rewriting.
// This is the single source of truth for which external binaries route through
// RTK and exactly which subcommands are covered.
//
// # Adding a new command
// 1. Add one `Route` entry to `ROUTES`.
// 2. Add a discover entry (PATTERNS + RULES) below if needed.
// 3. Done — hook routing is automatic.
// ---------------------------------------------------------------------------

/// Subcommand filter for a route entry.
#[derive(Debug, Clone, Copy)]
pub enum Subcmds {
    /// Route ALL subcommands of this binary (e.g., ls, curl, prettier).
    Any,
    /// Route ONLY these specific subcommands; others fall through to `rtk run -c`.
    Only(&'static [&'static str]),
}

/// One row in the static routing table.
///
/// - `binaries`: one or more external binary names mapping to the same RTK subcommand.
/// - `subcmds`: subcommand filter — `Any` matches everything, `Only` restricts to a list.
/// - `rtk_cmd`: the RTK subcommand name (e.g., `"grep"`, `"lint"`, `"git"`).
///
/// For direct routes where `binary == rtk_cmd`, the hook uses `format!("rtk {raw}")`.
/// For renames (`rg` → `grep`, `eslint` → `lint`), it uses `replace_first_word`.
#[derive(Debug, Clone, Copy)]
pub struct Route {
    pub binaries: &'static [&'static str],
    pub subcmds: Subcmds,
    pub rtk_cmd: &'static str,
}

/// Static routing table. Single source of truth for hook routing.
///
/// Order does not matter — lookups use a HashMap built once at startup (O(1) per call).
///
/// Complex cases (vitest bare invocation, `uv pip`, `python -m pytest`, pnpm, npx)
/// require Rust logic and stay as match arms in `cmd::hook::route_native_command`.
pub const ROUTES: &[Route] = &[
    // Version control
    Route {
        binaries: &["git"],
        subcmds: Subcmds::Only(&[
            "status", "diff", "log", "add", "commit", "push", "pull", "branch", "fetch", "stash",
            "show",
        ]),
        rtk_cmd: "git",
    },
    // GitHub CLI
    Route {
        binaries: &["gh"],
        subcmds: Subcmds::Only(&["pr", "issue", "run"]),
        rtk_cmd: "gh",
    },
    // Rust build tools
    Route {
        binaries: &["cargo"],
        subcmds: Subcmds::Only(&["test", "build", "clippy", "check"]),
        rtk_cmd: "cargo",
    },
    // Search — two binaries, one RTK subcommand (rename)
    Route {
        binaries: &["rg", "grep"],
        subcmds: Subcmds::Any,
        rtk_cmd: "grep",
    },
    // JavaScript linting — rename
    Route {
        binaries: &["eslint"],
        subcmds: Subcmds::Any,
        rtk_cmd: "lint",
    },
    // File system
    Route {
        binaries: &["ls"],
        subcmds: Subcmds::Any,
        rtk_cmd: "ls",
    },
    // TypeScript compiler
    Route {
        binaries: &["tsc"],
        subcmds: Subcmds::Any,
        rtk_cmd: "tsc",
    },
    // JavaScript formatting
    Route {
        binaries: &["prettier"],
        subcmds: Subcmds::Any,
        rtk_cmd: "prettier",
    },
    // E2E testing
    Route {
        binaries: &["playwright"],
        subcmds: Subcmds::Any,
        rtk_cmd: "playwright",
    },
    // Database ORM
    Route {
        binaries: &["prisma"],
        subcmds: Subcmds::Any,
        rtk_cmd: "prisma",
    },
    // Network
    Route {
        binaries: &["curl"],
        subcmds: Subcmds::Any,
        rtk_cmd: "curl",
    },
    // Python testing
    Route {
        binaries: &["pytest"],
        subcmds: Subcmds::Any,
        rtk_cmd: "pytest",
    },
    // Go linting
    Route {
        binaries: &["golangci-lint"],
        subcmds: Subcmds::Any,
        rtk_cmd: "golangci-lint",
    },
    // Containers — read-only subcommands only
    Route {
        binaries: &["docker"],
        subcmds: Subcmds::Only(&["ps", "images", "logs"]),
        rtk_cmd: "docker",
    },
    // Kubernetes — read-only subcommands only
    Route {
        binaries: &["kubectl"],
        subcmds: Subcmds::Only(&["get", "logs"]),
        rtk_cmd: "kubectl",
    },
    // Go build tools
    Route {
        binaries: &["go"],
        subcmds: Subcmds::Only(&["test", "build", "vet"]),
        rtk_cmd: "go",
    },
    // Python linting/formatting
    Route {
        binaries: &["ruff"],
        subcmds: Subcmds::Only(&["check", "format"]),
        rtk_cmd: "ruff",
    },
    // Python package management
    Route {
        binaries: &["pip"],
        subcmds: Subcmds::Only(&["list", "outdated", "install", "show"]),
        rtk_cmd: "pip",
    },
];

/// Look up the routing entry for a binary + subcommand.
///
/// Returns `Some(route)` if the binary is in the table AND the subcommand matches
/// the entry's filter. Returns `None` if unrecognised or subcommand not in `Only` list.
///
/// The HashMap is built once per process (OnceLock). Each binary maps to the index of
/// its `Route` in `ROUTES`. Multiple binaries from the same entry (e.g., `rg`/`grep`)
/// both point to the same index.
pub fn lookup(binary: &str, sub: &str) -> Option<&'static Route> {
    static MAP: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    let map = MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for (i, route) in ROUTES.iter().enumerate() {
            for &bin in route.binaries {
                m.entry(bin).or_insert(i);
            }
        }
        m
    });

    let idx = *map.get(binary)?;
    let route = &ROUTES[idx];

    let matches = match route.subcmds {
        Subcmds::Any => true,
        Subcmds::Only(subs) => subs.contains(&sub),
    };

    if matches {
        Some(route)
    } else {
        None
    }
}

/// A rule mapping a shell command pattern to its RTK equivalent.
struct RtkRule {
    rtk_cmd: &'static str,
    category: &'static str,
    savings_pct: f64,
    subcmd_savings: &'static [(&'static str, f64)],
    subcmd_status: &'static [(&'static str, super::report::RtkStatus)],
}

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

// Patterns ordered to match RTK_RULES indices exactly.
const PATTERNS: &[&str] = &[
    r"^git\s+(status|log|diff|show|add|commit|push|pull|branch|fetch|stash|worktree)",
    r"^gh\s+(pr|issue|run|repo|api)",
    r"^cargo\s+(build|test|clippy|check|fmt)",
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
    // Python/Go tooling (added with Python & Go support)
    r"^pytest(\s|$)",
    r"^go\s+(test|build|vet)(\s|$)",
    r"^ruff\s+(check|format)(\s|$)",
    r"^(pip|pip3)\s+(list|outdated|install|show)(\s|$)",
    r"^golangci-lint(\s|$)",
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
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk gh",
        category: "GitHub",
        savings_pct: 82.0,
        subcmd_savings: &[("pr", 87.0), ("run", 82.0), ("issue", 80.0)],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk cargo",
        category: "Cargo",
        savings_pct: 80.0,
        subcmd_savings: &[("test", 90.0), ("check", 80.0)],
        subcmd_status: &[("fmt", super::report::RtkStatus::Passthrough)],
    },
    RtkRule {
        rtk_cmd: "rtk pnpm",
        category: "PackageManager",
        savings_pct: 80.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk npm",
        category: "PackageManager",
        savings_pct: 70.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk npx",
        category: "PackageManager",
        savings_pct: 70.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk read",
        category: "Files",
        savings_pct: 60.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk grep",
        category: "Files",
        savings_pct: 75.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk ls",
        category: "Files",
        savings_pct: 65.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk find",
        category: "Files",
        savings_pct: 70.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk tsc",
        category: "Build",
        savings_pct: 83.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk lint",
        category: "Build",
        savings_pct: 84.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk prettier",
        category: "Build",
        savings_pct: 70.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk next",
        category: "Build",
        savings_pct: 87.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk vitest",
        category: "Tests",
        savings_pct: 99.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk playwright",
        category: "Tests",
        savings_pct: 94.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk prisma",
        category: "Build",
        savings_pct: 88.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk docker",
        category: "Infra",
        savings_pct: 85.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk kubectl",
        category: "Infra",
        savings_pct: 85.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk curl",
        category: "Network",
        savings_pct: 70.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk wget",
        category: "Network",
        savings_pct: 65.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    // Python/Go tooling (added with Python & Go support)
    RtkRule {
        rtk_cmd: "rtk pytest",
        category: "Tests",
        savings_pct: 90.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk go",
        category: "Build",
        savings_pct: 85.0,
        subcmd_savings: &[("test", 90.0)],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk ruff",
        category: "Build",
        savings_pct: 80.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk pip",
        category: "PackageManager",
        savings_pct: 75.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk golangci-lint",
        category: "Build",
        savings_pct: 85.0,
        subcmd_savings: &[],
        subcmd_status: &[],
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

    // --- Tests for commands added in Python/Go support (must be in both ROUTES and PATTERNS) ---

    #[test]
    fn test_classify_pytest_bare() {
        match classify_command("pytest tests/") {
            Classification::Supported { rtk_equivalent, .. } => {
                assert_eq!(rtk_equivalent, "rtk pytest")
            }
            other => panic!("pytest should be Supported, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_pytest_flags() {
        match classify_command("pytest -x tests/unit") {
            Classification::Supported { rtk_equivalent, .. } => {
                assert_eq!(rtk_equivalent, "rtk pytest")
            }
            other => panic!("pytest -x should be Supported, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_go_test() {
        match classify_command("go test ./...") {
            Classification::Supported { rtk_equivalent, .. } => {
                assert_eq!(rtk_equivalent, "rtk go")
            }
            other => panic!("go test should be Supported, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_go_build() {
        match classify_command("go build ./...") {
            Classification::Supported { rtk_equivalent, .. } => {
                assert_eq!(rtk_equivalent, "rtk go")
            }
            other => panic!("go build should be Supported, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_go_vet() {
        match classify_command("go vet ./...") {
            Classification::Supported { rtk_equivalent, .. } => {
                assert_eq!(rtk_equivalent, "rtk go")
            }
            other => panic!("go vet should be Supported, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_go_unsupported_subcommand_not_matched() {
        // go mod tidy is not in the Only list; should not be classified as rtk go
        match classify_command("go mod tidy") {
            Classification::Unsupported { .. } | Classification::Ignored => {}
            Classification::Supported { rtk_equivalent, .. } => {
                panic!("go mod should not match, but got rtk_equivalent={rtk_equivalent}")
            }
        }
    }

    #[test]
    fn test_classify_ruff_check() {
        match classify_command("ruff check src/") {
            Classification::Supported { rtk_equivalent, .. } => {
                assert_eq!(rtk_equivalent, "rtk ruff")
            }
            other => panic!("ruff check should be Supported, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_ruff_format() {
        match classify_command("ruff format src/") {
            Classification::Supported { rtk_equivalent, .. } => {
                assert_eq!(rtk_equivalent, "rtk ruff")
            }
            other => panic!("ruff format should be Supported, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_pip_list() {
        match classify_command("pip list") {
            Classification::Supported { rtk_equivalent, .. } => {
                assert_eq!(rtk_equivalent, "rtk pip")
            }
            other => panic!("pip list should be Supported, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_pip_install() {
        match classify_command("pip install requests") {
            Classification::Supported { rtk_equivalent, .. } => {
                assert_eq!(rtk_equivalent, "rtk pip")
            }
            other => panic!("pip install should be Supported, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_pip3_list() {
        match classify_command("pip3 list") {
            Classification::Supported { rtk_equivalent, .. } => {
                assert_eq!(rtk_equivalent, "rtk pip")
            }
            other => panic!("pip3 list should be Supported, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_golangci_lint() {
        match classify_command("golangci-lint run ./...") {
            Classification::Supported { rtk_equivalent, .. } => {
                assert_eq!(rtk_equivalent, "rtk golangci-lint")
            }
            other => panic!("golangci-lint should be Supported, got {other:?}"),
        }
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

    // --- Route lookup tests ---

    #[test]
    fn test_lookup_direct_route() {
        let r = lookup("git", "status").unwrap();
        assert_eq!(r.rtk_cmd, "git");
    }

    #[test]
    fn test_lookup_git_unknown_subcommand_returns_none() {
        assert!(lookup("git", "rebase").is_none());
        assert!(lookup("git", "bisect").is_none());
    }

    #[test]
    fn test_lookup_rename_rg_to_grep() {
        let r = lookup("rg", "").unwrap();
        assert_eq!(r.rtk_cmd, "grep");
    }

    #[test]
    fn test_lookup_rename_grep_to_grep() {
        let r = lookup("grep", "-r").unwrap();
        assert_eq!(r.rtk_cmd, "grep");
    }

    #[test]
    fn test_lookup_rename_eslint_to_lint() {
        let r = lookup("eslint", "src/").unwrap();
        assert_eq!(r.rtk_cmd, "lint");
    }

    #[test]
    fn test_lookup_any_subcommand() {
        let r = lookup("ls", "-la").unwrap();
        assert_eq!(r.rtk_cmd, "ls");
        let r2 = lookup("ls", "").unwrap();
        assert_eq!(r2.rtk_cmd, "ls");
    }

    #[test]
    fn test_lookup_unknown_binary_returns_none() {
        assert!(lookup("unknownbinary99", "").is_none());
        // These stay as complex Rust match arms, not in ROUTES
        assert!(lookup("vitest", "").is_none());
        assert!(lookup("pnpm", "list").is_none());
        assert!(lookup("npx", "tsc").is_none());
        assert!(lookup("uv", "pip").is_none());
    }

    #[test]
    fn test_lookup_docker_subcommand_filter() {
        assert!(lookup("docker", "ps").is_some());
        assert!(lookup("docker", "images").is_some());
        assert!(lookup("docker", "build").is_none());
        assert!(lookup("docker", "run").is_none());
    }

    #[test]
    fn test_lookup_cargo_subcommand_filter() {
        assert!(lookup("cargo", "test").is_some());
        assert!(lookup("cargo", "clippy").is_some());
        assert!(lookup("cargo", "publish").is_none());
    }

    #[test]
    fn test_no_duplicate_binaries_in_routes() {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for route in ROUTES {
            for &bin in route.binaries {
                assert!(
                    seen.insert(bin),
                    "Binary '{bin}' appears in multiple ROUTES entries"
                );
            }
        }
    }

    #[test]
    fn test_lookup_is_o1_consistent() {
        let r1 = lookup("git", "status");
        let r2 = lookup("git", "status");
        assert_eq!(r1.map(|r| r.rtk_cmd), r2.map(|r| r.rtk_cmd));
    }
}
