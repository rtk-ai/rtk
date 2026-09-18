//! Compact `nix search` output. Every other nix command, and a search with
//! `--json`, `-v`/`--verbose`, `--debug`, `--debugger`, `--log-format` or
//! `--trace-function-calls`, is passed through.
//!
//! A broad `nix search` lists tens of thousands of packages over three lines
//! each. The filter reads nix's stdout only and keeps one line per package:
//! the first 20, with a count of the rest (a lower bound when nix failed or
//! its stdout came within 64 KiB of the runner's 10 MiB capture) and a recall
//! hint (which brings back as many as the recovery store's size limit holds).
//! stderr (errors, warnings, traces, nix 2.35's per-attribute progress) is
//! left to the runner, which forwards it on stderr: whole when the search
//! fails or stderr is short, its last lines and a recall hint otherwise.

use crate::core::arg_tokenizer::{self, Dialect, Flag, Grammar, ValueSpec};
use crate::core::runner::{self, RunOptions};
use crate::core::shell::display_args;
use crate::core::stream::RAW_CAP;
use crate::core::truncate::CAP_LIST;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;
use std::ffi::OsString;

const MAX_SEARCH_RESULTS: usize = CAP_LIST;
/// stdout this close to the runner's capture cap may have lost its tail, so
/// the result count is reported as a lower bound. A line longer than this (a
/// flake description over 64 KiB) skipped at the cap can still go unnoticed.
const CAPTURE_MARGIN: usize = 64 * 1024;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if nix_operation(args) != Some("search") || passes_through(args) {
        let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough("nix", &os_args, verbose);
    }

    let mut cmd = resolved_command("nix");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: nix {}", display_args(args));
    }

    // stdin stays the agent's: `nix search -f - …` reads the expression there.
    runner::run_filtered_with_exit(
        cmd,
        "nix",
        &display_args(args),
        filter_nix_search,
        RunOptions::stdout_only().inherit_stdin().tee("nix"),
    )
}

/// The nix subcommand, skipping global flags that take values.
///
/// Unknown flags are not assumed to take a value. Misreading one falls through
/// to the real command instead of filtering a build log.
fn nix_operation(args: &[String]) -> Option<&str> {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--" {
            return args.get(index + 1).map(String::as_str);
        }
        if let Some(name) = arg.strip_prefix("--") {
            let name = name.split('=').next().unwrap_or(name);
            index += 1;
            if !arg.contains('=') {
                index += flag_values(name).min(args.len().saturating_sub(index));
            }
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            index += 1;
            continue;
        }
        return Some(arg);
    }
    None
}

fn flag_values(name: &str) -> usize {
    match name {
        "option" => 2,
        "log-format" | "store" | "file" | "expr" | "include" => 1,
        _ => 0,
    }
}

/// `--flag=v` or `--flag v`.
const VALUE: ValueSpec = ValueSpec::value();

const DEBUG: Flag = Flag::long("debug");
const DEBUGGER: Flag = Flag::long("debugger");
const JSON: Flag = Flag::long("json");
const LOG_FORMAT: Flag = Flag::long("log-format").takes(VALUE);
const TRACE_FUNCTION_CALLS: Flag = Flag::long("trace-function-calls");
const VERBOSE: Flag = Flag::pair("v", "verbose");

/// The flags that make a search pass through, and the options of
/// `nix search --help` (nix 2.36) that take a value. The ones taking two
/// (`--arg`, `--option`, …) are given their first only: the second reads as a
/// positional, which costs nothing unless it is one of these flags.
const SEARCH_FLAGS: &[Flag] = &[
    Flag::long("arg").takes(VALUE),
    Flag::long("arg-from-file").takes(VALUE),
    Flag::long("arg-from-stdin").takes(VALUE),
    Flag::long("argstr").takes(VALUE),
    DEBUG,
    DEBUGGER,
    Flag::long("eval-store").takes(VALUE),
    Flag::pair("e", "exclude").takes(VALUE),
    Flag::long("expr").takes(VALUE),
    Flag::pair("f", "file").takes(VALUE),
    Flag::pair("I", "include").takes(VALUE),
    Flag::long("inputs-from").takes(VALUE),
    JSON,
    LOG_FORMAT,
    Flag::long("option").takes(VALUE),
    Flag::long("output-lock-file").takes(VALUE),
    Flag::long("override-flake").takes(VALUE),
    Flag::long("override-input").takes(VALUE),
    Flag::long("reference-lock-file").takes(VALUE),
    TRACE_FUNCTION_CALLS,
    Flag::long("update-input").takes(VALUE),
    VERBOSE,
];

