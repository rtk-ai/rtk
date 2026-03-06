//! MySQL client output compression.
//!
//! Detects table (box-drawing) and vertical (`\G`) formats, strips borders/padding,
//! and produces compact tab-separated or key=value output.

use crate::tracking;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

const MAX_TABLE_ROWS: usize = 30;
const MAX_VERTICAL_RECORDS: usize = 20;

lazy_static! {
    static ref TABLE_BORDER: Regex = Regex::new(r"^\+[-+]+\+$").unwrap();
    static ref ROW_COUNT: Regex = Regex::new(r"^\d+ rows? in set \(\d+\.\d+ sec\)$").unwrap();
    static ref EMPTY_SET: Regex = Regex::new(r"^Empty set \(\d+\.\d+ sec\)$").unwrap();
    static ref VERTICAL_HEADER: Regex = Regex::new(r"^\*{3,}\s+(\d+)\.\s+row\s+\*{3,}$").unwrap();
    static ref QUERY_OK: Regex = Regex::new(r"^Query OK,").unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = std::process::Command::new("mysql");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: mysql {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run mysql (is MySQL client installed?)")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let exit_code = output.status.code().unwrap_or(1);

    if !stderr.is_empty() {
        eprint!("{}", stderr);
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    let filtered = filter_mysql_output(&stdout);

    if let Some(hint) = crate::tee::tee_and_hint(&stdout, "mysql", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("mysql {}", args.join(" ")),
        &format!("rtk mysql {}", args.join(" ")),
        &stdout,
        &filtered,
    );

    Ok(())
}

fn filter_mysql_output(output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }

    if is_vertical_format(output) {
        filter_vertical(output)
    } else if is_table_format(output) {
        filter_table(output)
    } else {
        // Passthrough: Query OK, Empty set, ERROR, etc.
        output.to_string()
    }
}

fn is_table_format(output: &str) -> bool {
    output
        .lines()
        .any(|line| TABLE_BORDER.is_match(line.trim()))
}

fn is_vertical_format(output: &str) -> bool {
    output
        .lines()
        .any(|line| VERTICAL_HEADER.is_match(line.trim()))
}

/// Filter MySQL table format:
/// - Strip box-drawing border lines (+----+-----+)
/// - Strip "N rows in set" footer
/// - Trim column padding from `| val1 | val2 |`
/// - Output tab-separated
fn filter_table(output: &str) -> String {
    let mut result = Vec::new();
    let mut data_rows = 0;
    let mut total_rows = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip border lines
        if TABLE_BORDER.is_match(trimmed) {
            continue;
        }

        // Skip row count footer
        if ROW_COUNT.is_match(trimmed) {
            continue;
        }

        // Skip empty set
        if EMPTY_SET.is_match(trimmed) {
            continue;
        }

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        // Data/header rows: | val1 | val2 |
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            total_rows += 1;
            // First row is header
            if total_rows > 1 {
                data_rows += 1;
            }

            if data_rows <= MAX_TABLE_ROWS || total_rows == 1 {
                // Split on |, drop empty first/last from leading/trailing |
                let cols: Vec<&str> = trimmed
                    .split('|')
                    .map(|c| c.trim())
                    .filter(|c| !c.is_empty())
                    .collect();
                result.push(cols.join("\t"));
            }
        } else {
            // Passthrough lines (Query OK, ERROR, etc.)
            result.push(trimmed.to_string());
        }
    }

    if data_rows > MAX_TABLE_ROWS {
        result.push(format!("... +{} more rows", data_rows - MAX_TABLE_ROWS));
    }

    result.join("\n")
}

