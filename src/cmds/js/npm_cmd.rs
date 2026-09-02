//! Filters npm output and auto-injects the "run" subcommand when appropriate.

use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

/// Known npm subcommands that should NOT get "run" injected.
/// Source: union of `npm help` across npm 10 and 11, plus short aliases.
/// Entries removed from `npm help` in later versions are kept because the
/// commands still work and dropping them would cause false `run` injection.
const NPM_SUBCOMMANDS: &[&str] = &[
    // --- npm 10 + 11 `npm help` primary commands ---
    "access",
    "adduser",
    "approve-scripts",
    "audit",
    "bugs",
    "cache",
    "ci",
    "completion",
    "config",
    "dedupe",
    "deny-scripts",
    "deprecate",
    "diff",
    "dist-tag",
    "docs",
    "doctor",
    "edit",
    "exec",
    "explain",
    "explore",
    "find-dupes",
    "fund",
    "get",
    "help",
    "help-search",
    "init",
    "install",
    "install-ci-test",
    "install-test",
    "link",
    "ll",
    "login",
    "logout",
    "ls",
    "org",
    "outdated",
    "owner",
    "pack",
    "ping",
    "pkg",
    "prefix",
    "profile",
    "prune",
    "publish",
    "query",
    "rebuild",
    "repo",
    "restart",
    "root",
    "run",
    "run-script",
    "sbom",
    "search",
    "set",
    "shrinkwrap",
    "stage",
    "star",
    "stars",
    "start",
    "stop",
    "team",
    "test",
    "token",
    "trust",
    "undeprecate",
    "uninstall",
    "unpublish",
    "unstar",
    "update",
    "version",
    "view",
    "whoami",
    // --- kept from npm 10 (demoted to aliases in npm 11, still work) ---
    "bin",
    "create",
    "home",
    "hook",
    "info",
    "list",
    "remove",
    "rm",
    "show",
    "up",
    "why",
    // --- short aliases (npm help <alias> confirms these) ---
    "add",
    "cit",
    "ddp",
    "i",
    "it",
    "ln",
    "r",
    "rb",
    "s",
    "se",
    "t",
    "un",
    "x",
];

/// Build the effective npm args, injecting "run" when the first arg looks
/// like a script name rather than a known npm subcommand or flag.
fn build_effective_npm_args(args: &[String]) -> Vec<String> {
    let first_arg = args.first().map(|s| s.as_str());
    let is_run_explicit = first_arg == Some("run");
    let is_npm_subcommand = first_arg
        .map(|a| NPM_SUBCOMMANDS.contains(&a) || a.starts_with('-'))
        .unwrap_or(false);

    let mut effective_args: Vec<String> = Vec::with_capacity(args.len() + 1);
    if !(is_run_explicit || is_npm_subcommand) {
        effective_args.push("run".to_string());
    }
    effective_args.extend_from_slice(args);
    effective_args
}

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    let effective_args = build_effective_npm_args(args);
    run_filtered("npm", &effective_args, verbose, skip_env)
}

/// Run an npx tool through the same filtered pipeline as `npm`.
///
/// Used for unrouted tools in the `Commands::Npx` fallback so that
/// `rtk npx cowsay hello` dispatches to `npx`, not `npm`. Honors `--skip-env`
/// the same way `run` does.
pub fn exec(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    run_filtered("npx", args, verbose, skip_env)
}

/// Shared command-execution path for `run` (npm) and `exec` (npx).
///
/// Builds the resolved command, appends args, applies `SKIP_ENV_VALIDATION`,
/// emits the verbose log line, and routes through `runner::run_filtered` with
/// the npm output filter.
fn run_filtered(name: &str, args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    let mut cmd = resolved_command(name);
    for arg in args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    let args_display = args.join(" ");
    if verbose > 0 {
        eprintln!("Running: {} {}", name, args_display);
    }

    runner::run_filtered(
        cmd,
        name,
        &args_display,
        filter_npm_output,
        runner::RunOptions::default(),
    )
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

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().copied().map(String::from).collect()
    }

    #[test]
    fn test_npm_subcommand_routing() {
        for subcmd in NPM_SUBCOMMANDS {
            let result = build_effective_npm_args(&args(&[subcmd]));
            assert_eq!(
                result[0], *subcmd,
                "'npm {subcmd}' should NOT inject 'run'"
            );
        }

        for script in &["build", "dev", "lint", "typecheck", "deploy"] {
            let result = build_effective_npm_args(&args(&[script]));
            assert_eq!(
                result[0], "run",
                "'npm {script}' SHOULD inject 'run'"
            );
            assert_eq!(result[1], *script);
        }

        // Flags should NOT get "run" injected
        assert_eq!(build_effective_npm_args(&args(&["--version"]))[0], "--version");
        assert_eq!(build_effective_npm_args(&args(&["-h"]))[0], "-h");
    }

    #[test]
    fn test_npm_run_no_double_injection() {
        let result = build_effective_npm_args(&args(&["run", "build"]));
        assert_eq!(result, vec!["run", "build"]);

        let result = build_effective_npm_args(&args(&["run"]));
        assert_eq!(result, vec!["run"]);
    }

    #[test]
    fn test_npm_aliases_no_run_injection() {
        for alias in &["add", "x", "ln", "un", "r", "s", "se", "rb", "ddp", "it", "cit"] {
            let result = build_effective_npm_args(&args(&[alias]));
            assert_eq!(
                result[0], *alias,
                "'npm {alias}' incorrectly got 'run' injected"
            );
        }
    }

    #[test]
    fn test_npm11_commands_no_run_injection() {
        for cmd in &[
            "approve-scripts",
            "deny-scripts",
            "stage",
            "trust",
            "undeprecate",
        ] {
            let result = build_effective_npm_args(&args(&[cmd]));
            assert_eq!(
                result[0], *cmd,
                "'npm {cmd}' incorrectly got 'run' injected"
            );
        }
    }

    #[test]
    fn test_filter_npm_output_empty() {
        let output = "\n\n\n";
        let result = filter_npm_output(output);
        assert_eq!(result, "ok");
    }
}
