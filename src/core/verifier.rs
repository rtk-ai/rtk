//! Two-pass verifier — post-compression invariant checker with passthrough fallback.
//! Checks 6 invariants; falls back to original output if confidence < threshold.

pub struct VerifyResult {
    pub confidence: f64,
    pub passed: Vec<String>,
    pub failed: Vec<(String, String)>,
}

impl VerifyResult {
    pub fn is_safe(&self, threshold: f64) -> bool {
        // error_lines is a hard blocker — dropping diagnostics is never safe
        let critical_ok = !self.failed.iter().any(|(k, _)| k == "error_lines");
        critical_ok && self.confidence >= threshold
    }
}

pub struct Verifier {
    pub threshold: f64,
}

impl Default for Verifier {
    fn default() -> Self {
        Self::new()
    }
}

impl Verifier {
    pub fn new() -> Self {
        Self { threshold: 0.6 }
    }

    /// Verify that `compressed` is a safe reduction of `original`.
    /// Returns a `VerifyResult` with per-check details.
    pub fn verify(&self, original: &str, compressed: &str) -> VerifyResult {
        let mut passed = Vec::new();
        let mut failed = Vec::new();

        // Check 1: min_retention — output must be >= 10% of input length
        let retention = if original.is_empty() {
            1.0
        } else {
            compressed.len() as f64 / original.len() as f64
        };
        if retention >= 0.10 {
            passed.push("min_retention".into());
        } else {
            failed.push((
                "min_retention".into(),
                format!("output is {:.1}% of input (min 10%)", retention * 100.0),
            ));
        }

        // Check 2: critical diagnostic lines must be preserved
        // Use "error:" with colon to avoid false positive from clippy "1 errors"
        let error_lines: Vec<&str> = original
            .lines()
            .filter(|l| {
                let lo = l.to_lowercase();
                lo.contains("error:")
                    || lo.contains("warning:")
                    || lo.contains("fatal:")
                    || lo.contains("panic:")
                    || lo.contains("exception:")
            })
            .collect();
        if error_lines.is_empty() {
            passed.push("error_lines".into());
        } else {
            let missing = error_lines
                .iter()
                .filter(|&&l| !compressed.contains(l.trim()))
                .count();
            if missing == 0 {
                passed.push("error_lines".into());
            } else {
                failed.push((
                    "error_lines".into(),
                    format!("{missing} critical line(s) dropped"),
                ));
            }
        }

        // Check 3: file paths must not be truncated
        let missing_paths = original
            .lines()
            .filter(|l| {
                (l.contains('/') || l.contains('\\'))
                    && l.chars().any(|c| c == '.')
                    && l.len() < 200
            })
            .take(20)
            .flat_map(|l| l.split_whitespace())
            .filter(|t| t.contains('/') || t.contains('\\'))
            .filter(|&t| !compressed.contains(t))
            .count();
        if missing_paths == 0 {
            passed.push("file_paths".into());
        } else {
            failed.push((
                "file_paths".into(),
                format!("{missing_paths} file path(s) missing"),
            ));
        }

        // Check 4: JSON top-level keys — >= 50% must be present in output
        let orig_trimmed = original.trim();
        if orig_trimmed.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(orig_trimmed) {
                if let Some(obj) = v.as_object() {
                    let keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
                    if !keys.is_empty() {
                        let present = keys.iter().filter(|&&k| compressed.contains(k)).count();
                        let ratio = present as f64 / keys.len() as f64;
                        if ratio >= 0.5 {
                            passed.push("json_keys".into());
                        } else {
                            failed.push((
                                "json_keys".into(),
                                format!("{:.0}% of JSON keys retained", ratio * 100.0),
                            ));
                        }
                    } else {
                        passed.push("json_keys".into());
                    }
                } else {
                    passed.push("json_keys".into());
                }
            } else {
                passed.push("json_keys".into());
            }
        } else {
            passed.push("json_keys".into());
        }

        // Check 5: diff hunk headers @@ must be preserved
        let hunks: Vec<&str> = original.lines().filter(|l| l.starts_with("@@")).collect();
        if hunks.is_empty() {
            passed.push("diff_hunks".into());
        } else {
            let missing = hunks.iter().filter(|&&h| !compressed.contains(h)).count();
            if missing == 0 {
                passed.push("diff_hunks".into());
            } else {
                failed.push((
                    "diff_hunks".into(),
                    format!("{missing} @@ hunk header(s) missing"),
                ));
            }
        }

