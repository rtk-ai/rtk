//! Parser infrastructure for tool output transformation
//!
//! This module provides a unified interface for parsing tool outputs with graceful degradation:
//! - Tier 1 (Full): Complete JSON parsing with all fields
//! - Tier 2 (Degraded): Partial parsing with warnings
//! - Tier 3 (Passthrough): Raw output truncation with error marker
//!
//! The three-tier system ensures RTK never returns false data silently.

pub mod error;
pub mod formatter;
pub mod types;

pub use formatter::{FormatMode, TokenFormatter};
pub use types::*;

/// Parse result with degradation tier
#[derive(Debug)]
pub enum ParseResult<T> {
    /// Tier 1: Full parse with complete structured data
    Full(T),

    /// Tier 2: Degraded parse with partial data and warnings
    Degraded(T, Vec<String>),

    /// Tier 3: Passthrough - parsing failed, returning truncated raw output
    Passthrough(String),
}

impl<T> ParseResult<T> {
    /// Unwrap the parsed data, panicking on Passthrough
    pub fn unwrap(self) -> T {
        match self {
            ParseResult::Full(data) => data,
            ParseResult::Degraded(data, _) => data,
            ParseResult::Passthrough(_) => panic!("Called unwrap on Passthrough result"),
        }
    }

    /// Get the tier level (1 = Full, 2 = Degraded, 3 = Passthrough)
    pub fn tier(&self) -> u8 {
        match self {
            ParseResult::Full(_) => 1,
            ParseResult::Degraded(_, _) => 2,
            ParseResult::Passthrough(_) => 3,
        }
    }

    /// Check if parsing succeeded (Full or Degraded)
    pub fn is_ok(&self) -> bool {
        !matches!(self, ParseResult::Passthrough(_))
    }

    /// Map the parsed data while preserving tier
    pub fn map<U, F>(self, f: F) -> ParseResult<U>
    where
        F: FnOnce(T) -> U,
    {
        match self {
            ParseResult::Full(data) => ParseResult::Full(f(data)),
            ParseResult::Degraded(data, warnings) => ParseResult::Degraded(f(data), warnings),
            ParseResult::Passthrough(raw) => ParseResult::Passthrough(raw),
        }
    }

    /// Get warnings if Degraded tier
    pub fn warnings(&self) -> Vec<String> {
        match self {
            ParseResult::Degraded(_, warnings) => warnings.clone(),
            _ => vec![],
        }
    }
}

/// Unified parser trait for tool outputs
pub trait OutputParser: Sized {
    type Output;

    /// Parse raw output into structured format
    ///
    /// Implementation should follow three-tier fallback:
    /// 1. Try JSON parsing (if tool supports --json/--format json)
    /// 2. Try regex/text extraction with partial data
    /// 3. Return truncated passthrough with `[RTK:PASSTHROUGH]` marker
    fn parse(input: &str) -> ParseResult<Self::Output>;

    /// Parse with explicit tier preference (for testing/debugging)
    fn parse_with_tier(input: &str, max_tier: u8) -> ParseResult<Self::Output> {
        let result = Self::parse(input);
        if result.tier() > max_tier {
            // Force degradation to passthrough if exceeds max tier
            return ParseResult::Passthrough(truncate_output(input, 500));
        }
        result
    }
}

/// Truncate output to max length with ellipsis
pub fn truncate_output(output: &str, max_chars: usize) -> String {
    let chars: Vec<char> = output.chars().collect();
    if chars.len() <= max_chars {
        return output.to_string();
    }

    let truncated: String = chars[..max_chars].iter().collect();
    format!(
        "{}\n\n[RTK:PASSTHROUGH] Output truncated ({} chars → {} chars)",
        truncated,
        chars.len(),
        max_chars
    )
}

/// Helper to emit degradation warning
pub fn emit_degradation_warning(tool: &str, reason: &str) {
    eprintln!("[RTK:DEGRADED] {} parser: {}", tool, reason);
}

/// Helper to emit passthrough warning
pub fn emit_passthrough_warning(tool: &str, reason: &str) {
    eprintln!("[RTK:PASSTHROUGH] {} parser: {}", tool, reason);
}

