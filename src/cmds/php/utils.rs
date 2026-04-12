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
        if let Ok(resolved_tool) = resolve_binary(&local_tool_name) {
            return Command::new(resolved_tool);
        }

        if local_tool.exists() {
            return Command::new(local_tool);
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
    if composer_tool_exists("pest") || Path::new("pest.php").exists() {
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
