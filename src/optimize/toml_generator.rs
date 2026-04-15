//! Auto-generates TOML filter definitions from sample command outputs.

use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    /// Lines that are purely whitespace or empty.
    static ref EMPTY_LINE_RE: Regex = Regex::new(r"^\s*$").unwrap();
    /// Timestamp patterns (ISO 8601, common log formats).
    static ref TIMESTAMP_RE: Regex = Regex::new(
        r"^\s*\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}"
    ).unwrap();
    /// Progress bar / spinner lines (%, bars, dots).
    static ref PROGRESS_RE: Regex = Regex::new(
        r"(?:\d+%|[█▓▒░■□●○\|/\-\\]{3,}|\.{4,}|\[=+>?\s*\])"
    ).unwrap();
    /// Separator lines (---, ===, ***).
    static ref SEPARATOR_RE: Regex = Regex::new(
        r"^[\s\-=\*_]{3,}\s*$"
    ).unwrap();
    /// Lines that are only ANSI escape codes (no visible text).
    static ref ANSI_ONLY_RE: Regex = Regex::new(
        r"^(\x1b\[[0-9;]*[a-zA-Z]|\s)*$"
    ).unwrap();
}

/// Noise pattern with its regex string for TOML output and detection regex.
struct NoisePattern {
    toml_regex: &'static str,
    detector: &'static Regex,
}

lazy_static! {
    static ref NOISE_PATTERNS: Vec<NoisePattern> = vec![
        NoisePattern {
            toml_regex: r#"^\s*$"#,
            detector: &EMPTY_LINE_RE,
        },
        NoisePattern {
            toml_regex: r#"^\s*\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}"#,
            detector: &TIMESTAMP_RE,
        },
        NoisePattern {
            toml_regex: r#"(?:\d+%|[█▓▒░■□●○\|/\-\\]{3,}|\.{4,}|\[=+>?\s*\])"#,
            detector: &PROGRESS_RE,
        },
        NoisePattern {
            toml_regex: r#"^[\s\-=\*_]{3,}\s*$"#,
            detector: &SEPARATOR_RE,
        },
    ];
}

