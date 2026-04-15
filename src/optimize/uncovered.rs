//! Detects high-frequency commands not covered by RTK filters.

use std::collections::HashMap;

use crate::discover::provider::ExtractedCommand;
use crate::discover::registry::{classify_command, split_command_chain, Classification};
use crate::optimize::suggestions::{Suggestion, SuggestionKind};
use crate::optimize::toml_generator;

/// Statistics for an uncovered command.
struct UncoveredStats {
    count: usize,
    total_output_chars: usize,
    sample_outputs: Vec<String>,
}

/// Analyze extracted commands and find uncovered high-frequency commands.
pub fn analyze_uncovered(
    commands: &[ExtractedCommand],
    min_frequency: usize,
    min_savings_pct: f64,
    days_covered: u64,
) -> Vec<Suggestion> {
    let mut uncovered: HashMap<String, UncoveredStats> = HashMap::new();

    for cmd in commands {
        // Split compound commands (&&, ||, ;)
        let parts = split_command_chain(&cmd.command);
        for part in parts {
            let classification = classify_command(part);
            if let Classification::Unsupported { base_command } = classification {
                let stats = uncovered.entry(base_command).or_insert(UncoveredStats {
                    count: 0,
                    total_output_chars: 0,
                    sample_outputs: Vec::new(),
                });
                stats.count += 1;
                if let Some(len) = cmd.output_len {
                    stats.total_output_chars += len;
                }
                if stats.sample_outputs.len() < 5 {
                    if let Some(ref content) = cmd.output_content {
                        if !content.is_empty() {
                            stats.sample_outputs.push(content.clone());
                        }
                    }
                }
            }
        }
    }

    let days = if days_covered == 0 { 1 } else { days_covered };

    let mut suggestions = Vec::new();

    for (base_command, stats) in &uncovered {
        if stats.count < min_frequency {
            continue;
        }

        let avg_output_chars = if stats.count > 0 {
            stats.total_output_chars / stats.count
        } else {
            0
        };

        // Estimate monthly token savings:
        // (avg_output_chars/4) * (min_savings_pct/100) * count * 30/days
        let avg_tokens = avg_output_chars as f64 / 4.0;
        let monthly_count = stats.count as f64 * 30.0 / days as f64;
        let estimated_savings = (avg_tokens * (min_savings_pct / 100.0) * monthly_count) as u64;

        // Try to generate a TOML filter
        let toml_content =
            toml_generator::generate_toml_filter(base_command, &stats.sample_outputs);

        let (description, kind) = if let Some(toml) = toml_content {
            (
                format!(
                    "Generate TOML filter for `{}` ({} uses, ~{} tokens/month saved)",
                    base_command, stats.count, estimated_savings
                ),
                SuggestionKind::GenerateTomlFilter { toml_content: toml },
            )
        } else {
            (
                format!(
                    "Generate TOML filter for `{}` ({} uses, ~{} tokens/month saved)",
                    base_command, stats.count, estimated_savings
                ),
                SuggestionKind::GenerateTomlFilter {
                    toml_content: format!(
                        "[filters.{}]\ndescription = \"{} output filter\"\nmatch_command = \"^{}\"\nstrip_ansi = true\nstrip_lines_matching = [\"^\\\\s*$\"]\n",
                        base_command.replace(' ', "-"),
                        base_command,
                        regex::escape(&base_command.replace(' ', "\\\\s+"))
                    ),
                },
            )
        };

        // Impact score based on frequency and output size
        let impact = ((stats.count as f64 * avg_output_chars as f64 / 1000.0)
            .sqrt()
            .min(100.0)) as u32;

        suggestions.push(Suggestion {
            kind,
            category: "TOML Filter".to_string(),
            impact_score: impact.max(10),
            estimated_tokens_saved: estimated_savings,
            confidence: 0.7,
            description,
        });
    }

    // Sort by impact descending
    suggestions.sort_by(|a, b| b.impact_score.cmp(&a.impact_score));
    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cmd(command: &str, output_len: usize, output: &str) -> ExtractedCommand {
        ExtractedCommand {
            command: command.to_string(),
            output_len: Some(output_len),
            session_id: "test".to_string(),
            output_content: Some(output.to_string()),
            is_error: false,
            sequence_index: 0,
        }
    }

    #[test]
    fn test_analyze_uncovered_filters_low_frequency() {
        // Use a command not in discover rules (truly unsupported)
        let commands = vec![
            make_cmd("bazel build //...", 5000, "Building..."),
            make_cmd("bazel build //...", 4000, "Build complete"),
        ];
        // min_frequency=5, should return nothing (only 2 occurrences)
        let results = analyze_uncovered(&commands, 5, 30.0, 30);
        assert!(results.is_empty());
    }

    #[test]
    fn test_analyze_uncovered_detects_frequent_commands() {
        // "bazel build" is not in RTK's discover registry
        let commands: Vec<ExtractedCommand> = (0..10)
            .map(|i| make_cmd("bazel build //...", 5000, &format!("Building target {}", i)))
            .collect();

        let results = analyze_uncovered(&commands, 5, 30.0, 30);
        assert!(!results.is_empty());
        assert_eq!(results[0].category, "TOML Filter");
        assert!(results[0].description.contains("bazel"));
    }

    #[test]
    fn test_analyze_uncovered_ignores_supported_commands() {
        // "git log" is supported by RTK
        let commands: Vec<ExtractedCommand> = (0..10)
            .map(|_| make_cmd("git log --oneline", 2000, "abc123 commit msg"))
            .collect();

        let results = analyze_uncovered(&commands, 5, 30.0, 30);
        // git log is supported, so no suggestions
        assert!(results.is_empty());
    }
}
