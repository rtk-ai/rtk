use super::lint_cmd;
use super::prettier_cmd;
use super::tsc_cmd;
use super::vitest_cmd;
use crate::core::tracking;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::{Context, Result};
use regex::Regex;

/// Native yarn commands that should never be intercepted — always passthrough.
/// Sorted for binary_search.
const NATIVE_YARN_COMMANDS: &[&str] = &[
    "add",
    "audit",
    "autoclean",
    "bin",
    "cache",
    "check",
    "config",
    "constraints",
    "create",
    "dedupe",
    "dlx",
    "exec",
    "explain",
    "generate-lock-entry",
    "global",
    "import",
    "info",
    "init",
    "install",
    "licenses",
    "link",
    "list",
    "login",
    "logout",
    "node",
    "outdated",
    "owner",
    "pack",
    "patch",
    "plugin",
    "policies",
    "publish",
    "rebuild",
    "remove",
    "set",
    "tag",
    "unlink",
    "unplug",
    "up",
    "upgrade",
    "upgrade-interactive",
    "version",
    "versions",
    "why",
    "workspace",
    "workspaces",
];

lazy_static::lazy_static! {
    static ref RE_YN_PREFIX: Regex = Regex::new(r"^(?:➤\s*)?YN\d{4}:").expect("RE_YN_PREFIX regex");
    static ref RE_RESOLUTION: Regex = Regex::new(r"^(Resolution|Fetch|Link) step \d+/\d+").expect("RE_RESOLUTION regex");
    static ref RE_PROGRESS: Regex = Regex::new(r"^\[[\d/]+\]").expect("RE_PROGRESS regex");
    static ref RE_YARN_CLASSIC_HEADER: Regex = Regex::new(r"^yarn (run|workspace) v\d").expect("RE_YARN_CLASSIC_HEADER regex");
    static ref RE_DONE: Regex = Regex::new(r"^Done in \d").expect("RE_DONE regex");
    static ref RE_INFO: Regex = Regex::new(r"^info ").expect("RE_INFO regex");
    static ref RE_SCRIPT_ECHO: Regex = Regex::new(r"^\$ \S+").expect("RE_SCRIPT_ECHO regex");
    static ref RE_WARNING: Regex = Regex::new(r"^warning ").expect("RE_WARNING regex");
}

/// Strip yarn boilerplate from stdout, preserving actual command output.
pub(crate) fn filter_yarn_output(stdout: &str) -> String {
    let clean = strip_ansi(stdout);
    let filtered: Vec<&str> = clean
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return false;
            }
            !RE_YN_PREFIX.is_match(trimmed)
                && !RE_RESOLUTION.is_match(trimmed)
                && !RE_PROGRESS.is_match(trimmed)
                && !RE_YARN_CLASSIC_HEADER.is_match(trimmed)
                && !RE_DONE.is_match(trimmed)
                && !RE_INFO.is_match(trimmed)
                && !RE_SCRIPT_ECHO.is_match(trimmed)
                && !RE_WARNING.is_match(trimmed)
        })
        .collect();
    filtered.join("\n")
}

/// Strip yarn stderr noise (ELIFECYCLE, etc.), keep real errors.
fn strip_yarn_stderr(stderr: &str) -> String {
    let clean = strip_ansi(stderr);
    clean
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.contains("ELIFECYCLE")
                && !trimmed.contains("This is probably not a problem with npm")
                && !RE_YN_PREFIX.is_match(trimmed)
                && !RE_WARNING.is_match(trimmed)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Identify which RTK filter to apply based on script name.
/// For compound scripts (e.g. `test:lib`), routes based on the prefix before `:`,
/// but checks the suffix first for known tool names (e.g. `test:e2e` → no filter,
/// since e2e is typically Playwright/Cypress, not vitest).
fn identify_filter(script_name: &str) -> Option<&'static str> {
    // For compound scripts, check suffix first for specific tool hints
    if let Some((prefix, suffix)) = script_name.split_once(':') {
        return match suffix {
            "e2e" | "cypress" | "playwright" => None,
            _ => match prefix {
                "test" | "vitest" => Some("vitest"),
                "typecheck" | "tsc" | "type-check" => Some("tsc"),
                "lint" | "eslint" | "biome" => Some("lint"),
                "prettier" | "format" => Some("prettier"),
                _ => None,
            },
        };
    }
    match script_name {
        "test" | "vitest" => Some("vitest"),
        "typecheck" | "tsc" | "type-check" => Some("tsc"),
        "lint" | "eslint" | "biome" => Some("lint"),
        "prettier" | "format" => Some("prettier"),
        _ => None,
    }
}

