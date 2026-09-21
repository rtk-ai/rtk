//! cover (Devel::Cover) filter: what is not covered, and nothing else.
//!
//! The summary table keeps its header, the Total row and every file below 100% in some column;
//! fully covered files are dropped. `cover -test` runs `make test` first, and that part goes
//! through the shared TAP filter. The per-line text report (`-report text`) keeps, per file,
//! only the lines where a statement, branch, condition or subroutine was missed, the missed
//! rows of the Branches and Conditions tables with their column header, and the Uncovered
//! Subroutines. Run metadata, covered lines, Covered Subroutines and the database and HTML
//! paths are dropped. A line flagged only for missing POD is not reported per line; the
//! Uncovered Subroutines table and the summary's pod column already say it.

use regex::Regex;
use std::sync::LazyLock;

use super::tap::filter_harness_run;
use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;

static COVER_NOISE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(?:Deleting database |cover: running |Reading database from |",
        r"HTML output written to |done\.$|Devel::Cover: |",
        r"(?:Run|Perl version|OS|Start|Finish):\s)"
    ))
    .unwrap()
});
/// `(Selecting|Ignoring) packages matching:` heads an indented list of patterns.
static PACKAGE_LIST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:Selecting|Ignoring) packages (?:matching|in):").unwrap());
/// A summary row: a file (or Total) and seven numeric or `n/a` columns.
static SUMMARY_ROW_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\S+)((?:\s+(?:\d+\.\d|n/a)){7})$").unwrap());
/// A report-section rule: dashes, optionally in columns.
static RULE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-\s]+$").unwrap());
/// The per-line listing header of the text report.
static LISTING_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^line\s+err\s+stmt\s+bran\s+cond\s+sub\s+pod\s+time\s+code$").unwrap()
});
/// Column headers of the Branches and Conditions tables.
static TABLE_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^line\s+err\s+%\s").unwrap());

/// The coverage columns whose miss makes a line worth reporting (not `pod`, not `time`).
const LISTED_COLUMNS: [&str; 4] = ["stmt", "bran", "cond", "sub"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    /// Outside the text report: summary table, harness output.
    Top,
    Listing,
    /// Branches or Conditions.
    Table,
    CoveredSubs,
    UncoveredSubs,
}

/// Where each of `LISTED_COLUMNS` sits in the listing, from its header. Numbers are
/// right-aligned under their label, so a column's cell runs from the end of the label before it
/// to the end of its own.
struct ListingColumns {
    cells: Vec<(usize, usize, &'static str)>,
    code_start: usize,
}

impl ListingColumns {
    fn from_header(header: &str) -> Option<Self> {
        let end_of = |label: &str| header.find(label).map(|i| i + label.len());
        let mut start = end_of("err")?;
        let mut cells = Vec::new();
        for label in LISTED_COLUMNS {
            let end = end_of(label)?;
            cells.push((start, end, label));
            start = end;
        }
        Some(Self {
            cells,
            code_start: header.find("code")?,
        })
    }

    /// The listed columns this row misses: a `*` in the column's cell.
    fn missed(&self, row: &str) -> Vec<&'static str> {
        self.cells
            .iter()
            .filter(|(start, end, _)| row.get(*start..*end).is_some_and(|c| c.contains('*')))
            .map(|(_, _, label)| *label)
            .collect()
    }
}

/// The summary columns are stmt, bran, cond, sub, pod, time, total. `time` is a share of the
/// run time, not coverage, so it does not count.
fn is_fully_covered(columns: &str) -> bool {
    columns
        .split_whitespace()
        .enumerate()
        .filter(|(i, _)| *i != 5)
        .all(|(_, c)| c == "100.0" || c == "n/a")
}

