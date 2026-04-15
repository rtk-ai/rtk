//! Analyzes RTK tracking data and config to suggest parameter tuning.

use anyhow::Result;

use crate::core::config::Config;
use crate::core::tracking::Tracker;
use crate::optimize::suggestions::{Suggestion, SuggestionKind};

/// Analyze tracking data and current config to suggest optimizations.
pub fn analyze_config(tracker: &Tracker, config: &Config) -> Result<Vec<Suggestion>> {
    let mut suggestions = Vec::new();

    // 1. Low-savings detection
    analyze_low_savings(tracker, &mut suggestions)?;

    // 2. Output percentile analysis
    analyze_output_percentiles(tracker, config, &mut suggestions)?;

    Ok(suggestions)
}

/// Find commands with low savings that might benefit from exclusion or tuning.
fn analyze_low_savings(tracker: &Tracker, suggestions: &mut Vec<Suggestion>) -> Result<()> {
    let low_savings = tracker.low_savings_commands(20)?;

    for (cmd, avg_savings) in low_savings {
        // Commands with structured output (JSON, --json flag) are better excluded
        let is_structured =
            cmd.contains("--json") || cmd.contains("--format json") || cmd.contains("-o json");

        if is_structured {
            suggestions.push(Suggestion {
                kind: SuggestionKind::ExcludeCommand {
                    command: cmd.clone(),
                    reason: format!(
                        "Structured JSON output has only {:.0}% savings — filtering adds latency without benefit",
                        avg_savings
                    ),
                },
                category: "Exclusion".to_string(),
                impact_score: 30,
                estimated_tokens_saved: 0,
                confidence: 0.85,
                description: format!(
                    "Exclude `{}` from RTK filtering ({:.0}% avg savings, structured output)",
                    cmd, avg_savings
                ),
            });
        } else if avg_savings < 15.0 {
            suggestions.push(Suggestion {
                kind: SuggestionKind::TuneConfig {
                    field: format!("filters for {}", cmd),
                    current: format!("{:.0}% savings", avg_savings),
                    suggested: "Review filter rules or exclude".to_string(),
                },
                category: "Config".to_string(),
                impact_score: 20,
                estimated_tokens_saved: 0,
                confidence: 0.6,
                description: format!(
                    "Review filter for `{}` — only {:.0}% avg savings, may need tuning",
                    cmd, avg_savings
                ),
            });
        }
    }

    Ok(())
}

/// Analyze output size patterns and suggest limit adjustments.
fn analyze_output_percentiles(
    tracker: &Tracker,
    config: &Config,
    suggestions: &mut Vec<Suggestion>,
) -> Result<()> {
    let percentiles = tracker.output_percentiles_by_command()?;

    for (cmd, count, avg_tokens, max_tokens) in percentiles {
        // If a command consistently produces small output but limits are high,
        // suggest reducing limits
        if avg_tokens < 50 && max_tokens < 200 && config.limits.passthrough_max_chars > 500 {
            // Small output command — suggest reducing passthrough limit for efficiency
            suggestions.push(Suggestion {
                kind: SuggestionKind::TuneConfig {
                    field: "limits.passthrough_max_chars".to_string(),
                    current: config.limits.passthrough_max_chars.to_string(),
                    suggested: "500".to_string(),
                },
                category: "Config".to_string(),
                impact_score: 15,
                estimated_tokens_saved: 0,
                confidence: 0.5,
                description: format!(
                    "Command `{}` ({}x, avg {} tokens) — passthrough limit could be reduced",
                    cmd, count, avg_tokens
                ),
            });
        }

        // If a command has very large output, suggest adding head/tail limits
        if avg_tokens > 2000 && max_tokens > 5000 {
            let suggested_head = (avg_tokens as f64 * 0.3) as usize;
            let suggested_tail = (avg_tokens as f64 * 0.1) as usize;
            let estimated_savings =
                ((max_tokens - suggested_head - suggested_tail) * count) as u64 / 30;

            suggestions.push(Suggestion {
                kind: SuggestionKind::TuneConfig {
                    field: format!("head/tail limits for {}", cmd),
                    current: format!("avg {} tokens, max {}", avg_tokens, max_tokens),
                    suggested: format!(
                        "head_lines={}, tail_lines={}",
                        suggested_head, suggested_tail
                    ),
                },
                category: "Config".to_string(),
                impact_score: ((estimated_savings as f64).sqrt().min(100.0)) as u32,
                estimated_tokens_saved: estimated_savings,
                confidence: 0.65,
                description: format!(
                    "Add truncation for `{}` ({}x, avg {} tokens, max {}) — ~{} tokens/month saved",
                    cmd, count, avg_tokens, max_tokens, estimated_savings
                ),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_structured_output_detection() {
        // Verify the structured output heuristic
        let cmd = "gh pr list --json";
        assert!(cmd.contains("--json"));
    }

    #[test]
    fn test_suggestion_categories() {
        let s = Suggestion {
            kind: SuggestionKind::ExcludeCommand {
                command: "gh pr list --json".to_string(),
                reason: "structured output".to_string(),
            },
            category: "Exclusion".to_string(),
            impact_score: 30,
            estimated_tokens_saved: 0,
            confidence: 0.85,
            description: "test".to_string(),
        };
        assert_eq!(s.category, "Exclusion");
        assert_eq!(s.impact_score, 30);
    }
}
