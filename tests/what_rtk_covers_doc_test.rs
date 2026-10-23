#![cfg(unix)]
//! Verifies that commands documented in `what-rtk-covers.md` are actually
//! rewritten by RTK's hook rules (#4695).
//!
//! Keeps documentation honest and prevents drift when rewrite rules change.

mod common;

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