/// Apply the identified filter to command output.
fn apply_identified_filter(filter_type: &str, output: &str) -> Result<String> {
    match filter_type {
        "vitest" => Ok(vitest_cmd::filter_vitest_output(output)),
        "tsc" => Ok(tsc_cmd::filter_tsc_output(output)),
        "lint" => Ok(lint_cmd::filter_generic_lint(output)),
        "prettier" => Ok(prettier_cmd::filter_prettier_output(output)),
        _ => anyhow::bail!("Unknown filter type: {}", filter_type),
    }
}

// skip_env unused: yarn does not have env validation like Next.js/tsc
pub fn run(args: &[String], verbose: u8, _skip_env: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Non-workspace commands
    if args.is_empty() || args[0] != "workspace" {
        // Classify: no args, flags, or native yarn commands → direct passthrough (no capture)
        let is_passthrough = args.is_empty()
            || args[0].starts_with('-')
            || NATIVE_YARN_COMMANDS
                .binary_search(&args[0].as_str())
                .is_ok();

        if is_passthrough {
            let mut cmd = resolved_command("yarn");
            for arg in args {
                cmd.arg(arg);
            }
            let status = cmd.status().context("Failed to run yarn")?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
            let args_str = args.join(" ");
            timer.track_passthrough(
                &format!("yarn {}", args_str),
                &format!("rtk yarn {} (native passthrough)", args_str),
            );
            return Ok(());
        }

        // Script path: extract script name, capture output, try filter routing
        let script_name = if args[0] == "run" {
            if args.len() < 2 {
                // `yarn run` with no script → passthrough
                let mut cmd = resolved_command("yarn");
                cmd.arg("run");
                let status = cmd.status().context("Failed to run yarn")?;
                timer.track_passthrough("yarn run", "rtk yarn run (passthrough)");
                if !status.success() {
                    std::process::exit(status.code().unwrap_or(1));
                }
                return Ok(());
            }
            args[1].as_str()
        } else {
            args[0].as_str()
        };

        let mut cmd = resolved_command("yarn");
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().context("Failed to run yarn")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let raw = format!("{}{}", stdout, stderr);
        let exit_code = output.status.code().unwrap_or(1);

        // On failure: print stripped output, no filter routing
        if !output.status.success() {
            let filtered_stdout = filter_yarn_output(&stdout);
            let filtered_stderr = strip_yarn_stderr(&stderr);
            if !filtered_stdout.is_empty() {
                println!("{}", filtered_stdout);
            }
            if !filtered_stderr.is_empty() {
                eprintln!("{}", filtered_stderr);
            }
            let args_str = args.join(" ");
            timer.track(
                &format!("yarn {}", args_str),
                &format!("rtk yarn {} (failed)", args_str),
                &raw,
                &format!("{}\n{}", filtered_stdout, filtered_stderr),
            );
            std::process::exit(exit_code);
        }

        // Success path: strip boilerplate first
        let filtered_stdout = filter_yarn_output(&stdout);

        // Empty after strip + success → "ok ✓"
        if filtered_stdout.is_empty() {
            println!("ok \u{2713}");
            let args_str = args.join(" ");
            timer.track(
                &format!("yarn {}", args_str),
                &format!("rtk yarn {}", args_str),
                &raw,
                "ok \u{2713}",
            );
            return Ok(());
        }

        // Try to identify and apply a specialized filter
        let (display, label) = match identify_filter(script_name) {
            Some(filter_type) => match apply_identified_filter(filter_type, &filtered_stdout) {
                Ok(result) if !result.is_empty() => (result, format!("{} (via yarn)", filter_type)),
                Ok(_) => (filtered_stdout.clone(), "yarn (passthrough)".to_string()),
                Err(e) => {
                    if verbose > 0 {
                        eprintln!(
                            "rtk: yarn filter '{}' failed: {}, using passthrough",
                            filter_type, e
                        );
                    }
                    (filtered_stdout.clone(), "yarn (passthrough)".to_string())
                }
            },
            None => (filtered_stdout.clone(), "yarn (passthrough)".to_string()),
        };

        println!("{}", display);

        let stripped_stderr = if !stderr.is_empty() {
            strip_yarn_stderr(&stderr)
        } else {
            String::new()
        };
        if !stripped_stderr.is_empty() {
            eprintln!("{}", stripped_stderr);
        }

        let args_str = args.join(" ");
        timer.track(
            &format!("yarn {}", args_str),
            &format!("rtk yarn {} [{}]", args_str, label),
            &raw,
            &display,
        );

        return Ok(());
    }

    // yarn workspace <pkg> [run] <script> [script_args...]
    if args.len() < 3 {
        anyhow::bail!("Usage: rtk yarn workspace <package> [run] <script> [args...]");
    }

    let pkg = &args[1];
    let (script, script_args) = if args[2] == "run" {
        if args.len() < 4 {
            anyhow::bail!("Usage: rtk yarn workspace <package> run <script> [args...]");
        }
        (&args[3], &args[4..])
    } else {
        (&args[2], &args[3..])
    };

    // Native yarn commands: passthrough
    if NATIVE_YARN_COMMANDS.binary_search(&script.as_str()).is_ok() {
        let mut cmd = resolved_command("yarn");
        for arg in args {
            cmd.arg(arg);
        }
        let status = cmd.status().context("Failed to run yarn")?;
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
        let args_str = args.join(" ");
        timer.track_passthrough(
            &format!("yarn {}", args_str),
            &format!("rtk yarn {} (native passthrough)", args_str),
        );
        return Ok(());
    }

    if verbose > 0 {
        eprintln!(
            "Running: yarn workspace {} run {} {}",
            pkg,
            script,
            script_args.join(" ")
        );
    }

    // Execute: yarn workspace <pkg> run <script> <script_args>
    let mut cmd = resolved_command("yarn");
    cmd.args(["workspace", pkg, "run", script]);
    for arg in script_args {
        cmd.arg(arg);
    }

    let output = cmd
        .output()
        .context("Failed to run yarn workspace command")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}{}", stdout, stderr);
    let exit_code = output.status.code().unwrap_or(1);

    // On failure: print stripped output, no filter routing
    if !output.status.success() {
        let filtered_stdout = filter_yarn_output(&stdout);
        let filtered_stderr = strip_yarn_stderr(&stderr);

        if !filtered_stdout.is_empty() {
            println!("{}", filtered_stdout);
        }
        if !filtered_stderr.is_empty() {
            eprintln!("{}", filtered_stderr);
        }

        timer.track(
            &format!("yarn workspace {} run {}", pkg, script),
            &format!("rtk yarn workspace {} {} (failed)", pkg, script),
            &raw,
            &format!("{}\n{}", filtered_stdout, filtered_stderr),
        );

        std::process::exit(exit_code);
    }

    // Success path: strip boilerplate first
    let filtered_stdout = filter_yarn_output(&stdout);

    // Empty after strip + success → "ok ✓"
    if filtered_stdout.is_empty() {
        println!("ok \u{2713}");
        timer.track(
            &format!("yarn workspace {} run {}", pkg, script),
            &format!("rtk yarn workspace {} {}", pkg, script),
            &raw,
            "ok \u{2713}",
        );
        return Ok(());
    }

    // Try to identify and apply a specialized filter
    let (display, label) = match identify_filter(script) {
        Some(filter_type) => match apply_identified_filter(filter_type, &filtered_stdout) {
            Ok(result) if !result.is_empty() => {
                (result, format!("{} (via yarn workspace)", filter_type))
            }
            Ok(_) => (
                filtered_stdout.clone(),
                "yarn workspace (passthrough)".to_string(),
            ),
            Err(e) => {
                if verbose > 0 {
                    eprintln!(
                        "rtk: yarn filter '{}' failed: {}, using passthrough",
                        filter_type, e
                    );
                }
                (
                    filtered_stdout.clone(),
                    "yarn workspace (passthrough)".to_string(),
                )
            }
        },
        None => (
            filtered_stdout.clone(),
            "yarn workspace (passthrough)".to_string(),
        ),
    };

    println!("{}", display);

    timer.track(
        &format!("yarn workspace {} run {}", pkg, script),
        &format!("rtk yarn workspace {} {} [{}]", pkg, script, label),
        &raw,
        &display,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Boilerplate stripping (9 tests) ──

    #[test]
    fn test_filter_clean_output() {
        let input = "Tests: 5 passed\nAll tests passed";
        let output = filter_yarn_output(input);
        assert!(output.contains("Tests: 5 passed"));
        assert!(output.contains("All tests passed"));
    }

    #[test]
    fn test_filter_yn_prefix() {
        let input = "YN0000: Done\nYN0002: Missing peer\nReal output here";
        let output = filter_yarn_output(input);
        assert!(!output.contains("YN0000"));
        assert!(!output.contains("YN0002"));
        assert!(output.contains("Real output here"));
    }

    #[test]
    fn test_filter_resolution_steps() {
        let input = "Resolution step 1/3\nFetch step 2/3\nLink step 3/3\nActual output";
        let output = filter_yarn_output(input);
        assert!(!output.contains("Resolution step"));
        assert!(!output.contains("Fetch step"));
        assert!(!output.contains("Link step"));
        assert!(output.contains("Actual output"));
    }

    #[test]
    fn test_filter_yarn_classic_headers() {
        let input = "yarn run v1.22.19\nyarn workspace v1.22.19\nTest results here";
        let output = filter_yarn_output(input);
        assert!(!output.contains("yarn run v1"));
        assert!(!output.contains("yarn workspace v1"));
        assert!(output.contains("Test results here"));
    }

    #[test]
    fn test_filter_warning_lines() {
        let input = "warning package.json: No license field\nActual output";
        let output = filter_yarn_output(input);
        assert!(!output.contains("warning"));
        assert!(output.contains("Actual output"));
    }

    #[test]
    fn test_filter_script_echo() {
        let input = "$ vitest run --reporter=json\nTest output here";
        let output = filter_yarn_output(input);
        assert!(!output.contains("$ vitest"));
        assert!(output.contains("Test output here"));
    }

    #[test]
    fn test_filter_mixed_output() {
        let input = "\
yarn run v1.22.19
warning package.json: No license field
$ vitest run --reporter=json
YN0000: Done
Resolution step 1/2
[1/4] Resolving packages
info Visit https://yarnpkg.com
Done in 1.23s
Real test output here
Another real line";
        let output = filter_yarn_output(input);
        assert_eq!(output, "Real test output here\nAnother real line");
    }

    #[test]
    fn test_filter_empty_returns_empty() {
        assert_eq!(filter_yarn_output(""), "");
    }

    #[test]
    fn test_filter_done_in_xs() {
        let input = "Done in 1.23s\nDone in 45.67s";
        let output = filter_yarn_output(input);
        assert_eq!(output, "");
    }

    // ── identify_filter (6 tests) ──

    #[test]
    fn test_identify_vitest() {
        assert_eq!(identify_filter("test"), Some("vitest"));
    }

    #[test]
    fn test_identify_tsc() {
        assert_eq!(identify_filter("typecheck"), Some("tsc"));
    }

    #[test]
    fn test_identify_lint() {
        assert_eq!(identify_filter("lint"), Some("lint"));
    }

    #[test]
    fn test_identify_unknown() {
        assert_eq!(identify_filter("build"), None);
    }

    #[test]
    fn test_identify_test_run() {
        assert_eq!(identify_filter("test:run"), Some("vitest"));
    }

    #[test]
    fn test_identify_compound_test_routes_vitest() {
        assert_eq!(identify_filter("test:lib"), Some("vitest"));
        assert_eq!(identify_filter("test:unit"), Some("vitest"));
    }

    #[test]
    fn test_identify_compound_lint_routes_lint() {
        assert_eq!(identify_filter("lint:fix"), Some("lint"));
        assert_eq!(identify_filter("lint:check"), Some("lint"));
    }

    #[test]
    fn test_identify_compound_unknown_prefix() {
        assert_eq!(identify_filter("build:prod"), None);
        assert_eq!(identify_filter("dev:watch"), None);
    }

    // ── Native commands (3 tests) ──

    #[test]
    fn test_native_commands_sorted() {
        for i in 1..NATIVE_YARN_COMMANDS.len() {
            assert!(
                NATIVE_YARN_COMMANDS[i - 1] < NATIVE_YARN_COMMANDS[i],
                "NATIVE_YARN_COMMANDS not sorted at index {}: {:?} >= {:?}",
                i,
                NATIVE_YARN_COMMANDS[i - 1],
                NATIVE_YARN_COMMANDS[i]
            );
        }
    }

    #[test]
    fn test_native_commands_not_intercepted() {
        assert!(NATIVE_YARN_COMMANDS.binary_search(&"install").is_ok());
        assert!(NATIVE_YARN_COMMANDS.binary_search(&"add").is_ok());
        assert!(NATIVE_YARN_COMMANDS.binary_search(&"why").is_ok());
    }

    #[test]
    fn test_workspace_is_in_native() {
        assert!(NATIVE_YARN_COMMANDS.binary_search(&"workspace").is_ok());
    }

    // ── Integration-style unit tests (3 tests) ──

    #[test]
    fn test_filter_then_identify_vitest() {
        let input = "\
yarn run v1.22.19
$ vitest run --reporter=json
warning package.json: No license field
{\"numTotalTests\": 5, \"numPassedTests\": 5, \"numFailedTests\": 0, \"numPendingTests\": 0, \"testResults\": [], \"startTime\": 1000, \"endTime\": 1200}
Done in 2.34s";
        let filtered = filter_yarn_output(input);
        assert!(filtered.contains("numTotalTests"));

        let filter_type = identify_filter("test");
        assert_eq!(filter_type, Some("vitest"));

        let result = apply_identified_filter("vitest", &filtered).unwrap();
        assert!(result.contains("PASS (5)"));
    }

    #[test]
    fn test_e2e_not_routed_to_vitest() {
        assert_eq!(identify_filter("test:e2e"), None);
        assert_eq!(identify_filter("test:cypress"), None);
        assert_eq!(identify_filter("test:playwright"), None);
        // But regular test compounds still route to vitest
        assert_eq!(identify_filter("test:unit"), Some("vitest"));
        assert_eq!(identify_filter("test:lib"), Some("vitest"));
    }

    #[test]
    fn test_empty_after_strip_produces_empty() {
        let input = "\
yarn run v1.22.19
$ yarn install
warning No license field
YN0000: Done
Done in 0.5s";
        let filtered = filter_yarn_output(input);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_apply_filter_tsc() {
        let tsc_output =
            "src/index.ts(10,5): error TS2322: Type 'string' is not assignable to type 'number'.";
        let result = apply_identified_filter("tsc", tsc_output).unwrap();
        assert!(
            result.contains("TS2322"),
            "Expected TS error code in output, got: {}",
            result
        );
    }

    // ── Clap parsing (5 tests) ──

    #[test]
    fn test_yarn_basic_parse() {
        use crate::Cli;
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["rtk", "yarn", "workspace", "my-app", "run", "test"]).unwrap();
        match cli.command {
            crate::Commands::Yarn { args } => {
                assert_eq!(args, vec!["workspace", "my-app", "run", "test"]);
            }
            _ => panic!("Expected Yarn command"),
        }
    }

    #[test]
    fn test_yarn_scoped_package() {
        use crate::Cli;
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["rtk", "yarn", "workspace", "@scope/pkg", "run", "test"]).unwrap();
        match cli.command {
            crate::Commands::Yarn { args } => {
                assert_eq!(args, vec!["workspace", "@scope/pkg", "run", "test"]);
            }
            _ => panic!("Expected Yarn command"),
        }
    }

    #[test]
    fn test_yarn_without_run() {
        use crate::Cli;
        use clap::Parser;
        let cli = Cli::try_parse_from(["rtk", "yarn", "workspace", "my-app", "test"]).unwrap();
        match cli.command {
            crate::Commands::Yarn { args } => {
                assert_eq!(args, vec!["workspace", "my-app", "test"]);
            }
            _ => panic!("Expected Yarn command"),
        }
    }

    #[test]
    fn test_yarn_non_workspace() {
        use crate::Cli;
        use clap::Parser;
        let cli = Cli::try_parse_from(["rtk", "yarn", "install"]).unwrap();
        match cli.command {
            crate::Commands::Yarn { args } => {
                assert_eq!(args, vec!["install"]);
            }
            _ => panic!("Expected Yarn command"),
        }
    }

    #[test]
    fn test_yarn_extra_args() {
        use crate::Cli;
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "rtk",
            "yarn",
            "workspace",
            "my-app",
            "run",
            "test",
            "--",
            "--verbose",
        ])
        .unwrap();
        match cli.command {
            crate::Commands::Yarn { args } => {
                assert_eq!(
                    args,
                    vec!["workspace", "my-app", "run", "test", "--", "--verbose"]
                );
            }
            _ => panic!("Expected Yarn command"),
        }
    }

    // ── P0: strip_yarn_stderr tests (3 tests) ──

    #[test]
    fn test_strip_stderr_elifecycle() {
        let input =
            "error Command failed with exit code 1.\nELIFECYCLE\nReal error: Cannot find module";
        let output = strip_yarn_stderr(input);
        assert!(!output.contains("ELIFECYCLE"));
        assert!(output.contains("Real error"));
    }

    #[test]
    fn test_strip_stderr_keeps_real_errors() {
        let input = "error: Cannot find module 'react'\nwarning No license field";
        let output = strip_yarn_stderr(input);
        assert!(output.contains("Cannot find module"));
        assert!(!output.contains("warning"));
    }

    #[test]
    fn test_strip_stderr_empty() {
        assert_eq!(strip_yarn_stderr(""), "");
    }

    // ── P0: Arrow-prefix YN test ──

    #[test]
    fn test_filter_yn_prefix_with_arrow() {
        let input = "➤ YN0000: Done\n➤ YN0001: Warning message\nReal output";
        let output = filter_yarn_output(input);
        assert!(!output.contains("YN0000"));
        assert!(!output.contains("YN0001"));
        assert!(output.contains("Real output"));
    }

    // ── P1: Token savings test ──

    #[test]
    fn test_token_savings_filter_yarn_output() {
        fn count_tokens(text: &str) -> usize {
            text.split_whitespace().count()
        }
        let input = "\
yarn run v1.22.19
warning package.json: No license field
$ vitest run --reporter=json
YN0000: Done
Resolution step 1/2
Fetch step 2/2
[1/4] Resolving packages
info Visit https://yarnpkg.com for documentation
Done in 1.23s
warning some other warning here
$ node scripts/build.js
info More yarn boilerplate
Real test output line one
Real test output line two
Real test output line three";
        let output = filter_yarn_output(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected >=60% savings, got {:.1}% (input={}, output={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ── P1: ANSI codes test ──

    #[test]
    fn test_filter_strips_ansi_before_matching() {
        let input = "\x1b[2mYN0000\x1b[0m: Done\nReal output here";
        let output = filter_yarn_output(input);
        assert!(output.contains("Real output here"));
        assert!(!output.contains("YN0000"));
    }

    // ── P2: identify_filter alias coverage ──

    #[test]
    fn test_identify_vitest_alias() {
        assert_eq!(identify_filter("vitest"), Some("vitest"));
    }

    #[test]
    fn test_identify_tsc_aliases() {
        assert_eq!(identify_filter("tsc"), Some("tsc"));
        assert_eq!(identify_filter("type-check"), Some("tsc"));
    }

    #[test]
    fn test_identify_prettier() {
        assert_eq!(identify_filter("prettier"), Some("prettier"));
        assert_eq!(identify_filter("format"), Some("prettier"));
    }

    // ── P3: Empty args Clap test ──

    #[test]
    fn test_yarn_no_args() {
        use crate::Cli;
        use clap::Parser;
        let cli = Cli::try_parse_from(["rtk", "yarn"]).unwrap();
        match cli.command {
            crate::Commands::Yarn { args } => {
                assert!(args.is_empty());
            }
            _ => panic!("Expected Yarn command"),
        }
    }

    // ── Non-workspace filter routing (4 tests) ──

    #[test]
    fn test_non_workspace_test_routes_vitest() {
        // Simulate: `rtk yarn test` → yarn boilerplate + vitest JSON output
        let raw_output = "\
yarn run v1.22.19
$ vitest run --reporter=json
{\"numTotalTests\": 10, \"numPassedTests\": 10, \"numFailedTests\": 0, \"numPendingTests\": 0, \"testResults\": [], \"startTime\": 1000, \"endTime\": 2000}
Done in 3.45s";
        let filtered = filter_yarn_output(raw_output);
        let filter_type = identify_filter("test");
        assert_eq!(filter_type, Some("vitest"));
        let result = apply_identified_filter("vitest", &filtered).unwrap();
        assert!(
            result.contains("PASS"),
            "Expected vitest filter applied, got: {}",
            result
        );
    }

    #[test]
    fn test_non_workspace_lint_routes_lint() {
        let raw_output = "\
yarn run v1.22.19
$ eslint src/
Done in 5.00s
src/index.ts
  10:5  error  Unexpected console statement  no-console
  15:1  error  Missing return type  @typescript-eslint/explicit-function-return-type";
        let filtered = filter_yarn_output(raw_output);
        let filter_type = identify_filter("lint");
        assert_eq!(filter_type, Some("lint"));
        let result = apply_identified_filter("lint", &filtered).unwrap();
        // lint filter should process the output (at minimum preserve error info)
        assert!(
            result.contains("error") || result.contains("no-console"),
            "Expected lint filter applied, got: {}",
            result
        );
    }

    #[test]
    fn test_non_workspace_build_passthrough() {
        // "build" is not a known filter script → identify_filter returns None
        assert_eq!(identify_filter("build"), None);
        assert_eq!(identify_filter("dev"), None);
        assert_eq!(identify_filter("start"), None);
        assert_eq!(identify_filter("clean"), None);
    }

    #[test]
    fn test_non_workspace_run_keyword() {
        // When args = ["run", "test"], script should be "test" (second element)
        let args: Vec<String> = vec!["run".into(), "test".into()];
        let script_name = if args[0] == "run" {
            args[1].as_str()
        } else {
            args[0].as_str()
        };
        assert_eq!(script_name, "test");
        assert_eq!(identify_filter(script_name), Some("vitest"));
    }
}
