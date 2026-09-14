//! MySQL client (mysql) output compression.
//!
//! Detects table-mode output (`+----+` box borders, as produced by `-t` or the
//! interactive default), strips the borders, padding, and `... (N.NN sec)`
//! footer, and delegates row de-formatting to the shared [`strip_ascii_table`].
//! Batch mode (`-B`, already tab-separated) and vertical `\G` output are passed
//! through unchanged.
//!
//! Piped input (`mysql db < dump.sql`) is forwarded to the client; see
//! [`forward_stdin`] for why an interactive terminal is treated differently.
//!
//! Credential safety: the child command receives every argument untouched (so
//! `--defaults-file` and any auth flags work), but the label handed to the
//! runner for tracking/logging is scrubbed — an inline `-p<password>`, a
//! bundled `-tp<password>`, or a `--password=<value>` is never persisted. See
//! [`redact_credentials`].
//!
//! # Example
//!
//! Raw `mysql -t` table output:
//!
//! ```text
//! +----+-------------+-------------------+--------+
//! | id | username    | email             | status |
//! +----+-------------+-------------------+--------+
//! |  1 | alice_smith | alice@example.com | active |
//! |  2 | bob_jones   | bob@example.com   | active |
//! +----+-------------+-------------------+--------+
//! 2 rows in set (0.00 sec)
//! ```
//!
//! Compressed to tab-separated rows (`\t` marks each tab; borders and footer gone):
//!
//! ```text
//! id\tusername\temail\tstatus
//! 1\talice_smith\talice@example.com\tactive
//! 2\tbob_jones\tbob@example.com\tactive
//! ```

use crate::core::args_utils;
use crate::core::ascii_table::{strip_ascii_table_with_stats, TableShape};
use crate::core::runner::{self, RunOptions};
use crate::core::tee;
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::io::IsTerminal;
use std::sync::LazyLock;

/// mysql short options that consume a value, so a `p` appearing later in the
/// same token is part of that value rather than a password flag (`-uroot`,
/// `-hprimary`, `-Dpayments`). Conservative by design: an option missing here
/// only causes a label to be over-redacted, never a password to be kept.
const VALUE_TAKING_SHORTS: &str = "uhPDSe";

/// A mysql box-table border: `+----+------+` (starts and ends with `+`).
static TABLE_BORDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\+[-+]+\+$").unwrap());
/// The status line that closes a *result set*: `N rows in set (0.00 sec)`,
/// `Empty set (0.00 sec)`, either with a trailing `, 1 warning`. The `[.,]`
/// tolerates a comma decimal separator in localized builds.
///
/// Both ends are pinned. The tail is what mysql always prints; the head is
/// what makes a false positive impossible, because a table row starts with `|`
/// and can therefore never match however odd its cell contents are. An
/// unrecognized footer dialect leaks one cosmetic line rather than deleting a
/// row — the failure that reaches the user is the harmless one.
///
/// `Query OK, N rows affected (0.00 sec)` is deliberately *not* here: it is a
/// DML statement\'s own confirmation, not a table footer. See [`is_footer_line`].
static FOOTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\d+ rows? in set|Empty set)\b.*\(\d+[.,]\d+ sec\)$").unwrap()
});

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("mysql");
    for arg in args {
        // Every argument is forwarded untouched so `--defaults-file`, auth
        // flags, `-e`, etc. behave exactly as they would for raw `mysql`.
        cmd.arg(arg);
    }

    // Scrubbed for anything that gets logged or persisted — never the raw args.
    let display = redact_credentials(args);

    if verbose > 0 {
        eprintln!("Running: mysql {}", display);
    }

    let mut opts = RunOptions::stdout_only().tee("mysql").early_exit_on_failure();
    if forward_stdin(std::io::stdin().is_terminal()) {
        opts = opts.inherit_stdin();
    }

    runner::run_filtered(cmd, "mysql", &display, filter_mysql_output, opts)
}

/// Should rtk hand its own stdin to the mysql client?
///
/// Yes when stdin is a pipe or a file: `mysql db < dump.sql` and
/// `pg_dump ... | mysql db` are ordinary usage, and the default
/// [`StdinMode::Null`] closes the child's stdin before it reads a byte — the
/// import silently does nothing and still exits 0.
///
/// No when stdin is a terminal. rtk captures the child's output and prints it
/// after exit, so an interactive mysql REPL would render nothing until the
/// session ended. Keeping stdin closed there makes the client exit immediately,
/// which is the behaviour rtk has always had for interactive use.
///
/// [`StdinMode::Null`]: crate::core::stream::StdinMode::Null
fn forward_stdin(stdin_is_terminal: bool) -> bool {
    !stdin_is_terminal
}

