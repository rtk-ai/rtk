use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("npm");
    cmd.arg("run");

    // Strip leading "run" to avoid doubling (rtk npm run build → npm run build, not npm run run build)
    let effective_args = if args.first().map(|s| s.as_str()) == Some("run") {
        &args[1..]
    } else {
        args
    };

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
    fn test_strip_leading_run_from_args() {
        // When user runs `rtk npm run build`, args = ["run", "build"]
        // The "run" should be stripped since cmd.arg("run") already adds it
        let args: Vec<String> = vec!["run".into(), "build".into()];
        let effective_args = if args.first().map(|s| s.as_str()) == Some("run") {
            &args[1..]
        } else {
            &args[..]
        };
        assert_eq!(effective_args, &["build"]);

        // When user runs `rtk npm build`, args = ["build"]
        // No stripping needed
        let args2: Vec<String> = vec!["build".into()];
        let effective_args2 = if args2.first().map(|s| s.as_str()) == Some("run") {
            &args2[1..]
        } else {
            &args2[..]
        };
        assert_eq!(effective_args2, &["build"]);

        // When user runs `rtk npm run`, args = ["run"]
        // Strip "run" → empty args (npm run with no script)
        let args3: Vec<String> = vec!["run".into()];
        let effective_args3 = if args3.first().map(|s| s.as_str()) == Some("run") {
            &args3[1..]
        } else {
            &args3[..]
        };
        assert!(effective_args3.is_empty());
    }

    #[test]
    fn test_filter_npm_output_empty() {
        let output = "\n\n\n";
        let result = filter_npm_output(output);
        assert_eq!(result, "ok ✓");
    }
}
