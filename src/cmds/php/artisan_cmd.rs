//! Laravel Artisan output cleanup helpers.

use super::test_output::filter_test_runner_output;
use super::utils::{PhpTestRunner, strip_ansi_and_controls};
use crate::core::arg_tokenizer::{Dialect, TokenKind, ValueSpec, tokenize_grammar};
use regex::Regex;
use std::sync::LazyLock;

static BOX_CHARS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\u{2500}-\u{257F}\u{2580}-\u{259F}\u{25A0}-\u{25FF}\u{27A0}-\u{27BF}]+").unwrap()
});
static DOTS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.{3,}").unwrap());
static MULTI_SPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]{2,}").unwrap());
static MULTI_BLANK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());
/// Collision/Pest rule that opens the failure details after the per-file results.
static FAILURE_RULE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\u{2500}{8,}\s*$").unwrap());
static PASS_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*PASS\s").unwrap());
static PASS_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[\u{2713}\u{221A}]\s").unwrap());

/// What the user asked `artisan` to do, as far as filtering is concerned.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ArtisanCall {
    /// The artisan command (`test`, `migrate:fresh`, ...), absent for bare `artisan`.
    pub name: Option<String>,
    /// `-v`/`--verbose`/`--testdox`: the user wants the detail the filter would drop.
    pub verbose: bool,
    /// `--json`: machine-readable output that text cleanup would corrupt.
    pub json: bool,
}

impl ArtisanCall {
    pub fn is_test(&self) -> bool {
        self.name.as_deref() == Some("test")
    }
}

/// Parses the arguments that follow `artisan` (so not including the word itself).
pub fn parse_artisan_call(args: &[String]) -> ArtisanCall {
    // `--env` is the only global option that takes a separate value; without it
    // `artisan --env testing test` would read `testing` as the command.
    let tokens = tokenize_grammar(
        args,
        &|kind, name| match (kind, name) {
            (TokenKind::Long, "env") | (TokenKind::Short, "e") => Some(ValueSpec::value()),
            _ => None,
        },
        Dialect::Posix,
    );
    let name = tokens
        .iter()
        .find(|t| t.is_free_positional())
        .map(|t| t.text.to_string());
    let has_long = |wanted: &str| {
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::Long && t.text == wanted)
    };
    let verbose = has_long("verbose")
        || has_long("testdox")
        || tokens
            .iter()
            .any(|t| t.kind == TokenKind::Short && t.text == "v");
    ArtisanCall {
        name,
        verbose,
        json: has_long("json"),
    }
}

pub fn filter_artisan_output(output: &str) -> String {
    let mut cleaned = strip_ansi_and_controls(output);
    cleaned = BOX_CHARS_RE.replace_all(&cleaned, "").to_string();
    cleaned = DOTS_RE.replace_all(&cleaned, "..").to_string();
    cleaned = MULTI_SPACE_RE.replace_all(&cleaned, " ").to_string();
    cleaned = MULTI_BLANK_RE.replace_all(&cleaned, "\n\n").to_string();
    cleaned.trim().to_string()
}

/// Drops what a passing run adds to Collision's (and Pest's) per-file listing: `PASS` headers,
/// `✓` lines and blank lines. Failing, skipped and warning tests, the failure details that
/// follow the first rule line, and the `Tests:`/`Duration:` summary are kept.
pub fn filter_collision_test_output(output: &str) -> String {
    let plain = strip_ansi_and_controls(output);
    let mut kept: Vec<&str> = Vec::new();
    let mut in_details = false;

    for line in plain.lines() {
        let line = line.trim_end();
        if FAILURE_RULE_RE.is_match(line) {
            in_details = true;
        }
        if in_details {
            // Keep one blank line between detail blocks, no more.
            if !(line.is_empty() && kept.last().is_none_or(|l| l.is_empty())) {
                kept.push(line);
            }
        } else if !(line.is_empty() || PASS_HEADER_RE.is_match(line) || PASS_LINE_RE.is_match(line))
        {
            kept.push(line);
        }
    }

    kept.join("\n").trim().to_string()
}

pub fn filter_artisan_test_output(output: &str, runner: PhpTestRunner) -> String {
    let collision = filter_collision_test_output(output);
    match runner {
        PhpTestRunner::Pest | PhpTestRunner::Phpunit => filter_test_runner_output(&collision),
        // Whitespace is meaningful in failure snippets, so skip the generic cleanup.
        PhpTestRunner::Unknown => collision,
    }
}

