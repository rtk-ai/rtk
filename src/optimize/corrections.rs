//! Wraps learn::detector to extract CLI correction suggestions.

use crate::learn::detector::{
    deduplicate_corrections, find_corrections, CommandExecution, CorrectionRule,
};
use crate::optimize::suggestions::{Suggestion, SuggestionKind};

/// Analyze command executions for repeated CLI mistakes.
pub fn analyze_corrections(
    commands: &[CommandExecution],
    min_confidence: f64,
    min_occurrences: usize,
) -> Vec<Suggestion> {
    let corrections = find_corrections(commands);

    if corrections.is_empty() {
        return vec![];
    }

    // Filter by confidence
    let filtered: Vec<_> = corrections
        .into_iter()
        .filter(|c| c.confidence >= min_confidence)
        .collect();

    // Deduplicate
    let rules = deduplicate_corrections(filtered);

    // Filter by occurrences and convert to suggestions
    rules
        .into_iter()
        .filter(|r| r.occurrences >= min_occurrences)
        .map(|r| rule_to_suggestion(&r))
        .collect()
}

fn rule_to_suggestion(rule: &CorrectionRule) -> Suggestion {
    let impact = (rule.occurrences as u32 * 15).min(80);

    Suggestion {
        kind: SuggestionKind::WriteCorrection {
            wrong: rule.wrong_pattern.clone(),
            right: rule.right_pattern.clone(),
            error_type: rule.error_type.as_str().to_string(),
        },
        category: "Correction".to_string(),
        impact_score: impact,
        estimated_tokens_saved: (rule.occurrences as u64) * 50, // ~50 tokens per avoided error cycle
        confidence: 0.8,
        description: format!(
            "CLI correction: `{}` → `{}` ({} type, {}x seen)",
            rule.wrong_pattern,
            rule.right_pattern,
            rule.error_type.as_str(),
            rule.occurrences
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_execution(cmd: &str, is_error: bool, output: &str) -> CommandExecution {
        CommandExecution {
            command: cmd.to_string(),
            is_error,
            output: output.to_string(),
        }
    }

    #[test]
    fn test_analyze_corrections_empty_input() {
        let result = analyze_corrections(&[], 0.6, 1);
        assert!(result.is_empty());
    }

    #[test]
    fn test_analyze_corrections_no_errors() {
        let commands = vec![
            make_execution("git status", false, "On branch master"),
            make_execution("git log", false, "abc123 commit"),
        ];
        let result = analyze_corrections(&commands, 0.6, 1);
        assert!(result.is_empty());
    }

    #[test]
    fn test_analyze_corrections_finds_typo() {
        let commands = vec![
            make_execution(
                "git commit --ammend",
                true,
                "error: unexpected argument '--ammend'",
            ),
            make_execution("git commit --amend", false, "[master abc123] fix typo"),
        ];
        let result = analyze_corrections(&commands, 0.5, 1);
        assert!(!result.is_empty());
        assert_eq!(result[0].category, "Correction");
        assert!(result[0].description.contains("ammend"));
    }

    #[test]
    fn test_rule_to_suggestion_impact() {
        use crate::learn::detector::ErrorType;
        let rule = CorrectionRule {
            wrong_pattern: "git commit --ammend".to_string(),
            right_pattern: "git commit --amend".to_string(),
            error_type: ErrorType::UnknownFlag,
            occurrences: 5,
            base_command: "git commit".to_string(),
            example_error: "error: unexpected argument".to_string(),
        };
        let suggestion = rule_to_suggestion(&rule);
        assert_eq!(suggestion.impact_score, 75); // 5 * 15
        assert_eq!(suggestion.estimated_tokens_saved, 250); // 5 * 50
    }
}
