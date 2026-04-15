//! Applies optimization suggestions (TOML filters, config changes, corrections).

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::config::Config;
use crate::optimize::suggestions::{Suggestion, SuggestionKind};

/// Result of applying a single suggestion.
#[derive(Debug)]
pub struct ApplyResult {
    pub description: String,
    pub target_path: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Apply all suggestions to disk.
pub fn apply_all(suggestions: &[Suggestion]) -> Result<Vec<ApplyResult>> {
    let mut results = Vec::new();

    for suggestion in suggestions {
        let result = apply_one(suggestion);
        results.push(result);
    }

    Ok(results)
}

/// Apply a single suggestion.
fn apply_one(suggestion: &Suggestion) -> ApplyResult {
    match &suggestion.kind {
        SuggestionKind::GenerateTomlFilter { toml_content } => apply_toml_filter(toml_content),
        SuggestionKind::TuneConfig {
            field,
            suggested,
            current: _,
        } => apply_config_tune(field, suggested),
        SuggestionKind::WriteCorrection {
            wrong,
            right,
            error_type,
        } => apply_correction(wrong, right, error_type),
        SuggestionKind::ExcludeCommand { command, reason: _ } => apply_exclude(command),
    }
}

/// Append a TOML filter to the user's global filters file.
fn apply_toml_filter(toml_content: &str) -> ApplyResult {
    let filters_path = get_global_filters_path();
    let path_str = filters_path.display().to_string();

    match append_to_file(&filters_path, toml_content) {
        Ok(()) => ApplyResult {
            description: "Added TOML filter".to_string(),
            target_path: path_str,
            success: true,
            error: None,
        },
        Err(e) => ApplyResult {
            description: "Failed to add TOML filter".to_string(),
            target_path: path_str,
            success: false,
            error: Some(e.to_string()),
        },
    }
}

/// Modify a config field.
fn apply_config_tune(field: &str, suggested: &str) -> ApplyResult {
    let result = (|| -> Result<String> {
        let mut config = Config::load().context("Failed to load config")?;
        let path = "config.toml".to_string();

        // Apply known field changes
        if field == "limits.passthrough_max_chars" {
            if let Ok(val) = suggested.parse::<usize>() {
                config.limits.passthrough_max_chars = val;
            }
        } else if field == "limits.grep_max_results" {
            if let Ok(val) = suggested.parse::<usize>() {
                config.limits.grep_max_results = val;
            }
        }
        // Other fields are informational — no auto-apply

        config.save().context("Failed to save config")?;
        Ok(path)
    })();

    match result {
        Ok(path) => ApplyResult {
            description: format!("Updated config: {} = {}", field, suggested),
            target_path: path,
            success: true,
            error: None,
        },
        Err(e) => ApplyResult {
            description: format!("Failed to update config: {}", field),
            target_path: "config.toml".to_string(),
            success: false,
            error: Some(e.to_string()),
        },
    }
}

/// Write a CLI correction to rules file.
fn apply_correction(wrong: &str, right: &str, error_type: &str) -> ApplyResult {
    let rules_path = ".claude/rules/cli-corrections.md";

    let content = format!("- Use `{}` not `{}` ({})\n", right, wrong, error_type);

    match append_to_file(Path::new(rules_path), &content) {
        Ok(()) => ApplyResult {
            description: format!("Added correction: {} -> {}", wrong, right),
            target_path: rules_path.to_string(),
            success: true,
            error: None,
        },
        Err(e) => ApplyResult {
            description: "Failed to write correction rule".to_string(),
            target_path: rules_path.to_string(),
            success: false,
            error: Some(e.to_string()),
        },
    }
}

/// Add a command to the exclude list in config.
fn apply_exclude(command: &str) -> ApplyResult {
    let result = (|| -> Result<()> {
        let mut config = Config::load().context("Failed to load config")?;

        if !config.hooks.exclude_commands.contains(&command.to_string()) {
            config.hooks.exclude_commands.push(command.to_string());
        }

        config.save().context("Failed to save config")?;
        Ok(())
    })();

    match result {
        Ok(()) => ApplyResult {
            description: format!("Excluded command: {}", command),
            target_path: "config.toml".to_string(),
            success: true,
            error: None,
        },
        Err(e) => ApplyResult {
            description: format!("Failed to exclude: {}", command),
            target_path: "config.toml".to_string(),
            success: false,
            error: Some(e.to_string()),
        },
    }
}

/// Get the global user filters.toml path.
fn get_global_filters_path() -> PathBuf {
    let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    config_dir.join("rtk").join("filters.toml")
}

/// Append content to a file, creating parent dirs and backing up if needed.
fn append_to_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Backup existing file
    if path.exists() {
        let backup = path.with_extension("toml.bak");
        fs::copy(path, &backup).with_context(|| format!("Failed to backup: {}", path.display()))?;
    }

