//! Compact formatter for Vite production builds.

use crate::core::runner::{self, RunMode, RunOptions};
use crate::core::truncate::{reduced, CAP_WARNINGS};
use crate::core::utils::{resolved_command, strip_ansi, tool_exists, truncate};
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

const MAX_LARGEST_ASSETS: usize = reduced(CAP_WARNINGS, 5);

#[derive(Debug, Clone)]
struct Asset {
    path: String,
    size_bytes: f64,
    gzip_bytes: Option<f64>,
}

static ASSET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(\S.*?)\s{2,}(\d+(?:\.\d+)?)\s*(B|kB|MB)(?:\s*│\s*gzip:\s*(\d+(?:\.\d+)?)\s*(B|kB|MB))?(?:\s*│.*)?\s*$"
    )
    .unwrap()
});
static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^vite v([^\s]+) building").unwrap());
static MODULES_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[✓✔]\s+(\d+) modules? transformed").unwrap());
static BUILT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[✓✔]\s+built in (.+)$").unwrap());

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let full = args.iter().any(|arg| arg == "--full");
    let native_args: Vec<String> = args
        .iter()
        .filter(|arg| arg.as_str() != "--full")
        .cloned()
        .collect();
    let filter_build = native_args.first().map(String::as_str) == Some("build")
        && !native_args.iter().any(|arg| arg == "--watch")
        && !full;

    let vite_exists = tool_exists("vite");
    let mut cmd = if vite_exists {
        resolved_command("vite")
    } else {
        let mut command = resolved_command("npx");
        command.arg("vite");
        command
    };
    cmd.args(&native_args);

    let tool = if vite_exists { "vite" } else { "npx vite" };
    let args_display = native_args.join(" ");
    if verbose > 0 {
        eprintln!("Running: {} {}", tool, args_display);
    }

    if !filter_build {
        return runner::run(
            cmd,
            tool,
            &args_display,
            RunMode::Passthrough,
            RunOptions::default(),
        );
    }

    runner::run_filtered_with_exit(
        cmd,
        "vite",
        &args_display,
        filter_vite_output,
        RunOptions::with_tee("vite").early_exit_on_failure(),
    )
}

pub(crate) fn looks_like_vite_build_output(output: &str) -> bool {
    let clean = strip_ansi(output);
    clean
        .lines()
        .any(|line| VERSION_RE.is_match(line.trim()))
}

