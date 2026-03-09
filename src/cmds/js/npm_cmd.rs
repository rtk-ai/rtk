//! Filters npm output and auto-injects the "run" subcommand when appropriate.

use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::process::Command;

/// Execute `npm run <script>` with boilerplate filtering.
/// Called for unrecognised scripts — well-known ones (build, test, lint,
/// typecheck) are routed to their specialised filters in main.rs before
/// reaching this function.
pub fn run_script(script: &str, args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("npm");
    cmd.arg("run").arg(script);

    for arg in args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    if verbose > 0 {
        eprintln!("Running: npm run {} {}", script, args.join(" "));
    }

    let output = cmd.output().context("Failed to run npm")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_npm_output(&raw);
    println!("{}", filtered);

    timer.track(
        &format!("npm run {}", script),
        &format!("rtk npm run {}", script),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Passthrough for non-`run` npm subcommands (install, ci, audit, init, …).
pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("npm passthrough: {:?}", args);
    }

    let status = Command::new("npm")
        .args(args)
        .status()
        .context("Failed to run npm")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("npm {}", args_str),
        &format!("rtk npm {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

/// Filter npm run output - strip boilerplate, progress bars, npm WARN
pub fn filter_npm_output(output: &str) -> String {
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
        "ok".to_string()
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
        assert_eq!(result, "ok");
    }
}