const SEARCH_GRAMMAR: Grammar = Grammar::new(Dialect::Posix, &[SEARCH_FLAGS]);

/// Whether the search is left as nix prints it. `--json` is output for a
/// program; `--debugger` is interactive. `-v`, `--debug` and
/// `--log-format internal-json` print a record per attribute on stderr (14 to
/// 24 MB on nixpkgs, nix 2.36), `--trace-function-calls` one per function
/// call: past the runner's 10 MiB capture, which skips lines once full, the
/// error nix prints last can be lost. Any `--log-format` is left alone. Read
/// from tokens, so that a flag's value (`-e --json`) or an argument past `--`
/// is not mistaken for them.
fn passes_through(args: &[String]) -> bool {
    arg_tokenizer::tokenize_grammar(args, &SEARCH_GRAMMAR)
        .iter()
        .any(|t| {
            t.is_one_of(&[
                DEBUG,
                DEBUGGER,
                JSON,
                LOG_FORMAT,
                TRACE_FUNCTION_CALLS,
                VERBOSE,
            ])
        })
}

fn filter_nix_search(stdout: &str, exit_code: i32) -> String {
    render_search(stdout, exit_code != 0, |entries, first_hidden| {
        crate::core::tee::force_tee_tail_hint(entries, "nix-search", first_hidden)
    })
}

/// Renders the stdout of `nix search`: per result, `* name (version)` at
/// column 0 and its description indented on the next line, with a blank line
/// before each later result.
/// `failed`: nix exited non-zero. `recovery` stores every entry line and
/// returns the hint that recalls the ones past the cap (1-based first hidden
/// line).
fn render_search(
    stdout: &str,
    failed: bool,
    recovery: impl Fn(&str, usize) -> Option<String>,
) -> String {
    let mut entries: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;

    for raw_line in stdout.lines() {
        // nix 2.35 colours its output even outside a terminal.
        let line = strip_ansi(raw_line);
        let trimmed = line.trim();
        if let Some(name) = line.strip_prefix("* ") {
            if let Some(entry) = pending.replace(normalize_entry_name(name)) {
                entries.push(entry);
            }
        } else if let Some(entry) = pending.take() {
            if line.starts_with(' ') {
                entries.push(if trimmed.is_empty() {
                    entry
                } else {
                    format!("{entry} - {}", trimmed.replace('\r', " "))
                });
            } else if trimmed.is_empty() {
                entries.push(entry);
            } else {
                // The rest of a name holding a newline: nix prints it as is.
                // (A segment that is empty, or starts with a space or `* `,
                // is misread; no nixpkgs name holds a newline at all.)
                pending = Some(format!("{entry} {}", trimmed.replace('\r', " ")));
            }
        }
        // Left: the blank line before a result, and an indented line with no
        // entry before it, a description whose entry the runner's capture
        // skipped at the cap (it keeps shorter lines after one that no longer
        // fits).
    }
    if let Some(entry) = pending {
        entries.push(entry);
    }

    if entries.is_empty() {
        // Nothing to compact: stdout as nix printed it.
        return stdout.to_string();
    }
    let total = entries.len();
    let shown = total.min(MAX_SEARCH_RESULTS);
    // The count is a floor when the runner's capture cap dropped the tail, and
    // when nix failed: it stops at the first evaluation error, after printing
    // the results found so far.
    let at_least = if failed || stdout.len() + CAPTURE_MARGIN >= RAW_CAP {
        "at least "
    } else {
        ""
    };
    let noun = if total == 1 { "result" } else { "results" };
    let mut out = vec![format!("Nix: {at_least}{total} {noun} (showing {shown})")];
    out.extend(entries.iter().take(shown).cloned());
    if total > shown {
        out.push(format!(
            "… {at_least}+{} more, refine the query",
            total - shown
        ));
        if let Some(hint) = recovery(&entries.join("\n"), MAX_SEARCH_RESULTS + 1) {
            out.push(hint);
        }
    }
    out.join("\n")
}

