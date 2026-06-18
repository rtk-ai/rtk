//! SQLite3 CLI output compression.
//!
//! Detects table output formats (list, column, line), strips separators,
//! and produces compact tab-separated output.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

const MAX_DATA_ROWS: usize = 50;

lazy_static! {
    static ref COLUMN_SEPARATOR: Regex = Regex::new(r"^[-]+(\s+[-]+)+$").unwrap();
    static ref LINE_MODE_RE: Regex = Regex::new(r"(?m)^\s*\w+\s*=\s*.+").unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("sqlite3");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: sqlite3 {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "sqlite3",
        &args.join(" "),
        filter_sqlite_output,
        RunOptions::stdout_only()
            .tee("sqlite3")
            .early_exit_on_failure(),
    )
}

fn filter_sqlite_output(output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }

    if is_column_format(output) {
        filter_column(output)
    } else if is_line_format(output) {
        filter_line(output)
    } else if is_list_format(output) {
        filter_list(output)
    } else {
        output.to_string()
    }
}

fn is_column_format(output: &str) -> bool {
    COLUMN_SEPARATOR.is_match(output)
}

fn is_line_format(output: &str) -> bool {
    LINE_MODE_RE.is_match(output)
}

fn is_list_format(output: &str) -> bool {
    output.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && trimmed.contains('|') && !trimmed.starts_with("Error:")
    })
}

/// Filter column mode: strip separator lines, compact whitespace
fn filter_column(output: &str) -> String {
    let mut result = Vec::new();
    let mut data_rows = 0;
    let mut header_seen = false;
    let mut total_data = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        if COLUMN_SEPARATOR.is_match(trimmed) {
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        if !header_seen {
            header_seen = true;
            let cols: Vec<&str> = trimmed.split_whitespace().collect();
            result.push(cols.join("\t"));
        } else {
            total_data += 1;
            if data_rows < MAX_DATA_ROWS {
                let cols: Vec<&str> = trimmed.split_whitespace().collect();
                result.push(cols.join("\t"));
                data_rows += 1;
            }
        }
    }

    if total_data > MAX_DATA_ROWS {
        result.push(format!("... +{} more rows", total_data - MAX_DATA_ROWS));
    }

    result.join("\n")
}

/// Filter line mode: key = value, keep as-is (already compact per-record)
fn filter_line(output: &str) -> String {
    let mut result = Vec::new();
    let mut record_lines: Vec<&str> = Vec::new();
    let mut record_count = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if !record_lines.is_empty() {
                if record_count < MAX_DATA_ROWS {
                    result.push(record_lines.join(" "));
                }
                record_lines.clear();
                record_count += 1;
            }
            continue;
        }

        if LINE_MODE_RE.is_match(trimmed) {
            record_lines.push(trimmed);
        }
    }

    if !record_lines.is_empty() && record_count < MAX_DATA_ROWS {
        result.push(record_lines.join(" "));
        record_count += 1;
    }

    if record_count > MAX_DATA_ROWS {
        result.push(format!("... +{} more records", record_count - MAX_DATA_ROWS));
    }

    result.join("\n")
}

