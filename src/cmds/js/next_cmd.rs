//! Filters Next.js build output down to route metrics and bundle sizes.

use crate::core::runner;
use crate::core::shell::display_args;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::{resolved_command, strip_ansi, tool_exists, truncate};
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    // Try next directly first, fallback to npx if not found
    let next_exists = tool_exists("next");

    let mut cmd = if next_exists {
        resolved_command("next")
    } else {
        let mut c = resolved_command("npx");
        c.arg("next");
        c
    };

    cmd.arg("build");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        let tool = if next_exists { "next" } else { "npx next" };
        eprintln!("Running: {} build", tool);
    }

    runner::run_filtered(
        cmd,
        "next build",
        &display_args(args),
        filter_next_build,
        runner::RunOptions::default(),
    )
}

/// Filter Next.js build output - extract routes, bundles, warnings
fn filter_next_build(output: &str) -> String {
    // Bundle size pattern - matches lines starting with route markers (including tree prefixes)
    static BUNDLE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:^|[┌├└])\s*[○●◐λƒ✓]\s+([\w/\-\.]+)\s+(\d+(?:\.\d+)?)\s*(kB|B)\s+(\d+(?:\.\d+)?)\s*(kB|B)")
            .unwrap()
    });

    let mut routes_static = 0;
    let mut routes_dynamic = 0;
    let mut routes_total = 0;
    let mut bundles: Vec<(String, f64, Option<f64>)> = Vec::new();
    let mut warnings = 0;
    let mut errors = 0;
    let mut build_time = String::new();

    // Strip ANSI codes
    let clean_output = strip_ansi(output);

    for line in clean_output.lines() {
        // Count route types by symbol (handle tree prefixes like ┌, ├, └)
        let trimmed = line.trim_start();
        // Check for marker after optional tree prefix (┌, ├, └ followed by space)
        // Tree chars are 3 bytes each in UTF-8, so use char-based indexing
        let after_prefix = if let Some(first_char) = trimmed.chars().next() {
            if ['┌', '├', '└'].contains(&first_char) {
                // Skip first char (tree) and next char (space)
                trimmed.chars().skip(2).collect::<String>()
            } else {
                trimmed.to_string()
            }
        } else {
            trimmed.to_string()
        };

        // Check if this is a route line (marker + space + path starting with /)
        // vs a legend line (marker + spaces + explanation like (Static))
        let is_route_line = after_prefix.chars().nth(2) == Some('/');

        let is_static = is_route_line && after_prefix.starts_with("○");
        let is_dynamic = is_route_line
            && (after_prefix.starts_with("●")
                || after_prefix.starts_with("◐")
                || after_prefix.starts_with("ƒ"));
        let is_server = is_route_line && after_prefix.starts_with("λ");

        if is_static {
            routes_static += 1;
            routes_total += 1;
        } else if is_dynamic {
            routes_dynamic += 1;
            routes_total += 1;
        } else if is_server {
            routes_total += 1;
        }

        // Extract bundle information (route + size + total size)
        if let Some(caps) = BUNDLE_PATTERN.captures(line) {
            let route = caps[1].to_string();
            let size: f64 = caps[2].parse().unwrap_or(0.0);
            let total: f64 = caps[4].parse().unwrap_or(0.0);

            // Calculate percentage increase if both sizes present
            let pct_change = if total > 0.0 {
                Some(((total - size) / size) * 100.0)
            } else {
                None
            };

            bundles.push((route, total, pct_change));
        }

        // Count warnings and errors
        if line.to_lowercase().contains("warning") {
            warnings += 1;
        }
        if line.to_lowercase().contains("error") && !line.contains("0 error") {
            errors += 1;
        }

        // Extract build time
        if (line.contains("Compiled") || line.contains("in"))
            && let Some(time_match) = extract_time(line)
        {
            build_time = time_match;
        }
    }

    // Detect if build was skipped (already built)
    let already_built = clean_output.contains("already optimized")
        || clean_output.contains("Cache")
        || (routes_total == 0 && clean_output.contains("Ready"));

    // Build filtered output
    let mut result = String::new();
    result.push_str("Next.js Build\n");

    if already_built && routes_total == 0 {
        result.push_str("Already built (using cache)\n\n");
    } else if routes_total > 0 {
        result.push_str(&format!(
            "{} routes ({} static, {} dynamic)\n\n",
            routes_total, routes_static, routes_dynamic
        ));
    }

    if !bundles.is_empty() {
        result.push_str("Bundles:\n");

        // Sort by size (descending) and show top 10
        bundles.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        const MAX_BUNDLES: usize = CAP_WARNINGS;
        for (route, size, pct_change) in bundles.iter().take(MAX_BUNDLES) {
            let warning_marker = if let Some(pct) = pct_change {
                if *pct > 10.0 {
                    format!(" [warn] (+{:.0}%)", pct)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            result.push_str(&format!(
                "  {:<30} {:>6.0} kB{}\n",
                truncate(route, 30),
                size,
                warning_marker
            ));
        }

        if bundles.len() > MAX_BUNDLES {
            result.push_str(&format!(
                "\n  ... +{} more routes\n",
                bundles.len() - MAX_BUNDLES
            ));
        }

        result.push('\n');
    }

    // Show build time and status (compile time, not total build duration)
    if !build_time.is_empty() {
        result.push_str(&format!("Compile time: {} | ", build_time));
    }

    result.push_str(&format!("Errors: {} | Warnings: {}\n", errors, warnings));

    result.trim().to_string()
}

/// Extract time from build output (e.g., "Compiled in 34.2s")
fn extract_time(line: &str) -> Option<String> {
    static TIME_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d+(?:\.\d+)?)\s*(s|ms)").unwrap());

    TIME_RE
        .captures(line)
        .map(|caps| format!("{}{}", &caps[1], &caps[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_next_build() {
        let output = r#"
   ▲ Next.js 15.2.0

   Creating an optimized production build ...
✓ Compiled successfully
✓ Linting and checking validity of types
✓ Collecting page data
○ /                            1.2 kB        132 kB
● /dashboard                   2.5 kB        156 kB
○ /api/auth                    0.5 kB         89 kB

Route (app)                    Size     First Load JS
┌ ○ /                          1.2 kB        132 kB
├ ● /dashboard                 2.5 kB        156 kB
└ ○ /api/auth                  0.5 kB         89 kB

○  (Static)  prerendered as static content
●  (SSG)     prerendered as static HTML
λ  (Server)  server-side renders at runtime

✓ Built in 34.2s
"#;
        let result = filter_next_build(output);
        assert!(result.contains("Next.js Build"));
        assert!(result.contains("routes"));
        assert!(!result.contains("Creating an optimized")); // Should filter verbose logs
    }

    #[test]
    fn test_extract_time() {
        assert_eq!(extract_time("Built in 34.2s"), Some("34.2s".to_string()));
        assert_eq!(
            extract_time("Compiled in 1250ms"),
            Some("1250ms".to_string())
        );
        assert_eq!(extract_time("No time here"), None);
    }

    // Frozen requirement: mixed_prefixed_route_counts
    // baseline: fail
    // target: unit
    // expectedBehavior: For a Next.js route table containing `┌ ƒ / 184 B 105 kB`, `├ ○ /_not-found 977 B 102 kB`, and `└ ƒ /pase/[id] 184 B 105 kB`, followed by the Static and Dynamic explanatory legend lines shown in the issue, the pure build filter must report exactly `3 routes (1 static, 2 dynamic)`. Explanatory legends are not additional routes.
    // citation: dynamic route disappears from the summary
    #[test]
    fn bugfix_issue_3530_mixed_prefixed_route_counts() {
        let output = r#"
Route (app)                                 Size  First Load JS
┌ ƒ /                                      184 B         105 kB
├ ○ /_not-found                            977 B         102 kB
└ ƒ /pase/[id]                             184 B         105 kB

○  (Static)   prerendered as static content
ƒ  (Dynamic)  server-rendered on demand

✓ Compiled successfully in 1000ms
"#;
        let result = filter_next_build(output);
        // Should report 3 routes (1 static, 2 dynamic)
        assert!(
            result.contains("3 routes (1 static, 2 dynamic)"),
            "Expected '3 routes (1 static, 2 dynamic)', got: {}",
            result
        );
        // Legends should not be counted as routes
        assert!(!result.contains("4 routes"));
        assert!(!result.contains("5 routes"));
    }

    // Frozen requirement: modern_dynamic_marker_counts
    // baseline: fail
    // target: unit
    // expectedBehavior: For a reduced route table containing only the undecorated rows `ƒ /admin/casas 184 B 105 kB` and `ƒ /pase/[id] 184 B 105 kB`, the pure build filter must report exactly `2 routes (0 static, 2 dynamic)`. Removing tree decoration from the issue rows must not cause the modern dynamic markers to be omitted.
    // citation: 18 routes (1 static, 17 dynamic)
    #[test]
    fn bugfix_issue_3530_modern_dynamic_marker_counts() {
        let output = r#"
ƒ /admin/casas                           184 B         105 kB
ƒ /pase/[id]                             184 B         105 kB

✓ Compiled successfully in 1000ms
"#;
        let result = filter_next_build(output);
        // Should report 2 routes (0 static, 2 dynamic)
        assert!(
            result.contains("2 routes (0 static, 2 dynamic)"),
            "Expected '2 routes (0 static, 2 dynamic)', got: {}",
            result
        );
    }

    // Frozen requirement: compile_time_label
    // baseline: fail
    // target: unit
    // expectedBehavior: For pure build-filter input `✓ Compiled successfully in 1000ms`, the displayed 1000ms must be clearly identified as compile time; it must not be presented as total build duration. A value extracted only from the compiler marker is not evidence of elapsed whole-run time. This exercises the issue-permitted compile-time-label option without prescribing label punctuation or capitalization.
    // citation: renaming the field to compile time
    #[test]
    fn bugfix_issue_3530_compile_time_label() {
        let output = r#"
✓ Compiled successfully in 1000ms
"#;
        let result = filter_next_build(output);
        // The time should be labeled as compile time, not just "Time:"
        // Accept either "Compile time:" or "Compile Time:" or "compile time:" etc.
        let has_compile_label = result.contains("compile time")
            || result.contains("Compile time")
            || result.contains("Compile Time");
        assert!(
            has_compile_label,
            "Time should be labeled as compile time, got: {}",
            result
        );
        // Should still contain the time value
        assert!(result.contains("1000ms"), "Should contain the time value");
    }

    // Frozen requirement: static_route_count_control
    // baseline: pass
    // target: unit
    // expectedBehavior: The already-supported undecorated rows `○ / 1.2 kB 132 kB` and `○ /about 0.5 kB 89 kB` must continue to report exactly `2 routes (2 static, 0 dynamic)` and retain the /about bundle label.
    // citation: it counts the `○` (static) marker
    #[test]
    fn bugfix_issue_3530_static_route_count_control() {
        let output = r#"
○ /                            1.2 kB        132 kB
○ /about                       0.5 kB         89 kB

✓ Compiled successfully in 1000ms
"#;
        let result = filter_next_build(output);
        // Should report 2 routes (2 static, 0 dynamic)
        assert!(
            result.contains("2 routes (2 static, 0 dynamic)"),
            "Expected '2 routes (2 static, 0 dynamic)', got: {}",
            result
        );
        // Should retain the /about bundle label
        assert!(
            result.contains("/about"),
            "Should retain /about bundle label, got: {}",
            result
        );
    }
}