fn normalize_entry_name(entry: &str) -> String {
    let entry = entry.replace('\r', " ");
    if let Some(rest) = entry.strip_prefix("legacyPackages.")
        && let Some(dot_idx) = rest.find('.')
    {
        return rest[dot_idx + 1..].to_string();
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::arg_tokenizer::TokenKind;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    // Fixtures are the stdout of real nix 2.36 runs (2.35 where named).

    /// `tests.*` attributes are results like any other: shown and counted.
    #[test]
    fn test_attributes_are_shown_and_counted() {
        let raw = include_str!("../../../tests/fixtures/nix/search_hello_stdout.txt");
        assert_eq!(
            filter_nix_search(raw, 0),
            "Nix: 6 results (showing 6)\n\
             haskellPackages.hello (1.0.0.2) - Hello World, an example package\n\
             hello (2.12.3) - Program that produces a familiar, friendly greeting\n\
             tests.arrayUtilities.getRunpathEntries.hello\n\
             tests.config.allowPkgsInPermittedInsecurePackages (2.12.3) - Program that produces a familiar, friendly greeting\n\
             tests.haskell.cabalSdist.helloFromCabalSdist (1.0.0.2) - Hello World, an example package\n\
             vdrPlugins.hello (2.8.4)"
        );
    }

    /// A search whose only match is a test attribute keeps the name with its
    /// description.
    #[test]
    fn only_match_under_tests_keeps_its_name() {
        let raw = include_str!("../../../tests/fixtures/nix/search_only_tests_stdout.txt");
        assert_eq!(
            filter_nix_search(raw, 0),
            "Nix: 1 result (showing 1)\ntests.gamma-test (3.0) - gamma only a test"
        );
    }

    /// Descriptions are read by position, whatever they start with.
    #[test]
    fn descriptions_shaped_like_entries_or_diagnostics_stay_descriptions() {
        let raw = include_str!("../../../tests/fixtures/nix/search_desc_shapes_stdout.txt");
        assert_eq!(
            filter_nix_search(raw, 0),
            "Nix: 10 results (showing 10)\n\
             packages.x86_64-linux.errlike (1.0) - error: looks like an error\n\
             packages.x86_64-linux.evallike (1.0) - evaluating 'something'\n\
             packages.x86_64-linux.last (9.9)\n\
             packages.x86_64-linux.locish (1.0) - at default.nix:3: odd\n\
             packages.x86_64-linux.nodesc (1.0)\n\
             packages.x86_64-linux.noversion (0) - has no version\n\
             packages.x86_64-linux.padded (1.0) - padded description\n\
             packages.x86_64-linux.starred (1.0) - * starred description\n\
             packages.x86_64-linux.unicode (1.0) - Ünïcödé — 日本語 description\n\
             packages.x86_64-linux.warnlike (1.0) - warning: looks like a warning"
        );
    }

    /// A description holding a `\r` (text read from a CRLF file) shows it as
    /// a space; a whitespace-only one gives a bare entry.
    #[test]
    fn carriage_return_and_blank_descriptions() {
        let raw =
            include_str!("../../../tests/fixtures/nix/search_cr_and_blank_descriptions_stdout.txt");
        assert_eq!(
            filter_nix_search(raw, 0),
            "Nix: 3 results (showing 3)\n\
             packages.x86_64-linux.a-cr (1) - first part second part\n\
             packages.x86_64-linux.b-ws (1)\n\
             packages.x86_64-linux.c-plain (1) - plain"
        );
    }

    /// Attribute names holding a newline or a `\r`, which nix prints as is.
    #[test]
    fn newline_and_carriage_return_in_names() {
        let raw =
            include_str!("../../../tests/fixtures/nix/search_newline_and_cr_names_stdout.txt");
        assert_eq!(
            filter_nix_search(raw, 0),
            "Nix: 3 results (showing 3)\n\
             packages.x86_64-linux.a b (1) - newline attr\n\
             packages.x86_64-linux.c d (1) - cr attr\n\
             packages.x86_64-linux.zz (1) - after"
        );
    }

    /// `CLICOLOR_FORCE=1` colours names and matches: nothing shown changes.
    #[test]
    fn colour_codes_are_ignored() {
        let colour = include_str!("../../../tests/fixtures/nix/search_jq_color_stdout.txt");
        let plain = include_str!("../../../tests/fixtures/nix/search_jq_stdout.txt");
        assert!(colour.contains('\x1b'));
        assert_eq!(
            render_search(colour, false, |_, _| None),
            render_search(plain, false, |_, _| None)
        );
    }

    /// nix 2.35 colours its stdout even outside a terminal.
    #[test]
    fn nix_2_35_coloured_stdout() {
        let raw = include_str!("../../../tests/fixtures/nix/search_nix235_ripgrep_stdout.txt");
        let filtered = filter_nix_search(raw, 0);
        assert!(
            filtered.starts_with("Nix: 9 results (showing 9)\nbat-extras.batgrep ("),
            "{filtered}"
        );
        assert_eq!(filtered.lines().count(), 10, "{filtered}");
        assert!(!filtered.contains('\x1b'));
    }

    /// Past the cap: the count of the rest, then the hint that recalls exactly
    /// the hidden entry lines, as displayed.
    #[test]
    fn truncated_search_counts_the_rest_and_can_recall_it() {
        let raw = include_str!("../../../tests/fixtures/nix/search_jq_stdout.txt");
        let stored = std::cell::RefCell::new(None);
        let filtered = render_search(raw, false, |entries, first_hidden| {
            *stored.borrow_mut() = Some((entries.to_string(), first_hidden));
            Some("[recall hint]".to_string())
        });
        let lines: Vec<&str> = filtered.lines().collect();
        assert_eq!(lines[0], "Nix: 24 results (showing 20)");
        assert_eq!(lines[1], "fq (0.18.0) - jq for binary formats");
        assert_eq!(lines.len(), 23, "{filtered}");
        assert_eq!(lines[21], "… +4 more, refine the query");
        assert_eq!(lines[22], "[recall hint]");

        let (entries, first_hidden) = stored.into_inner().expect("recovery called");
        assert_eq!(first_hidden, 21);
        let stored_lines: Vec<&str> = entries.lines().collect();
        assert_eq!(stored_lines.len(), 24);
        assert_eq!(stored_lines[..20], lines[1..21]);
        assert_eq!(
            stored_lines[23],
            "python314Packages.xstatic-jquery-ui (1.13.0.2) - jquery-ui packaged static files for python"
        );
    }

    #[test]
    fn untruncated_search_has_no_more_hint() {
        let raw = include_str!("../../../tests/fixtures/nix/search_ripgrep_stdout.txt");
        let filtered = render_search(raw, false, |_, _| panic!("nothing to recall"));
        let lines: Vec<&str> = filtered.lines().collect();
        assert_eq!(lines[0], "Nix: 9 results (showing 9)");
        assert_eq!(lines.len(), 10, "{filtered}");
        assert!(lines.contains(&"vimPlugins.blink-ripgrep-nvim (2.2.7)"));
    }

    /// nix prints no blank line after the last result.
    #[test]
    fn last_entry_without_description_is_kept() {
        let raw = include_str!("../../../tests/fixtures/nix/search_nodesc_last_stdout.txt");
        assert_eq!(
            filter_nix_search(raw, 0),
            "Nix: 1 result (showing 1)\nvimPlugins.blink-ripgrep-nvim (2.2.7)"
        );
    }

    #[test]
    fn empty_stdout_shows_nothing() {
        assert_eq!(filter_nix_search("", 0), "");
    }

    /// nix stops at the first evaluation error, after printing the results
    /// found so far (here one, then the throw): a failed search's count is a
    /// floor.
    #[test]
    fn failed_search_count_is_a_floor() {
        let raw = include_str!("../../../tests/fixtures/nix/search_warn_then_throw_stdout.txt");
        assert_eq!(
            filter_nix_search(raw, 1),
            "Nix: at least 1 result (showing 1)\npackages.x86_64-linux.a-warned (1) - warned"
        );
        assert!(filter_nix_search(raw, 0).starts_with("Nix: 1 result "));
    }

    /// The capture the runner hands over when stdout passes the cap: it skips
    /// each line that no longer fits and keeps shorter ones after it
    /// (src/core/stream.rs, CaptureOnly).
    #[test]
    fn count_is_a_lower_bound_at_the_capture_cap() {
        let colour = include_str!("../../../tests/fixtures/nix/search_jq_color_stdout.txt");
        let stdout: Vec<&str> = colour.lines().collect();
        // The cut from real lines: entry E1 still fits but its long
        // description D1 does not, the blank after it does, the next entry E2
        // does not, its short description D2 does and arrives with no entry
        // before it. Whole lines up to a few KiB short of the cap, then the
        // blank lines the runner keeps while it skips longer ones, leave
        // exactly the room this cut needs.
        let pairs: Vec<(&str, &str)> = stdout
            .windows(2)
            .filter(|w| strip_ansi(w[0]).starts_with("* ") && strip_ansi(w[1]).starts_with("  "))
            .map(|w| (w[0], w[1]))
            .collect();
        let (e1, d1) = *pairs.iter().max_by_key(|(_, d)| d.len()).expect("pair");
        let (e2, d2) = *pairs
            .iter()
            .filter(|(e, d)| e.len() > d.len() + 1)
            .min_by_key(|(_, d)| d.len())
            .expect("pair");
        assert!(d1.len() >= d2.len() + 2);
        let mut capped = String::new();
        'fill: loop {
            for line in &stdout {
                if capped.len() + line.len() + 1 + 4096 > RAW_CAP {
                    break 'fill;
                }
                capped.push_str(line);
                capped.push('\n');
            }
        }
        capped.push('\n');
        while RAW_CAP - capped.len() > e1.len() + 1 + d2.len() + 2 {
            capped.push('\n');
        }
        for line in [e1, d1, "", e2, d2, ""] {
            if capped.len() + line.len() < RAW_CAP {
                capped.push_str(line);
                capped.push('\n');
            }
        }
        assert!(capped.ends_with(&format!("\n{e1}\n\n{d2}\n")), "the cut");
        let stored = std::cell::RefCell::new(String::new());
        let filtered = render_search(&capped, false, |entries, _| {
            *stored.borrow_mut() = entries.to_string();
            None
        });
        let lines: Vec<&str> = filtered.lines().collect();
        let total = capped
            .lines()
            .filter(|l| strip_ansi(l).starts_with("* "))
            .count();
        assert_eq!(
            lines[0],
            format!("Nix: at least {total} results (showing 20)")
        );
        assert_eq!(
            lines[21],
            format!("… at least +{} more, refine the query", total - 20)
        );
        assert_eq!(lines.len(), 22, "the orphan description is not shown");
        // E1 lost its description and D2 its entry: D2 belongs to nothing.
        let e1_name = normalize_entry_name(strip_ansi(e1).strip_prefix("* ").expect("entry"));
        assert_eq!(stored.borrow().lines().last(), Some(e1_name.as_str()));

        // A long line skipped at the cut leaves stdout short of the cap:
        // ripwire's 256-byte description, the longest in the ripgrep capture.
        // Its entry fits, it does not, the blank after it does, and stdout
        // ends 255 bytes short of the cap: still a floor. (This pins the
        // margin from below, at 256 bytes, not its 64 KiB.)
        let ripgrep = include_str!("../../../tests/fixtures/nix/search_ripgrep_stdout.txt");
        let lines_rg: Vec<&str> = ripgrep.lines().collect();
        let long = lines_rg
            .iter()
            .position(|l| l.len() > 250)
            .expect("long line");
        let (entry, description) = (lines_rg[long - 1], lines_rg[long]);
        let mut short = String::new();
        'fill: loop {
            for line in &lines_rg {
                if short.len() + line.len() + 1 + 4096 > RAW_CAP {
                    break 'fill;
                }
                short.push_str(line);
                short.push('\n');
            }
        }
        short.push('\n');
        while RAW_CAP - short.len() > entry.len() + 1 + description.len() {
            short.push('\n');
        }
        for line in [entry, description, ""] {
            if short.len() + line.len() < RAW_CAP {
                short.push_str(line);
                short.push('\n');
            }
        }
        assert_eq!(RAW_CAP - short.len(), description.len() - 1);
        assert!(render_search(&short, false, |_, _| None).starts_with("Nix: at least "));

        let block = include_str!("../../../tests/fixtures/nix/search_jq_stdout.txt");
        assert!(render_search(block, false, |_, _| None).starts_with("Nix: 24 results"));
    }

    /// Every result is either shown or counted, for every captured stdout. The
    /// expected count comes from the same search run with `--json` on the nix
    /// that printed the fixture, not from the parser under test; a fixture
    /// added without its count fails here.
    #[test]
    fn every_printed_result_is_shown_or_counted() {
        const JSON_COUNTS: &[(&str, usize)] = &[
            ("search_cr_and_blank_descriptions_stdout.txt", 3),
            ("search_desc_shapes_stdout.txt", 10),
            ("search_dirty_flake_stdout.txt", 2),
            ("search_hello_stdout.txt", 6),
            ("search_jq_broad_stdout.txt", 64),
            ("search_jq_color_stdout.txt", 24),
            ("search_jq_stdout.txt", 24),
            ("search_newline_and_cr_names_stdout.txt", 3),
            ("search_nix235_ripgrep_stdout.txt", 9),
            ("search_nodesc_last_stdout.txt", 1),
            ("search_only_tests_stdout.txt", 1),
            ("search_ripgrep_stdout.txt", 9),
            // `--json` fails on this flake: counted from its stdout capture.
            ("search_warn_then_throw_stdout.txt", 1),
        ];
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nix");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("fixture dir") {
            let path = entry.expect("dir entry").path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !(name.starts_with("search_") && name.ends_with("_stdout.txt")) {
                continue;
            }
            let raw = std::fs::read_to_string(&path).expect("read fixture");
            let printed = JSON_COUNTS
                .iter()
                .find(|(fixture, _)| *fixture == name)
                .map(|(_, count)| *count)
                .unwrap_or_else(|| panic!("{name}: add its `nix search --json` count"));
            let filtered = render_search(&raw, false, |_, _| None);
            checked += 1;
            let noun = if printed == 1 { "result" } else { "results" };
            assert!(
                filtered.starts_with(&format!("Nix: {printed} {noun} ")),
                "{name}: header\n{filtered}"
            );
            let shown = filtered
                .lines()
                .skip(1)
                .take_while(|l| !l.starts_with('…'))
                .count();
            let hidden: usize = filtered
                .lines()
                .find_map(|l| l.strip_prefix("… +")?.split(' ').next()?.parse().ok())
                .unwrap_or(0);
            assert_eq!(shown + hidden, printed, "{name}:\n{filtered}");
        }
        assert_eq!(checked, JSON_COUNTS.len(), "a listed fixture is missing");
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    /// Savings on a broad search (`nix search nixpkgs jq`, 64 results). Narrow
    /// ones gain little or nothing: there the runner prints nix's own output
    /// when the summary would not be smaller.
    #[test]
    fn broad_search_meets_the_savings_floor() {
        let raw = include_str!("../../../tests/fixtures/nix/search_jq_broad_stdout.txt");
        let filtered = render_search(raw, false, |_, _| None);
        let savings = 100.0 - count_tokens(&filtered) as f64 / count_tokens(raw) as f64 * 100.0;
        assert!(savings >= 20.0, "expected >=20% savings, got {savings:.1}%");
    }

    #[test]
    fn pass_through_flags_are_read_from_tokens() {
        for words in [
            &["--debug", "search", "nixpkgs", "jq"][..],
            &["--log-format", "raw", "search", "nixpkgs", "jq"],
            &["--offline", "search", "--json", "nixpkgs", "jq"],
            &["--trace-function-calls", "search", "nixpkgs", "jq"],
            &["--verbose", "search", "nixpkgs", "jq"],
            &["-v", "search", "nixpkgs", "jq"],
            &["-vv", "search", "nixpkgs", "jq"],
            &["search", "--debugger", "nixpkgs", "jq"],
            &["search", "-Lv", "nixpkgs", "jq"],
            &["search", "nixpkgs", "jq", "--json"],
            &["search", "nixpkgs", "jq", "--log-format=internal-json"],
            &["search", "nixpkgs", "jq", "--trace-function-calls"],
        ] {
            assert!(passes_through(&args(words)), "{words:?}");
        }
        for words in [
            &["-L", "search", "nixpkgs", "jq"][..],
            &["search", "--exclude", "--json", "nixpkgs"],
            &["search", "--no-trace-function-calls", "nixpkgs", "jq"],
            &["search", "-e", "--json", "nixpkgs", "jq"],
            &["search", "-e", "-v", "nixpkgs", "jq"],
            &["search", "nixpkgs", "--", "--json"],
            &["search", "nixpkgs", "jsonnet"],
        ] {
            assert!(!passes_through(&args(words)), "{words:?}");
        }
    }

    #[test]
    fn search_grammar_matches_its_table() {
        arg_tokenizer::assert_takes_value_table(
            &SEARCH_GRAMMAR,
            &[
                (
                    TokenKind::Long,
                    &[
                        "arg",
                        "arg-from-file",
                        "arg-from-stdin",
                        "argstr",
                        "eval-store",
                        "exclude",
                        "expr",
                        "file",
                        "include",
                        "inputs-from",
                        "log-format",
                        "option",
                        "output-lock-file",
                        "override-flake",
                        "override-input",
                        "reference-lock-file",
                        "update-input",
                    ],
                    Some(ValueSpec::value()),
                ),
                (
                    TokenKind::Long,
                    &[
                        "debug",
                        "debugger",
                        "json",
                        "trace-function-calls",
                        "verbose",
                    ],
                    None,
                ),
                (TokenKind::Short, &["e", "f", "I"], Some(ValueSpec::value())),
                (TokenKind::Short, &["v"], None),
            ],
        );
    }

    #[test]
    fn search_is_the_only_filtered_operation() {
        assert_eq!(
            nix_operation(&["search".into(), "nixpkgs".into(), "hello".into()]),
            Some("search")
        );
        assert_eq!(
            nix_operation(&[
                "--offline".into(),
                "search".into(),
                "nixpkgs".into(),
                "ripgrep".into()
            ]),
            Some("search")
        );
        assert_eq!(
            nix_operation(&[
                "--log-format".into(),
                "bar".into(),
                "search".into(),
                "nixpkgs".into(),
                "hello".into()
            ]),
            Some("search")
        );
        assert_eq!(
            nix_operation(&["build".into(), "nixpkgs#hello".into()]),
            Some("build")
        );
        assert_eq!(
            nix_operation(&[
                "shell".into(),
                "nixpkgs#cargo".into(),
                "-c".into(),
                "cargo".into(),
                "build".into()
            ]),
            Some("shell")
        );
        assert_eq!(
            nix_operation(&[
                "--option".into(),
                "sandbox".into(),
                "false".into(),
                "eval".into(),
                "--expr".into(),
                "foo".into()
            ]),
            Some("eval")
        );
    }

    /// Flags in front of the subcommand: nix 2.36 accepts `-L` and `--`.
    #[test]
    fn operation_after_short_flag_attached_value_and_double_dash() {
        assert_eq!(
            nix_operation(&args(&["-L", "search", "nixpkgs", "jq"])),
            Some("search")
        );
        assert_eq!(
            nix_operation(&args(&["--log-format=raw", "search", "nixpkgs", "jq"])),
            Some("search")
        );
        assert_eq!(
            nix_operation(&args(&["--", "search", "nixpkgs", "jq"])),
            Some("search")
        );
        assert_eq!(nix_operation(&args(&["--offline"])), None);
    }
}
