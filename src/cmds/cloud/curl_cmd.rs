//! Runs curl and auto-compresses JSON responses.

use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command, truncate};
use crate::json_cmd;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref HTML_TAG_RE: Regex = Regex::new(r"<[^>]+>").unwrap();
    static ref HTML_ENTITY_RE: Regex = Regex::new(r"&(amp|lt|gt|quot|nbsp|#\d+);").unwrap();
    static ref SCRIPT_RE: Regex =
        Regex::new(r"(?si)<script[^>]*>.*?</script>").unwrap();
    static ref STYLE_RE: Regex =
        Regex::new(r"(?si)<style[^>]*>.*?</style>").unwrap();
}

/// Not using run_filtered: on failure, curl can return HTML error pages (404, 500)
/// that the JSON schema filter would mangle. The early exit skips filtering entirely.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("curl");
    cmd.arg("-s"); // Silent mode (no progress bar)

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: curl -s {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run curl")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Early exit: don't feed HTTP error bodies (HTML 404 etc.) through JSON schema filter
    if !output.status.success() {
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        eprintln!("FAILED: curl {}", msg);
        return Ok(exit_code_from_output(&output, "curl"));
    }

    let raw = stdout.to_string();

    // Auto-detect JSON and pipe through filter
    let filtered = filter_curl_output(&stdout);
    println!("{}", filtered);

    timer.track(
        &format!("curl {}", args.join(" ")),
        &format!("rtk curl {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(0)
}

fn filter_curl_output(output: &str) -> String {
    let trimmed = output.trim();

    // Try JSON detection: starts with { or [
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && (trimmed.ends_with('}') || trimmed.ends_with(']'))
    {
        if let Ok(schema) = json_cmd::filter_json_string(trimmed, 5) {
            // Only use schema if it's actually shorter than the original (#297)
            if schema.len() <= trimmed.len() {
                return schema;
            }
        }
    }

    // Detect HTML and strip tags for cleaner output
    let text = if trimmed.contains("<!DOCTYPE") || trimmed.contains("<html") || trimmed.contains("<HTML") {
        strip_html_tags(trimmed)
    } else {
        trimmed.to_string()
    };

    // Truncate long output
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() > 30 {
        let mut result: Vec<&str> = lines[..30].to_vec();
        result.push("");
        let msg = format!(
            "... ({} more lines, {} bytes total)",
            lines.len() - 30,
            text.len()
        );
        return format!("{}\n{}", result.join("\n"), msg);
    }

    // Short output: return as-is but truncate long lines
    lines
        .iter()
        .map(|l| truncate(l, 200))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strip HTML tags and entities, returning only visible text content.
fn strip_html_tags(html: &str) -> String {
    // Remove <script> and <style> blocks entirely
    let no_script = SCRIPT_RE.replace_all(html, "");
    let no_style = STYLE_RE.replace_all(&no_script, "");
    // Strip remaining tags
    let no_tags = HTML_TAG_RE.replace_all(&no_style, " ");
    // Decode common entities
    let decoded = HTML_ENTITY_RE.replace_all(&no_tags, |caps: &regex::Captures| {
        match &caps[1] {
            "amp" => "&",
            "lt" => "<",
            "gt" => ">",
            "quot" => "\"",
            "nbsp" => " ",
            _ => "",
        }
        .to_string()
    });
    // Collapse whitespace within lines, remove blank lines
    decoded
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_curl_json() {
        // Large JSON where schema is shorter than original — schema should be returned
        let output = r#"{"name": "a very long user name here", "count": 42, "items": [1, 2, 3], "description": "a very long description that takes up many characters in the original JSON payload", "status": "active", "url": "https://example.com/api/v1/users/123"}"#;
        let result = filter_curl_output(output);
        assert!(result.contains("name"));
        assert!(result.contains("string"));
        assert!(result.contains("int"));
    }

    #[test]
    fn test_filter_curl_json_array() {
        let output = r#"[{"id": 1}, {"id": 2}]"#;
        let result = filter_curl_output(output);
        assert!(result.contains("id"));
    }

    #[test]
    fn test_filter_curl_non_json() {
        let output = "Hello, World!\nThis is plain text.";
        let result = filter_curl_output(output);
        assert!(result.contains("Hello, World!"));
        assert!(result.contains("plain text"));
    }

    #[test]
    fn test_filter_curl_json_small_returns_original() {
        // Small JSON where schema would be larger than original (issue #297)
        let output = r#"{"r2Ready":true,"status":"ok"}"#;
        let result = filter_curl_output(output);
        // Schema would be "{\n  r2Ready: bool,\n  status: string\n}" which is longer
        // Should return the original JSON unchanged
        assert_eq!(result.trim(), output.trim());
    }

    #[test]
    fn test_filter_curl_long_output() {
        let lines: Vec<String> = (0..50).map(|i| format!("Line {}", i)).collect();
        let output = lines.join("\n");
        let result = filter_curl_output(&output);
        assert!(result.contains("Line 0"));
        assert!(result.contains("Line 29"));
        assert!(result.contains("more lines"));
    }

    #[test]
    fn test_strip_html_tags_basic() {
        let html = "<!DOCTYPE html><html><head><title>Error</title></head><body><h1>404 Not Found</h1><p>The page was not found.</p></body></html>";
        let result = strip_html_tags(html);
        assert!(result.contains("404 Not Found"));
        assert!(result.contains("page was not found"));
        assert!(!result.contains("<h1>"));
        assert!(!result.contains("<p>"));
    }

    #[test]
    fn test_strip_html_removes_script() {
        let html = "<html><body><script>var x = 1;</script><p>visible</p></body></html>";
        let result = strip_html_tags(html);
        assert!(result.contains("visible"));
        assert!(!result.contains("var x"));
    }

    #[test]
    fn test_filter_curl_html_response() {
        let html = "<!DOCTYPE html><html><body><h1>Server Error</h1><p>Something went wrong</p></body></html>";
        let result = filter_curl_output(html);
        assert!(result.contains("Server Error"));
        assert!(!result.contains("<h1>"));
    }

    #[test]
    fn test_strip_html_entities() {
        let html = "<html><body><p>A &amp; B &lt; C</p></body></html>";
        let result = strip_html_tags(html);
        assert!(result.contains("A & B < C"));
    }
}
