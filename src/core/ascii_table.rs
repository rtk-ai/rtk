//! Strips ASCII table decoration (box borders and cell padding) into compact
//! tab-separated rows. Generic: it knows nothing about SQL. Footer lines and
//! any format-specific quirks are the caller's responsibility to strip before
//! calling here.
//!
//! # Example
//!
//! Raw table body (borders + alignment padding):
//!
//! ```text
//! +----+-------------+-------------------+--------+
//! | id | username    | email             | status |
//! +----+-------------+-------------------+--------+
//! |  1 | alice_smith | alice@example.com | active |
//! |  2 | bob_jones   | bob@example.com   | active |
//! +----+-------------+-------------------+--------+
//! ```
//!
//! Compressed to tab-separated rows (`\t` marks each tab; borders and padding gone):
//!
//! ```text
//! id\tusername\temail\tstatus
//! 1\talice_smith\talice@example.com\tactive
//! 2\tbob_jones\tbob@example.com\tactive
//! ```

use crate::core::truncate::CAP_LIST;
use regex::Regex;
use std::sync::LazyLock;

/// Column junction character in the border row (`----+----`). The paired cell
/// separator is [`SEPARATOR`]. Both follow the near-universal ASCII box-table
/// convention; if a dialect ever differs, promote these to fields on
/// [`TableShape`] with that consumer as justification.
const JUNCTION: char = '+';
/// Cell separator character in data rows (`| a | b |`).
const SEPARATOR: u8 = b'|';

static BORDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-+]+$").unwrap());

pub struct TableShape {
    /// Rows are wrapped in outer bars (`| a | b |`) as in mysql. psql tables
    /// have no outer bars, so this is `false` for them.
    pub has_outer_pipes: bool,
}

/// Byte positions of the column junctions (`+`) in one border row.
fn junctions_in(border: &str) -> Vec<usize> {
    border.match_indices(JUNCTION).map(|(i, _)| i).collect()
}

/// Byte positions of the column junctions (`+`) in the first border row.
/// Returns `[]` when no `+`-bearing border exists, which disables offset
/// slicing and falls the caller back to naive splitting.
///
/// This only *seeds* the layout: psql prints its header above the first
/// border, so that row needs offsets before the loop reaches one. Every border
/// the loop meets re-derives them — see [`strip_ascii_table`].
fn junction_offsets(body: &str) -> Vec<usize> {
    body.lines()
        .find(|l| BORDER.is_match(l.trim()) && l.contains(JUNCTION))
        .map(junctions_in)
        .unwrap_or_default()
}

/// Slice a data row at the fixed column boundaries derived from the border,
/// so a `|` *inside* a cell value (config strings, stored regexes) is never
/// mistaken for a column separator.
///
/// Returns `None` — signalling the caller to fall back to naive splitting —
/// when the row doesn't carry a `|` at every junction offset. That happens for
/// borderless input or when a multi-byte cell shifts the byte offsets; the
/// fallback already handles those correctly (they have no interior pipes).
fn split_by_offsets(line: &str, junctions: &[usize], has_outer_pipes: bool) -> Option<Vec<String>> {
    if junctions.is_empty() {
        return None;
    }
    // Every junction offset must line up with a `|` in this row.
    if !junctions
        .iter()
        .all(|&i| line.as_bytes().get(i) == Some(&SEPARATOR))
    {
        return None;
    }

    let cells: Vec<String> = if has_outer_pipes {
        // The outer `+` are the bars; columns are the interior windows. The
        // slice bounds (`j + 1` and `j`) are ASCII `|` positions, so they are
        // always valid char boundaries.
        if junctions.len() < 2 {
            return None;
        }
        junctions
            .windows(2)
            .map(|w| line[w[0] + 1..w[1]].trim().to_string())
            .collect()
    } else {
        // No outer bars: add virtual edges at 0 and end around the interior
        // junctions. The first cell starts at 0 (content, not a separator).
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut start = 0usize;
        for &j in junctions {
            spans.push((start, j));
            start = j + 1;
        }
        spans.push((start, line.len()));
        spans
            .into_iter()
            .map(|(s, e)| line[s..e].trim().to_string())
            .collect()
    };
    Some(cells)
}

/// De-decorate an ASCII table body. The first row containing `|` is treated as
/// the header (always kept); data rows are capped at [`CAP_LIST`], with an
/// `... +N more rows` marker when truncated.
///
/// A body can hold more than one table — a multi-statement query returns one
/// result set per statement. A border whose junction layout differs from the
/// current one starts a new table: offsets are re-derived from it, and the
/// header/cap accounting restarts so the second result set is not counted
/// against the first one\'s cap. Reusing the first border\'s byte offsets for
/// every later row is what previously shredded a second, narrower table into
/// the wrong columns.
///
/// Two adjacent tables with *identical* column widths are indistinguishable
/// here and merge into one. That is harmless: the offsets are correct for
/// both, so only the row count and the header/data split are affected.
pub fn strip_ascii_table(body: &str, shape: TableShape) -> String {
    strip_ascii_table_with_stats(body, shape).0
}