pub fn filter_cover_report(raw: &str) -> String {
    let clean = strip_ansi(raw);
    let mut out: Vec<String> = Vec::new();
    let mut section = Section::Top;
    let mut in_package_list = false;
    let mut listing: Option<ListingColumns> = None;
    let mut last_line_no = String::new();
    let mut table_header: Option<String> = None;
    let mut table_title = String::new();

    for line in clean.lines() {
        let line = line.trim_end();
        if in_package_list {
            if line.starts_with(char::is_whitespace) {
                continue;
            }
            in_package_list = false;
        }
        if PACKAGE_LIST_RE.is_match(line) {
            in_package_list = true;
            continue;
        }
        if line.is_empty() || COVER_NOISE_RE.is_match(line) || RULE_RE.is_match(line) {
            continue;
        }

        match line {
            "Branches" | "Conditions" => {
                section = Section::Table;
                table_title = line.to_string();
                table_header = None;
                continue;
            }
            "Covered Subroutines" => {
                section = Section::CoveredSubs;
                continue;
            }
            "Uncovered Subroutines" => {
                section = Section::UncoveredSubs;
                out.push(line.to_string());
                continue;
            }
            _ => {}
        }

        if let Some(caps) = SUMMARY_ROW_RE.captures(line) {
            if &caps[1] == "Total" || !is_fully_covered(&caps[2]) {
                out.push(line.to_string());
            }
            continue;
        }
        if line.starts_with("File ") && line.contains(" stmt ") {
            section = Section::Top;
            out.push(line.to_string());
            continue;
        }
        if LISTING_HEADER_RE.is_match(line) {
            section = Section::Listing;
            listing = ListingColumns::from_header(line);
            continue;
        }
        // A file's section of the text report starts with its path alone on a line.
        if !line.starts_with(char::is_whitespace)
            && !line.contains(' ')
            && (line.ends_with(".pm") || line.ends_with(".pl") || line.ends_with(".t"))
        {
            section = Section::Top;
            out.push(line.to_string());
            continue;
        }

        match section {
            Section::Top => out.push(line.to_string()),
            Section::Listing => {
                let Some(columns) = &listing else {
                    continue;
                };
                let line_no: String = line.chars().take_while(char::is_ascii_digit).collect();
                if !line_no.is_empty() {
                    last_line_no = line_no;
                }
                let missed = columns.missed(line);
                if missed.is_empty() {
                    continue;
                }
                let code = line.get(columns.code_start..).unwrap_or("").trim();
                out.push(format!(
                    "  {}: {}  [{}]",
                    last_line_no,
                    code,
                    missed.join(" ")
                ));
            }
            Section::Table => {
                if TABLE_HEADER_RE.is_match(line) {
                    table_header = Some(line.to_string());
                    continue;
                }
                if !line.contains("***") {
                    continue;
                }
                if !table_title.is_empty() {
                    out.push(std::mem::take(&mut table_title));
                }
                if let Some(header) = table_header.take() {
                    out.push(header);
                }
                out.push(line.to_string());
            }
            Section::CoveredSubs => {}
            Section::UncoveredSubs => out.push(line.to_string()),
        }
    }
    out.join("\n")
}

fn is_cover_noise(line: &str) -> bool {
    COVER_NOISE_RE.is_match(line)
}