/// Extract a complete JSON object from input that may have non-JSON prefix (pnpm banner, dotenv messages, etc.)
///
/// Strategy:
/// 1. Find `"numTotalTests"` (vitest-specific marker) or first standalone `{`
/// 2. Brace-balance forward to find matching `}`
/// 3. Return slice containing complete JSON object
///
/// Handles: nested braces, string escapes, pnpm prefixes, dotenv banners
///
/// Returns `None` if no valid JSON object found.
pub fn extract_json_object(input: &str) -> Option<&str> {
    // Try vitest-specific marker first (most reliable)
    let start_pos = if let Some(pos) = input.find("\"numTotalTests\"") {
        // Walk backward to find opening brace of this object
        input[..pos].rfind('{').unwrap_or(0)
    } else {
        // Fallback: find first `{` on its own line or after whitespace
        let mut found_start = None;
        for (idx, line) in input.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with('{') {
                // Calculate byte offset
                found_start = Some(
                    input[..]
                        .lines()
                        .take(idx)
                        .map(|l| l.len() + 1)
                        .sum::<usize>(),
                );
                break;
            }
        }
        found_start?
    };

    // Brace-balance forward from start_pos
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;
    let chars: Vec<char> = input[start_pos..].chars().collect();

    for (i, &ch) in chars.iter().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    // Found matching closing brace
                    let end_pos = start_pos + i + 1; // +1 to include the `}`
                    return Some(&input[start_pos..end_pos]);
                }
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_result_tier() {
        let full: ParseResult<i32> = ParseResult::Full(42);
        assert_eq!(full.tier(), 1);
        assert!(full.is_ok());

        let degraded: ParseResult<i32> = ParseResult::Degraded(42, vec!["warning".to_string()]);
        assert_eq!(degraded.tier(), 2);
        assert!(degraded.is_ok());
        assert_eq!(degraded.warnings().len(), 1);

        let passthrough: ParseResult<i32> = ParseResult::Passthrough("raw".to_string());
        assert_eq!(passthrough.tier(), 3);
        assert!(!passthrough.is_ok());
    }

    #[test]
    fn test_parse_result_map() {
        let full: ParseResult<i32> = ParseResult::Full(42);
        let mapped = full.map(|x| x * 2);
        assert_eq!(mapped.tier(), 1);
        assert_eq!(mapped.unwrap(), 84);

        let degraded: ParseResult<i32> = ParseResult::Degraded(42, vec!["warn".to_string()]);
        let mapped = degraded.map(|x| x * 2);
        assert_eq!(mapped.tier(), 2);
        assert_eq!(mapped.warnings().len(), 1);
        assert_eq!(mapped.unwrap(), 84);
    }

    #[test]
    fn test_truncate_output() {
        let short = "hello";
        assert_eq!(truncate_output(short, 10), "hello");

        let long = "a".repeat(1000);
        let truncated = truncate_output(&long, 100);
        assert!(truncated.contains("[RTK:PASSTHROUGH]"));
        assert!(truncated.contains("1000 chars → 100 chars"));
    }

    #[test]
    fn test_truncate_output_multibyte() {
        // Thai text: each char is 3 bytes
        let thai = "สวัสดีครับ".repeat(100);
        // Try truncating at a byte offset that might land mid-character
        let result = truncate_output(&thai, 50);
        assert!(result.contains("[RTK:PASSTHROUGH]"));
        // Should be valid UTF-8 (no panic)
        let _ = result.len();
    }

    #[test]
    fn test_truncate_output_emoji() {
        let emoji = "🎉".repeat(200);
        let result = truncate_output(&emoji, 100);
        assert!(result.contains("[RTK:PASSTHROUGH]"));
    }

    #[test]
    fn test_extract_json_object_clean() {
        let input = r#"{"numTotalTests": 13, "numPassedTests": 13}"#;
        let extracted = extract_json_object(input);
        assert_eq!(extracted, Some(input));
    }

    #[test]
    fn test_extract_json_object_with_pnpm_prefix() {
        let input = r#"
Scope: all 6 workspace projects
 WARN  deprecated inflight@1.0.6: This module is not supported

{"numTotalTests": 13, "numPassedTests": 13, "numFailedTests": 0}
"#;
        let extracted = extract_json_object(input).expect("Should extract JSON");
        assert!(extracted.contains("numTotalTests"));
        assert!(extracted.starts_with('{'));
        assert!(extracted.ends_with('}'));
    }

    #[test]
    fn test_extract_json_object_with_dotenv_prefix() {
        let input = r#"[dotenv] Loading environment variables from .env
[dotenv] Injected 5 variables

{"numTotalTests": 5, "testResults": [{"name": "test.js"}]}
"#;
        let extracted = extract_json_object(input).expect("Should extract JSON");
        assert!(extracted.contains("numTotalTests"));
        assert!(extracted.contains("testResults"));
    }

    #[test]
    fn test_extract_json_object_nested_braces() {
        let input = r#"prefix text
{"numTotalTests": 2, "testResults": [{"name": "test", "data": {"nested": true}}]}
"#;
        let extracted = extract_json_object(input).expect("Should extract JSON");
        assert!(extracted.contains("\"nested\": true"));
        assert!(extracted.starts_with('{'));
        assert!(extracted.ends_with('}'));
    }

    #[test]
    fn test_extract_json_object_no_json() {
        let input = "Just plain text with no JSON";
        let extracted = extract_json_object(input);
        assert_eq!(extracted, None);
    }

    #[test]
    fn test_extract_json_object_string_with_braces() {
        let input = r#"{"numTotalTests": 1, "message": "test {should} not confuse parser"}"#;
        let extracted = extract_json_object(input).expect("Should extract JSON");
        assert!(extracted.contains("test {should} not confuse parser"));
        assert_eq!(extracted, input);
    }
}
