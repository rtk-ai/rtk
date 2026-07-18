use crate::core::utils::{composer_tool_paths, resolve_binary, resolved_command};
use lazy_static::lazy_static;
use regex::Regex;
use std::path::Path;
use std::process::Command;

lazy_static! {
    static ref ANSI_RE: Regex = Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap();
    static ref CONTROL_RE: Regex = Regex::new(r"[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]").unwrap();
}

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
