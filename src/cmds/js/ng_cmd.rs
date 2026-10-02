//! Compacts Angular build table padding without discarding bundles or diagnostics.

use crate::core::arg_tokenizer::{TokenKind, tokenize};
use crate::core::args_utils::restore_double_dash;
use crate::core::guard::never_worse;
use crate::core::runner;
use crate::core::stream;
use crate::core::tracking::TimedExecution;
use crate::core::utils::{ChildArgExt, resolved_command, strip_ansi};
use anyhow::{Context, Result};
use regex::Regex;
use std::io::Write;
use std::path::Path;
use std::sync::LazyLock;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let args = restore_double_dash(args);
    if !should_filter(&args)
        || crate::core::user_dirs::current_dir().map_or(true, |dir| workspace_may_watch(&dir))
    {
        return runner::run_passthrough(
            "ng",
            &args.iter().map(Into::into).collect::<Vec<_>>(),
            verbose,
        );
    }

    let result = run_build(&args, verbose);
    // Cancellation still delivers captured diagnostics before reproducing the signal.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    stream::die_by_relayed_signal();
    result
}

fn run_build(args: &[String], verbose: u8) -> Result<i32> {
    let display = args.join(" ");
    if verbose > 0 {
        eprintln!("Running: ng {display}");
    }
    let timer = TimedExecution::start();
    let mut cmd = resolved_command("ng");
    cmd.child_args(args);
    // Angular warnings are stderr, even on success. The shared stdout-only runner's
    // stderr cap would hide budget diagnostics, so keep that stream in full here.
    let result = stream::exec_capture_stdin_with_relay(&mut cmd).context("Failed to run ng")?;
    let raw = result.combined();
    let filtered = filter_build(&result.stdout);
    let hint = crate::core::tee::tee_and_hint(&raw, "ng_build", result.exit_code);
    let candidate = match hint {
        Some(hint) if filtered != result.stdout => {
            let separator = if filtered.ends_with('\n') { "" } else { "\n" };
            format!("{filtered}{separator}{hint}\n")
        }
        _ => filtered,
    };
    let shown = never_worse(&result.stdout, &candidate);
    print!("{shown}");
    eprint!("{}", result.stderr);
    timer.track(
        &format!("ng {display}"),
        &format!("rtk ng {display}"),
        &raw,
        &format!("{shown}{}", result.stderr),
    );
    Ok(result.exit_code)
}

fn should_filter(args: &[String]) -> bool {
    // Angular/yargs treats a separate option-looking argument as an option even
    // after a value-taking flag (`--output-path --watch`). We only detect flags,
    // never read their values, so do not consume them as linked value tokens.
    let tokens = tokenize(args);
    tokens
        .first()
        .is_some_and(|t| t.is_free_positional() && t.text == "build")
        && !tokens.iter().any(|t| match t.kind {
            // Pass through even explicit false values: correctness before savings.
            TokenKind::Long => matches!(
                t.text,
                "help" | "verbose" | "watch" | "no-help" | "no-verbose" | "no-watch" | "json-help"
            ),
            TokenKind::Short => matches!(t.text, "h" | "v" | "w"),
            _ => false,
        })
}

fn workspace_may_watch(start: &Path) -> bool {
    // Named/default configurations may enable watch without a CLI flag. Avoid
    // duplicating Angular's merge rules: any watch:true opts the workspace out.
    // JSONC/unknown configuration is also passed through rather than guessed.
    for dir in crate::core::user_dirs::ancestors(start) {
        for name in ["angular.json", ".angular.json"] {
            match std::fs::read(dir.join(name)) {
                Ok(bytes) => {
                    return serde_json::from_slice::<serde_json::Value>(&bytes)
                        .map_or(true, |value| {
                            contains_watch(&value) || !known_builders(&value)
                        });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return true,
            }
        }
    }
    // Angular can fall back to a workspace beside its installed CLI. Without a
    // local workspace we cannot establish that the selected builder is finite.
    true
}

fn known_builders(value: &serde_json::Value) -> bool {
    let Some(projects) = value.get("projects").and_then(serde_json::Value::as_object) else {
        return false;
    };
    !projects.is_empty()
        && projects.values().all(|project| {
            let builder = project
                .get("architect")
                .or_else(|| project.get("targets"))
                .and_then(|targets| targets.get("build"))
                .and_then(|build| build.get("builder"))
                .and_then(serde_json::Value::as_str);
            matches!(
                builder,
                Some(
                    "@angular/build:application"
                        | "@angular-devkit/build-angular:application"
                        | "@angular-devkit/build-angular:browser"
                        | "@angular-devkit/build-angular:browser-esbuild"
                )
            )
        })
}

fn contains_watch(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => {
            object.get("watch").and_then(serde_json::Value::as_bool) == Some(true)
                || object.values().any(contains_watch)
        }
        serde_json::Value::Array(array) => array.iter().any(contains_watch),
        _ => false,
    }
}