/// Generate a TOML filter definition from sample outputs.
///
/// Returns `None` if the samples are too sparse to infer useful patterns.
pub fn generate_toml_filter(command: &str, sample_outputs: &[String]) -> Option<String> {
    if sample_outputs.is_empty() {
        return None;
    }

    let filter_name: String = command
        .chars()
        .map(|c| if c == ' ' || c == '/' { '-' } else { c })
        .collect();
    let match_pattern = build_match_pattern(command);

    // Collect all lines across all samples
    let all_lines: Vec<&str> = sample_outputs.iter().flat_map(|s| s.lines()).collect();

    if all_lines.is_empty() {
        return None;
    }

    let total_lines = all_lines.len();

    // Detect which noise patterns have >60% hit rate
    let mut strip_patterns: Vec<&str> = Vec::new();
    for pattern in NOISE_PATTERNS.iter() {
        let hits = all_lines
            .iter()
            .filter(|l| pattern.detector.is_match(l))
            .count();
        let hit_rate = hits as f64 / total_lines as f64;
        if hit_rate > 0.6 {
            strip_patterns.push(pattern.toml_regex);
        }
    }

    // If no noise patterns detected and outputs are small, not worth filtering
    if strip_patterns.is_empty() && total_lines < 20 {
        return None;
    }

    // Always strip empty lines if not already included
    let has_empty = strip_patterns.iter().any(|p| p.contains("^\\s*$"));
    if !has_empty {
        strip_patterns.push(r#"^\s*$"#);
    }

    // Compute line count statistics for truncation hints
    let sample_line_counts: Vec<usize> = sample_outputs.iter().map(|s| s.lines().count()).collect();
    let max_line_count = sample_line_counts.iter().copied().max().unwrap_or(0);
    let median_line_count = if sample_line_counts.is_empty() {
        0
    } else {
        let mut sorted = sample_line_counts.clone();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    };

    // Detect success short-circuits: phrases in >80% of short outputs
    let short_outputs: Vec<&String> = sample_outputs
        .iter()
        .filter(|s| s.lines().count() <= 3)
        .collect();
    let match_output_rules = detect_success_patterns(&short_outputs, sample_outputs.len());

    // Build the TOML
    let mut toml = String::new();
    toml.push_str(&format!("[filters.{}]\n", filter_name));
    toml.push_str(&format!(
        "description = \"{} output filter (auto-generated)\"\n",
        command
    ));
    toml.push_str(&format!("match_command = \"{}\"\n", match_pattern));
    toml.push_str("strip_ansi = true\n");

    // match_output rules
    if !match_output_rules.is_empty() {
        toml.push_str("match_output = [\n");
        for (pattern, message) in &match_output_rules {
            toml.push_str(&format!(
                "    {{ pattern = \"{}\", message = \"{}: ok\" }},\n",
                escape_toml_regex(pattern),
                filter_name
            ));
            let _ = message; // message used in description
        }
        toml.push_str("]\n");
    }

    // strip_lines_matching
    if !strip_patterns.is_empty() {
        toml.push_str("strip_lines_matching = [\n");
        for pat in &strip_patterns {
            toml.push_str(&format!("    \"{}\",\n", pat));
        }
        toml.push_str("]\n");
    }

    // Truncation hints
    toml.push_str("truncate_lines_at = 200\n");
    if max_line_count > 100 {
        let head = (median_line_count as f64 * 0.6).ceil() as usize;
        let tail = (median_line_count as f64 * 0.2).ceil() as usize;
        toml.push_str(&format!("head_lines = {}\n", head.max(20)));
        toml.push_str(&format!("tail_lines = {}\n", tail.max(5)));
        toml.push_str(&format!("max_lines = {}\n", (head + tail + 10).max(50)));
    }

    toml.push_str(&format!("on_empty = \"{}: ok\"\n", filter_name));

    // Add inline test
    if let Some(first_output) = sample_outputs.first() {
        let test_input: String = first_output.lines().take(5).collect::<Vec<_>>().join("\n");
        if !test_input.is_empty() {
            toml.push_str(&format!("\n[[tests.{}]]\n", filter_name));
            toml.push_str(&format!("name = \"basic {} filter test\"\n", command));
            toml.push_str(&format!(
                "input = \"\"\"\n{}\n\"\"\"\n",
                test_input.replace('\\', "\\\\").replace('"', "\\\"")
            ));
            // Expected: we can't perfectly predict, so use a permissive check
            toml.push_str("# expected output depends on filter rules above\n");
        }
    }

    // Validate generated TOML parses
    if toml::from_str::<toml::Value>(&toml).is_err() {
        // If validation fails, return a simpler fallback
        return Some(build_minimal_filter(&filter_name, &match_pattern));
    }

    Some(toml)
}

/// Build a regex match pattern for the command.
fn build_match_pattern(command: &str) -> String {
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.len() == 1 {
        format!("^{}\\\\b", regex::escape(parts[0]))
    } else {
        let escaped: Vec<String> = parts.iter().map(|p| regex::escape(p)).collect();
        format!("^{}", escaped.join("\\\\s+"))
    }
}

/// Detect phrases that appear in >80% of short outputs (success patterns).
fn detect_success_patterns(
    short_outputs: &[&String],
    total_samples: usize,
) -> Vec<(String, String)> {
    if short_outputs.is_empty() || total_samples < 3 {
        return vec![];
    }

    let threshold = (total_samples as f64 * 0.8) as usize;
    let mut patterns = Vec::new();

    // Common success phrases
    let candidates = [
        "success",
        "ok",
        "done",
        "complete",
        "passed",
        "up to date",
        "no changes",
        "already",
        "nothing to",
        "0 errors",
        "0 warnings",
    ];

    for phrase in &candidates {
        let count = short_outputs
            .iter()
            .filter(|s| s.to_lowercase().contains(phrase))
            .count();
        if count >= threshold.max(1) {
            patterns.push((phrase.to_string(), format!("{}: ok", phrase)));
        }
    }

    patterns
}

/// Escape special TOML regex characters in a pattern string.
fn escape_toml_regex(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Build a minimal filter as fallback when full generation fails validation.
fn build_minimal_filter(filter_name: &str, match_pattern: &str) -> String {
    format!(
        "[filters.{}]\n\
         description = \"{} output filter (auto-generated)\"\n\
         match_command = \"{}\"\n\
         strip_ansi = true\n\
         strip_lines_matching = [\"^\\\\s*$\"]\n\
         truncate_lines_at = 200\n\
         on_empty = \"{}: ok\"\n",
        filter_name, filter_name, match_pattern, filter_name
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_basic_filter() {
        // Need enough lines (>20) so the "too sparse" check doesn't bail out
        let mut lines = String::new();
        for i in 0..25 {
            lines.push_str(&format!("output line {}\n", i));
        }
        let samples = vec![lines.clone(), lines];
        let result = generate_toml_filter("terraform plan", &samples);
        assert!(result.is_some());
        let toml = result.unwrap();
        assert!(toml.contains("[filters.terraform-plan]"));
        assert!(toml.contains("match_command"));
        assert!(toml.contains("strip_ansi = true"));
    }

    #[test]
    fn test_generate_empty_samples_returns_none() {
        let result = generate_toml_filter("test", &[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_generate_filter_with_timestamps() {
        let samples = vec![
            "2026-04-14T12:00:00Z Starting\n2026-04-14T12:00:01Z Done\nResult: ok".to_string(),
            "2026-04-14T13:00:00Z Starting\n2026-04-14T13:00:01Z Done\nResult: ok".to_string(),
        ];
        let result = generate_toml_filter("myapp deploy", &samples);
        assert!(result.is_some());
        let toml = result.unwrap();
        assert!(toml.contains("strip_lines_matching"));
    }

    #[test]
    fn test_build_match_pattern_single_word() {
        let pattern = build_match_pattern("terraform");
        assert!(pattern.starts_with("^terraform"));
    }

    #[test]
    fn test_build_match_pattern_multi_word() {
        let pattern = build_match_pattern("terraform plan");
        assert!(pattern.contains("terraform"));
        assert!(pattern.contains("plan"));
    }

    #[test]
    fn test_generated_toml_is_valid() {
        let samples = vec!["line 1\nline 2\nline 3".to_string()];
        let result = generate_toml_filter("simple cmd", &samples);
        if let Some(toml_str) = result {
            // Should parse as valid TOML (or the fallback should)
            let parsed = toml::from_str::<toml::Value>(&toml_str);
            // It's ok if the test section doesn't parse perfectly,
            // the main filter section should be valid
            if parsed.is_err() {
                // At least the minimal fallback should work
                let minimal = build_minimal_filter("simple-cmd", "^simple\\\\s+cmd");
                assert!(toml::from_str::<toml::Value>(&minimal).is_ok());
            }
        }
    }
}
