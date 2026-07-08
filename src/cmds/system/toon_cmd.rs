//! Converts JSON to TOON (Token-Oriented Object Notation) for LLM token savings.
//!
//! TOON encodes JSON data with 30–60% fewer tokens by using tabular arrays,
//! indentation-based objects, and minimal quoting. Use this when feeding JSON
//! API responses to LLMs or agents.

use crate::core::guard::never_worse;
use crate::core::toon;
use crate::core::tracking;
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{self, Read};
use std::path::Path;

/// Reject non-JSON files with a clear error before doing any I/O.
fn validate_json_extension(file: &Path) -> Result<()> {
    if let Some(ext) = file.extension().and_then(|e| e.to_str()) {
        let format_name = match ext {
            "toml" => Some("TOML"),
            "yaml" | "yml" => Some("YAML"),
            "xml" => Some("XML"),
            "csv" => Some("CSV"),
            "ini" => Some("INI"),
            "env" => Some("env"),
            "txt" => Some("plain text"),
            _ => None,
        };
        if let Some(fmt) = format_name {
            bail!(
                "{} is not a JSON file (detected {}). Use `rtk read` for non-JSON files.",
                file.display(),
                fmt
            );
        }
    }
    Ok(())
}

/// Convert a JSON file to TOON format.
pub fn run(file: &Path, verbose: u8) -> Result<()> {
    validate_json_extension(file)?;
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Converting JSON to TOON: {}", file.display());
    }

    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let output = toon::json_str_to_toon(&content)?;
    let shown = never_worse(&content, &output);
    println!("{}", shown);
    timer.track(
        &format!("cat {}", file.display()),
        "rtk toon",
        &content,
        shown,
    );
    Ok(())
}

/// Convert JSON from stdin to TOON format.
pub fn run_stdin(verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Converting JSON from stdin to TOON");
    }

    let mut content = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut content)
        .context("Failed to read from stdin")?;

    let output = toon::json_str_to_toon(&content)?;
    let shown = never_worse(&content, &output);
    println!("{}", shown);
    timer.track("cat - (stdin)", "rtk toon -", &content, shown);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toml_rejected() {
        let err = validate_json_extension(Path::new("config.toml")).unwrap_err();
        assert!(err.to_string().contains("not a JSON file"));
    }

    #[test]
    fn test_json_accepted() {
        assert!(validate_json_extension(Path::new("data.json")).is_ok());
    }

    #[test]
    fn test_no_extension_accepted() {
        assert!(validate_json_extension(Path::new("Makefile")).is_ok());
    }
}
