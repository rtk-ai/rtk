//! Formats optimization reports for text and JSON output.

use anyhow::Result;

use crate::optimize::suggestions::{OptimizeReport, SuggestionKind};

/// Format report as human-readable text.
pub fn format_text(report: &OptimizeReport) -> String {
    let mut out = String::new();

    // Header
    out.push_str(&format!(
        "RTK Optimize -- {} suggestions from {} commands ({} sessions, {} days)\n",
        report.suggestions.len(),
        report.commands_analyzed,
        report.sessions_analyzed,
        report.days_covered,
    ));
    out.push_str(&format!(
        "Coverage: {:.0}% -> {:.0}% (projected)\n",
        report.current_coverage_pct, report.projected_coverage_pct,
    ));
    if report.total_estimated_monthly_savings > 0 {
        out.push_str(&format!(
            "Estimated monthly savings: ~{} tokens\n",
            format_tokens(report.total_estimated_monthly_savings),
        ));
    }

    if report.suggestions.is_empty() {
        out.push_str("\nNo optimization suggestions found.\n");
        return out;
    }

    // Group by category
    let toml_suggestions: Vec<_> = report
        .suggestions
        .iter()
        .filter(|s| s.category == "TOML Filter")
        .collect();
    let config_suggestions: Vec<_> = report
        .suggestions
        .iter()
        .filter(|s| s.category == "Config" || s.category == "Exclusion")
        .collect();
    let correction_suggestions: Vec<_> = report
        .suggestions
        .iter()
        .filter(|s| s.category == "Correction")
        .collect();

    // TOML Filter suggestions
    if !toml_suggestions.is_empty() {
        out.push_str(&format!(
            "\n--- TOML Filters ({}) ---\n",
            toml_suggestions.len()
        ));
        for s in &toml_suggestions {
            out.push_str(&format!(
                "  [{}] {}\n",
                format_impact(s.impact_score),
                s.description,
            ));
        }
    }

    // Config suggestions
    if !config_suggestions.is_empty() {
        out.push_str(&format!(
            "\n--- Config Tuning ({}) ---\n",
            config_suggestions.len()
        ));
        for s in &config_suggestions {
            match &s.kind {
                SuggestionKind::TuneConfig {
                    field,
                    current,
                    suggested,
                } => {
                    out.push_str(&format!(
                        "  [{}] {}: {} -> {}\n",
                        format_impact(s.impact_score),
                        field,
                        current,
                        suggested,
                    ));
                }
                SuggestionKind::ExcludeCommand { command, reason } => {
                    out.push_str(&format!(
                        "  [{}] Exclude `{}`: {}\n",
                        format_impact(s.impact_score),
                        command,
                        reason,
                    ));
                }
                _ => {
                    out.push_str(&format!(
                        "  [{}] {}\n",
                        format_impact(s.impact_score),
                        s.description,
                    ));
                }
            }
        }
    }

    // Correction suggestions
    if !correction_suggestions.is_empty() {
        out.push_str(&format!(
            "\n--- CLI Corrections ({}) ---\n",
            correction_suggestions.len()
        ));
        for s in &correction_suggestions {
            if let SuggestionKind::WriteCorrection {
                wrong,
                right,
                error_type,
            } = &s.kind
            {
                out.push_str(&format!(
                    "  [{}] {}  ->  {} ({})\n",
                    format_impact(s.impact_score),
                    wrong,
                    right,
                    error_type,
                ));
            }
        }
    }

    // Footer
    out.push_str("\nApply: rtk optimize --apply\n");
    out.push_str("Preview: rtk optimize --dry-run\n");

    out
}

/// Format report as JSON.
pub fn format_json(report: &OptimizeReport) -> Result<String> {
    Ok(serde_json::to_string_pretty(report)?)
}

/// Format impact score as visual indicator.
fn format_impact(score: u32) -> &'static str {
    match score {
        0..=25 => "LOW",
        26..=50 => "MED",
        51..=75 => "HIGH",
        _ => "CRIT",
    }
}

/// Format token count with K/M suffixes.
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::suggestions::{OptimizeReport, Suggestion, SuggestionKind};

    #[test]
    fn test_format_text_empty_report() {
        let report = OptimizeReport {
            sessions_analyzed: 5,
            commands_analyzed: 100,
            days_covered: 30,
            suggestions: vec![],
            total_estimated_monthly_savings: 0,
            current_coverage_pct: 85.0,
            projected_coverage_pct: 85.0,
        };
        let text = format_text(&report);
        assert!(text.contains("0 suggestions"));
        assert!(text.contains("No optimization suggestions"));
    }

    #[test]
    fn test_format_text_with_suggestions() {
        let report = OptimizeReport {
            sessions_analyzed: 10,
            commands_analyzed: 500,
            days_covered: 30,
            suggestions: vec![Suggestion {
                kind: SuggestionKind::GenerateTomlFilter {
                    toml_content: "[filters.test]".to_string(),
                },
                category: "TOML Filter".to_string(),
                impact_score: 75,
                estimated_tokens_saved: 5000,
                confidence: 0.8,
                description: "Generate filter for terraform plan".to_string(),
            }],
            total_estimated_monthly_savings: 5000,
            current_coverage_pct: 80.0,
            projected_coverage_pct: 90.0,
        };
        let text = format_text(&report);
        assert!(text.contains("1 suggestions"));
        assert!(text.contains("TOML Filters"));
        assert!(text.contains("terraform"));
        assert!(text.contains("80%"));
        assert!(text.contains("90%"));
    }

    #[test]
    fn test_format_json() {
        let report = OptimizeReport {
            sessions_analyzed: 3,
            commands_analyzed: 50,
            days_covered: 7,
            suggestions: vec![],
            total_estimated_monthly_savings: 0,
            current_coverage_pct: 85.0,
            projected_coverage_pct: 85.0,
        };
        let json = format_json(&report).unwrap();
        assert!(json.contains("sessions_analyzed"));
        assert!(json.contains("\"3\"") || json.contains(": 3"));
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1500), "1.5K");
        assert_eq!(format_tokens(1_500_000), "1.5M");
    }

    #[test]
    fn test_format_impact() {
        assert_eq!(format_impact(10), "LOW");
        assert_eq!(format_impact(40), "MED");
        assert_eq!(format_impact(60), "HIGH");
        assert_eq!(format_impact(90), "CRIT");
    }
}