/// `cover -test` output: the harness run, then the report.
pub fn filter_cover(raw: &str) -> String {
    let clean = strip_ansi(raw);
    // The report starts at the summary table; everything before it is the test run.
    let split = clean
        .lines()
        .position(|l| l.starts_with("File ") && l.contains(" stmt "))
        .map(|i| {
            // Include the rule line above the header in the report half.
            clean
                .lines()
                .take(i.saturating_sub(1))
                .map(|l| l.len() + 1)
                .sum::<usize>()
        });
    let (run, report) = match split {
        Some(at) if at <= clean.len() => clean.split_at(at),
        _ => (clean.as_str(), ""),
    };
    let run_out = filter_harness_run(run, is_cover_noise);
    let report_out = filter_cover_report(report);
    match (run_out.is_empty(), report_out.is_empty()) {
        (true, _) => report_out,
        (_, true) => run_out,
        _ => format!("{}\n{}", run_out, report_out),
    }
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("cover");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: cover {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "cover",
        &args.join(" "),
        filter_cover,
        runner::RunOptions::with_tee("cover"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn savings(input: &str, output: &str) -> f64 {
        100.0 - (count_tokens(output) as f64 / count_tokens(input) as f64 * 100.0)
    }

    #[test]
    fn test_cover_test_run() {
        let input = include_str!("../../../tests/fixtures/perl/cover_test_raw.txt");
        let output = filter_cover(input);
        assert!(
            output.starts_with("# Failed test 'two plus two is five' (t/02-fail-more.t line 10)"),
            "{}",
            output
        );
        assert!(output.contains("t/02-fail-more.t: Failed 3/5 subtests (exit 3)"));
        assert!(output.ends_with(
            "File                           stmt   bran   cond    sub    pod   time  total\n\
             blib/lib/Acme/RtkSample.pm     73.5   75.0    n/a   77.7    0.0   99.2   66.1\n\
             ...ib/Acme/RtkSample/Util.pm   64.2    n/a    n/a   75.0    0.0    0.7   60.0\n\
             Total                          70.8   75.0    n/a   76.9    0.0  100.0   64.6"
        ));
        assert!(!output.contains("cover_db"));
        assert!(!output.contains("PERL_DL_NONLAZY"));
    }

    #[test]
    fn test_summary_drops_fully_covered_files() {
        let input = "File          stmt   bran   cond    sub    pod   time  total\n\
                     lib/A.pm     100.0  100.0    n/a  100.0  100.0   10.0  100.0\n\
                     lib/B.pm      50.0  100.0    n/a  100.0  100.0   90.0   80.0\n\
                     Total         75.0  100.0    n/a  100.0  100.0  100.0   90.0\n";
        let output = filter_cover_report(input);
        assert!(!output.contains("lib/A.pm"));
        assert!(output.contains("lib/B.pm"));
        assert!(output.contains("Total"));
    }

    #[test]
    fn test_text_report_keeps_misses_only() {
        let input = include_str!("../../../tests/fixtures/perl/cover_report_raw.txt");
        let output = filter_cover_report(input);
        assert!(
            output.contains(
                "blib/lib/Acme/RtkSample.pm\n  23: return sort @{ $self->{names} || [] };  [bran]\n"
            ),
            "{}",
            output
        );
        assert!(output.contains("  28: open FH, $path or die \"cannot open $path\";  [stmt bran]"));
        assert!(output.contains("  60: my $s = eval \"1 + 1\";  [stmt]"));
        // Covered lines, and lines missing only POD, are not listed.
        assert!(!output.contains("  18: return $a + $b;"));
        assert!(!output.contains("  10: my $class = shift;"));
        assert!(output.contains(
            "Branches\nline  err      %   true  false   branch\n23    ***     50      0      1   unless $self->{'names'}"
        ));
        assert!(!output.contains("if ($n > 1000)"));
        assert!(output.contains("Uncovered Subroutines\nSubroutine      Count Pod Location"));
        assert!(output.contains("read_first_line     0   0 blib/lib/Acme/RtkSample.pm:27"));
        assert!(!output.contains("Covered Subroutines"));
        assert!(!output.contains("Perl version"));
        let pct = savings(input, &output);
        assert!(pct >= 60.0, "expected >=60% savings, got {:.1}%", pct);
    }

    #[test]
    fn test_missing_database_error_kept() {
        let input = include_str!("../../../tests/fixtures/perl/cover_test_nomakefile_raw.txt");
        let output = filter_cover(input);
        assert!(output.contains("make: *** No rule to make target 'test'.  Stop."));
        assert!(
            output.contains(
                "Can't stat /home/user/Acme-RtkSample/cover_db: No such file or directory"
            )
        );
    }

    #[test]
    fn test_package_lists_dropped() {
        let input = "Selecting packages matching:\nIgnoring packages matching:\n    /Devel/Cover[./]\n    ^t/\nIgnoring packages in:\n    /opt/perl5/lib\nDevel::Cover: 100% - done\nok\n";
        assert_eq!(filter_cover_report(input), "ok");
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(filter_cover(""), "");
    }
}
