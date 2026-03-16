use crate::tracking;
use crate::utils::resolved_command;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref NPM_SUMMARY_RE: Regex = Regex::new(r"(?i)^added \d+ packages? in").unwrap();
    static ref NPM_AUDIT_RE: Regex =
        Regex::new(r"(?i)^(\d+ (vulnerabilit|package)|found \d+ vulnerabilit)").unwrap();
    static ref NPM_TIMING_RE: Regex = Regex::new(r"^npm timing").unwrap();
    static ref NPM_REIFY_RE: Regex = Regex::new(r"^npm http |^npm timing|reify:").unwrap();
}

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

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("npm");

    // Determine if this is "npm run <script>" or another npm subcommand (install, list, etc.)
    // Only inject "run" when args look like a script name, not a known npm subcommand.
    let first_arg = args.first().map(|s| s.as_str());
    let is_run_explicit = first_arg == Some("run");
    let is_npm_subcommand = first_arg
        .map(|a| NPM_SUBCOMMANDS.contains(&a) || a.starts_with('-'))
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

    let is_ci = first_arg == Some("ci");
    let filtered = if is_ci {
        filter_npm_ci(&raw)
    } else {
        filter_npm_output(&raw)
    };
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

/// Filter npm ci output — strip per-package reification noise, keep summary + audit + errors
fn filter_npm_ci(output: &str) -> String {
    let mut summary_line: Option<&str> = None;
    let mut audit_lines: Vec<&str> = Vec::new();
    let mut error_lines: Vec<&str> = Vec::new();
    let mut deprecation_warnings: Vec<&str> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Capture final summary (e.g., "added 1234 packages in 15s")
        if NPM_SUMMARY_RE.is_match(trimmed) {
            summary_line = Some(trimmed);
            continue;
        }

        // Capture audit summary lines
        if NPM_AUDIT_RE.is_match(trimmed) {
            audit_lines.push(trimmed);
            continue;
        }

        // Capture errors
        if trimmed.starts_with("npm ERR!") || trimmed.starts_with("npm error") {
            error_lines.push(trimmed);
            continue;
        }

        // Capture deprecation warnings (useful signal)
        if trimmed.starts_with("npm WARN deprecated") {
            deprecation_warnings.push(trimmed);
            continue;
        }

        // Skip: reification lines, timing, progress, http, other warnings/notices
        if NPM_REIFY_RE.is_match(trimmed)
            || NPM_TIMING_RE.is_match(trimmed)
            || trimmed.starts_with("npm WARN")
            || trimmed.starts_with("npm notice")
            || trimmed.contains("⸩")
            || trimmed.contains("⸨")
        {
            continue;
        }
    }

    let mut result: Vec<&str> = Vec::new();

    // Errors first (most important)
    for line in &error_lines {
        result.push(line);
    }

    // Summary line
    if let Some(summary) = summary_line {
        result.push(summary);
    }

    // Deprecation warnings (limited to first 5)
    for warn in deprecation_warnings.iter().take(5) {
        result.push(warn);
    }
    if deprecation_warnings.len() > 5 {
        // Can't push a formatted string into Vec<&str>, so we handle this below
    }

    // Audit lines
    for line in &audit_lines {
        result.push(line);
    }

    if result.is_empty() {
        "ok ✓".to_string()
    } else {
        let mut out = result.join("\n");
        if deprecation_warnings.len() > 5 {
            out.push_str(&format!(
                "\n... +{} more deprecation warnings",
                deprecation_warnings.len() - 5
            ));
        }
        out
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
        assert_eq!(result, "ok ✓");
    }

    // --- npm ci tests ---

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_npm_ci_basic() {
        let output = r#"npm WARN deprecated inflight@1.0.6: This module is not supported
npm WARN deprecated glob@7.2.3: Glob versions prior to v9 are no longer supported
npm timing reifyNode:node_modules/@babel/core Completed in 150ms
npm timing reifyNode:node_modules/react Completed in 100ms
npm timing reifyNode:node_modules/next Completed in 200ms
npm http fetch GET 200 https://registry.npmjs.org/express 50ms
npm http fetch GET 200 https://registry.npmjs.org/lodash 30ms

added 1234 packages in 15s

42 packages are looking for funding
  run `npm fund` for details

found 0 vulnerabilities
"#;
        let result = filter_npm_ci(output);
        assert!(result.contains("added 1234 packages in 15s"));
        assert!(result.contains("0 vulnerabilities"));
        assert!(!result.contains("reifyNode"));
        assert!(!result.contains("npm http"));
        assert!(!result.contains("npm timing"));
    }

    #[test]
    fn test_filter_npm_ci_with_vulnerabilities() {
        let output = r#"npm WARN deprecated mkdirp@0.5.6: Legacy versions of mkdirp are no longer supported
npm timing reifyNode:node_modules/lodash Completed in 50ms

added 856 packages in 10s

8 vulnerabilities (2 moderate, 6 high)

To address all issues, run:
  npm audit fix
"#;
        let result = filter_npm_ci(output);
        assert!(result.contains("added 856 packages in 10s"));
        assert!(result.contains("8 vulnerabilities"));
        assert!(!result.contains("reifyNode"));
    }

    #[test]
    fn test_filter_npm_ci_with_errors() {
        let output = r#"npm ERR! code ERESOLVE
npm ERR! ERESOLVE unable to resolve dependency tree
npm ERR!
npm ERR! Could not resolve dependency:
npm timing reifyNode:node_modules/react Completed in 50ms
"#;
        let result = filter_npm_ci(output);
        assert!(result.contains("npm ERR! code ERESOLVE"));
        assert!(result.contains("npm ERR! ERESOLVE unable to resolve"));
        assert!(!result.contains("reifyNode"));
    }

    #[test]
    fn test_npm_ci_token_savings() {
        let mut lines: Vec<String> = Vec::new();
        // Simulate 50 reification lines
        for i in 0..50 {
            lines.push(format!(
                "npm timing reifyNode:node_modules/@scope/package-{} Completed in {}ms",
                i,
                i * 10
            ));
        }
        // Add http lines
        for i in 0..20 {
            lines.push(format!(
                "npm http fetch GET 200 https://registry.npmjs.org/package-{} {}ms",
                i,
                i * 5
            ));
        }
        lines.push(String::new());
        lines.push("added 1234 packages in 25s".to_string());
        lines.push(String::new());
        lines.push("found 0 vulnerabilities".to_string());

        let output = lines.join("\n");
        let result = filter_npm_ci(&output);
        let input_tokens = count_tokens(&output);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 70.0,
            "npm ci filter: expected >=70% savings, got {:.1}%",
            savings
        );
    }
}