/// [`strip_ascii_table`], additionally reporting how many data rows the cap hid
/// across every result set in the body. A caller that shows `... +N more rows`
/// needs the count to decide whether to offer a recovery path for them.
pub fn strip_ascii_table_with_stats(body: &str, shape: TableShape) -> (String, usize) {
    let mut junctions = junction_offsets(body);
    let mut hidden = 0usize;

    let mut out: Vec<String> = Vec::new();
    let mut pipe_rows = 0usize; // header + data rows encountered
    let mut data_rows = 0usize; // data rows only (header excluded)

    for line in body.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Skip border lines: `----+----` (psql) and `+----+----+` (mysql).
        if BORDER.is_match(trimmed) {
            if line.contains(JUNCTION) {
                let layout = junctions_in(line);
                if layout != junctions {
                    // A different column layout means a new result set.
                    hidden += push_overflow(&mut out, data_rows);
                    pipe_rows = 0;
                    data_rows = 0;
                    junctions = layout;
                }
            }
            continue;
        }

        if trimmed.contains('|') {
            pipe_rows += 1;
            let is_header = pipe_rows == 1;
            if !is_header {
                data_rows += 1;
            }

            // Keep the header and the first CAP_LIST data rows; drop the rest.
            if is_header || data_rows <= CAP_LIST {
                let joined = match split_by_offsets(line, &junctions, shape.has_outer_pipes) {
                    Some(cells) => cells.join("\t"),
                    None => naive_split(
                        trimmed,
                        shape.has_outer_pipes,
                        column_count(&junctions, shape.has_outer_pipes),
                    ),
                };
                out.push(joined);
            }
        } else {
            // Non-table line the caller left in (notice, etc.) — pass through.
            out.push(trimmed.to_string());
        }
    }

    hidden += push_overflow(&mut out, data_rows);

    (out.join("\n"), hidden)
}

/// Append the `... +N more rows` marker for a table that hit [`CAP_LIST`],
/// returning how many rows it hid. Called once per result set, not once per
/// body.
fn push_overflow(out: &mut Vec<String>, data_rows: usize) -> usize {
    if data_rows > CAP_LIST {
        let hidden = data_rows - CAP_LIST;
        out.push(format!("... +{} more rows", hidden));
        hidden
    } else {
        0
    }
}

/// How many columns the border implies, or `None` when there is no border to
/// learn it from (a borderless body, where any split is a guess anyway).
fn column_count(junctions: &[usize], has_outer_pipes: bool) -> Option<usize> {
    if junctions.is_empty() {
        return None;
    }
    if has_outer_pipes {
        // The junctions are the bars themselves; columns sit between them.
        junctions.len().checked_sub(1).filter(|n| *n > 0)
    } else {
        // The junctions are interior separators, with an edge on each side.
        Some(junctions.len() + 1)
    }
}

/// Fallback used when the border-offset slice can't apply: split on every `|`.
/// Correct for borderless tables and unicode rows without interior pipes.
///
/// `expected_columns` is the guard. mysql pads to *display* width while the
/// offsets are counted in bytes, so one wide character in a row is enough to
/// fail the offset check and land here — and if that row also carries a `|`
/// inside a cell, splitting on every bar invents columns and silently reports
/// the wrong data. When the split yields more fields than the border has
/// columns, the extra bars are content that cannot be placed, so the row is
/// returned unsplit instead. One visibly-unsplit field is recoverable; wrongly
/// split fields are not.
fn naive_split(trimmed: &str, has_outer_pipes: bool, expected_columns: Option<usize>) -> String {
    let cells: Vec<&str> = trimmed.split('|').map(|c| c.trim()).collect();
    let sliced: &[&str] = if has_outer_pipes && cells.len() > 2 {
        // Drop the empty edge cells produced by the outer bars. Interior empty
        // cells (NULLs) are preserved.
        &cells[1..cells.len() - 1]
    } else {
        // Fewer than two bars means the row is malformed (a wrapped cell, a
        // truncated line). `cells.len() == 2` would slice to nothing and drop
        // the content entirely, so keep whatever is there.
        &cells
    };

    if matches!(expected_columns, Some(expected) if sliced.len() > expected) {
        return unsplit_row(trimmed, has_outer_pipes);
    }

    sliced.join("\t")
}

