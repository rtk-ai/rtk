//! MySQL client (mysql) output compression.
//!
//! Handles both of the mysql CLI's output shapes: the interactive box-table
//! format (`+---+---+` borders) and the batch/tab-separated format produced
//! by `-B`/`-N` or by piping stdout to a non-tty (which is how most
//! automation invokes `mysql -e`). Strips borders/footers in box mode and
//! caps row count in both modes.

use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_LIST;
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

const MAX_TABLE_ROWS: usize = CAP_LIST;

static BOX_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\+[-+]+\+$").unwrap());
static ROW_COUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+ rows? in set\b").unwrap());

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("mysql");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: mysql {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "mysql",
        &args.join(" "),
        filter_mysql_output,
        RunOptions::stdout_only()
            .tee("mysql")
            .early_exit_on_failure(),
    )
}

pub(crate) fn filter_mysql_output(output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }

    if is_box_format(output) {
        filter_box(output)
    } else {
        filter_batch(output)
    }
}

fn is_box_format(output: &str) -> bool {
    output.lines().any(|line| BOX_SEPARATOR.is_match(line.trim()))
}

/// Strip `+---+---+` borders and the `N row(s) in set (...)` footer from the
/// interactive box-table format, trim padding, and join with tabs.
fn filter_box(output: &str) -> String {
    let mut result = Vec::new();
    let mut data_rows = 0;
    let mut total_rows = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        if BOX_SEPARATOR.is_match(trimmed) || ROW_COUNT.is_match(trimmed) || trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('|') {
            total_rows += 1;
            if total_rows > 1 {
                data_rows += 1;
            }

            if data_rows <= MAX_TABLE_ROWS || total_rows == 1 {
                let cols: Vec<&str> = trimmed
                    .trim_start_matches('|')
                    .trim_end_matches('|')
                    .split('|')
                    .map(|c| c.trim())
                    .collect();
                result.push(cols.join("\t"));
            }
        } else {
            result.push(trimmed.to_string());
        }
    }

    if data_rows > MAX_TABLE_ROWS {
        result.push(format!("... +{} more rows", data_rows - MAX_TABLE_ROWS));
    }

    result.join("\n")
}

/// Batch/tab-separated format (`-B`/`-N`, or any non-tty stdout): already
/// compact, so just cap the row count for large dumps.
fn filter_batch(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= MAX_TABLE_ROWS {
        return output.trim_end_matches('\n').to_string();
    }

    let mut result: Vec<&str> = lines[..MAX_TABLE_ROWS].to_vec();
    let remainder = lines.len() - MAX_TABLE_ROWS;
    let overflow = format!("... +{} more rows", remainder);
    result.push(&overflow);
    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_box_format_strips_borders_and_footer() {
        let input = "+----+-------+\n\
| id | name  |\n\
+----+-------+\n\
|  1 | alice |\n\
|  2 | bob   |\n\
+----+-------+\n\
2 rows in set (0.00 sec)\n";
        let result = filter_box(input);
        assert!(result.contains("id\tname"));
        assert!(result.contains("1\talice"));
        assert!(result.contains("2\tbob"));
        assert!(!result.contains('+'));
        assert!(!result.contains("rows in set"));
    }

    #[test]
    fn test_is_box_format_detects_border() {
        assert!(is_box_format("+----+\n| id |\n+----+\n"));
    }

    #[test]
    fn test_is_box_format_rejects_batch() {
        assert!(!is_box_format("1\talice\n2\tbob\n"));
    }

    #[test]
    fn test_filter_batch_passthrough_under_cap() {
        let input = "1\talice\n2\tbob\n";
        let result = filter_batch(input);
        assert_eq!(result, "1\talice\n2\tbob");
    }

    #[test]
    fn test_filter_batch_caps_overflow() {
        let mut lines = Vec::new();
        for i in 1..=40 {
            lines.push(format!("{}\trow{}", i, i));
        }
        let input = lines.join("\n");
        let result = filter_batch(&input);
        assert!(result.contains("... +20 more rows"));
        assert_eq!(result.lines().count(), MAX_TABLE_ROWS + 1);
    }

    #[test]
    fn test_filter_mysql_output_empty() {
        assert_eq!(filter_mysql_output(""), "");
    }

    #[test]
    fn test_filter_mysql_output_routes_to_box() {
        let input = "+----+\n| id |\n+----+\n|  1 |\n+----+\n1 row in set (0.00 sec)\n";
        let result = filter_mysql_output(input);
        assert!(result.contains("id"));
        assert!(!result.contains("row in set"));
    }

    #[test]
    fn test_filter_mysql_output_routes_to_batch() {
        let input = "id\tname\n1\talice\n";
        let result = filter_mysql_output(input);
        assert_eq!(result, "id\tname\n1\talice");
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_box_token_savings() {
        let input = "+----+-------------------+--------------------------------+-----------+\n\
| id | username          | email                          | status    |\n\
+----+-------------------+--------------------------------+-----------+\n\
|  1 | alice_smith       | alice@example.com              | active    |\n\
|  2 | bob_jones         | bob.jones@company.org          | active    |\n\
|  3 | carol_white       | carol.white@example.com        | inactive  |\n\
+----+-------------------+--------------------------------+-----------+\n\
3 rows in set (0.00 sec)\n";
        let result = filter_box(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "Box filter: expected >=40% savings, got {:.1}%",
            savings
        );
    }
}
