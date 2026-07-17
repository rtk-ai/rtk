//! Snowflake CLI (snow) output compression.
//!
//! Detects snow's ASCII box table format, strips borders/separators,
//! and produces compact tab-separated output. Handles multi-line cell
//! values by keeping only the first line of each row.

use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_LIST;
use crate::core::utils::resolved_command;
use anyhow::Result;

const MAX_TABLE_ROWS: usize = CAP_LIST;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("snow");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: snow {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "snow",
        &args.join(" "),
        filter_snow_output,
        RunOptions::stdout_only()
            .tee("snow")
            .early_exit_on_failure(),
    )
}

fn filter_snow_output(output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }

    // JSON passthrough: user explicitly requested --format json
    let trimmed = output.trim_start();
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        return output.to_string();
    }

    if is_snow_table(output) {
        filter_snow_table(output)
    } else {
        output.to_string()
    }
}

/// Detects snow's ASCII box table format.
///
/// Snow tables have a `+---...---+` top border line where every character
/// is either `+` or `-`.
fn is_snow_table(output: &str) -> bool {
    output.lines().any(|line| {
        let t = line.trim();
        !t.is_empty()
            && t.starts_with('+')
            && t.ends_with('+')
            && t.chars().all(|c| c == '+' || c == '-')
    })
}