static SIZE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\d+(?:\.\d+)? (?:bytes|[kMGT]?B)|-)$").expect("size regex"));

fn cells(line: &str) -> Vec<&str> {
    line.trim_end_matches(['\r', '\n'])
        .split('|')
        .map(str::trim)
        .collect()
}

fn header_columns(line: &str) -> Option<usize> {
    // Indented lookalikes can be compiler/source context, not an Angular table.
    if !line.starts_with("Initial chunk files") && !line.starts_with("Lazy chunk files") {
        return None;
    }
    let cells = cells(line);
    if matches!(
        cells.as_slice(),
        [
            "Initial chunk files" | "Lazy chunk files",
            "Names",
            "Raw size"
        ]
    ) || matches!(
        cells.as_slice(),
        [
            "Initial chunk files" | "Lazy chunk files",
            "Names",
            "Raw size",
            "Estimated transfer size"
        ]
    ) {
        Some(cells.len())
    } else {
        None
    }
}

fn is_row(line: &str, columns: usize) -> bool {
    let cells = cells(line);
    cells.len() == columns
        && (cells[0].ends_with(".js")
            || cells[0].ends_with(".mjs")
            || cells[0].ends_with(".css")
            || (cells[0].is_empty() && cells[1] == "Initial total"))
        && !cells[1].is_empty()
        && cells[2..].iter().all(|cell| SIZE.is_match(cell))
}

fn compact_line(line: &str) -> String {
    let ending = if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    format!("{}{ending}", cells(line).join(" | "))
}