        // Check 6: numeric values — spot-check first 10 numbers >= 2 digits
        let numbers: Vec<&str> = original
            .split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .filter(|s| s.len() >= 2 && s.parse::<f64>().is_ok())
            .take(10)
            .collect();
        if numbers.is_empty() {
            passed.push("numeric_values".into());
        } else {
            let missing = numbers.iter().filter(|&&n| !compressed.contains(n)).count();
            if missing == 0 {
                passed.push("numeric_values".into());
            } else {
                failed.push((
                    "numeric_values".into(),
                    format!("{missing} numeric value(s) altered"),
                ));
            }
        }

        let total = passed.len() + failed.len();
        let confidence = if total == 0 {
            1.0
        } else {
            passed.len() as f64 / total as f64
        };
        VerifyResult {
            confidence,
            passed,
            failed,
        }
    }

    /// Apply compression `f` to `input`. If verification fails, return `input` unchanged.
    pub fn verified_compress<F>(&self, input: &str, compress: F) -> String
    where
        F: FnOnce(&str) -> String,
    {
        let compressed = compress(input);
        let result = self.verify(input, &compressed);
        if result.is_safe(self.threshold) {
            compressed
        } else {
            input.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_input_full_confidence() {
        let v = Verifier::new();
        let text = "error: type mismatch at src/main.rs:42\nCompiling rtk v0.37.2";
        let result = v.verify(text, text);
        assert_eq!(result.confidence, 1.0);
        assert!(result.failed.is_empty());
    }

    #[test]
    fn test_fails_when_error_line_dropped() {
        let v = Verifier::new();
        let original = "Compiling foo\nerror: type mismatch\nCompiling bar";
        let compressed = "Compiling foo\nCompiling bar";
        let result = v.verify(original, compressed);
        assert!(
            result.failed.iter().any(|(k, _)| k == "error_lines"),
            "should flag missing error line"
        );
        assert!(!result.is_safe(0.6));
    }

    #[test]
    fn test_fails_on_too_short_output() {
        let v = Verifier::new();
        let original = "a".repeat(1000);
        let compressed = "x";
        let result = v.verify(&original, compressed);
        assert!(result.failed.iter().any(|(k, _)| k == "min_retention"));
    }

    #[test]
    fn test_passes_on_normal_compressed_output() {
        let v = Verifier::new();
        let original = "Compiling foo v0.1\nCompiling bar v0.2\nFinished dev [unoptimized] in 3.5s";
        let compressed = "Finished dev in 3.5s";
        let result = v.verify(original, compressed);
        assert!(result.passed.contains(&"min_retention".to_string()));
    }

    #[test]
    fn test_preserves_diff_hunk_headers() {
        let v = Verifier::new();
        let original = "@@ -1,5 +1,6 @@\n line1\n-old\n+new\n line2";
        let result = v.verify(original, original);
        assert!(result.passed.contains(&"diff_hunks".to_string()));
    }

    #[test]
    fn test_fails_when_hunk_header_dropped() {
        let v = Verifier::new();
        let original = "@@ -1,5 +1,6 @@\n line1\n-old\n+new\n line2";
        let compressed = "line1\n-old\n+new\nline2";
        let result = v.verify(original, compressed);
        assert!(result.failed.iter().any(|(k, _)| k == "diff_hunks"));
    }

    #[test]
    fn test_numeric_value_preserved() {
        let v = Verifier::new();
        let original = "tests: 42 passed, 0 failed, 100ms elapsed";
        let result = v.verify(original, original);
        assert!(result.passed.contains(&"numeric_values".to_string()));
    }

    #[test]
    fn test_verified_compress_passthrough_on_bad_compress() {
        let v = Verifier::new();
        let input = "error: something failed\nCompiling foo\nCompiling bar\nFinished in 3.5s";
        // Compress function that aggressively drops lines including the error
        let result = v.verified_compress(input, |_| "Finished in 3.5s".to_string());
        // Should fall back to original because error line was dropped
        assert!(result.contains("error:"));
    }
}