/// Filters the output of one `artisan` run. Anything the user asked to see in more detail
/// or in a machine format is passed through untouched.
pub fn filter_artisan_call(call: &ArtisanCall, runner: PhpTestRunner, output: &str) -> String {
    if call.json || (call.is_test() && call.verbose) {
        return output.to_string();
    }
    if call.is_test() {
        return filter_artisan_test_output(output, runner);
    }
    filter_artisan_output(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artisan_cleanup() {
        let out = "\u{1b}[32mEnvironment .....\u{1b}[0m\n\u{2502} Laravel Version \u{2502} 13.0.0 \u{2502}\n\n\n";
        let filtered = filter_artisan_output(out);
        assert!(!filtered.contains('\u{1b}'));
        assert!(!filtered.contains('\u{2502}'));
        assert!(filtered.contains("Environment .."));
    }

    #[test]
    fn test_artisan_test_prefers_runner_filter() {
        let output = "PHPUnit 12.2.0\n....\nOK (4 tests, 4 assertions)\n";
        let filtered = filter_artisan_test_output(output, PhpTestRunner::Phpunit);
        assert!(!filtered.contains("PHPUnit 12.2.0"));
        assert!(filtered.contains("OK (4 tests, 4 assertions)"));
    }

    const TEST_FAIL: &str =
        include_str!("../../../tests/fixtures/laravel/artisan_test_fail_raw.txt");
    const TEST_FAIL_PAO: &str =
        include_str!("../../../tests/fixtures/laravel/artisan_test_fail_pao_raw.txt");
    const TEST_PASS: &str = "\n  \x1b[30;42;1m PASS \x1b[39;49;22m Tests\\Unit\\ExampleTest\n  \
        \x1b[32;1m\u{2713}\x1b[39;22m that true is true  0.01s\n\n  Tests:    1 passed (1 assertions)\n  Duration: 0.02s\n";

    fn call(words: &[&str]) -> ArtisanCall {
        let args: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        parse_artisan_call(&args)
    }

    fn test_run(raw: &str) -> String {
        filter_artisan_call(&call(&["test"]), PhpTestRunner::Phpunit, raw)
    }

    #[test]
    fn passing_run_keeps_only_the_summary() {
        let out = test_run(TEST_PASS);
        assert_eq!(
            out.lines().map(str::trim).collect::<Vec<_>>(),
            ["Tests:    1 passed (1 assertions)", "Duration: 0.02s"]
        );
    }

    #[test]
    fn failing_run_keeps_every_failure_and_the_summary() {
        let out = test_run(TEST_FAIL);
        for needle in [
            "FAIL  Tests\\Feature\\BrokenTest",
            "\u{2a2f} total is calculated",
            "\u{2a2f} missing route returns ok",
            "\u{2a2f} service throws",
            "- skipped for now",
            "Failed asserting that 48 is identical to 42.",
            "Expected response status code [200] but received 404.",
            "Target class [App\\Services\\Missing] does not exist.",
            "at tests/Feature/BrokenTest.php:11",
            "+7 vendor frames",
            "Tests:    3 failed, 1 skipped, 38 passed (52 assertions)",
        ] {
            assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
        }
        assert!(!out.contains('\u{2713}'), "passing tests leaked:\n{out}");
        assert!(!out.contains("PASS"), "PASS header leaked:\n{out}");
        assert!(out.len() * 100 <= TEST_FAIL.len() * 30);
    }

    #[test]
    fn failure_snippets_keep_their_indentation() {
        let out = filter_artisan_test_output(TEST_FAIL, PhpTestRunner::Unknown);
        assert!(out.contains("  \u{279c}  11\u{2595}         $this->assertSame(42, 6 * 8);"));
    }

    #[test]
    fn agent_json_summary_is_left_as_is() {
        assert_eq!(test_run(TEST_FAIL_PAO).trim(), TEST_FAIL_PAO.trim());
    }

    #[test]
    fn verbose_test_runs_and_json_output_are_not_filtered() {
        for flag in ["-v", "-vvv", "--verbose", "--testdox"] {
            let out =
                filter_artisan_call(&call(&["test", flag]), PhpTestRunner::Phpunit, TEST_FAIL);
            assert_eq!(out, TEST_FAIL, "{flag}");
        }
        let json = "{\n  \"routes\": []\n}";
        let out = filter_artisan_call(
            &call(&["route:list", "--json"]),
            PhpTestRunner::Unknown,
            json,
        );
        assert_eq!(out, json);
    }

    #[test]
    fn command_name_survives_global_options() {
        assert_eq!(
            call(&["--env", "testing", "test"]).name.as_deref(),
            Some("test")
        );
        assert_eq!(
            call(&["-n", "make:model", "Post"]).name.as_deref(),
            Some("make:model")
        );
        assert_eq!(call(&[]).name, None);
        assert!(!call(&["test", "--filter=verbose"]).verbose);
    }
}
