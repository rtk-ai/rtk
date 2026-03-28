//! PostgreSQL client (psql) output compression.
//!
//! Detects table and expanded display formats, strips borders/padding,
//! and produces compact tab-separated or key=value output.

use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

const MAX_TABLE_ROWS: usize = 30;
const MAX_EXPANDED_RECORDS: usize = 20;

lazy_static! {
    static ref EXPANDED_RECORD: Regex = Regex::new(r"-\[ RECORD \d+ \]-").unwrap();
    static ref SEPARATOR: Regex = Regex::new(r"^[-+]+$").unwrap();
    static ref ROW_COUNT: Regex = Regex::new(r"^\(\d+ rows?\)$").unwrap();
    static ref RECORD_HEADER: Regex = Regex::new(r"^-\[ RECORD (\d+) \]-").unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("psql");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: psql {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run psql (is PostgreSQL client installed?)")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let exit_code = exit_code_from_output(&output, "psql");

    if !stderr.is_empty() {
        eprint!("{}", stderr);
    }

    if exit_code != 0 {
        return Ok(exit_code);
    }

    let filtered = filter_psql_output(&stdout);

    if let Some(hint) = crate::core::tee::tee_and_hint(&stdout, "psql", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("psql {}", args.join(" ")),
        &format!("rtk psql {}", args.join(" ")),
        &stdout,
        &filtered,
    );

    Ok(0)
}

fn filter_psql_output(output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }

    if is_expanded_format(output) {
        filter_expanded(output)
    } else if is_table_format(output) {
        filter_table(output)
    } else {
        // Passthrough: COPY results, notices, etc.
        output.to_string()
    }
}

fn is_table_format(output: &str) -> bool {
    output.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.contains("-+-") || trimmed.contains("---+---")
    })
}

fn is_expanded_format(output: &str) -> bool {
    EXPANDED_RECORD.is_match(output)
}

/// Filter psql table format:
/// - Strip separator lines (----+----)
/// - Strip (N rows) footer
/// - Trim column padding
/// - Output tab-separated
fn filter_table(output: &str) -> String {
    let mut result = Vec::new();
    let mut data_rows = 0;
    let mut total_rows = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip separator lines
        if SEPARATOR.is_match(trimmed) {
            continue;
        }

        // Skip row count footer
        if ROW_COUNT.is_match(trimmed) {
            continue;
        }

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        // This is a data or header row with | delimiters
        if trimmed.contains('|') {
            total_rows += 1;
            // First row is header, don't count it as data
            if total_rows > 1 {
                data_rows += 1;
            }

            if data_rows <= MAX_TABLE_ROWS || total_rows == 1 {
                let cols: Vec<&str> = trimmed.split('|').map(|c| c.trim()).collect();
                result.push(cols.join("\t"));
            }
        } else {
            // Non-table line (e.g., command output like SET, NOTICE)
            result.push(trimmed.to_string());
        }
    }

    if data_rows > MAX_TABLE_ROWS {
        result.push(format!("... +{} more rows", data_rows - MAX_TABLE_ROWS));
    }

    result.join("\n")
}

