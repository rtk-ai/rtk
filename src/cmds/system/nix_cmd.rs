//! Compact `nix search` output. Every other nix command is passed through.
//!
//! `nix search` outside a terminal prints one `evaluating …` line per package
//! (hundreds of thousands of lines on nixpkgs). The filter keeps the package
//! list and diagnostics, including the location lines that follow an error.

use crate::core::runner::{self, RunOptions};
use crate::core::shell::display_args;
use crate::core::utils::{ChildArgExt, resolved_command, strip_ansi};
use anyhow::Result;
use std::ffi::OsString;

const MAX_SEARCH_RESULTS: usize = 20;
const MAX_FALLBACK_LINES: usize = 40;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if nix_operation(args) != Some("search") {
        let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough("nix", &os_args, verbose);
    }

    let mut cmd = resolved_command("nix");
    cmd.child_args(args);

    if verbose > 0 {
        eprintln!("Running: nix {}", display_args(args));
    }

    runner::run_filtered(
        cmd,
        "nix",
        &display_args(args),
        filter_nix_search,
        RunOptions::with_tee("nix"),
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

fn filter_nix_search(raw: &str) -> String {
    let clean = strip_ansi(raw);
    let mut entries: Vec<String> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();
    let mut fallback: Vec<String> = Vec::new();
    let mut pending_entry: Option<String> = None;
    let mut in_diagnostic = false;
    let mut diagnostic_blanks = 0;

    for line in clean.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if let Some(entry) = pending_entry.take() {
                entries.push(entry);
            }
            if in_diagnostic {
                diagnostic_blanks += 1;
                if diagnostic_blanks >= 2 {
                    in_diagnostic = false;
                }
            }
            continue;
        }
        diagnostic_blanks = 0;

        if let Some(entry) = pending_entry.take() {
            if line.starts_with("  ") && !trimmed.starts_with('*') {
                entries.push(format!("{entry} - {}", trimmed));
                in_diagnostic = false;
                continue;
            }
            entries.push(entry);
        }

        if trimmed.starts_with("* ") {
            let normalized = normalize_entry_name(trimmed.trim_start_matches("* "));
            if is_low_signal_entry(&normalized) {
                in_diagnostic = false;
                continue;
            }
            pending_entry = Some(normalized);
            in_diagnostic = false;
            continue;
        }

        let lower = trimmed.to_lowercase();
        if lower.starts_with("error:") || lower.starts_with("warning:") {
            diagnostics.push(trimmed.to_string());
            in_diagnostic = true;
            continue;
        }

        if in_diagnostic && (line.starts_with(' ') || is_location_line(trimmed)) {
            diagnostics.push(trimmed.to_string());
            continue;
        }
        in_diagnostic = false;

        if is_noise_line(trimmed) {
            continue;
        }

        fallback.push(trimmed.to_string());
    }

    if let Some(entry) = pending_entry.take() {
        entries.push(entry);
    }

    if !entries.is_empty() {
        let shown = entries.len().min(MAX_SEARCH_RESULTS);
        let mut out = vec![format!(
            "Nix: {} results (showing {})",
            entries.len(),
            shown
        )];
        out.extend(entries.into_iter().take(shown));
        if !diagnostics.is_empty() {
            out.push(String::new());
            out.extend(diagnostics);
        }
        return out.join("\n");
    }

    if !diagnostics.is_empty() {
        return diagnostics.join("\n");
    }

    fallback
        .into_iter()
        .take(MAX_FALLBACK_LINES)
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_noise_line(line: &str) -> bool {
    line.starts_with("evaluating '")
        || line.starts_with("copying path '")
        || line.starts_with("copying ")
        || line.starts_with("downloading ")
        || line.starts_with("building '")
        || line.starts_with("unpacking ")
        || line.starts_with("querying info about ")
        || line.starts_with("warning: ignoring the client-specified setting")
}

fn is_location_line(line: &str) -> bool {
    line.starts_with("at ") || line.contains(".nix:")
}

fn normalize_entry_name(entry: &str) -> String {
    if let Some(rest) = entry.strip_prefix("legacyPackages.")
        && let Some(dot_idx) = rest.find('.')
    {
        return rest[dot_idx + 1..].to_string();
    }
    entry.to_string()
}

fn is_low_signal_entry(entry: &str) -> bool {
    entry.starts_with("tests.") || entry.contains(".tests.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_nix_search_drops_evaluating_noise() {
        let raw = r#"
evaluating 'legacyPackages.x86_64-linux.hello'...
* legacyPackages.x86_64-linux.hello (2.12.2)
  Program that produces a familiar, friendly greeting
evaluating 'legacyPackages.x86_64-linux.hello-wayland'...
* legacyPackages.x86_64-linux.hello-wayland (0-unstable)
  Hello world Wayland client
"#;
        let filtered = filter_nix_search(raw);
        assert!(filtered.contains("Nix: 2 results"));
        assert!(filtered.contains("hello (2.12.2) - Program"));
        assert!(!filtered.contains("evaluating"));
    }

    #[test]
    fn test_filter_nix_keeps_errors() {
        let raw = r#"
evaluating 'legacyPackages.x86_64-linux.foo'...
error: attribute 'foo' missing
"#;
        let filtered = filter_nix_search(raw);
        assert!(filtered.contains("error: attribute 'foo' missing"));
        assert!(!filtered.contains("evaluating"));
    }

    #[test]
    fn test_filter_nix_keeps_error_location() {
        let raw = "\
error: undefined variable 'foo'

       at /tmp/bad-eval.nix:5:30:
";
        let filtered = filter_nix_search(raw);
        assert!(filtered.contains("error: undefined variable 'foo'"));
        assert!(filtered.contains("at /tmp/bad-eval.nix:5:30:"));
    }

    #[test]
    fn test_filter_nix_drops_test_entries_and_normalizes_prefix() {
        let raw = r#"
* legacyPackages.x86_64-linux.tests.foo
  test fixture
* legacyPackages.x86_64-linux.hello (2.12.2)
  Program that produces a familiar, friendly greeting
"#;
        let filtered = filter_nix_search(raw);
        assert!(!filtered.contains("tests.foo"));
        assert!(filtered.contains("hello (2.12.2)"));
        assert!(!filtered.contains("legacyPackages.x86_64-linux"));
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
}
