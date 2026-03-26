use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

/// Known yarn subcommands that should NOT get "run" injected.
/// If the first arg is one of these, pass it directly to yarn.
/// Otherwise, assume it's a script name and inject "run".
const YARN_SUBCOMMANDS: &[&str] = &[
    // Package management
    "install",
    "add",
    "remove",
    "upgrade",
    "upgrade-interactive",
    // Info / inspection
    "list",
    "info",
    "why",
    "outdated",
    "audit",
    // Project lifecycle
    "init",
    "create",
    "publish",
    "pack",
    "link",
    "unlink",
    // Config
    "cache",
    "config",
    "set",
    "get",
    // Execution
    "run",
    "exec",
    "dlx",
    // Workspace
    "workspaces",
    "workspace",
    "global",
    // Misc
    "bin",
    "rebuild",
    "plugin",
    "patch",
    "npm",
    "version",
    "help",
    "check",
    // Lifecycle shortcuts
    "test",
    "start",
    "stop",
];

/// Tools that have specialized RTK filters — delegate instead of running yarn.
enum KnownTool {
    Vitest,
    Tsc,
    Lint,
    Next,
    Prettier,
    Playwright,
}

fn detect_known_tool(script: &str) -> Option<KnownTool> {
    match script {
        "vitest" => Some(KnownTool::Vitest),
        "tsc" => Some(KnownTool::Tsc),
        "eslint" | "biome" => Some(KnownTool::Lint),
        "next" => Some(KnownTool::Next),
        "prettier" => Some(KnownTool::Prettier),
        "playwright" => Some(KnownTool::Playwright),
        _ => None,
    }
}

