//! Shared compact output filtering for PHP test runners.

use super::utils::strip_ansi_and_controls;

fn is_progress_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    let has_dot = trimmed.contains('.');
    let progress_charset = trimmed.chars().all(|c| {
        matches!(
            c,
            '.' | 'F' | 'E' | 'W' | 'R' | 'S' | 'I' | 'D' | 'N' | 'O' | 'K' | '0'
                ..='9' | ' ' | '/' | '%' | '(' | ')' | '-'
        )
    });

    has_dot && progress_charset
}

pub fn filter_test_runner_output(output: &str) -> String {
    let mut lines = Vec::new();

    for line in strip_ansi_and_controls(output).lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            continue;
        }

        if trimmed.starts_with("PHPUnit ")
            || trimmed.starts_with("Pest ")
            || trimmed.starts_with("ParaTest ")
            || trimmed.starts_with("Runtime:")
            || trimmed.starts_with("Configuration:")
            || trimmed.starts_with("Random Seed:")
        {
            continue;
        }

        if is_progress_line(trimmed) {
            continue;
        }

        lines.push(trimmed.to_string());
    }

    if lines.is_empty() {
        return "ok".to_string();
    }

    if lines.len() > 120 {
        let mut reduced = Vec::new();
        reduced.extend(lines.iter().take(80).cloned());
        reduced.push(format!("... +{} more lines", lines.len() - 120));
        reduced.extend(lines.iter().skip(lines.len() - 40).cloned());
        return reduced.join("\n");
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filters_phpunit_headers_and_progress() {
        let output = "PHPUnit 12.2.0\n....\nOK (4 tests, 4 assertions)\n";
        let filtered = filter_test_runner_output(output);
        assert!(!filtered.contains("PHPUnit 12.2.0"));
        assert!(!filtered.contains("...."));
        assert!(filtered.contains("OK (4 tests, 4 assertions)"));
    }

    #[test]
    fn test_keeps_failures() {
        let output = "..F\nThere was 1 failure:\nFailed asserting true is false\n";
        let filtered = filter_test_runner_output(output);
        assert!(filtered.contains("There was 1 failure"));
        assert!(filtered.contains("Failed asserting true is false"));
    }
}
