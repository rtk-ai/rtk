//! Type definitions for optimization suggestions and reports.

use serde::Serialize;

/// The kind of optimization being suggested.
#[derive(Debug, Clone, Serialize)]
pub enum SuggestionKind {
    /// Generate a new TOML filter for an uncovered command.
    GenerateTomlFilter {
        /// The generated TOML filter definition.
        toml_content: String,
    },
    /// Tune an existing config parameter.
    TuneConfig {
        /// Config field path (e.g. "limits.grep_max_results").
        field: String,
        /// Current value as string.
        current: String,
        /// Suggested value as string.
        suggested: String,
    },
    /// Write a CLI correction rule.
    WriteCorrection {
        /// Wrong command pattern.
        wrong: String,
        /// Correct command pattern.
        right: String,
        /// Error type description.
        error_type: String,
    },
    /// Exclude a command from RTK filtering.
    ExcludeCommand {
        /// The command to exclude.
        command: String,
        /// Reason for exclusion.
        reason: String,
    },
}

/// A single optimization suggestion.
#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    /// What kind of optimization this is.
    pub kind: SuggestionKind,
    /// Category (e.g. "TOML Filter", "Config", "Correction", "Exclusion").
    pub category: String,
    /// Impact score 0-100 (higher = more impactful).
    pub impact_score: u32,
    /// Estimated monthly tokens saved by this suggestion.
    pub estimated_tokens_saved: u64,
    /// Confidence 0.0-1.0.
    pub confidence: f64,
    /// Human-readable description.
    pub description: String,
}

/// The full optimization report.
#[derive(Debug, Clone, Serialize)]
pub struct OptimizeReport {
    /// Number of sessions analyzed.
    pub sessions_analyzed: usize,
    /// Number of commands analyzed.
    pub commands_analyzed: usize,
    /// Time span covered in days.
    pub days_covered: u64,
    /// All suggestions sorted by impact.
    pub suggestions: Vec<Suggestion>,
    /// Total estimated monthly token savings across all suggestions.
    pub total_estimated_monthly_savings: u64,
    /// Current RTK coverage percentage.
    pub current_coverage_pct: f64,
    /// Projected coverage after applying suggestions.
    pub projected_coverage_pct: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suggestion_serialize() {
        let s = Suggestion {
            kind: SuggestionKind::GenerateTomlFilter {
                toml_content: "[filters.test]\nmatch_command = \"^test\"".to_string(),
            },
            category: "TOML Filter".to_string(),
            impact_score: 75,
            estimated_tokens_saved: 5000,
            confidence: 0.8,
            description: "Generate filter for test command".to_string(),
        };
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("GenerateTomlFilter"));
        assert!(json.contains("5000"));
    }

    #[test]
    fn test_report_serialize() {
        let report = OptimizeReport {
            sessions_analyzed: 10,
            commands_analyzed: 500,
            days_covered: 30,
            suggestions: vec![],
            total_estimated_monthly_savings: 0,
            current_coverage_pct: 85.0,
            projected_coverage_pct: 85.0,
        };
        let json = serde_json::to_string_pretty(&report).expect("serialize");
        assert!(json.contains("sessions_analyzed"));
        assert!(json.contains("85.0"));
    }
}
