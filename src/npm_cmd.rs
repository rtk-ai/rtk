use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Strip a leading "run" if present — `rtk npm run build` and `rtk npm build`
    // should both invoke `npm run build`. Without this, the former would produce
    // `npm run run build` because this function always injects "run".
    let effective_args = if args.first().map(|s| s == "run").unwrap_or(false) {
        &args[1..]
    } else {
        args
    };

    let mut cmd = Command::new("npm");
    cmd.arg("run");

    for arg in effective_args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    if verbose > 0 {
        eprintln!("Running: npm run {}", effective_args.join(" "));
    }

    let output = cmd.output().context("Failed to run npm run")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_npm_output(&raw);
    println!("{}", filtered);

    timer.track(
        &format!("npm run {}", effective_args.join(" ")),
        &format!("rtk npm run {}", effective_args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Filter npm run output - strip boilerplate, progress bars, npm WARN
fn filter_npm_output(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        // Skip npm boilerplate
        if line.starts_with('>') && line.contains('@') {
            continue;
        }
        // Skip npm lifecycle scripts
        if line.trim_start().starts_with("npm WARN") {
            continue;
        }
        if line.trim_start().starts_with("npm notice") {
            continue;
        }
        // Skip progress indicators
        if line.contains("⸩") || line.contains("⸨") || line.contains("...") && line.len() < 10 {
            continue;
        }
        // Skip empty lines
        if line.trim().is_empty() {
            continue;
        }

        result.push(line.to_string());
    }

    if result.is_empty() {
        "ok ✓".to_string()
    } else {
        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_npm_output() {
        let output = r#"
> project@1.0.0 build
> next build

npm WARN deprecated inflight@1.0.6: This module is not supported
npm notice

   Creating an optimized production build...
   ✓ Build completed
"#;
        let result = filter_npm_output(output);
        assert!(!result.contains("npm WARN"));
        assert!(!result.contains("npm notice"));
        assert!(!result.contains("> project@"));
        assert!(result.contains("Build completed"));
    }

    #[test]
    fn test_filter_npm_output_empty() {
        let output = "\n\n\n";
        let result = filter_npm_output(output);
        assert_eq!(result, "ok ✓");
    }

    #[test]
    fn test_run_prefix_stripped() {
        // "rtk npm run build" should pass ["run", "build"] as args.
        // Verify that the leading "run" is recognised and stripped so we don't
        // invoke `npm run run build`.
        let args: Vec<String> = vec!["run".to_string(), "build".to_string()];
        let effective: &[String] = if args.first().map(|s| s == "run").unwrap_or(false) {
            &args[1..]
        } else {
            &args
        };
        assert_eq!(effective, &["build".to_string()]);
    }

    #[test]
    fn test_no_run_prefix_unchanged() {
        // "rtk npm build" should pass ["build"] and remain unchanged.
        let args: Vec<String> = vec!["build".to_string()];
        let effective: &[String] = if args.first().map(|s| s == "run").unwrap_or(false) {
            &args[1..]
        } else {
            &args
        };
        assert_eq!(effective, &["build".to_string()]);
    }
}