/// Build a display string with inline credentials redacted. Only the *label*
/// is affected — the executed command still receives the real values.
///
/// MySQL only accepts a password *attached* to the flag (`-pSECRET`,
/// `--password=SECRET`); a bare `-p`/`--password` prompts interactively and a
/// space-separated token is treated as a database name, so a single-arg scan is
/// sufficient. `-P` (uppercase, port) and `--defaults-file` are left intact.
///
/// The attachment can be *inside* a bundle — `my_getopt` reads `-tpSECRET` as
/// `-t` plus `-p SECRET` — so the scan cannot assume `-p` heads the token. See
/// [`args_utils::redact_clustered_password`].
fn redact_credentials(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if let Some(rest) = arg.strip_prefix("--password=") {
                if rest.is_empty() {
                    arg.clone()
                } else {
                    "--password=***".to_string()
                }
            } else if let Some(redacted) =
                args_utils::redact_clustered_password(arg, VALUE_TAKING_SHORTS)
            {
                redacted
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn filter_mysql_output(output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }

    if is_table_format(output) {
        filter_table(output)
    } else {
        // Batch mode (`-B`, already TSV), vertical `\G`, `Query OK` status
        // lines, notices — nothing to compress, pass through untouched.
        output.to_string()
    }
}

/// A mysql box-table row: `| a | b |`, outer bars on both ends.
fn is_table_row(trimmed: &str) -> bool {
    trimmed.len() >= 2 && trimmed.starts_with('|') && trimmed.ends_with('|')
}

/// Is this output a box table, rather than something that merely contains a
/// border-shaped line?
///
/// A lone `+----+` is not enough evidence. `mysql -B -e "SELECT sep FROM t"`
/// prints tab-separated rows, and a row whose value *is* `+----+` occupies a
/// whole line — sniffing for the border alone routed that TSV into the table
/// stripper, which deletes border lines, and the row disappeared with no
/// marker. A real table always follows its border with a bar-wrapped row, so
/// require the pair.
fn is_table_format(output: &str) -> bool {
    let mut after_border = false;
    for line in output.lines() {
        let trimmed = line.trim();
        if after_border && is_table_row(trimmed) {
            return true;
        }
        after_border = TABLE_BORDER.is_match(trimmed);
    }
    false
}

/// Is this line the status line that closes a result set?
///
/// Only those are dropped. `is_table_format` looks at the whole body, so a
/// batched `UPDATE ...; SELECT ...;` sends the UPDATE\'s output down this path
/// too even though it rendered no table; deleting its `Query OK, 1 row
/// affected (0.01 sec)` would throw away the only confirmation that statement
/// produced. [`FOOTER`] matches result-set footers alone, so it survives.
fn is_footer_line(line: &str) -> bool {
    FOOTER.is_match(line.trim())
}

/// Drop the result-set footers, then hand the rest to the shared ASCII-table
/// stripper. mysql wraps every row in outer bars, so `has_outer_pipes` is true.
///
/// The body is assembled in one walk rather than `collect`-ing a `Vec<&str>`
/// and re-joining it, which cost a second traversal and an extra allocation.
fn filter_table(output: &str) -> String {
    let mut body = String::with_capacity(output.len());
    let mut first = true;
    for line in output.lines().filter(|line| !is_footer_line(line)) {
        if !first {
            body.push('\n');
        }
        body.push_str(line);
        first = false;
    }

    let (text, hidden_rows) = strip_ascii_table_with_stats(
        &body,
        TableShape {
            has_outer_pipes: true,
        },
    );

    if hidden_rows == 0 {
        return text;
    }

    // `.tee("mysql")` only writes on a non-zero exit, but a truncated SELECT
    // succeeds — without this the `... +N more rows` marker promises rows that
    // nothing can produce, and the caller re-runs the query with LIMIT/OFFSET
    // to get them back. `force_tee_hint` writes regardless of exit code; the
    // plain "full output" form is the one to use here because a multi-statement
    // body has several sections and no single line offset covers the gap.
    match tee::force_tee_hint(output, "mysql-rows") {
        Some(hint) => format!("{}\n{}", text, hint),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_table_format_stripped() {
        let input = "\
+----+-------------+-------------------+--------+
| id | username    | email             | status |
+----+-------------+-------------------+--------+
|  1 | alice_smith | alice@example.com | active |
|  2 | bob_jones   | bob@example.com   | active |
+----+-------------+-------------------+--------+
2 rows in set (0.00 sec)";
        let out = filter_mysql_output(input);
        assert_eq!(
            out,
            "id\tusername\temail\tstatus\n\
             1\talice_smith\talice@example.com\tactive\n\
             2\tbob_jones\tbob@example.com\tactive"
        );
        assert!(!out.contains('+'));
        assert!(!out.contains("2 rows in set"));
    }

    #[test]
    fn test_footer_variants_stripped() {
        for footer in [
            "2 rows in set (0.00 sec)",
            "Empty set (0.00 sec)",
            "1 row in set, 1 warning (0.01 sec)",
        ] {
            let input = format!("+----+\n| id |\n+----+\n|  1 |\n+----+\n{}", footer);
            let out = filter_mysql_output(&input);
            assert!(!out.contains("sec)"), "footer leaked: {}", footer);
        }
    }

    #[test]
    fn test_footer_shaped_cell_content_preserved() {
        // Regression: an unanchored, pipe-blind footer match deleted any data
        // row whose cell text happened to contain `(N.NN sec)`.
        let input = "\
+----+------------------------------+
| id | note                         |
+----+------------------------------+
|  1 | backup done (12.34 sec) ok   |
|  2 | second row                   |
+----+------------------------------+
2 rows in set (0.00 sec)";
        let out = filter_mysql_output(input);
        assert_eq!(
            out,
            "id\tnote\n1\tbackup done (12.34 sec) ok\n2\tsecond row"
        );
        assert!(!out.contains("2 rows in set"));
    }

    #[test]
    fn test_footer_shaped_cell_at_row_end_preserved() {
        // Worst case: the timing substring is the last thing on the line before
        // the closing bar, so even an end-anchored match must not fire.
        let input = "\
+----+---------------------+
| id | note                |
+----+---------------------+
|  1 | done (12.34 sec)    |
+----+---------------------+
1 row in set (0.00 sec)";
        let out = filter_mysql_output(input);
        assert_eq!(out, "id\tnote\n1\tdone (12.34 sec)");
    }

    #[test]
    fn test_is_footer_line() {
        assert!(is_footer_line("2 rows in set (0.00 sec)"));
        assert!(is_footer_line("Empty set, 1 warning (0,01 sec)"));
        // A DML confirmation is not a result-set footer — see the test below.
        assert!(!is_footer_line("Query OK, 1 row affected (0.01 sec)"));
        assert!(!is_footer_line("|  1 | backup done (12.34 sec) ok |"));
        // Mentions a duration but is not a status line.
        assert!(!is_footer_line(
            "ERROR 1205 (HY000): Lock wait timeout (51.00 sec) exceeded"
        ));
    }

    #[test]
    fn test_dml_confirmation_survives_batched_select() {
        // `mysql -t -e "UPDATE t SET x=1; SELECT * FROM t;"`. The SELECT\'s
        // border makes the whole body "table format", so the UPDATE\'s status
        // line came down the footer path and was deleted — losing the only
        // confirmation that statement produced.
        let input = "\
Query OK, 1 row affected (0.01 sec)
Rows matched: 1  Changed: 1  Warnings: 0

+----+---+
| id | x |
+----+---+
|  1 | 1 |
+----+---+
1 row in set (0.00 sec)";
        let out = filter_mysql_output(input);
        assert!(
            out.contains("Query OK, 1 row affected"),
            "DML confirmation dropped:\n{}",
            out
        );
        assert!(out.contains("Rows matched: 1"));
        // The SELECT\'s own footer is still removed.
        assert!(!out.contains("1 row in set"));
        assert!(out.contains("id\tx"));
    }

    #[test]
    fn test_untruncated_output_gets_no_hint() {
        // The recovery hint must appear only when rows were actually hidden,
        // otherwise every small query pays for a tee file it does not need.
        let input = "+----+\n| id |\n+----+\n|  1 |\n+----+\n1 row in set (0.00 sec)";
        let out = filter_mysql_output(input);
        assert_eq!(out, "id\n1");
        assert!(!out.contains("full output"));
    }

    #[test]
    fn test_batch_mode_passthrough() {
        // `-B` output is already tab-separated with no borders — leave it alone.
        let input = "id\tusername\temail\n1\talice_smith\talice@example.com\n2\tbob_jones\tbob@example.com";
        let out = filter_mysql_output(input);
        assert_eq!(out, input);
    }

    #[test]
    fn test_vertical_passthrough() {
        // `\G` vertical output is not table-mode; deferred to a follow-up.
        let input = "*************************** 1. row ***************************\n      id: 1\nusername: alice_smith";
        let out = filter_mysql_output(input);
        assert_eq!(out, input);
    }

    #[test]
    fn test_interior_null_preserved() {
        let input = "+----+------+------+\n| id | name | note |\n+----+------+------+\n|  1 | foo  |      |\n+----+------+------+\n1 row in set (0.00 sec)";
        let out = filter_mysql_output(input);
        // The empty NULL cell must survive as an empty tab-delimited field.
        assert!(out.contains("1\tfoo\t"));
    }

    #[test]
    fn test_forward_stdin_only_when_piped() {
        // Piped/redirected stdin must reach the client, or `mysql db < dump.sql`
        // imports nothing and still exits 0.
        assert!(forward_stdin(false));
        // A terminal stays closed: rtk buffers output until exit, so an
        // interactive REPL would look frozen.
        assert!(!forward_stdin(true));
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(filter_mysql_output(""), "");
    }

    #[test]
    fn test_batch_row_shaped_like_a_border_is_not_a_table() {
        // Regression: `-B` output whose value is `+----+` tripped the border
        // sniff, went through the table stripper, and had that row deleted —
        // silently, with no `... +N more rows` marker.
        let input = "sep\n+----+\nplain\n";
        let out = filter_mysql_output(input);
        assert_eq!(out, input, "batch output must pass through untouched");
    }

    #[test]
    fn test_border_without_a_following_row_is_not_a_table() {
        // Same sniff, border last in the body.
        let input = "note\nplain\n+----+\n";
        assert!(!is_table_format(input));
    }

    #[test]
    fn test_is_table_format() {
        assert!(is_table_format("+----+\n| id |\n+----+"));
        assert!(!is_table_format("id\tname\n1\tfoo"));
        assert!(!is_table_format("Query OK, 1 row affected (0.00 sec)"));
    }

    #[test]
    fn test_token_savings() {
        let input = "\
+----+-------------+-------------------+--------+
| id | username    | email             | status |
+----+-------------+-------------------+--------+
|  1 | alice_smith | alice@example.com | active |
|  2 | bob_jones   | bob@example.com   | active |
+----+-------------+-------------------+--------+
2 rows in set (0.00 sec)";
        let out = filter_mysql_output(input);
        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60% savings, got {:.1}%", savings);
    }

    // --- credential safety ---

    #[test]
    fn test_redact_inline_password() {
        let args = vec!["-t".to_string(), "-pSecret123".to_string()];
        let display = redact_credentials(&args);
        assert_eq!(display, "-t -p***");
        assert!(!display.contains("Secret123"));
    }

    #[test]
    fn test_redact_bundled_password() {
        // Regression: `my_getopt` reads `-tpSecret123` as `-t` + `-p Secret123`,
        // but a `starts_with("-p")` check sees only the leading `-t` and let the
        // password through to the tracking DB and to verbose stderr.
        let args = vec![
            "-tpSecret123".to_string(),
            "-uroot".to_string(),
            "-e".to_string(),
            "SELECT 1".to_string(),
        ];
        let display = redact_credentials(&args);
        assert_eq!(display, "-tp*** -uroot -e SELECT 1");
        assert!(!display.contains("Secret123"));
    }

    #[test]
    fn test_redact_password_flag() {
        let args = vec!["--password=hunter2".to_string()];
        let display = redact_credentials(&args);
        assert_eq!(display, "--password=***");
        assert!(!display.contains("hunter2"));
    }

    #[test]
    fn test_defaults_file_preserved() {
        let args = vec![
            "--defaults-file=/tmp/.my.cnf".to_string(),
            "-t".to_string(),
            "-e".to_string(),
            "SELECT 1".to_string(),
        ];
        let display = redact_credentials(&args);
        // A path is not a credential — it must remain intact for readability.
        assert!(display.contains("--defaults-file=/tmp/.my.cnf"));
        assert!(display.contains("SELECT 1"));
    }

    #[test]
    fn test_non_credential_flags_untouched() {
        // Bare `-p` (prompt), `-P` (port), `--password` (prompt) are not secrets.
        let args = vec![
            "-p".to_string(),
            "-P3306".to_string(),
            "--password".to_string(),
            "-uroot".to_string(),
        ];
        let display = redact_credentials(&args);
        assert_eq!(display, "-p -P3306 --password -uroot");
    }
}
