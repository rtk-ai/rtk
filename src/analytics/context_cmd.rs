//! Claude Code context-budget inspection.
//!
//! This command reports measurable always-on prompt surfaces before a session
//! starts. It is intentionally conservative: it counts files RTK can read and
//! separates configured MCP servers from unmeasurable built-in/system prompt
//! overhead.

use crate::core::tracking::estimate_tokens;
use crate::discover::provider::ClaudeProvider;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
struct BudgetRow {
    component: &'static str,
    tokens: usize,
    note: String,
}

impl BudgetRow {
    fn new(component: &'static str, tokens: usize, note: impl Into<String>) -> Self {
        Self {
            component,
            tokens,
            note: note.into(),
        }
    }
}

/// Run `rtk context`.
pub fn run(_verbose: u8) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let home = dirs::home_dir();

    let Some(home_dir) = home else {
        println!("Claude Code not detected: could not determine home directory");
        return Ok(());
    };

    let claude_dir = home_dir.join(".claude");
    if !claude_dir.exists() {
        println!("Claude Code not detected: {} not found", claude_dir.display());
        return Ok(());
    }

    let rows = collect_rows(&claude_dir, &cwd)?;
    print_table(&rows);
    Ok(())
}

fn collect_rows(claude_dir: &Path, cwd: &Path) -> Result<Vec<BudgetRow>> {
    let encoded_cwd = ClaudeProvider::encode_project_path(&cwd.to_string_lossy());
    let project_memory = claude_dir
        .join("projects")
        .join(encoded_cwd)
        .join("memory")
        .join("MEMORY.md");

    let mut rows = Vec::new();
    rows.push(file_row(
        "CLAUDE.md (global)",
        &claude_dir.join("CLAUDE.md"),
        ImportMode::Detect,
    )?);
    rows.push(file_row("CLAUDE.md (project)", &cwd.join("CLAUDE.md"), ImportMode::Detect)?);
    rows.push(file_row(
        "MEMORY.md (global)",
        &claude_dir.join("memory").join("MEMORY.md"),
        ImportMode::None,
    )?);
    rows.push(file_row(
        "MEMORY.md (project)",
        &project_memory,
        ImportMode::None,
    )?);
    rows.push(dir_markdown_row("Skills", &claude_dir.join("skills"), "SKILL.md")?);
    rows.push(dir_markdown_row("Commands", &claude_dir.join("commands"), ".md")?);
    rows.push(dir_markdown_row("Rules", &cwd.join(".claude").join("rules"), ".md")?);
    rows.push(mcp_row(&claude_dir.join("settings.json"))?);
    Ok(rows)
}

#[derive(Debug, Clone, Copy)]
enum ImportMode {
    None,
    Detect,
}

fn file_row(component: &'static str, path: &Path, imports: ImportMode) -> Result<BudgetRow> {
    if !path.exists() {
        return Ok(BudgetRow::new(component, 0, "not found"));
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let mut note = relative_note(path);
    if matches!(imports, ImportMode::Detect) {
        let detected = detect_imports(&text);
        if !detected.is_empty() {
            note = format!(
                "{}; @imports detected: {} (not resolved)",
                note,
                detected.join(", ")
            );
        }
    }
    Ok(BudgetRow::new(component, estimate_tokens(&text), note))
}

fn dir_markdown_row(component: &'static str, dir: &Path, suffix: &str) -> Result<BudgetRow> {
    if !dir.exists() {
        return Ok(BudgetRow::new(component, 0, "not found"));
    }

    let mut count = 0usize;
    let mut tokens = 0usize;
    for entry in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if suffix == "SKILL.md" {
            if name != "SKILL.md" {
                continue;
            }
        } else if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        count += 1;
        tokens += estimate_tokens(&text);
    }

    Ok(BudgetRow::new(
        component,
        tokens,
        format!("{} file{}", count, if count == 1 { "" } else { "s" }),
    ))
}

fn mcp_row(path: &Path) -> Result<BudgetRow> {
    if !path.exists() {
        return Ok(BudgetRow::new("MCP servers", 0, "settings.json not found"));
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return Ok(BudgetRow::new("MCP servers", 0, "settings.json malformed"));
    };
    let count = json
        .get("mcpServers")
        .and_then(Value::as_object)
        .map_or(0, |servers| servers.len());
    Ok(BudgetRow::new(
        "MCP servers",
        0,
        format!("{} configured (tool schemas not measurable)", count),
    ))
}

fn detect_imports(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix('@')
                .and_then(|rest| rest.split_whitespace().next())
                .filter(|import| import.ends_with(".md"))
                .map(|import| format!("@{import}"))
        })
        .collect()
}

fn relative_note(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| path.display().to_string(), ToString::to_string)
}

fn print_table(rows: &[BudgetRow]) {
    let total: usize = rows.iter().map(|row| row.tokens).sum();
    println!("RTK Context Budget");
    println!("--------------------------------------");
    println!("{:<24} {:>8}  {}", "Component", "Tokens", "Note");
    println!("--------------------------------------");
    for row in rows {
        println!("{:<24} {:>8}  {}", row.component, row.tokens, row.note);
    }
    println!("--------------------------------------");
    println!("{:<24} {:>8}", "TOTAL MEASURABLE", total);
    println!("--------------------------------------");
    println!("Note: system prompt + built-in tools are not measurable by RTK");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_markdown_imports_at_line_start() {
        let imports = detect_imports("hello\n@RTK.md\n  @docs/TONE.md extra\nnot @inline.md\n@script.js");
        assert_eq!(imports, vec!["@RTK.md", "@docs/TONE.md"]);
    }

    #[test]
    fn missing_file_is_zero_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let row = file_row("CLAUDE.md (project)", &dir.path().join("CLAUDE.md"), ImportMode::Detect)
            .unwrap();
        assert_eq!(row.tokens, 0);
        assert_eq!(row.note, "not found");
    }

    #[test]
    fn mcp_row_counts_server_keys_without_token_estimate() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        fs::write(&settings, r#"{"mcpServers":{"a":{},"b":{}}}"#).unwrap();
        let row = mcp_row(&settings).unwrap();
        assert_eq!(row.tokens, 0);
        assert_eq!(row.note, "2 configured (tool schemas not measurable)");
    }
}