lazy_static! {
    // Yarn Classic v1 boilerplate
    static ref YARN_RUN_HEADER: Regex = Regex::new(r"^yarn run v\d+\.\d+").unwrap();
    static ref YARN_DONE: Regex = Regex::new(r"^Done in \d+\.\d+s\.?$").unwrap();
    static ref YARN_DOLLAR_CMD: Regex = Regex::new(r"^\$ .+").unwrap();
    // Yarn Berry info codes (YN0000, YN0007, etc.)
    static ref YARN_BERRY_INFO: Regex = Regex::new(r"^➤ YN\d{4}:").unwrap();
    // Install progress phases [1/4] Resolving...
    static ref INSTALL_PHASE: Regex = Regex::new(r"^\[\d+/\d+\] ").unwrap();
}

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    let first_arg = args.first().map(|s| s.as_str());
    let is_run_explicit = first_arg == Some("run");
    let is_yarn_subcommand = first_arg
        .map(|a| YARN_SUBCOMMANDS.contains(&a) || a.starts_with('-'))
        .unwrap_or(false);

    // Determine the effective script name for tool detection
    let script_name = if is_run_explicit {
        args.get(1).map(|s| s.as_str())
    } else if is_yarn_subcommand {
        None // Not a script, it's a native yarn subcommand
    } else {
        first_arg
    };

    // Check if the script maps to a tool with a specialized RTK filter
    if let Some(name) = script_name {
        if let Some(tool) = detect_known_tool(name) {
            // Remaining args after the tool name
            let remaining: Vec<String> = if is_run_explicit {
                args[2..].to_vec()
            } else {
                args[1..].to_vec()
            };

            return match tool {
                KnownTool::Vitest => crate::vitest_cmd::run(
                    crate::vitest_cmd::VitestCommand::Run,
                    &remaining,
                    verbose,
                ),
                KnownTool::Tsc => crate::tsc_cmd::run(&remaining, verbose),
                KnownTool::Lint => crate::lint_cmd::run(&remaining, verbose),
                KnownTool::Next => crate::next_cmd::run(&remaining, verbose),
                KnownTool::Prettier => crate::prettier_cmd::run(&remaining, verbose),
                KnownTool::Playwright => crate::playwright_cmd::run(&remaining, verbose),
            };
        }
    }

    // No specialized filter — run yarn directly and apply generic filtering
    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("yarn");

    let is_install_cmd = matches!(first_arg, Some("install" | "add" | "remove" | "upgrade"));

    if is_run_explicit {
        cmd.arg("run");
        for arg in &args[1..] {
            cmd.arg(arg);
        }
    } else if is_yarn_subcommand {
        for arg in args {
            cmd.arg(arg);
        }
    } else {
        cmd.arg("run");
        for arg in args {
            cmd.arg(arg);
        }
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    if verbose > 0 {
        eprintln!("Running: yarn {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run yarn")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = if is_install_cmd {
        filter_yarn_install_output(&raw)
    } else {
        filter_yarn_output(&raw)
    };
    print!("{}", filtered);

    timer.track(
        &format!("yarn {}", args.join(" ")),
        &format!("rtk yarn {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Generic filter for yarn script output — strip boilerplate wrapper lines.
fn filter_yarn_output(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        // Yarn Classic v1 boilerplate
        if YARN_RUN_HEADER.is_match(line) {
            continue;
        }
        if YARN_DOLLAR_CMD.is_match(line) {
            continue;
        }
        if YARN_DONE.is_match(line) {
            continue;
        }
        // Yarn Berry info lines
        if YARN_BERRY_INFO.is_match(line) {
            continue;
        }
        // Warning lines
        if line.starts_with("warning ") {
            continue;
        }
        // Empty lines
        if line.trim().is_empty() {
            continue;
        }

        result.push(line);
    }

    if result.is_empty() {
        "ok".to_string()
    } else {
        result.join("\n")
    }
}

/// Specialized filter for yarn install/add/remove — strip progress phases and noise.
fn filter_yarn_install_output(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        // Strip install progress phases: [1/4] Resolving packages...
        if INSTALL_PHASE.is_match(line) {
            continue;
        }
        // Strip info lines
        if line.starts_with("info ") {
            continue;
        }
        // Strip warning lines
        if line.starts_with("warning ") {
            continue;
        }
        // Strip "Fetching" lines (Berry)
        if line.contains("Fetching") && line.contains("->") {
            continue;
        }
        // Yarn Berry info lines
        if YARN_BERRY_INFO.is_match(line) {
            continue;
        }
        // Yarn Classic v1 boilerplate
        if YARN_RUN_HEADER.is_match(line) {
            continue;
        }
        if YARN_DONE.is_match(line) {
            continue;
        }
        // Empty lines
        if line.trim().is_empty() {
            continue;
        }

        result.push(line);
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

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_yarn_output() {
        let output = r#"yarn run v1.22.22
$ next build

   Creating an optimized production build...
   ✓ Compiled successfully
   ✓ Linting and checking validity
   ✓ Collecting page data

Done in 12.34s.
"#;
        let result = filter_yarn_output(output);
        assert!(!result.contains("yarn run v1.22"));
        assert!(!result.contains("$ next build"));
        assert!(!result.contains("Done in"));
        assert!(result.contains("Compiled successfully"));
        assert!(result.contains("Collecting page data"));
    }

    #[test]
    fn test_filter_yarn_output_berry() {
        let output = "➤ YN0000: · Yarn 4.1.0\n➤ YN0000: ┌ Resolution step\n➤ YN0000: └ Completed\nBuild output here\n";
        let result = filter_yarn_output(output);
        assert!(!result.contains("YN0000"));
        assert!(result.contains("Build output here"));
    }

    #[test]
    fn test_filter_yarn_output_empty() {
        let output = "yarn run v1.22.22\n$ echo done\n\nDone in 0.05s.\n";
        let result = filter_yarn_output(output);
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_filter_yarn_output_warnings() {
        let output = "yarn run v1.22.22\n$ tsc --noEmit\nwarning package.json: No license field\nwarning peer dependency missing\nsrc/index.ts(1,1): error TS2307\n\nDone in 3.20s.\n";
        let result = filter_yarn_output(output);
        assert!(!result.contains("warning"));
        assert!(result.contains("error TS2307"));
    }

    #[test]
    fn test_filter_yarn_install_output() {
        let output = r#"yarn install v1.22.22
info No lockfile found.
[1/4] Resolving packages...
[2/4] Fetching packages...
[3/4] Linking dependencies...
[4/4] Building fresh packages...
warning react-dom@18.2.0: peer dependency react@^18.2.0 not satisfied
info "fsevents@2.3.3" is an optional dependency
success Saved lockfile.
Done in 8.45s.
"#;
        let result = filter_yarn_install_output(output);
        assert!(!result.contains("[1/4]"));
        assert!(!result.contains("[2/4]"));
        assert!(!result.contains("[3/4]"));
        assert!(!result.contains("[4/4]"));
        assert!(!result.contains("info "));
        assert!(!result.contains("warning "));
        assert!(!result.contains("Done in"));
        assert!(result.contains("success Saved lockfile."));
    }

    #[test]
    fn test_filter_yarn_install_empty() {
        let output = "[1/4] Resolving packages...\n[2/4] Fetching packages...\n[3/4] Linking dependencies...\n[4/4] Building fresh packages...\nDone in 0.50s.\n";
        let result = filter_yarn_install_output(output);
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_yarn_install_token_savings() {
        let input = r#"yarn install v1.22.22
info No lockfile found.
[1/4] Resolving packages...
[2/4] Fetching packages...
info fsevents@2.3.3: The platform "linux" is incompatible with this module
info "fsevents@2.3.3" is an optional dependency
[3/4] Linking dependencies...
warning react > react-dom > scheduler: peer dependency react@>=16.8.0 not satisfied
warning typescript@5.3.3: peer dependency @types/node not satisfied
warning eslint-config-next > @typescript-eslint/parser > @typescript-eslint/typescript-estree: peer dependency typescript@>=4.0 not satisfied
[4/4] Building fresh packages...
success Saved lockfile.
Done in 45.67s.
"#;
        let output = filter_yarn_install_output(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Yarn install filter: expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_yarn_run_token_savings() {
        let input = r#"yarn run v1.22.22
$ next build

   ▲ Next.js 14.1.0

   Creating an optimized production build ...
   ✓ Compiled successfully
   ✓ Linting and checking validity of types
   ✓ Collecting page data
   ✓ Generating static pages (12/12)
   ✓ Collecting build traces
   ✓ Finalizing page optimization

Route (app)                              Size     First Load JS
┌ ○ /                                    5.17 kB        89.2 kB
├ ○ /about                               1.42 kB        85.5 kB
└ ○ /contact                             2.31 kB        86.4 kB

Done in 23.45s.
"#;
        let output = filter_yarn_output(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        // Generic filter only strips yarn boilerplate (3 lines + empties).
        // Real savings come from delegation to specialized filters (next_cmd, etc.)
        assert!(
            savings >= 10.0,
            "Yarn run filter: expected ≥10% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_yarn_subcommand_routing() {
        fn needs_run_injection(args: &[&str]) -> bool {
            let first = args.first().copied();
            let is_run_explicit = first == Some("run");
            let is_subcommand = first
                .map(|a| YARN_SUBCOMMANDS.contains(&a) || a.starts_with('-'))
                .unwrap_or(false);
            !is_run_explicit && !is_subcommand
        }

        // Known subcommands should NOT get "run" injected
        for subcmd in YARN_SUBCOMMANDS {
            assert!(
                !needs_run_injection(&[subcmd]),
                "'yarn {}' should NOT inject 'run'",
                subcmd
            );
        }

        // Script names SHOULD get "run" injected
        for script in &["build", "dev", "lint", "typecheck", "deploy"] {
            assert!(
                needs_run_injection(&[script]),
                "'yarn {}' SHOULD inject 'run'",
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
    fn test_detect_known_tool() {
        assert!(matches!(
            detect_known_tool("vitest"),
            Some(KnownTool::Vitest)
        ));
        assert!(matches!(detect_known_tool("tsc"), Some(KnownTool::Tsc)));
        assert!(matches!(detect_known_tool("eslint"), Some(KnownTool::Lint)));
        assert!(matches!(detect_known_tool("biome"), Some(KnownTool::Lint)));
        assert!(matches!(detect_known_tool("next"), Some(KnownTool::Next)));
        assert!(matches!(
            detect_known_tool("prettier"),
            Some(KnownTool::Prettier)
        ));
        assert!(matches!(
            detect_known_tool("playwright"),
            Some(KnownTool::Playwright)
        ));
        assert!(detect_known_tool("build").is_none());
        assert!(detect_known_tool("dev").is_none());
        assert!(detect_known_tool("validate-translations").is_none());
    }

    // --- Edge cases ---

    #[test]
    fn test_filter_yarn_output_preserves_errors() {
        let output = r#"yarn run v1.22.22
$ tsc --noEmit
src/app.ts(10,5): error TS2322: Type 'string' is not assignable to type 'number'.
src/utils.ts(3,1): error TS2304: Cannot find name 'foo'.

error Command failed with exit code 2.
Done in 1.50s.
"#;
        let result = filter_yarn_output(output);
        assert!(!result.contains("yarn run v1.22"));
        assert!(!result.contains("$ tsc"));
        assert!(!result.contains("Done in"));
        assert!(result.contains("error TS2322"));
        assert!(result.contains("error TS2304"));
        assert!(result.contains("error Command failed"));
    }

    #[test]
    fn test_filter_yarn_output_ansi_codes() {
        // ANSI codes in output should pass through (strip_ansi is not applied in generic filter)
        let output =
            "yarn run v1.22.22\n$ lint\n\x1b[31merror\x1b[0m: unused variable\n\nDone in 0.80s.\n";
        let result = filter_yarn_output(output);
        assert!(!result.contains("yarn run v1.22"));
        assert!(result.contains("error"));
        assert!(result.contains("unused variable"));
    }

    #[test]
    fn test_filter_yarn_output_unicode() {
        let output = "yarn run v1.22.22\n$ build\nコンパイル成功 ✓\n日本語のエラーメッセージ\n\nDone in 2.00s.\n";
        let result = filter_yarn_output(output);
        assert!(result.contains("コンパイル成功"));
        assert!(result.contains("日本語のエラーメッセージ"));
    }

    #[test]
    fn test_filter_yarn_output_only_warnings() {
        // If output is ONLY warnings + boilerplate, should return "ok"
        let output = "yarn run v1.22.22\n$ check\nwarning package.json: No license field\nwarning no description\n\nDone in 0.10s.\n";
        let result = filter_yarn_output(output);
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_filter_yarn_install_add_package() {
        let output = r#"yarn add v1.22.22
[1/4] Resolving packages...
[2/4] Fetching packages...
[3/4] Linking dependencies...
[4/4] Building fresh packages...
success Saved lockfile.
success Saved 5 new dependencies.
info Direct dependencies
├─ express@4.18.2
└─ cors@2.8.5
info All dependencies
├─ accepts@1.3.8
├─ array-flatten@1.1.1
├─ body-parser@1.20.2
├─ content-disposition@0.5.4
└─ cookie@0.6.0
Done in 3.21s.
"#;
        let result = filter_yarn_install_output(output);
        assert!(!result.contains("[1/4]"));
        assert!(!result.contains("[2/4]"));
        assert!(!result.contains("info Direct"));
        assert!(!result.contains("info All"));
        assert!(!result.contains("Done in"));
        assert!(result.contains("success Saved lockfile."));
        assert!(result.contains("success Saved 5 new dependencies."));
    }

    #[test]
    fn test_filter_yarn_install_with_errors() {
        let output = r#"[1/4] Resolving packages...
[2/4] Fetching packages...
error An unexpected error occurred: "https://registry.yarnpkg.com/nonexistent: Not found".
info If you think this is a bug, please open a bug report.
info Visit https://yarnpkg.com/en/docs/cli/install for documentation.
"#;
        let result = filter_yarn_install_output(output);
        assert!(!result.contains("[1/4]"));
        assert!(!result.contains("info "));
        assert!(result.contains("error An unexpected error"));
    }

    #[test]
    fn test_filter_yarn_install_token_savings_add() {
        let input = r#"yarn add v1.22.22
[1/4] Resolving packages...
[2/4] Fetching packages...
[3/4] Linking dependencies...
[4/4] Building fresh packages...
warning "eslint-config-next > @typescript-eslint/parser > @typescript-eslint/typescript-estree > tsutils@3.21.0" has unmet peer dependency "typescript@>=2.8.0 || >= 3.2.0-dev".
warning "react-hook-form > @hookform/resolvers" has unmet peer dependency "react-hook-form@^7.0.0".
warning "@tanstack/react-query > @tanstack/query-core" has unmet peer dependency "@tanstack/query-core@>=5.0.0".
info Direct dependencies
├─ @tanstack/react-query@5.17.9
├─ axios@1.6.5
├─ date-fns@3.2.0
├─ lodash@4.17.21
└─ zod@3.22.4
info All dependencies
├─ @tanstack/query-core@5.17.9
├─ asynckit@0.4.0
├─ combined-stream@1.0.8
├─ delayed-stream@1.0.0
├─ follow-redirects@1.15.4
├─ form-data@4.0.0
├─ mime-types@2.1.35
├─ mime-db@1.52.0
└─ proxy-from-env@1.1.0
success Saved lockfile.
success Saved 9 new dependencies.
Done in 12.34s.
"#;
        let output = filter_yarn_install_output(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 55.0,
            "Yarn add filter: expected ≥55% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_yarn_install_berry_output() {
        let output = "➤ YN0000: · Yarn 4.1.0\n➤ YN0000: ┌ Resolution step\n➤ YN0000: └ Completed in 0s 234ms\n➤ YN0000: ┌ Fetch step\n➤ YN0000: └ Completed in 0s 120ms\n➤ YN0000: ┌ Link step\n➤ YN0000: └ Completed in 0s 53ms\n➤ YN0000: · Done in 0s 407ms\n";
        let result = filter_yarn_install_output(output);
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_filter_yarn_output_multiline_error() {
        let output = r#"yarn run v1.22.22
$ eslint src/

/src/app.ts
  1:1  error  Unexpected console statement  no-console
  5:3  error  'x' is assigned but never used  no-unused-vars

/src/utils.ts
  10:1  warning  Missing return type  @typescript-eslint/explicit-function-return-type

✖ 3 problems (2 errors, 1 warning)

Done in 4.56s.
"#;
        let result = filter_yarn_output(output);
        assert!(!result.contains("yarn run v1.22"));
        assert!(!result.contains("$ eslint"));
        assert!(!result.contains("Done in"));
        assert!(result.contains("error  Unexpected console"));
        assert!(result.contains("no-unused-vars"));
        // warning lines from eslint (indented, not starting with "warning ") should be kept
        assert!(result.contains("Missing return type"));
        assert!(result.contains("3 problems"));
    }

    #[test]
    fn test_yarn_subcommand_routing_discover_commands() {
        // Verify the specific commands from rtk discover output
        fn needs_run_injection(args: &[&str]) -> bool {
            let first = args.first().copied();
            let is_run_explicit = first == Some("run");
            let is_subcommand = first
                .map(|a| YARN_SUBCOMMANDS.contains(&a) || a.starts_with('-'))
                .unwrap_or(false);
            !is_run_explicit && !is_subcommand
        }

        // "yarn build" from discover → SHOULD inject "run"
        assert!(needs_run_injection(&["build"]));
        // "yarn lint" from discover → SHOULD inject "run"
        assert!(needs_run_injection(&["lint"]));
        // "yarn vitest" from discover → SHOULD inject "run"
        assert!(needs_run_injection(&["vitest"]));
        // "yarn tsc" from discover → SHOULD inject "run"
        assert!(needs_run_injection(&["tsc"]));
        // "yarn validate-translations" from discover → SHOULD inject "run"
        assert!(needs_run_injection(&["validate-translations"]));

        // "yarn check" from discover → check is a native subcommand, should NOT inject
        assert!(!needs_run_injection(&["check"]));
        // "yarn install" → native subcommand
        assert!(!needs_run_injection(&["install"]));
        // "yarn add" → native subcommand
        assert!(!needs_run_injection(&["add"]));
    }

    #[test]
    fn test_detect_known_tool_does_not_match_subcommands() {
        // Native yarn subcommands should NOT be detected as known tools
        assert!(detect_known_tool("install").is_none());
        assert!(detect_known_tool("add").is_none());
        assert!(detect_known_tool("remove").is_none());
        assert!(detect_known_tool("check").is_none());
        assert!(detect_known_tool("list").is_none());
        assert!(detect_known_tool("audit").is_none());
        assert!(detect_known_tool("why").is_none());
    }

    #[test]
    fn test_filter_yarn_output_malformed_input() {
        // Malformed input that doesn't look like yarn output at all
        let output = "this is not yarn output at all\njust random text here\n";
        let result = filter_yarn_output(output);
        // Should passthrough unchanged (minus empty lines)
        assert!(result.contains("this is not yarn output"));
        assert!(result.contains("just random text here"));
    }

    #[test]
    fn test_filter_yarn_output_mixed_berry_and_content() {
        let output = r#"➤ YN0000: · Yarn 4.1.0
➤ YN0000: ┌ Project validation
➤ YN0007: core@workspace:. must be built because it never has been
➤ YN0000: └ Completed
Build successful: 42 modules compiled
Wrote bundle to dist/main.js (1.2 MB)
"#;
        let result = filter_yarn_output(output);
        assert!(!result.contains("YN0000"));
        assert!(!result.contains("YN0007"));
        assert!(result.contains("Build successful"));
        assert!(result.contains("Wrote bundle"));
    }
}
