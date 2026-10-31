#![cfg(unix)]
//! Verifies that commands documented in `what-rtk-covers.md` are actually
//! rewritten by RTK's hook rules (#4695).
//!
//! Keeps documentation honest and prevents drift when rewrite rules change.

mod common;

#[allow(dead_code)]
mod report {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RtkStatus {
        Existing,
        Passthrough,
        NotSupported,
    }
}

#[allow(dead_code)]
#[path = "../src/discover/rules.rs"]
mod rules;

const DOC: &str = include_str!("../docs/guide/resources/what-rtk-covers.md");

/// Runs `rtk rewrite "<cmd>"`. Returns Some(rewritten) if rewritten (exit 0 or 3),
/// or None if left alone (exit 1).
fn rewrite(cmd: &str) -> Option<String> {
    let out = common::rtk_command()
        .env("LC_ALL", "C")
        .args(["rewrite", cmd])
        .output()
        .expect("rtk rewrite");
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match out.status.code() {
        Some(0) | Some(3) if !text.is_empty() => Some(text),
        _ => None,
    }
}

/// Instantiates realistic invocation arguments for commands documented
/// as bare names or placeholders.
fn sample_invocation(documented: &str) -> Option<String> {
    // Skip non-command markdown tables (e.g. global flags table)
    if documented.starts_with('-') {
        return None;
    }

    let cmd = match documented {
        "cat <file>" => "cat README.md".to_string(),
        "head <file>" => "head README.md".to_string(),
        "tail -n <N> <file>" => "tail -n 10 README.md".to_string(),
        "find" => "find .".to_string(),
        "grep" => "grep foo src".to_string(),
        "ast-grep" => "ast-grep run -p foo".to_string(),
        "diff" => "diff a b".to_string(),
        "aws" => "aws sts get-caller-identity".to_string(),
        "curl" => "curl https://example.com".to_string(),
        other => other.to_string(),
    };
    Some(cmd)
}

#[test]
fn every_documented_command_is_rewritten_by_hook() {
    let mut in_table = false;
    let mut table_command_count = 0;

    for line in DOC.lines() {
        if line.starts_with("| Command |") {
            in_table = true;
            continue;
        }

        // Section header or non-table line ends the current table
        if line.starts_with("## ") || (!line.starts_with('|') && in_table) {
            in_table = false;
            continue;
        }

        if in_table && line.starts_with('|') {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            if cells.len() >= 3 {
                let cmd_cell = cells[1];
                // Extract all backticked snippets from the Command cell
                let mut start = 0;
                while let Some(open) = cmd_cell[start..].find('`') {
                    let open_idx = start + open + 1;
                    if let Some(close) = cmd_cell[open_idx..].find('`') {
                        let backticked = &cmd_cell[open_idx..open_idx + close];
                        if let Some(sample) = sample_invocation(backticked) {
                            table_command_count += 1;
                            let res = rewrite(&sample);
                            assert!(
                                res.is_some(),
                                "documented command `{backticked}` (tested as `{sample}`) is not rewritten by RTK hook"
                            );
                        }
                        start = open_idx + close + 1;
                    } else {
                        break;
                    }
                }
            }
        }
    }

    // Ensure we actually checked a substantial number of commands and didn't accidentally parse nothing
    assert!(
        table_command_count >= 40,
        "expected at least 40 documented commands to be tested, found {table_command_count}"
    );
}

/// Extracts the base tool name from each backticked command in the markdown tables.
fn documented_tools() -> std::collections::BTreeSet<String> {
    let mut tools = std::collections::BTreeSet::new();
    let mut in_table = false;

    for line in DOC.lines() {
        if line.starts_with("| Command |") {
            in_table = true;
            continue;
        }

        if line.starts_with("## ") || (!line.starts_with('|') && in_table) {
            in_table = false;
            continue;
        }

        if in_table && line.starts_with('|') {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            if cells.len() >= 3 {
                let cmd_cell = cells[1];
                let mut start = 0;
                while let Some(open) = cmd_cell[start..].find('`') {
                    let open_idx = start + open + 1;
                    if let Some(close) = cmd_cell[open_idx..].find('`') {
                        let backticked = &cmd_cell[open_idx..open_idx + close];
                        if let Some(first_word) = backticked
                            .split_whitespace()
                            .next()
                            .filter(|w| !w.starts_with('-'))
                        {
                            tools.insert(first_word.to_string());
                        }
                        start = open_idx + close + 1;
                    } else {
                        break;
                    }
                }
            }
        }
    }
    tools
}

/// Tools with rewrite rules in `src/discover/rules.rs` that are not yet
/// documented in `what-rtk-covers.md`.
///
/// NOTE: This completeness check is tool-level (derived from `rtk_cmd`), so a
/// new subcommand on an already-documented tool is not caught.
const UNDOCUMENTED: &[&str] = &[
    "ansible-playbook",
    "brew",
    "bun",
    "bundle",
    "bunx",
    "composer",
    "ctest",
    "deno",
    "df",
    "du",
    "ecs",
    "fail2ban-client",
    "gcloud",
    "glab",
    "gradlew",
    "hadolint",
    "iptables",
    "lint",
    "liquibase",
    "markdownlint",
    "mix",
    "mvn",
    "mvnd",
    "npm",
    "npx",
    "oc",
    "paratest",
    "pest",
    "php",
    "phpstan",
    "phpt",
    "phpunit",
    "ping",
    "pint",
    "pio",
    "poetry",
    "pre-commit",
    "prettier",
    "ps",
    "pulumi",
    "quarto",
    "read",
    "rg",
    "rsync",
    "sbt",
    "shellcheck",
    "shopify",
    "sops",
    "sqlfluff",
    "swift",
    "systemctl",
    "tofu",
    "tree",
    "trunk",
    "uv",
    "wget",
    "yamllint",
];

#[test]
fn every_rule_tool_is_documented_or_allowlisted() {
    let documented = documented_tools();
    let allowlist: std::collections::BTreeSet<&str> = UNDOCUMENTED.iter().copied().collect();

    // Derive distinct tool names from RULES (rtk_cmd, e.g. "rtk git" -> "git")
    let mut rule_tools = std::collections::BTreeSet::new();
    for rule in rules::RULES {
        let tool = rule
            .rtk_cmd
            .split_whitespace()
            .nth(1)
            .unwrap_or(rule.rtk_cmd);
        rule_tools.insert(tool);
    }

    // Fail on stale allowlist entries: any tool in allowlist that is now documented
    // or no longer exists in RULES.
    for &tool in &allowlist {
        assert!(
            rule_tools.contains(tool),
            "stale UNDOCUMENTED entry `{tool}`: tool no longer exists in RULES"
        );
        assert!(
            !documented.contains(tool),
            "stale UNDOCUMENTED entry `{tool}`: tool is now documented in what-rtk-covers.md, remove it from UNDOCUMENTED"
        );
    }

    // Every tool in RULES must be named in the doc or in the explicit UNDOCUMENTED allowlist.
    for &tool in &rule_tools {
        let is_doc = documented.contains(tool);
        let is_allow = allowlist.contains(tool);
        assert!(
            is_doc || is_allow,
            "undocumented tool `{tool}` found in RULES: document it in what-rtk-covers.md or add to UNDOCUMENTED allowlist"
        );
    }
}