fn filter_build(output: &str) -> String {
    let lines: Vec<_> = output.split_inclusive('\n').collect();
    let mut filtered = String::with_capacity(output.len());
    let mut columns = None;
    let mut saw_table = false;
    for (index, line) in lines.iter().enumerate() {
        let clean = strip_ansi(line);
        if let Some(count) = header_columns(&clean)
            && lines
                .get(index + 1)
                .is_some_and(|next| is_row(&strip_ansi(next), count))
        {
            columns = Some(count);
            saw_table = true;
            filtered.push_str(&compact_line(&clean));
        } else if columns.is_some_and(|count| is_row(&clean, count)) {
            filtered.push_str(&compact_line(&clean));
        } else {
            // Angular separates the initial total from its rows with a blank line.
            if !clean.trim().is_empty() {
                columns = None;
            }
            filtered.push_str(line);
        }
    }
    // Only remove the exact leading progress pair once a real table was recognized.
    // Error-only output and unfamiliar builders are returned unchanged.
    if saw_table {
        for prefix in [
            "❯ Building...\n✔ Building...\n",
            "❯ Building...\r\n✔ Building...\r\n",
        ] {
            if let Some(rest) = filtered.strip_prefix(prefix) {
                return rest.to_string();
            }
        }
    }
    filtered
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUCCESS: &str = include_str!("../../../tests/fixtures/ng_build/success.stdout.txt");
    const LAZY: &str = include_str!("../../../tests/fixtures/ng_build/lazy-chunks.stdout.txt");

    fn filter_args(args: &[&str]) -> bool {
        should_filter(
            &args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn only_default_build_is_filtered() {
        for args in [
            vec!["build"],
            vec!["build", "app", "-c", "production"],
            vec!["build", "--", "--watch"],
        ] {
            assert!(filter_args(&args), "{args:?}");
        }
        for args in [
            vec![],
            vec!["test"],
            vec!["serve"],
            vec!["b"],
            vec!["build", "--watch"],
            vec!["build", "--output-path", "--watch"],
            vec!["build", "--define", "--verbose"],
            vec!["build", "--configuration", "--help"],
            vec!["build", "--watch=false"],
            vec!["build", "-w"],
            vec!["build", "--verbose"],
            vec!["build", "--help"],
            vec!["--help", "build"],
        ] {
            assert!(!filter_args(&args), "{args:?}");
        }
    }

    #[test]
    fn unknown_or_empty_output_is_byte_identical() {
        for raw in [
            "",
            "custom builder output\n  source | context\n",
            "❯ Building...\n✔ Building...\n",
            "Initial chunk files | Names | Unknown size\nthing.js | thing | huge\n",
            "Initial chunk files | Names | Raw size\nunsupported | data | 2 kB\n",
            "warning without final newline",
        ] {
            assert_eq!(filter_build(raw), raw);
        }
    }

    #[test]
    fn diagnostic_context_is_never_treated_as_a_table_without_a_header() {
        let raw =
            "✘ [ERROR] failed\n\n  main.js  | name | 10 kB\n  source context   stays   unchanged\n";
        assert_eq!(filter_build(raw), raw);
        let indented = "  Initial chunk files | Names | Raw size\n  main.js  | main  | 10 kB\n";
        assert_eq!(filter_build(indented), indented);
    }

    #[test]
    fn real_success_snapshot_keeps_all_native_fields() {
        let expected = "Initial chunk files | Names | Raw size | Estimated transfer size\n\
main-VYJRYTPO.js | main | 190.01 kB | 51.90 kB\n\
styles-3IO2SMMH.css | styles | 61 bytes | 61 bytes\n\n\
\x20| Initial total | 190.07 kB | 51.96 kB\n\n\
Application bundle generation complete. [1.777 seconds] - 2026-10-02T20:01:32.621Z\n\n\
Output location: /workspace/scratch/72640dd8823a/rtk-angular-fixture-work/fixture-app/dist/fixture-app\n\n";
        assert_eq!(filter_build(SUCCESS), expected);
    }

    #[test]
    fn real_builds_retain_every_non_progress_token() {
        let budget_warning =
            include_str!("../../../tests/fixtures/ng_build/budget-warning.stdout.txt");
        let budget_error = include_str!("../../../tests/fixtures/ng_build/budget-error.stdout.txt");
        for raw in [SUCCESS, LAZY, budget_warning, budget_error] {
            let filtered = filter_build(raw);
            let without_progress = raw
                .strip_prefix("❯ Building...\n✔ Building...\n")
                .expect("native progress");
            assert_eq!(
                filtered.split_whitespace().collect::<Vec<_>>(),
                without_progress.split_whitespace().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn real_compiler_failures_and_multiline_context_are_unchanged() {
        for raw in [
            include_str!("../../../tests/fixtures/ng_build/typescript-error.stdout.txt"),
            include_str!("../../../tests/fixtures/ng_build/typescript-error.stderr.txt"),
            include_str!("../../../tests/fixtures/ng_build/template-error.stdout.txt"),
            include_str!("../../../tests/fixtures/ng_build/template-error.stderr.txt"),
            include_str!("../../../tests/fixtures/ng_build/budget-warning.stderr.txt"),
            include_str!("../../../tests/fixtures/ng_build/budget-error.stderr.txt"),
        ] {
            assert_eq!(filter_build(raw), raw);
        }
    }

    #[test]
    fn real_default_and_lazy_builds_clear_twenty_percent_with_the_production_estimator() {
        use crate::core::tracking::estimate_tokens;
        for raw in [SUCCESS, LAZY] {
            let before = estimate_tokens(raw);
            let after = estimate_tokens(&filter_build(raw));
            assert!(after * 100 <= before * 80, "{before} -> {after}");
        }
    }

    #[test]
    fn crlf_color_and_missing_final_newline_do_not_damage_unknown_lines() {
        let raw = "\x1b[32mInitial chunk files\x1b[0m | Names | Raw size\r\nmain-a.js   | main   | 1.00 kB\r\n\r\n\x1b[33mCustom diagnostic\x1b[0m\r\n  context   spacing";
        assert_eq!(
            filter_build(raw),
            "Initial chunk files | Names | Raw size\r\nmain-a.js | main | 1.00 kB\r\n\r\n\x1b[33mCustom diagnostic\x1b[0m\r\n  context   spacing"
        );
    }

    #[test]
    fn malformed_rows_and_unknown_builder_sections_stay_verbatim() {
        let raw = "Initial chunk files | Names | Raw size\nmain-a.js   | main   | 1.00 kB\nCUSTOM UNKNOWN   | main | 2.00 kB\n  compiler context  | stays | unchanged\n";
        assert!(filter_build(raw).ends_with(
            "CUSTOM UNKNOWN   | main | 2.00 kB\n  compiler context  | stays | unchanged\n"
        ));
    }
}
