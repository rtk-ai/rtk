//! PHP command filter: syntax-check summaries and generic cleanup.

use super::artisan_cmd::{filter_artisan_output, filter_artisan_test_output};
use super::utils::{detect_php_test_runner, strip_ansi_and_controls, PhpTestRunner};
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("php");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: php {}", args.join(" "));
    }

    let is_artisan = args.first().map(String::as_str) == Some("artisan");
    let is_artisan_test = is_artisan && args.get(1).map(String::as_str) == Some("test");
    let is_lint = args.iter().any(|a| a == "-l" || a == "--syntax-check");
    let detected_runner = if is_artisan_test {
        detect_php_test_runner()
    } else {
        PhpTestRunner::Unknown
    };

    if verbose > 0 && is_artisan_test {
        eprintln!("Detected artisan test runner: {:?}", detected_runner);
    }

    runner::run_filtered(
        cmd,
        "php",
        &args.join(" "),
        move |raw| {
            if is_lint {
                return filter_php_lint_output(raw);
            }
            if is_artisan_test {
                return filter_artisan_test_output(raw, detected_runner);
            }
            if is_artisan {
                return filter_artisan_output(raw);
            }
            filter_php_output(raw)
        },
        runner::RunOptions::default(),
    )
}

fn filter_php_lint_output(output: &str) -> String {
    let mut lines = Vec::new();

    for line in strip_ansi_and_controls(output).lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(file) = trimmed.strip_prefix("No syntax errors detected in ") {
            lines.push(format!("ok {}", file.trim()));
            continue;
        }

        if trimmed.starts_with("Errors parsing ")
            || trimmed.contains("Parse error")
            || trimmed.contains("Fatal error")
            || trimmed.contains("syntax error")
        {
            lines.push(trimmed.to_string());
        }
    }

    if lines.is_empty() {
        let fallback = output.trim();
        if fallback.is_empty() {
            "ok".to_string()
        } else {
            fallback.to_string()
        }
    } else {
        lines.join("\n")
    }
}

fn filter_php_output(output: &str) -> String {
    let cleaned = strip_ansi_and_controls(output);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "ok".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_php_lint_summary() {
        let out = "No syntax errors detected in app/Http/Controller.php\n";
        assert_eq!(filter_php_lint_output(out), "ok app/Http/Controller.php");
    }

    #[test]
    fn test_php_lint_error_preserved() {
        let out = "Errors parsing app/Foo.php\nParse error: syntax error, unexpected ')' in app/Foo.php on line 10\n";
        let filtered = filter_php_lint_output(out);
        assert!(filtered.contains("Errors parsing app/Foo.php"));
        assert!(filtered.contains("Parse error"));
    }
}
