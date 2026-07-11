use crate::core::utils::{resolve_binary, resolved_command};
use lazy_static::lazy_static;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;

lazy_static! {
    static ref ANSI_RE: Regex = Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap();
    static ref CONTROL_RE: Regex = Regex::new(r"[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]").unwrap();
}

pub fn php_tool_command(tool: &str) -> Command {
    let local_tool = composer_tool_path(tool);
    let local_tool_name = local_tool.to_string_lossy().into_owned();
    // This branch predates the shared Composer-bin resolver. Keep the standard
    // project-local vendor/bin path before falling back to a global PATH tool.
    if resolve_binary(&local_tool_name).is_ok() || local_tool.exists() {
        return resolved_command(&local_tool_name);
    }

    resolved_command(tool)
}

fn composer_tool_exists(tool: &str) -> bool {
    let local_tool = composer_tool_path(tool);
    let local_tool_name = local_tool.to_string_lossy().into_owned();
    resolve_binary(&local_tool_name).is_ok() || local_tool.exists()
}

fn composer_tool_path(tool: &str) -> PathBuf {
    Path::new("vendor").join("bin").join(tool)
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
    // There is no root `pest.php` file 鈥?Pest's bootstrap lives at `tests/Pest.php`
    // 鈥?so a root-level `pest.php` check both never matches Pest and false-positives
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