/// Filter list mode: pipe-separated → tab-separated
fn filter_list(output: &str) -> String {
    let mut result = Vec::new();
    let mut data_rows = 0;
    let mut has_header = false;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        if !has_header {
            has_header = true;
            result.push(trimmed.replace('|', "\t"));
        } else {
            if data_rows < MAX_DATA_ROWS {
                result.push(trimmed.replace('|', "\t"));
                data_rows += 1;
            }
        }
    }

    if data_rows > MAX_DATA_ROWS {
        result.push(format!("... +{} more rows", data_rows - MAX_DATA_ROWS));
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_is_column_format() {
        let input = "id  name   email\n--  -----  -----\n1   alice  a@b.com\n";
        assert!(is_column_format(input));
    }

    #[test]
    fn test_is_column_format_rejects_list() {
        assert!(!is_column_format("1|alice|a@b.com\n2|bob|b@b.com\n"));
    }

    #[test]
    fn test_is_line_format() {
        let input = "     name = alice\n    email = a@b.com\n";
        assert!(is_line_format(input));
    }

    #[test]
    fn test_is_line_format_rejects_list() {
        assert!(!is_line_format("1|alice|a@b.com\n"));
    }

    #[test]
    fn test_is_list_format() {
        assert!(is_list_format("1|alice|a@b.com\n2|bob|b@b.com\n"));
    }

    #[test]
    fn test_is_list_format_rejects_empty() {
        assert!(!is_list_format(""));
    }

    #[test]
    fn test_filter_list_basic() {
        let input = "id|name|email\n1|alice|a@b.com\n2|bob|b@b.com\n";
        let result = filter_list(input);
        assert!(result.contains("id\tname\temail"));
        assert!(result.contains("1\talice\ta@b.com"));
        assert!(result.contains("2\tbob\tb@b.com"));
    }

    #[test]
    fn test_filter_list_empty() {
        assert_eq!(filter_list(""), "");
    }

    #[test]
    fn test_filter_list_overflow() {
        let mut lines = vec!["id|val".to_string()];
        for i in 1..=60 {
            lines.push(format!("{}|row{}", i, i));
        }
        let input = lines.join("\n");
        let result = filter_list(&input);
        assert!(result.contains("... +10 more rows"));
    }

    #[test]
    fn test_filter_column_basic() {
        let input = "id  name   email\n--  -----  -----\n1   alice  a@b.com\n2   bob    b@b.com\n";
        let result = filter_column(input);
        assert!(result.contains("id\tname\temail"));
        assert!(result.contains("1\talice\ta@b.com"));
        assert!(!result.contains("--"));
    }

    #[test]
    fn test_filter_column_overflow() {
        let mut lines = vec!["id  val".to_string(), "--  ---".to_string()];
        for i in 1..=60 {
            lines.push(format!("{}   row{}", i, i));
        }
        let input = lines.join("\n");
        let result = filter_column(&input);
        assert!(result.contains("... +10 more rows"));
    }

    #[test]
    fn test_filter_line_basic() {
        let input = "   name = alice\n  email = a@b.com\n\n   name = bob\n  email = b@b.com\n";
        let result = filter_line(input);
        assert!(result.contains("name = alice email = a@b.com"));
        assert!(result.contains("name = bob email = b@b.com"));
    }

    #[test]
    fn test_filter_sqlite_output_empty() {
        assert_eq!(filter_sqlite_output(""), "");
    }

    #[test]
    fn test_filter_sqlite_output_routes_to_list() {
        let input = "id|name|email\n1|alice|a@b.com\n";
        let result = filter_sqlite_output(input);
        assert!(result.contains("id\tname\temail"));
    }

    #[test]
    fn test_filter_sqlite_output_routes_to_column() {
        let input = "id  name\n--  ----\n1   alice\n";
        let result = filter_sqlite_output(input);
        assert!(result.contains("id\tname"));
        assert!(!result.contains("--"));
    }

    #[test]
    fn test_filter_sqlite_output_passthrough() {
        let input = "Error: no such table: users\n";
        let result = filter_sqlite_output(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_list_token_savings() {
        let input = "id|username|email|status|created_at|updated_at|role\n1|alice_smith|alice@example.com|active|2024-01-01|2024-01-15|admin\n2|bob_jones|bob.jones@company.org|active|2024-01-02|2024-01-16|user\n3|carol_white|carol.white@example.com|inactive|2024-01-03|2024-01-17|user\n4|dave_brown|dave@business.net|active|2024-01-04|2024-01-18|moderator\n5|eve_davis|eve.davis@example.com|active|2024-01-05|2024-01-19|user\n";
        let result = filter_list(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 20.0,
            "List filter: expected >=20% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_column_token_savings() {
        let input = "id  username      email                    status    created_at  updated_at  role\n--  ------------  -----------------------  --------  ----------  ----------  --------\n1   alice_smith   alice@example.com        active    2024-01-01  2024-01-15  admin\n2   bob_jones     bob.jones@company.org    active    2024-01-02  2024-01-16  user\n3   carol_white   carol.white@example.com  inactive  2024-01-03  2024-01-17  user\n4   dave_brown    dave@business.net        active    2024-01-04  2024-01-18  moderator\n5   eve_davis     eve.davis@example.com    active    2024-01-05  2024-01-19  user\n";
        let result = filter_column(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "Column filter: expected >=40% savings, got {:.1}%",
            savings
        );
    }
}