    // Append
    let mut existing = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("Failed to read: {}", path.display()))?
    } else {
        String::new()
    };

    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push('\n');
    existing.push_str(content);

    fs::write(path, &existing).with_context(|| format!("Failed to write: {}", path.display()))?;

    Ok(())
}

/// Format a dry-run preview showing what would be applied.
pub fn format_dry_run(suggestions: &[Suggestion]) -> String {
    let mut out = String::new();

    if suggestions.is_empty() {
        out.push_str("Nothing to apply.\n");
        return out;
    }

    out.push_str(&format!(
        "Dry run: {} suggestions would be applied:\n\n",
        suggestions.len()
    ));

    for (i, s) in suggestions.iter().enumerate() {
        out.push_str(&format!("{}. [{}] ", i + 1, s.category));
        match &s.kind {
            SuggestionKind::GenerateTomlFilter { toml_content } => {
                let filters_path = get_global_filters_path();
                out.push_str(&format!("Append to {}:\n", filters_path.display()));
                // Show first few lines of TOML
                for line in toml_content.lines().take(5) {
                    out.push_str(&format!("   {}\n", line));
                }
                if toml_content.lines().count() > 5 {
                    out.push_str("   ...\n");
                }
            }
            SuggestionKind::TuneConfig {
                field,
                current,
                suggested,
            } => {
                out.push_str(&format!(
                    "Config: {} = {} -> {}\n",
                    field, current, suggested
                ));
            }
            SuggestionKind::WriteCorrection {
                wrong,
                right,
                error_type,
            } => {
                out.push_str(&format!(
                    "Correction: {} -> {} ({})\n",
                    wrong, right, error_type
                ));
            }
            SuggestionKind::ExcludeCommand { command, reason } => {
                out.push_str(&format!("Exclude: {} ({})\n", command, reason));
            }
        }
        out.push('\n');
    }

    out
}

/// Format applied results as text.
pub fn format_applied(results: &[ApplyResult]) -> String {
    let mut out = String::new();
    let success_count = results.iter().filter(|r| r.success).count();
    let fail_count = results.len() - success_count;

    out.push_str(&format!(
        "Applied: {} succeeded, {} failed\n\n",
        success_count, fail_count
    ));

    for r in results {
        let status = if r.success { "OK" } else { "FAIL" };
        out.push_str(&format!(
            "  [{}] {} -> {}\n",
            status, r.description, r.target_path
        ));
        if let Some(err) = &r.error {
            out.push_str(&format!("        Error: {}\n", err));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_dry_run_empty() {
        let result = format_dry_run(&[]);
        assert!(result.contains("Nothing to apply"));
    }

    #[test]
    fn test_format_dry_run_with_suggestions() {
        let suggestions = vec![Suggestion {
            kind: SuggestionKind::GenerateTomlFilter {
                toml_content: "[filters.test]\nmatch_command = \"^test\"".to_string(),
            },
            category: "TOML Filter".to_string(),
            impact_score: 50,
            estimated_tokens_saved: 1000,
            confidence: 0.8,
            description: "test".to_string(),
        }];
        let result = format_dry_run(&suggestions);
        assert!(result.contains("1 suggestions"));
        assert!(result.contains("[filters.test]"));
    }

    #[test]
    fn test_format_applied() {
        let results = vec![
            ApplyResult {
                description: "Added filter".to_string(),
                target_path: "filters.toml".to_string(),
                success: true,
                error: None,
            },
            ApplyResult {
                description: "Failed config".to_string(),
                target_path: "config.toml".to_string(),
                success: false,
                error: Some("permission denied".to_string()),
            },
        ];
        let text = format_applied(&results);
        assert!(text.contains("1 succeeded, 1 failed"));
        assert!(text.contains("[OK]"));
        assert!(text.contains("[FAIL]"));
        assert!(text.contains("permission denied"));
    }
}
