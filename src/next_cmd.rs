use crate::tracking;
use crate::utils::{strip_ansi, truncate};
use anyhow::{Context, Result};
use regex::Regex;
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    // Try next directly first, fallback to npx if not found
    let next_exists = Command::new("which")
        .arg("next")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let mut cmd = if next_exists {
        Command::new("next")
    } else {
        let mut c = Command::new("npx");
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

    let output = cmd.output().context("Failed to run next build (try: npm install -g next)")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_next_build(&raw);

    println!("{}", filtered);

    tracking::track("next build", "rtk next build", &raw, &filtered);

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Filter Next.js build output - extract routes, bundles, warnings
fn filter_next_build(output: &str) -> String {
    lazy_static::lazy_static! {
        // Route line pattern: ○ /dashboard    1.2 kB  132 kB
        static ref ROUTE_PATTERN: Regex = Regex::new(
            r"^[○●◐λ✓]\s+(/[^\s]*)\s+(\d+(?:\.\d+)?)\s*(kB|B)"
        ).unwrap();

        // Bundle size pattern
        static ref BUNDLE_PATTERN: Regex = Regex::new(
            r"^[○●◐λ✓]\s+([\w/\-\.]+)\s+(\d+(?:\.\d+)?)\s*(kB|B)\s+(\d+(?:\.\d+)?)\s*(kB|B)"
        ).unwrap();
    }

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
        // Count route types by symbol
        if line.starts_with("○") {
            routes_static += 1;
            routes_total += 1;
        } else if line.starts_with("●") || line.starts_with("◐") {
            routes_dynamic += 1;
            routes_total += 1;
        } else if line.starts_with("λ") {
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
        if line.contains("Compiled") || line.contains("in") {
            if let Some(time_match) = extract_time(line) {
                build_time = time_match;
            }
        }
    }

    // Detect if build was skipped (already built)
    let already_built = clean_output.contains("already optimized")
        || clean_output.contains("Cache")
        || (routes_total == 0 && clean_output.contains("Ready"));

    // Build filtered output
    let mut result = String::new();
    result.push_str("⚡ Next.js Build\n");
    result.push_str("═══════════════════════════════════════\n");

    if already_built && routes_total == 0 {
        result.push_str("✓ Already built (using cache)\n\n");
    } else if routes_total > 0 {
        result.push_str(&format!(
            "✓ {} routes ({} static, {} dynamic)\n\n",
            routes_total, routes_static, routes_dynamic
        ));
    }

    if !bundles.is_empty() {
        result.push_str("Bundles:\n");

        // Sort by size (descending) and show top 10
        bundles.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        for (route, size, pct_change) in bundles.iter().take(10) {
            let warning_marker = if let Some(pct) = pct_change {
                if *pct > 10.0 {
                    format!(" ⚠️ (+{:.0}%)", pct)
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

        if bundles.len() > 10 {
            result.push_str(&format!("\n  ... +{} more routes\n", bundles.len() - 10));
        }

        result.push('\n');
    }

    // Show build time and status
    if !build_time.is_empty() {
        result.push_str(&format!("Time: {} | ", build_time));
    }

    result.push_str(&format!("Errors: {} | Warnings: {}\n", errors, warnings));

    result.trim().to_string()
}

/// Extract time from build output (e.g., "Compiled in 34.2s")
fn extract_time(line: &str) -> Option<String> {
    lazy_static::lazy_static! {
        static ref TIME_RE: Regex = Regex::new(r"(\d+(?:\.\d+)?)\s*(s|ms)").unwrap();
    }

    TIME_RE.captures(line).map(|caps| format!("{}{}", &caps[1], &caps[2]))
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
        assert!(result.contains("⚡ Next.js Build"));
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
}