/// Filter MySQL vertical format (`\G`):
/// Convert `*** N. row ***` blocks to one-liner key=val format
fn filter_vertical(output: &str) -> String {
    let mut result = Vec::new();
    let mut current_pairs: Vec<String> = Vec::new();
    let mut current_record: Option<String> = None;
    let mut record_count = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip row count footer
        if ROW_COUNT.is_match(trimmed) {
            continue;
        }

        if let Some(caps) = VERTICAL_HEADER.captures(trimmed) {
            // Flush previous record
            if let Some(rec) = current_record.take() {
                if record_count <= MAX_VERTICAL_RECORDS {
                    result.push(format!("{} {}", rec, current_pairs.join(" ")));
                }
                current_pairs.clear();
            }
            record_count += 1;
            current_record = Some(format!("[{}]", &caps[1]));
        } else if trimmed.contains(':') && current_record.is_some() {
            // key: value line
            let parts: Vec<&str> = trimmed.splitn(2, ':').collect();
            if parts.len() == 2 {
                let key = parts[0].trim();
                let val = parts[1].trim();
                current_pairs.push(format!("{}={}", key, val));
            }
        } else if trimmed.is_empty() {
            continue;
        } else if current_record.is_none() {
            // Non-record line before any record
            result.push(trimmed.to_string());
        }
    }

    // Flush last record
    if let Some(rec) = current_record.take() {
        if record_count <= MAX_VERTICAL_RECORDS {
            result.push(format!("{} {}", rec, current_pairs.join(" ")));
        }
    }

    if record_count > MAX_VERTICAL_RECORDS {
        result.push(format!(
            "... +{} more records",
            record_count - MAX_VERTICAL_RECORDS
        ));
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Format detection ---

    #[test]
    fn test_is_table_format_detects_border() {
        let input =
            "+----+-------+\n| id | name  |\n+----+-------+\n|  1 | alice |\n+----+-------+\n";
        assert!(is_table_format(input));
    }

    #[test]
    fn test_is_table_format_rejects_plain() {
        assert!(!is_table_format("Query OK, 1 row affected\n"));
        assert!(!is_table_format("Empty set (0.00 sec)\n"));
    }

    #[test]
    fn test_is_vertical_format_detects_header() {
        let input =
            "*************************** 1. row ***************************\nid: 1\nname: alice\n";
        assert!(is_vertical_format(input));
    }

    #[test]
    fn test_is_vertical_format_rejects_table() {
        let input =
            "+----+-------+\n| id | name  |\n+----+-------+\n|  1 | alice |\n+----+-------+\n";
        assert!(!is_vertical_format(input));
    }

    // --- Table filter ---

    #[test]
    fn test_filter_table_basic() {
        let input = "+----+-------+-----------+\n| id | name  | email     |\n+----+-------+-----------+\n|  1 | alice | a@b.com   |\n|  2 | bob   | b@b.com   |\n+----+-------+-----------+\n2 rows in set (0.00 sec)\n";
        let result = filter_table(input);
        assert!(result.contains("id\tname\temail"));
        assert!(result.contains("1\talice\ta@b.com"));
        assert!(result.contains("2\tbob\tb@b.com"));
        assert!(!result.contains("+----"));
        assert!(!result.contains("rows in set"));
    }

    #[test]
    fn test_filter_table_strips_borders() {
        let input = "+----+\n| id |\n+----+\n|  1 |\n+----+\n1 row in set (0.00 sec)\n";
        let result = filter_table(input);
        assert!(!result.contains('+'));
        assert!(!result.contains("1 row in set"));
    }

    #[test]
    fn test_filter_table_empty_set() {
        let input = "Empty set (0.00 sec)\n";
        let result = filter_table(input);
        assert!(result.is_empty());
    }

    #[test]
    fn test_filter_table_overflow() {
        let mut lines = vec![
            "+----+------+".to_string(),
            "| id | val  |".to_string(),
            "+----+------+".to_string(),
        ];
        for i in 1..=40 {
            lines.push(format!("| {:>2} | row{} |", i, i));
        }
        lines.push("+----+------+".to_string());
        lines.push("40 rows in set (0.01 sec)".to_string());
        let input = lines.join("\n");

        let result = filter_table(&input);
        assert!(result.contains("... +10 more rows"));
        let result_lines: Vec<&str> = result.lines().collect();
        assert_eq!(result_lines.len(), 32); // 1 header + 30 data + 1 overflow
    }

    // --- Vertical filter ---

    #[test]
    fn test_filter_vertical_basic() {
        let input = "*************************** 1. row ***************************\nid: 1\nname: alice\n*************************** 2. row ***************************\nid: 2\nname: bob\n2 rows in set (0.00 sec)\n";
        let result = filter_vertical(input);
        assert!(result.contains("[1] id=1 name=alice"));
        assert!(result.contains("[2] id=2 name=bob"));
        assert!(!result.contains("***"));
        assert!(!result.contains("rows in set"));
    }

    #[test]
    fn test_filter_vertical_colon_in_value() {
        let input = "*************************** 1. row ***************************\nhost: localhost:3306\nuser: root\n";
        let result = filter_vertical(input);
        assert!(result.contains("host=localhost:3306"));
    }

    #[test]
    fn test_filter_vertical_overflow() {
        let mut lines = Vec::new();
        for i in 1..=25 {
            lines.push(format!(
                "*************************** {}. row ***************************",
                i
            ));
            lines.push(format!("id: {}", i));
            lines.push(format!("name: user{}", i));
        }
        let input = lines.join("\n");

        let result = filter_vertical(&input);
        assert!(result.contains("... +5 more records"));
    }

    // --- Routing ---

    #[test]
    fn test_routing_table() {
        let input = "+----+-------+\n| id | name  |\n+----+-------+\n|  1 | alice |\n+----+-------+\n1 row in set (0.00 sec)\n";
        let result = filter_mysql_output(input);
        assert!(result.contains("id\tname"));
        assert!(!result.contains("+----"));
    }

    #[test]
    fn test_routing_vertical() {
        let input =
            "*************************** 1. row ***************************\nid: 1\nname: alice\n";
        let result = filter_mysql_output(input);
        assert!(result.contains("[1]"));
        assert!(result.contains("id=1"));
    }

    #[test]
    fn test_routing_passthrough() {
        let input = "Query OK, 1 row affected (0.01 sec)\n";
        let result = filter_mysql_output(input);
        assert_eq!(result, input);
    }

    // --- Empty ---

    #[test]
    fn test_filter_empty() {
        let result = filter_mysql_output("");
        assert!(result.is_empty());
    }

    // --- Token savings ---

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_table_token_savings() {
        let input = "+----+-------------------+--------------------------------+-----------+---------------------+---------------------+-----------+\n| id | username          | email                          | status    | created_at          | updated_at          | role      |\n+----+-------------------+--------------------------------+-----------+---------------------+---------------------+-----------+\n|  1 | alice_smith       | alice@example.com              | active    | 2024-01-01 09:00:00 | 2024-01-15 14:30:00 | admin     |\n|  2 | bob_jones         | bob.jones@company.org          | active    | 2024-01-02 10:15:00 | 2024-01-16 09:00:00 | user      |\n|  3 | carol_white       | carol.white@example.com        | inactive  | 2024-01-03 11:30:00 | 2024-01-17 11:00:00 | user      |\n|  4 | dave_brown        | dave@business.net              | active    | 2024-01-04 08:45:00 | 2024-01-18 16:00:00 | moderator |\n|  5 | eve_davis         | eve.davis@example.com          | active    | 2024-01-05 13:00:00 | 2024-01-19 10:30:00 | user      |\n+----+-------------------+--------------------------------+-----------+---------------------+---------------------+-----------+\n5 rows in set (0.00 sec)\n";
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
    fn test_vertical_token_savings() {
        let input = "*************************** 1. row ***************************\nid: 1\nusername: alice_smith\nemail: alice@example.com\nstatus: active\nrole: admin\ncreated_at: 2024-01-01 09:00:00\nupdated_at: 2024-01-15 14:30:00\nlast_login: 2024-02-01 08:00:00\nlogin_count: 42\npreferences: {\"theme\":\"dark\",\"notifications\":true}\n*************************** 2. row ***************************\nid: 2\nusername: bob_jones\nemail: bob.jones@company.org\nstatus: active\nrole: user\ncreated_at: 2024-01-02 10:15:00\nupdated_at: 2024-01-16 09:00:00\nlast_login: 2024-02-02 09:30:00\nlogin_count: 17\npreferences: {\"theme\":\"light\",\"notifications\":false}\n2 rows in set (0.00 sec)\n";
        let result = filter_vertical(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 50.0,
            "Vertical filter: expected >=50% savings, got {:.1}%",
            savings
        );
    }
}
