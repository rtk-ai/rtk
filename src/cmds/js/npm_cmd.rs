//! Filters npm output and auto-injects the "run" subcommand when appropriate.

use crate::core::runner;
use crate::core::stream::{LineHandler, LineStreamFilter};
use crate::core::utils::resolved_command;
use anyhow::Result;

/// Known npm subcommands that should NOT get "run" injected.
/// Shared between production code and tests to avoid drift.
const NPM_SUBCOMMANDS: &[&str] = &[
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

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    // Determine if this is "npm run <script>" or another npm subcommand (install, list, etc.)
    // Only inject "run" when args look like a script name, not a known npm subcommand.
    let first_arg = args.first().map(|s| s.as_str());
    let is_run_explicit = first_arg == Some("run");
    let is_npm_subcommand = first_arg
        .map(|a| NPM_SUBCOMMANDS.contains(&a) || a.starts_with('-'))
        .unwrap_or(false);

    let mut effective_args: Vec<String> = Vec::with_capacity(args.len() + 1);
    if is_run_explicit || is_npm_subcommand {
        effective_args.extend_from_slice(args);
    } else {
        // "rtk npm build" → "npm run build" (assume script name)
        effective_args.push("run".to_string());
        effective_args.extend_from_slice(args);
    }

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
/// emits the verbose log line, and streams through the npm output filter.
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

    runner::run_streamed(
        cmd,
        name,
        &args_display,
        Box::new(LineStreamFilter::new(NpmLineHandler::default())),
        runner::RunOptions::default(),
    )
}

#[derive(Default)]
struct NpmLineHandler {
    emitted_line: bool,
}

impl LineHandler for NpmLineHandler {
    fn should_skip(&mut self, line: &str) -> bool {
        // Skip npm boilerplate
        if line.starts_with('>') && line.contains('@') {
            return true;
        }
        // Skip npm lifecycle scripts
        if line.trim_start().starts_with("npm WARN") {
            return true;
        }
        if line.trim_start().starts_with("npm notice") {
            return true;
        }
        // Skip progress indicators
        if line.contains("⸩") || line.contains("⸨") || line.contains("...") && line.len() < 10 {
            return true;
        }
        // Skip empty lines
        if line.trim().is_empty() {
            return true;
        }

        false
    }

    fn observe_line(&mut self, _line: &str) {
        self.emitted_line = true;
    }

    fn format_summary(&self, _exit_code: i32, raw: &str) -> Option<String> {
        if self.emitted_line || raw.is_empty() {
            return None;
        }

        Some(crate::core::guard::never_worse(raw, "ok\n").to_string())
    }
}

#[cfg(test)]
fn filter_npm_output(output: &str) -> String {
    use crate::core::stream::StreamFilter;

    let mut filter = LineStreamFilter::new(NpmLineHandler::default());
    let mut filtered = output
        .lines()
        .filter_map(|line| filter.feed_line(line))
        .collect::<String>();
    if let Some(summary) = filter.on_exit(0, output) {
        filtered.push_str(&summary);
    }
    filtered.trim_end_matches('\n').to_string()
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
    fn test_filter_npm_output_preserves_line_filtering_semantics() {
        let output = "> project@1.0.0 build\n\
> next build\n\
  npm WARN deprecated package\n\
npm notice update available\n\
⸩\n\
...\n\
\n\
meaningful output\n";

        assert_eq!(filter_npm_output(output), "> next build\nmeaningful output");
    }

    #[test]
    fn test_filter_npm_output_all_filtered() {
        let output = "> project@1.0.0 build\nnpm WARN hidden\nnpm notice hidden\n\n";
        assert_eq!(filter_npm_output(output), "ok");
    }

    #[test]
    fn test_filter_npm_output_truly_silent() {
        assert_eq!(filter_npm_output(""), "");
    }

    #[test]
    fn test_npm_subcommand_routing() {
        // Uses the shared NPM_SUBCOMMANDS constant — no drift between prod and test
        fn needs_run_injection(args: &[&str]) -> bool {
            let first = args.first().copied();
            let is_run_explicit = first == Some("run");
            let is_subcommand = first
                .map(|a| NPM_SUBCOMMANDS.contains(&a) || a.starts_with('-'))
                .unwrap_or(false);
            !is_run_explicit && !is_subcommand
        }

        // Known subcommands should NOT get "run" injected
        for subcmd in NPM_SUBCOMMANDS {
            assert!(
                !needs_run_injection(&[subcmd]),
                "'npm {}' should NOT inject 'run'",
                subcmd
            );
        }

        // Script names SHOULD get "run" injected
        for script in &["build", "dev", "lint", "typecheck", "deploy"] {
            assert!(
                needs_run_injection(&[script]),
                "'npm {}' SHOULD inject 'run'",
                script
            );
        }

        // Flags should NOT get "run" injected
        assert!(!needs_run_injection(&["--version"]));
        assert!(!needs_run_injection(&["-h"]));

        // Explicit "run" should NOT inject another "run"
        assert!(!needs_run_injection(&["run", "build"]));
    }

    #[test]
    fn test_filter_npm_output_empty() {
        let output = "\n\n\n";
        let result = filter_npm_output(output);
        assert_eq!(result, "ok");
    }
}
