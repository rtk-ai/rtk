//! prove (TAP::Harness) filter: keeps failures, diagnostics and the summary, drops per-file `ok`.

use super::tap::filter_tap;
use super::utils::unfiltered;
use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

/// prove's value-taking options, from `prove --help` ("Options that take arguments").
fn prove_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match kind {
        TokenKind::Long => matches!(
            name,
            "exec"
                | "ext"
                | "harness"
                | "formatter"
                | "source"
                | "archive"
                | "jobs"
                | "state"
                | "statefile"
                | "rc"
                | "rules"
        )
        .then(ValueSpec::value),
        TokenKind::Short => {
            matches!(name, "I" | "P" | "M" | "e" | "a" | "j").then(ValueSpec::value)
        }
        _ => None,
    }
}

/// How the harness was asked to report, which decides whether its output is ours to compact.
#[derive(Debug, PartialEq, Eq)]
enum Mode {
    /// Compact the harness output, first adding whichever of `-v` / `-m` the user did not pass.
    Compact { add_verbose: bool, add_merge: bool },
    /// Help, version, dry runs and custom formatters print something that is not harness
    /// console output (or is a format another tool will parse), so it passes through.
    Raw,
}

fn mode(args: &[String]) -> Mode {
    // Everything after `::` is handed to the test scripts, not to prove.
    let own_args = args
        .iter()
        .position(|a| a == "::")
        .map_or(args, |i| &args[..i]);
    let tokens = arg_tokenizer::tokenize_grammar(own_args, &prove_takes_value, Dialect::Posix);
    let own = arg_tokenizer::before_dashdash(&tokens);

    let (mut verbose, mut merge, mut quiet) = (false, false, false);
    for t in own {
        match (t.kind, t.text) {
            (TokenKind::Long, "help" | "man" | "version" | "dry" | "formatter" | "harness")
            | (TokenKind::Short, "h" | "?" | "H" | "V" | "D") => return Mode::Raw,
            (TokenKind::Long, "verbose") | (TokenKind::Short, "v") => verbose = true,
            (TokenKind::Long, "merge") | (TokenKind::Short, "m") => merge = true,
            (TokenKind::Long, "quiet" | "QUIET") | (TokenKind::Short, "q" | "Q") => quiet = true,
            _ => {}
        }
    }
    // A user who asked for less output gets the harness as they configured it.
    Mode::Compact {
        add_verbose: !verbose && !quiet,
        add_merge: !merge && !quiet,
    }
}

/// Runs prove with `-v -m` added: verbose so each file's diagnostics print inside that file's
/// block, and merged so the test's STDERR (diagnostics, die messages, compile errors) lands in
/// the same stream in order. Without both, rtk captures stdout and stderr separately and every
/// diagnostic ends up after the summary, detached from its file. `-v` also sets `TEST_VERBOSE`
/// for the tests, and the filter removes the passing TAP it adds.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("prove");
    let mode = mode(args);
    if let Mode::Compact {
        add_verbose,
        add_merge,
    } = mode
    {
        if add_verbose {
            cmd.arg("-v");
        }
        if add_merge {
            cmd.arg("-m");
        }
    }
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: prove {} ({:?})", args.join(" "), mode);
    }

    let filter: fn(&str) -> String = match mode {
        Mode::Compact { .. } => filter_tap,
        Mode::Raw => unfiltered,
    };

    runner::run_filtered(
        cmd,
        "prove",
        &args.join(" "),
        filter,
        runner::RunOptions::with_tee("prove"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compact(add_verbose: bool, add_merge: bool) -> Mode {
        Mode::Compact {
            add_verbose,
            add_merge,
        }
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_mode_default_is_compact() {
        assert_eq!(mode(&args(&["-lr", "t"])), compact(true, true));
    }

    #[test]
    fn test_mode_verbose_in_cluster() {
        assert_eq!(mode(&args(&["-lv", "t"])), compact(false, true));
        assert_eq!(mode(&args(&["--verbose", "-m"])), compact(false, false));
    }

    #[test]
    fn test_mode_value_is_not_a_flag() {
        // `-I v` is a library path named "v", not the verbose flag.
        assert_eq!(mode(&args(&["-I", "v", "t"])), compact(true, true));
    }

    #[test]
    fn test_mode_script_args_ignored() {
        assert_eq!(mode(&args(&["t/a.t", "::", "-v"])), compact(true, true));
    }

    #[test]
    fn test_mode_quiet_is_respected() {
        assert_eq!(mode(&args(&["-Q", "t"])), compact(false, false));
        assert_eq!(mode(&args(&["-lq"])), compact(false, false));
    }

    #[test]
    fn test_mode_raw_for_formatter_and_help() {
        assert_eq!(
            mode(&args(&["--formatter", "TAP::Formatter::JUnit"])),
            Mode::Raw
        );
        assert_eq!(mode(&args(&["-D", "t"])), Mode::Raw);
        assert_eq!(mode(&args(&["--help"])), Mode::Raw);
    }
}