pub(crate) fn filter_vite_output(output: &str, exit_code: i32) -> String {
    if exit_code != 0 {
        return output.to_string();
    }

    let clean = strip_ansi(output);
    let mut version = None;
    let mut modules = None;
    let mut build_time = None;
    let mut assets = Vec::new();
    let mut signal = Vec::new();

    for line in clean.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(captures) = VERSION_RE.captures(trimmed) {
            version = Some(captures[1].to_string());
            continue;
        }
        if let Some(captures) = MODULES_RE.captures(trimmed) {
            modules = captures[1].parse::<usize>().ok();
            continue;
        }
        if let Some(captures) = BUILT_RE.captures(trimmed) {
            build_time = Some(captures[1].to_string());
            continue;
        }
        if let Some(asset) = parse_asset(trimmed) {
            assets.push(asset);
            continue;
        }
        if is_progress_line(trimmed) || is_npm_lifecycle_line(trimmed) {
            continue;
        }

        signal.push(line.trim_end().to_string());
    }

    let mut result = signal;
    let total_size: f64 = assets.iter().map(|asset| asset.size_bytes).sum();
    let total_gzip: f64 = assets.iter().filter_map(|asset| asset.gzip_bytes).sum();

    let mut summary = String::from("Vite");
    if let Some(version) = version {
        summary.push_str(&format!(" v{version}"));
    }
    summary.push_str(" build: ok");
    if let Some(modules) = modules {
        let label = if modules == 1 { "module" } else { "modules" };
        summary.push_str(&format!(" | {modules} {label}"));
    }
    if !assets.is_empty() {
        let label = if assets.len() == 1 { "asset" } else { "assets" };
        summary.push_str(&format!(
            " | {} {}, {}",
            assets.len(),
            label,
            format_bytes(total_size)
        ));
        if total_gzip > 0.0 {
            summary.push_str(&format!(" (gzip {})", format_bytes(total_gzip)));
        }
    }
    if let Some(build_time) = build_time {
        summary.push_str(&format!(" | {build_time}"));
    }
    result.push(summary);

    if !assets.is_empty() {
        assets.sort_by(|left, right| {
            right
                .size_bytes
                .partial_cmp(&left.size_bytes)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        result.push(if assets.len() == 1 {
            "Largest asset:".to_string()
        } else {
            "Largest assets:".to_string()
        });
        for asset in assets.iter().take(MAX_LARGEST_ASSETS) {
            let mut line = format!(
                "  {} {}",
                truncate(&asset.path, 60),
                format_bytes(asset.size_bytes)
            );
            if let Some(gzip) = asset.gzip_bytes {
                line.push_str(&format!(" (gzip {})", format_bytes(gzip)));
            }
            result.push(line);
        }
        if assets.len() > MAX_LARGEST_ASSETS {
            result.push(format!(
                "  ... +{} more assets",
                assets.len() - MAX_LARGEST_ASSETS
            ));
        }
    }

    result.join("\n")
}

fn parse_asset(line: &str) -> Option<Asset> {
    let captures = ASSET_RE.captures(line)?;
    let size = captures[2].parse::<f64>().ok()?;
    let gzip = captures
        .get(4)
        .and_then(|value| value.as_str().parse::<f64>().ok())
        .zip(captures.get(5))
        .map(|(value, unit)| to_bytes(value, unit.as_str()));

    Some(Asset {
        path: captures[1].to_string(),
        size_bytes: to_bytes(size, &captures[3]),
        gzip_bytes: gzip,
    })
}

fn to_bytes(value: f64, unit: &str) -> f64 {
    match unit {
        "MB" => value * 1_000_000.0,
        "kB" => value * 1_000.0,
        _ => value,
    }
}

fn format_bytes(bytes: f64) -> String {
    if bytes >= 1_000_000.0 {
        format!("{:.1} MB", bytes / 1_000_000.0)
    } else if bytes >= 1_000.0 {
        format!("{:.1} kB", bytes / 1_000.0)
    } else {
        format!("{bytes:.0} B")
    }
}

fn is_progress_line(line: &str) -> bool {
    matches!(
        line,
        "transforming..." | "rendering chunks..." | "computing gzip size..."
    )
}

fn is_npm_lifecycle_line(line: &str) -> bool {
    line.starts_with('>') && (line.contains('@') || line.contains("vite build"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUCCESS: &str = r#"vite v6.3.5 building for production...
transforming...
✓ 1234 modules transformed.
rendering chunks...
computing gzip size...
dist/index.html                   0.46 kB │ gzip:  0.30 kB
dist/assets/index-a1b2c3.css     12.34 kB │ gzip:  3.21 kB
dist/assets/vendor-d4e5f6.js    142.11 kB │ gzip: 45.67 kB
dist/assets/index-g7h8i9.js      88.02 kB │ gzip: 28.44 kB
✓ built in 7.04s"#;

    #[test]
    fn summarizes_vite_assets_and_totals() {
        let filtered = filter_vite_output(SUCCESS, 0);

        assert!(filtered.contains("Vite v6.3.5 build: ok | 1234 modules"));
        assert!(filtered.contains("4 assets, 242.9 kB (gzip 77.6 kB)"));
        assert!(filtered.contains("vendor-d4e5f6.js 142.1 kB"));
        assert!(filtered.contains("index-g7h8i9.js 88.0 kB"));
        assert!(!filtered.contains("index.html                   0.46"));
    }

    #[test]
    fn preserves_rollup_warning_text() {
        let output = format!(
            "{SUCCESS}\n(!) Some chunks are larger than 500 kB after minification.\nConsider using dynamic import() to code-split."
        );
        let filtered = filter_vite_output(&output, 0);

        assert!(filtered.contains("(!) Some chunks are larger than 500 kB"));
        assert!(filtered.contains("Consider using dynamic import()"));
    }

    #[test]
    fn failure_output_is_untouched() {
        let output = "vite v6.3.5 building for production...\nerror during build:\nError: Could not resolve entry module\n  at error (stack.js:1:2)\n";
        assert_eq!(filter_vite_output(output, 1), output);
    }

    #[test]
    fn detects_npm_wrapped_vite_output() {
        let output = "> app@1.0.0 build\n> vite build\n\nvite v6.3.5 building for production...";
        assert!(looks_like_vite_build_output(output));
        assert!(!looks_like_vite_build_output("webpack 5 build complete"));
        assert!(!looks_like_vite_build_output(
            "> app@1.0.0 build\n> vite build\ncustom wrapper output"
        ));
    }

    #[test]
    fn caps_largest_asset_list() {
        let output = (0..8)
            .map(|index| format!("dist/chunk-{index}.js  {}.00 kB │ gzip: 1.00 kB", index + 1))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_vite_output(&output, 0);

        assert!(filtered.contains("chunk-7.js 8.0 kB"));
        assert!(!filtered.contains("chunk-2.js 3.0 kB"));
        assert!(filtered.contains("... +3 more assets"));
    }

    #[test]
    fn large_asset_table_saves_at_least_seventy_five_percent() {
        let mut lines = vec![
            "vite v6.3.5 building for production...".to_string(),
            "✓ 1500 modules transformed.".to_string(),
        ];
        lines.extend((0..100).map(|index| {
            format!(
                "dist/assets/chunk-{index:03}-abcdef.js  {}.00 kB │ gzip: {}.00 kB",
                index + 1,
                (index + 1) / 3 + 1
            )
        }));
        lines.push("✓ built in 8.00s".to_string());
        let raw = lines.join("\n");
        let filtered = filter_vite_output(&raw, 0);

        assert!(
            filtered.len() * 4 <= raw.len(),
            "expected >=75% byte reduction: raw={} filtered={}",
            raw.len(),
            filtered.len()
        );
    }
}