/// Compress snow table output:
/// - Skip `+---+` top/bottom borders
/// - Skip `|---+---|` separator lines
/// - Skip continuation rows (all non-last columns blank — wrapping cells)
/// - For data/header rows: split by `|`, trim cells, join with `\t`
/// - Cap at MAX_TABLE_ROWS data rows
fn filter_snow_table(output: &str) -> String {
    let mut result = Vec::new();
    let mut data_rows: usize = 0;
    let mut total_rows: usize = 0;

    for line in output.lines() {
        let t = line.trim();

        if t.is_empty() {
            continue;
        }

        // Top/bottom border: `+------...------+`
        if t.starts_with('+') && t.ends_with('+') && t.chars().all(|c| c == '+' || c == '-') {
            continue;
        }

        // Separator line: `|---------+---------|`
        if t.starts_with("|-") {
            continue;
        }

        // Data / header row: `| col | col |`
        if t.starts_with("| ") && t.ends_with('|') {
            // Extract cells: split by `|`, drop first (empty before leading `|`)
            // and last (empty after trailing `|`), trim each.
            let cells: Vec<&str> = t
                .split('|')
                .skip(1)
                .filter(|s| {
                    // drop the final empty string from the trailing `|`
                    !s.is_empty() || false
                })
                .map(|s| s.trim())
                .collect();

            // Continuation row: first cell (and often others) is blank because
            // the previous row's cell wrapped. Skip — first-line content is enough.
            let non_empty = cells.iter().filter(|c| !c.is_empty()).count();
            if non_empty == 0 {
                continue;
            }

            // Heuristic: if only the LAST cell has content and previous cells are all
            // empty, this is a continuation of the previous row's last column. Skip.
            let first_has_content = cells.first().is_some_and(|c| !c.is_empty());
            if !first_has_content && non_empty <= 1 {
                continue;
            }

            total_rows += 1;
            if total_rows > 1 {
                data_rows += 1;
            }

            if data_rows <= MAX_TABLE_ROWS || total_rows == 1 {
                result.push(cells.join("\t"));
            }
        } else {
            // Non-table line (notices, errors, etc.)
            result.push(t.to_string());
        }
    }

    if data_rows > MAX_TABLE_ROWS {
        result.push(format!("... +{} more rows", data_rows - MAX_TABLE_ROWS));
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // --- detection ---

    #[test]
    fn test_is_snow_table_detects_border() {
        let input = "+------+------+\n| a | b |\n|------+------|\n| 1 | 2 |\n+------+------+\n";
        assert!(is_snow_table(input));
    }

    #[test]
    fn test_is_snow_table_rejects_plain() {
        assert!(!is_snow_table("No results.\n"));
        assert!(!is_snow_table(""));
        assert!(!is_snow_table("[{\"key\": \"val\"}]"));
    }

    // --- filter_snow_output dispatcher ---

    #[test]
    fn test_filter_json_passthrough() {
        let json = "[{\"name\": \"MY_TABLE\", \"kind\": \"TABLE\"}]\n";
        assert_eq!(filter_snow_output(json), json);
    }

    #[test]
    fn test_filter_plain_passthrough() {
        let plain = "No results were found.\n";
        assert_eq!(filter_snow_output(plain), plain);
    }

    #[test]
    fn test_filter_empty_passthrough() {
        assert_eq!(filter_snow_output(""), "");
        assert_eq!(filter_snow_output("   \n  "), "");
    }

    // --- filter_snow_table ---

    #[test]
    fn test_filter_basic_table() {
        let input = concat!(
            "+-------------------------------+\n",
            "| name    | database | kind    |\n",
            "|---------+----------+---------|\n",
            "| MY_VIEW | MY_DB    | VIEW    |\n",
            "| MY_TBL  | MY_DB    | TABLE   |\n",
            "+-------------------------------+\n",
        );
        let result = filter_snow_table(input);
        assert!(result.contains("name\tdatabase\tkind"));
        assert!(result.contains("MY_VIEW\tMY_DB\tVIEW"));
        assert!(result.contains("MY_TBL\tMY_DB\tTABLE"));
        assert!(!result.contains("---"));
        assert!(!result.contains("+++"));
    }

    #[test]
    fn test_filter_strips_borders() {
        let input = concat!(
            "+------+\n",
            "| a    |\n",
            "|------|\n",
            "| val  |\n",
            "+------+\n",
        );
        let result = filter_snow_table(input);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2, "header + 1 data row");
        assert_eq!(lines[0], "a");
        assert_eq!(lines[1], "val");
    }

    #[test]
    fn test_filter_skips_continuation_rows() {
        // Continuation row: first column blank, second column has wrapped content.
        let input = concat!(
            "+---------------------------------------------+\n",
            "| name | parameters               | default  |\n",
            "|------+--------------------------+----------|\n",
            "| prod | {'account': 'MY_ACCOUNT',| True     |\n",
            "|      | 'user': 'MY_USER'}        |          |\n",
            "+---------------------------------------------+\n",
        );
        let result = filter_snow_table(input);
        // Should have header + 1 data row (continuation skipped)
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(result.contains("prod"));
        // continuation content is dropped
        assert!(!result.contains("MY_USER"));
    }

    #[test]
    fn test_filter_overflow_cap() {
        let mut input = "+------+------+\n".to_string();
        input.push_str("| id   | val  |\n");
        input.push_str("|------+------|\n");
        for i in 1..=25 {
            input.push_str(&format!("| {:4} | r{}   |\n", i, i));
        }
        input.push_str("+------+------+\n");

        let result = filter_snow_table(&input);
        assert!(result.contains("... +5 more rows"));
        let lines: Vec<&str> = result.lines().collect();
        // 1 header + MAX_TABLE_ROWS data + overflow line
        assert_eq!(lines.len(), MAX_TABLE_ROWS + 2);
    }

    // --- real fixture tests ---

    #[test]
    fn test_connection_list_fixture() {
        let input = include_str!("../../../tests/fixtures/snow/connection_list.txt");
        let result = filter_snow_output(input);
        assert!(result.contains("connection_name\tparameters\tis_default"));
        assert!(result.contains("prod\t"));
        assert!(!result.contains("+---"));
        assert!(!result.contains("|-"));
    }

    #[test]
    fn test_sql_result_fixture() {
        let input = include_str!("../../../tests/fixtures/snow/sql_result.txt");
        let result = filter_snow_output(input);
        // Should have a header and at least one data row
        let lines: Vec<&str> = result.lines().filter(|l| !l.is_empty()).collect();
        assert!(lines.len() >= 2);
        assert!(!result.contains("+---"));
    }

    #[test]
    fn test_object_list_fixture() {
        let input = include_str!("../../../tests/fixtures/snow/object_list.txt");
        let result = filter_snow_output(input);
        assert!(result.contains("MY_TABLE"));
        assert!(result.contains("MY_DATABASE"));
        assert!(!result.contains("+---"));
    }

    // --- token savings ---

    #[test]
    fn test_token_savings_object_list() {
        let input = include_str!("../../../tests/fixtures/snow/object_list.txt");
        let result = filter_snow_output(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "Object list filter: expected >=40% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_token_savings_connection_list() {
        let input = include_str!("../../../tests/fixtures/snow/connection_list.txt");
        let result = filter_snow_output(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "Connection list filter: expected >=40% savings, got {:.1}%",
            savings
        );
    }
}
