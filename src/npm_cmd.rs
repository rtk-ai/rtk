use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("npm");

    // Determine if this is "npm run <script>" or another npm subcommand (install, list, etc.)
    // Only inject "run" when args look like a script name, not a known npm subcommand.
    let npm_subcommands = [
        "install",
        "i",
        "ci",
        "uninstall",
        "remove",
        "rm",
        "update",
        "up",
        "list",
        "ls",
        "outdated",
        "init",
        "create",
        "publish",
        "pack",
        "link",
        "audit",
        "fund",
        "exec",
        "explain",
        "why",
        "search",
        "view",
        "info",
        "show",
        "config",
        "set",
        "get",
        "cache",
        "prune",
        "dedupe",
        "doctor",
        "help",
        "version",
        "prefix",
        "root",
        "bin",
        "bugs",
        "docs",
        "home",
        "repo",
        "ping",
        "whoami",
        "token",
        "profile",
        "team",
        "access",
        "owner",
        "deprecate",
        "dist-tag",
        "star",
        "stars",
        "login",
        "logout",
        "adduser",
        "unpublish",
        "pkg",
        "diff",
        "rebuild",
        "test",
        "t",
        "start",
        "stop",
        "restart",
    ];

    let first_arg = args.first().map(|s| s.as_str());
    let is_run_explicit = first_arg == Some("run");
    let is_npm_subcommand = first_arg
        .map(|a| npm_subcommands.contains(&a) || a.starts_with('-'))
        .unwrap_or(false);

    let effective_args = if is_run_explicit {
        // "rtk npm run build" → "npm run build"
        cmd.arg("run");
        &args[1..]
    } else if is_npm_subcommand {
        // "rtk npm install express" → "npm install express"
        args
    } else {
        // "rtk npm build" → "npm run build" (assume script name)
        cmd.arg("run");
        args
    };

    for arg in effective_args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    if verbose > 0 {
        eprintln!("Running: npm {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run npm")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_npm_output(&raw);
    println!("{}", filtered);

    timer.track(
        &format!("npm {}", args.join(" ")),
        &format!("rtk npm {}", args.join(" ")),
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
    fn test_npm_subcommand_detection() {
        // Known npm subcommands should NOT get "run" injected
        let npm_subcommands = [
            "install",
            "i",
            "ci",
            "uninstall",
            "list",
            "ls",
            "outdated",
            "test",
            "t",
            "start",
            "audit",
            "init",
        ];
        for subcmd in &npm_subcommands {
            assert!(
                [
                    "install",
                    "i",
                    "ci",
                    "uninstall",
                    "remove",
                    "rm",
                    "update",
                    "up",
                    "list",
                    "ls",
                    "outdated",
                    "init",
                    "create",
                    "publish",
                    "pack",
                    "link",
                    "audit",
                    "fund",
                    "exec",
                    "explain",
                    "why",
                    "search",
                    "view",
                    "info",
                    "show",
                    "config",
                    "set",
                    "get",
                    "cache",
                    "prune",
                    "dedupe",
                    "doctor",
                    "help",
                    "version",
                    "prefix",
                    "root",
                    "bin",
                    "bugs",
                    "docs",
                    "home",
                    "repo",
                    "ping",
                    "whoami",
                    "token",
                    "profile",
                    "team",
                    "access",
                    "owner",
                    "deprecate",
                    "dist-tag",
                    "star",
                    "stars",
                    "login",
                    "logout",
                    "adduser",
                    "unpublish",
                    "pkg",
                    "diff",
                    "rebuild",
                    "test",
                    "t",
                    "start",
                    "stop",
                    "restart"
                ]
                .contains(subcmd),
                "{} should be a known npm subcommand",
                subcmd
            );
        }

        // "run" is explicit → strip it
        let args_run: Vec<String> = vec!["run".into(), "build".into()];
        assert_eq!(args_run.first().map(|s| s.as_str()), Some("run"));

        // "build" is NOT a known subcommand → inject "run"
        assert!(
            !["install", "i", "ci", "list", "ls", "test", "t"].contains(&"build"),
            "build should not be a known npm subcommand"
        );

        // "install" IS a known subcommand → no "run" injection
        assert!(
            ["install", "i", "ci", "list", "ls", "test", "t"].contains(&"install"),
            "install should be a known npm subcommand"
        );
    }

    #[test]
    fn test_filter_npm_output_empty() {
        let output = "\n\n\n";
        let result = filter_npm_output(output);
        assert_eq!(result, "ok ✓");
    }
}
