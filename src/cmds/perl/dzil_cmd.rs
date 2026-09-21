//! dzil (Dist::Zilla) filter: drops build progress, compacts the test run of `dzil test`.
//!
//! `[DZ]` progress lines (beginning to build, guessing the main module, writing files, where the
//! build directory is) carry nothing once the build succeeds or fails; every other plugin line,
//! and every error, stays. `dzil build` keeps its `built in` line. `dzil test`, `dzil release`
//! and `dzil install` run `make test`, whose output goes through the shared TAP filter.

use regex::Regex;
use std::sync::LazyLock;

use super::tap::filter_harness_run;
use super::utils::unfiltered;
use crate::core::arg_tokenizer::{self, Dialect};
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

static DZ_PROGRESS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^\[DZ\] (?:beginning to build |guessing dist's main_module is |writing \S+ in |",
        r"writing archive to |building (?:test )?distribution under |all's well; removing |",
        r"Extracting )"
    ))
    .unwrap()
});

/// Commands whose output is not a build log: help text, or lists meant for a pipe.
const RAW_COMMANDS: &[&str] = &["help", "commands", "version", "nop", "setup", "new"];

fn is_dzil_noise(line: &str) -> bool {
    DZ_PROGRESS_RE.is_match(line)
}

pub fn filter_dzil(raw: &str) -> String {
    filter_harness_run(raw, is_dzil_noise)
}

fn filters_this_invocation(args: &[String]) -> bool {
    // Only the command word matters, and dzil's global options take no values.
    let tokens = arg_tokenizer::tokenize_grammar(args, &|_, _| None, Dialect::Posix);
    let first = arg_tokenizer::before_dashdash(&tokens)
        .iter()
        .find(|t| t.is_free_positional())
        .map(|t| t.text);
    !matches!(first, Some(cmd) if RAW_COMMANDS.contains(&cmd))
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("dzil");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: dzil {}", args.join(" "));
    }

    let filter: fn(&str) -> String = if filters_this_invocation(args) {
        filter_dzil
    } else {
        unfiltered
    };

    runner::run_filtered(
        cmd,
        "dzil",
        &args.join(" "),
        filter,
        runner::RunOptions::with_tee("dzil"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_build() {
        let input = include_str!("../../../tests/fixtures/perl/dzil_build_raw.txt");
        assert_eq!(filter_dzil(input), "[DZ] built in Acme-RtkSample-0.01");
    }

    #[test]
    fn test_build_error_kept() {
        let input = include_str!("../../../tests/fixtures/perl/dzil_build_dupfile_raw.txt");
        let output = filter_dzil(input);
        assert!(output.starts_with("aborting; duplicate files would be produced"));
        assert!(output.contains("[DZ] attempt to add Makefile.PL multiple times"));
        assert!(!output.contains("beginning to build"));
    }

    #[test]
    fn test_test_run() {
        let input = include_str!("../../../tests/fixtures/perl/dzil_test_raw.txt");
        let output = filter_dzil(input);
        assert!(!output.contains("[DZ]"));
        assert!(!output.contains("Writing Makefile"));
        assert!(!output.contains("PERL_DL_NONLAZY"));
        assert!(output.contains("Passed: t/00-load.t, t/01-basic.t, t/06-todo-skip.t\n"));
        assert!(output.contains(
            "#     doesn't match '(?^:^big$)'\nt/02-fail-more.t: Failed 3/5 subtests (exit 3)\n  Failed tests: 2-4\n"
        ));
        assert!(output.contains("Failed 4/7 test programs. 4/17 subtests failed."));
        assert!(output.ends_with("error running make test"));
        let pct = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(pct >= 25.0, "expected >=25% savings, got {:.1}%", pct);
    }

    #[test]
    fn test_listdeps_passes_through() {
        let input = "Test::More\nTest2::V0\nstrict\nwarnings\n";
        assert_eq!(filter_dzil(input), input.trim_end());
    }

    #[test]
    fn test_invocation_detection() {
        assert!(filters_this_invocation(&args(&["build"])));
        assert!(filters_this_invocation(&args(&["test", "--release"])));
        assert!(!filters_this_invocation(&args(&["help", "build"])));
        assert!(!filters_this_invocation(&args(&["new", "Foo::Bar"])));
    }
}
