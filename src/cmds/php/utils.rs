use crate::core::utils::{composer_tool_paths, resolve_binary, resolved_command};
use regex::Regex;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

// A full CSI sequence: parameter bytes (including the `?` of private modes such
// as Laravel Prompts' cursor hide/show `ESC[?25l` / `ESC[?25h`), intermediate
// bytes, then one final byte anywhere in `@`..`~`. Matching only digits, `;`
// and a letter left `[?25l` behind once CONTROL_RE dropped the lone ESC.
static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap());
static CONTROL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]").unwrap());

pub fn php_tool_command(tool: &str) -> Command {
    for local_tool in composer_tool_paths(tool) {
        let local_tool_name = local_tool.to_string_lossy().into_owned();
        // Route through resolved_command (the sanctioned constructor) rather than
        // a raw dynamic command constructor, so the binary still resolves
        // PATHEXT-aware on Windows and the security scan's no-dynamic-exec rule holds.
        if resolve_binary(&local_tool_name).is_ok() || local_tool.exists() {
            return resolved_command(&local_tool_name);
        }
    }

    resolved_command(tool)
}

fn composer_tool_exists(tool: &str) -> bool {
    composer_tool_paths(tool).into_iter().any(|local_tool| {
        let local_tool_name = local_tool.to_string_lossy().into_owned();
        resolve_binary(&local_tool_name).is_ok() || local_tool.exists()
    })
}

pub fn strip_ansi_and_controls(input: &str) -> String {
    let no_ansi = ANSI_RE.replace_all(input, "");
    CONTROL_RE.replace_all(&no_ansi, "").to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhpTestRunner {
    Pest,
    Phpunit,
    Unknown,
}

pub fn detect_php_test_runner() -> PhpTestRunner {
    // Pest's canonical marker is the `vendor/bin/pest` binary (composer dep).
    // There is no root `pest.php` file — Pest's bootstrap lives at `tests/Pest.php`
    // — so a root-level `pest.php` check both never matches Pest and false-positives
    // on unrelated utility files in PHPUnit-only projects.
    if composer_tool_exists("pest") {
        return PhpTestRunner::Pest;
    }

    if composer_tool_exists("phpunit")
        || Path::new("phpunit.xml").exists()
        || Path::new("phpunit.xml.dist").exists()
    {
        return PhpTestRunner::Phpunit;
    }

    PhpTestRunner::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_and_controls_removes_private_mode_sequences() {
        let input = "\x1b[?25l\x1b[32mINFO\x1b[0m  Running migrations.\x1b[?25h";
        assert_eq!(strip_ansi_and_controls(input), "INFO  Running migrations.");
    }

    #[test]
    fn test_strip_ansi_and_controls_removes_non_letter_final_bytes() {
        // ESC[2~ (Insert) and ESC[1@ (insert character) end in `~` and `@`.
        assert_eq!(strip_ansi_and_controls("a\x1b[2~b\x1b[1@c"), "abc");
    }

    #[test]
    fn test_strip_ansi_and_controls_keeps_plain_brackets() {
        assert_eq!(
            strip_ansi_and_controls("[OK] done [1/3]"),
            "[OK] done [1/3]"
        );
    }
}