/// Filter psql expanded format:
/// Convert -[ RECORD N ]- blocks to one-liner key=val format
fn filter_expanded(output: &str) -> String {
    let mut result = Vec::new();
    let mut current_pairs: Vec<String> = Vec::new();
    let mut current_record: Option<String> = None;
    let mut record_count = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        if ROW_COUNT.is_match(trimmed) {
            continue;
        }

        if let Some(caps) = RECORD_HEADER.captures(trimmed) {
            // Flush previous record
            if let Some(rec) = current_record.take() {
                if record_count <= MAX_EXPANDED_RECORDS {
                    result.push(format!("{} {}", rec, current_pairs.join(" ")));
                }
                current_pairs.clear();
            }
            record_count += 1;
            current_record = Some(format!("[{}]", &caps[1]));
        } else if trimmed.contains('|') && current_record.is_some() {
            // key | value line
            let parts: Vec<&str> = trimmed.splitn(2, '|').collect();
            if parts.len() == 2 {
                let key = parts[0].trim();
                let val = parts[1].trim();
                current_pairs.push(format!("{}={}", key, val));
            }
        } else if trimmed.is_empty() {
            continue;
        } else if current_record.is_none() {
            // Non-record line before any record (notices, etc.)
            result.push(trimmed.to_string());
        }
    }

    // Flush last record
    if let Some(rec) = current_record.take() {
        if record_count <= MAX_EXPANDED_RECORDS {
            result.push(format!("{} {}", rec, current_pairs.join(" ")));
        }
    }

    if record_count > MAX_EXPANDED_RECORDS {
        result.push(format!(
            "... +{} more records",
            record_count - MAX_EXPANDED_RECORDS
        ));
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_table_format() {
        let input = " id | username    | email             | status\n----+-------------+-------------------+--------\n  1 | alice_smith  | alice@example.com | active\n  2 | bob_jones   | bob@example.com   | active\n(2 rows)\n";
        let result = filter_table(input);
        assert!(result.contains("id\tusername\temail\tstatus"));
        assert!(result.contains("alice_smith\talice@example.com"));
        assert!(!result.contains("---+---"));
        assert!(!result.contains("(2 rows)"));
    }

    #[test]
    fn test_snapshot_expanded_format() {
        let input = "-[ RECORD 1 ]------\nid       | 1\nusername | alice_smith\nemail    | alice@example.com\n-[ RECORD 2 ]------\nid       | 2\nusername | bob_jones\nemail    | bob@example.com\n(2 rows)\n";
        let result = filter_expanded(input);
        assert!(result.contains("[1] id=1 username=alice_smith"));
        assert!(result.contains("[2] id=2 username=bob_jones"));
        assert!(!result.contains("-[ RECORD"));
        assert!(!result.contains("(2 rows)"));
    }

    #[test]
    fn test_is_table_format_detects_separator() {
        let input = " id | name\n----+------\n  1 | foo\n(1 row)\n";
        assert!(is_table_format(input));
    }

    #[test]
    fn test_is_table_format_rejects_plain() {
        assert!(!is_table_format("COPY 5\n"));
        assert!(!is_table_format("SET\n"));
    }

    #[test]
    fn test_is_expanded_format_detects_records() {
        let input = "-[ RECORD 1 ]----\nid | 1\nname | foo\n";
        assert!(is_expanded_format(input));
    }

    #[test]
    fn test_is_expanded_format_rejects_table() {
        let input = " id | name\n----+------\n  1 | foo\n";
        assert!(!is_expanded_format(input));
    }

    #[test]
    fn test_filter_table_basic() {
        let input = " id | name  | email\n----+-------+---------\n  1 | alice | a@b.com\n  2 | bob   | b@b.com\n(2 rows)\n";
        let result = filter_table(input);
        assert!(result.contains("id\tname\temail"));
        assert!(result.contains("1\talice\ta@b.com"));
        assert!(result.contains("2\tbob\tb@b.com"));
        assert!(!result.contains("----"));
        assert!(!result.contains("(2 rows)"));
    }

    #[test]
    fn test_filter_table_overflow() {
        let mut lines = vec![" id | val".to_string(), "----+-----".to_string()];
        for i in 1..=40 {
            lines.push(format!("  {} | row{}", i, i));
        }
        lines.push("(40 rows)".to_string());
        let input = lines.join("\n");

        let result = filter_table(&input);
        assert!(result.contains("... +10 more rows"));
        // Header + 30 data rows + overflow line
        let result_lines: Vec<&str> = result.lines().collect();
        assert_eq!(result_lines.len(), 32); // 1 header + 30 data + 1 overflow
    }

    #[test]
    fn test_filter_table_empty() {
        let result = filter_psql_output("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_filter_expanded_basic() {
        let input = "\
-[ RECORD 1 ]----
id   | 1
name | alice
-[ RECORD 2 ]----
id   | 2
name | bob
";
        let result = filter_expanded(input);
        assert!(result.contains("[1] id=1 name=alice"));
        assert!(result.contains("[2] id=2 name=bob"));
    }

    #[test]
    fn test_filter_expanded_overflow() {
        let mut lines = Vec::new();
        for i in 1..=25 {
            lines.push(format!("-[ RECORD {} ]----", i));
            lines.push(format!("id   | {}", i));
            lines.push(format!("name | user{}", i));
        }
        let input = lines.join("\n");

        let result = filter_expanded(&input);
        assert!(result.contains("... +5 more records"));
    }

    #[test]
    fn test_filter_psql_passthrough() {
        let input = "COPY 5\n";
        let result = filter_psql_output(input);
        assert_eq!(result, "COPY 5\n");
    }

    #[test]
    fn test_filter_psql_routes_to_table() {
        let input = " id | name\n----+------\n  1 | foo\n(1 row)\n";
        let result = filter_psql_output(input);
        assert!(result.contains("id\tname"));
        assert!(!result.contains("----"));
    }

    #[test]
    fn test_filter_psql_routes_to_expanded() {
        let input = "-[ RECORD 1 ]----\nid | 1\nname | foo\n";
        let result = filter_psql_output(input);
        assert!(result.contains("[1]"));
        assert!(result.contains("id=1"));
    }

    #[test]
    fn test_filter_table_strips_row_count() {
        let input = " c\n---\n 1\n(1 row)\n";
        let result = filter_table(input);
        assert!(!result.contains("(1 row)"));
    }

    #[test]
    fn test_filter_expanded_strips_row_count() {
        let input = "-[ RECORD 1 ]----\nid | 1\n(1 row)\n";
        let result = filter_expanded(input);
        assert!(!result.contains("(1 row)"));
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_table_token_savings() {
        let input = " id | username          | email                          | status    | created_at          | updated_at          | role\n-------------+-------------------+--------------------------------+-----------+---------------------+---------------------+------------\n           1 | alice_smith       | alice@example.com              | active    | 2024-01-01 09:00:00 | 2024-01-15 14:30:00 | admin\n           2 | bob_jones         | bob.jones@company.org          | active    | 2024-01-02 10:15:00 | 2024-01-16 09:00:00 | user\n           3 | carol_white       | carol.white@example.com        | inactive  | 2024-01-03 11:30:00 | 2024-01-17 11:00:00 | user\n           4 | dave_brown        | dave@business.net              | active    | 2024-01-04 08:45:00 | 2024-01-18 16:00:00 | moderator\n           5 | eve_davis         | eve.davis@example.com          | active    | 2024-01-05 13:00:00 | 2024-01-19 10:30:00 | user\n(5 rows)\n";
        let result = filter_table(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "Table filter: expected >=40% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_expanded_token_savings() {
        let input = "-[ RECORD 1 ]-------------------------------\nid            | 1\nusername      | alice_smith\nemail         | alice@example.com\nstatus        | active\nrole          | admin\ncreated_at    | 2024-01-01 09:00:00\nupdated_at    | 2024-01-15 14:30:00\nlast_login    | 2024-02-01 08:00:00\nlogin_count   | 42\npreferences   | {\"theme\":\"dark\",\"notifications\":true}\n-[ RECORD 2 ]-------------------------------\nid            | 2\nusername      | bob_jones\nemail         | bob.jones@company.org\nstatus        | active\nrole          | user\ncreated_at    | 2024-01-02 10:15:00\nupdated_at    | 2024-01-16 09:00:00\nlast_login    | 2024-02-02 09:30:00\nlogin_count   | 17\npreferences   | {\"theme\":\"light\",\"notifications\":false}\n(2 rows)\n";
        let result = filter_expanded(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expanded filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }
}