/// The row with its outer bars removed and nothing else touched — used when the
/// column boundaries cannot be established, so no `\t` is invented.
fn unsplit_row(trimmed: &str, has_outer_pipes: bool) -> String {
    if !has_outer_pipes {
        return trimmed.to_string();
    }
    trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_outer_pipes_mysql_style() {
        let input = "\
+----+-------------+
| id | username    |
+----+-------------+
|  1 | alice_smith |
|  2 | bob_jones   |
+----+-------------+";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: true,
            },
        );
        assert_eq!(out, "id\tusername\n1\talice_smith\n2\tbob_jones");
    }

    #[test]
    fn test_no_outer_pipes_psql_style() {
        let input = " id | username\n----+-------------\n  1 | alice_smith\n  2 | bob_jones";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: false,
            },
        );
        assert_eq!(out, "id\tusername\n1\talice_smith\n2\tbob_jones");
    }

    #[test]
    fn test_pipe_inside_cell_not_split() {
        // A `|` inside a cell value must stay in that cell, not become a column
        // boundary. Border offsets pin the real columns; interior pipes never
        // sit at a junction. Regression for the split('|') bug.
        let input = "\
+----+-------+
| id | val   |
+----+-------+
| 1  | a|b|c |
+----+-------+";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: true,
            },
        );
        assert_eq!(out, "id\tval\n1\ta|b|c");
    }

    #[test]
    fn test_regex_inside_cell_not_split() {
        let input = "\
+----+-----------------+
| id | pattern         |
+----+-----------------+
| 1  | ^(foo|bar|baz)$ |
+----+-----------------+";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: true,
            },
        );
        assert_eq!(out, "id\tpattern\n1\t^(foo|bar|baz)$");
    }

    #[test]
    fn test_pipe_inside_cell_psql_style() {
        // Same guarantee for the no-outer-bars (psql) offset path.
        let input = " id | pattern\n----+-------------\n  1 | a|b|c";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: false,
            },
        );
        assert_eq!(out, "id\tpattern\n1\ta|b|c");
    }

    #[test]
    fn test_unicode_cell_falls_back_cleanly() {
        // Multi-byte content shifts the byte offsets so the junction check
        // fails on that row → naive-split fallback. The cell has no interior
        // pipe, so the fallback is correct; must not panic or corrupt.
        let input = "\
+----+--------+
| id | name   |
+----+--------+
| 1  | café☕ |
+----+--------+";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: true,
            },
        );
        assert_eq!(out, "id\tname\n1\tcafé☕");
    }

    #[test]
    fn test_interior_null_preserved() {
        // Middle cell is an empty NULL — must survive, only edges dropped.
        let input = "| 1 |  | c |";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: true,
            },
        );
        assert_eq!(out, "1\t\tc");
    }

    #[test]
    fn test_row_cap_and_overflow() {
        let mut lines = vec!["| id | val |".to_string()];
        for i in 1..=CAP_LIST + 5 {
            lines.push(format!("| {} | v{} |", i, i));
        }
        let input = lines.join("\n");
        let out = strip_ascii_table(
            &input,
            TableShape {
                has_outer_pipes: true,
            },
        );

        assert!(out.contains("... +5 more rows"));
        // header + CAP_LIST data rows + overflow marker
        assert_eq!(out.lines().count(), CAP_LIST + 2);
    }

    #[test]
    fn test_second_result_set_keeps_its_own_columns() {
        // Regression: offsets were derived once from the first border and
        // reused, so a second, narrower table failed the junction check, fell
        // to naive_split, and had every interior `|` turned into a column
        // break. `x|y|z` came out as three columns.
        let input = "\
+----+-------+
| id | val   |
+----+-------+
| 1  | a|b|c |
+----+-------+
+------+---------+
| k    | pattern |
+------+---------+
| 1    | x|y|z   |
+------+---------+";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: true,
            },
        );
        assert_eq!(out, "id\tval\n1\ta|b|c\nk\tpattern\n1\tx|y|z");
    }

    #[test]
    fn test_each_result_set_gets_its_own_cap() {
        // Regression: the cap was counted across the whole body, so a second
        // result set after a long first one vanished into the first table's
        // `... +N more rows`.
        let mut lines = vec![
            "+----+-----+".to_string(),
            "| id | val |".to_string(),
            "+----+-----+".to_string(),
        ];
        for i in 1..=CAP_LIST + 3 {
            lines.push(format!("| {} | v{} |", i, i));
        }
        lines.push("+----+-----+".to_string());
        lines.push("+------+".to_string());
        lines.push("| solo |".to_string());
        lines.push("+------+".to_string());
        lines.push("| only |".to_string());
        lines.push("+------+".to_string());
        let out = strip_ascii_table(
            &lines.join("\n"),
            TableShape {
                has_outer_pipes: true,
            },
        );

        // First table truncates and says so, before the second table starts.
        assert!(out.contains("... +3 more rows"));
        // The second result set survives in full.
        assert!(out.contains("solo"), "second table header lost:\n{}", out);
        assert!(out.contains("only"), "second table row lost:\n{}", out);
        // ...and the marker sits between the two, not at the very end.
        let marker = out
            .lines()
            .position(|l| l.starts_with("... +"))
            .expect("marker");
        let solo = out
            .lines()
            .position(|l| l == "solo")
            .expect("second header");
        assert!(marker < solo, "marker must close the first table:\n{}", out);
    }

    #[test]
    fn test_repeated_border_is_not_a_new_table() {
        // mysql draws three identical borders per table. Only a *different*
        // layout starts a new result set, otherwise every table would reset
        // its own header after the second border.
        let input = "+----+------+\n| id | name |\n+----+------+\n| 1  | foo  |\n+----+------+";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: true,
            },
        );
        assert_eq!(out, "id\tname\n1\tfoo");
    }

    #[test]
    fn test_malformed_single_bar_row_keeps_content() {
        // One interior bar and no outer bars: `cells.len() == 2`, which the old
        // slice turned into an empty string — a silent content drop on the path
        // that exists to be the safe fallback.
        let out = strip_ascii_table(
            "1 | 2",
            TableShape {
                has_outer_pipes: true,
            },
        );
        assert_eq!(out, "1\t2");
    }

    #[test]
    fn test_stats_report_hidden_rows_per_result_set() {
        // The count drives the caller\'s recovery hint, so it must total every
        // result set, not just the last one.
        let mut lines = Vec::new();
        for (width, extra) in [("+----+", 3usize), ("+------+", 5usize)] {
            lines.push(width.to_string());
            lines.push("| id |".to_string());
            lines.push(width.to_string());
            for i in 1..=CAP_LIST + extra {
                lines.push(format!("| {} |", i));
            }
            lines.push(width.to_string());
        }
        let (text, hidden) = strip_ascii_table_with_stats(
            &lines.join("\n"),
            TableShape {
                has_outer_pipes: true,
            },
        );
        assert_eq!(hidden, 8, "3 hidden in the first table, 5 in the second");
        assert!(text.contains("... +3 more rows"));
        assert!(text.contains("... +5 more rows"));
    }

    #[test]
    fn test_stats_report_zero_when_nothing_truncated() {
        let (_, hidden) = strip_ascii_table_with_stats(
            "+----+\n| id |\n+----+\n|  1 |\n+----+",
            TableShape {
                has_outer_pipes: true,
            },
        );
        assert_eq!(hidden, 0);
    }

    #[test]
    fn test_wide_char_row_with_pipe_cell_is_not_split_wrongly() {
        // mysql pads to display width, offsets are counted in bytes, so one
        // wide character fails the offset check and lands in naive_split. The
        // cell also holds a `|`, which used to be promoted to a column break:
        // `a|b☕` came back as two separate fields.
        let input = "\
+----+--------+
| id | val    |
+----+--------+
| 1  | a|b☕  |
+----+--------+";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: true,
            },
        );
        let row = out.lines().nth(1).expect("data row");
        assert!(
            !row.contains("a\tb"),
            "cell content was split into columns: {:?}",
            row
        );
        assert!(row.contains("a|b☕"), "cell content lost: {:?}", row);
    }

    #[test]
    fn test_wide_char_row_without_interior_pipe_still_splits() {
        // The guard must not disable the fallback for the ordinary case: no
        // extra bars, so the split is unambiguous and still happens.
        let input =
            "+----+--------+\n| id | name   |\n+----+--------+\n| 1  | café☕ |\n+----+--------+";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: true,
            },
        );
        assert_eq!(out, "id\tname\n1\tcafé☕");
    }

    #[test]
    fn test_column_count_from_border() {
        // mysql: junctions are the bars, columns sit between them.
        assert_eq!(column_count(&[0, 5, 14], true), Some(2));
        // psql: junctions are interior separators, with an edge each side.
        assert_eq!(column_count(&[4], false), Some(2));
        // No border to learn from.
        assert_eq!(column_count(&[], true), None);
        assert_eq!(column_count(&[0], true), None);
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(
            strip_ascii_table(
                "",
                TableShape {
                    has_outer_pipes: true
                }
            ),
            ""
        );
    }

    #[test]
    fn test_token_savings() {
        let input = "\
+----+-------------+-------------------+--------+
| id | username    | email             | status |
+----+-------------+-------------------+--------+
|  1 | alice_smith | alice@example.com | active |
|  2 | bob_jones   | bob@example.com   | active |
+----+-------------+-------------------+--------+";
        let out = strip_ascii_table(
            input,
            TableShape {
                has_outer_pipes: true,
            },
        );
        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "expected >=40% savings, got {:.1}%",
            savings
        );
    }
}
