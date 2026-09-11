//! Filters bun output — install logs, package lists, and pm commands.

use crate::core::utils::{join_or_ok, resolved_command, strip_ansi, truncate};
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;

/// JSON structure for `bun pm ls --json` output.
///
/// `version` is required on purpose. Serde ignores unknown keys, so an
/// optional-only struct also accepts a grouped shape like
/// `{"dependencies": {"express": {...}}}` and reports the group names as
/// packages. Requiring it makes that shape fail to parse and fall through to
/// the tree parser instead.
#[derive(Debug, Deserialize)]
struct BunPmPackage {
    version: String,
}

/// Build the argv for `bun <subcmd> <args>`. Specs pass through verbatim:
/// args reach bun as an argv vector (never a shell), so there is nothing to
/// escape or validate, and bun enforces its own spec syntax.
fn pkg_argv(subcmd: &str, args: &[String]) -> Vec<String> {
    std::iter::once(subcmd.to_string())
        .chain(args.iter().cloned())
        .collect()
}

/// Filter bun install/add/remove output — strip progress lines, version headers, empty lines.
pub fn filter_bun_pkg(output: &str) -> String {
    let cleaned = strip_ansi(output);
    let mut result = Vec::new();

    for line in cleaned.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Resolution progress. The running counts say nothing the trailing
        // "N packages installed" line does not, and bun only animates them on a
        // TTY, so piped output carries just these two.
        if trimmed == "Resolving dependencies" || trimmed.starts_with("Resolved, downloaded") {
            continue;
        }

        // Skip version headers like "bun install v1.1.0" / "bun add v1.1.0" / "bun remove v1.1.0"
        if (trimmed.starts_with("bun install v")
            || trimmed.starts_with("bun add v")
            || trimmed.starts_with("bun remove v"))
            && trimmed.split_whitespace().count() <= 4
        {
            continue;
        }

        // Push the original line, not `trimmed`: bun indents the frames and
        // hints under each error, and flattening them loses which hint belongs
        // to which package on a multi-error install.
        result.push(line);
    }

    join_or_ok(&result)
}

/// Parse JSON output from `bun pm ls --json`.
pub fn filter_bun_pm_ls_json(raw: &str) -> Option<String> {
    let packages: HashMap<String, BunPmPackage> = serde_json::from_str(raw).ok()?;

    if packages.is_empty() {
        return None;
    }

    let mut entries: Vec<String> = packages
        .iter()
        .map(|(name, pkg)| format!("{}@{}", name, pkg.version))
        .collect();

    entries.sort();

    let count = entries.len();
    let mut result = format!("{} deps\n", count);
    result.push_str(&entries.join("\n"));

    Some(result)
}

/// Parse the tree form of `bun pm ls` output (`\u{251c}\u{2500}\u{2500} name@version` rows).
/// This is what real bun 1.x prints even when --json is passed (the flag is
/// silently ignored), so this is the path real runs take.
fn filter_bun_pm_ls_tree(raw: &str) -> Option<String> {
    let cleaned = strip_ansi(raw);
    let mut entries: Vec<&str> = cleaned
        .lines()
        .filter(|line| {
            // Deeper levels are drawn "\u{2502}   \u{2514}\u{2500}\u{2500} name@version", so the pipe
            // starts an entry just as the tee and elbow do.
            let trimmed = line.trim_start();
            trimmed.starts_with('\u{251c}')
                || trimmed.starts_with('\u{2514}')
                || trimmed.starts_with('\u{2502}')
        })
        .map(|line| {
            line.trim_start_matches(['\u{251c}', '\u{2514}', '\u{2502}', '\u{2500}', ' '])
                .trim_end()
        })
        .filter(|entry| !entry.is_empty())
        .collect();

    if entries.is_empty() {
        return None;
    }

    entries.sort_unstable();
    entries.dedup();

    let mut result = format!("{} deps\n", entries.len());
    result.push_str(&entries.join("\n"));
    Some(result)
}

/// Pick the pm ls parser by what bun actually printed, not by the flags we
/// passed: JSON if the output is JSON, tree if it is a tree, raw text otherwise.
fn filter_bun_pm_ls(raw: &str) -> String {
    if let Some(json_result) = filter_bun_pm_ls_json(raw) {
        return json_result;
    }
    if let Some(tree_result) = filter_bun_pm_ls_tree(raw) {
        return tree_result;
    }
    filter_bun_pm_ls_text(raw)
}

/// Text fallback for `bun pm ls`.
pub fn filter_bun_pm_ls_text(raw: &str) -> String {
    // Strip first, like the JSON and tree paths: this is the path error output
    // takes, bun colorizes it, and the 500-char budget would otherwise be spent
    // on escape sequences and could cut one in half.
    let cleaned = strip_ansi(raw);
    let lines: Vec<&str> = cleaned.lines().filter(|l| !l.trim().is_empty()).collect();

    truncate(&join_or_ok(&lines), 500)
}

/// Run `bun install`, `bun add`, or `bun remove` with filtered output.
///
/// Goes through the shared core runner so stdout and stderr stay interleaved
/// in the order bun wrote them (bun puts progress on stderr), and so tracking
/// records the output that was actually shown rather than the pre-guard filter
/// result.
pub fn run_pkg(subcmd: &str, args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("bun");
    cmd.args(pkg_argv(subcmd, args));

    if verbose > 0 {
        eprintln!("Running: bun {} {}", subcmd, args.join(" "));
    }

    let display = format!("{} {}", subcmd, args.join(" "));
    let tee_label = format!("bun_{}", subcmd);
    crate::core::runner::run_filtered(
        cmd,
        "bun",
        display.trim_end(),
        filter_bun_pkg,
        crate::core::runner::RunOptions::with_tee(&tee_label),
    )
}

pub fn run_pm_ls(args: &[String], verbose: u8) -> Result<i32> {
    // No --json injection: bun 1.x ignores the flag, `filter_bun_pm_ls` selects
    // its parser from the output's shape, and a bun that rejected an unknown
    // flag would make rtk fail a command that succeeds on its own.
    let mut cmd = resolved_command("bun");
    cmd.arg("pm").arg("ls");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: bun pm ls {}", args.join(" "));
    }

    let display = format!("pm ls {}", args.join(" "));
    crate::core::runner::run_filtered(
        cmd,
        "bun",
        display.trim_end(),
        filter_bun_pm_ls,
        crate::core::runner::RunOptions::with_tee("bun_pm_ls"),
    )
}

/// Run `bun build`. Args are passed as a vector, never via a shell.
///
/// With no output flag bun writes the bundled JS to stdout, so the command is
/// a plain passthrough: filtering it would replace a user's bundle with a
/// status line, and `bun build ./index.ts > bundle.js` would silently write
/// that line to the file. Only the write-to-disk forms print a summary that is
/// safe to filter.
pub fn run_build(args: &[String], verbose: u8) -> Result<i32> {
    // Unfiltered: without an output flag the bundle goes to stdout, and with
    // one the summary naming the emitted files is the whole point of the run.
    // The errors-only filter kept neither, and the output is a few lines.
    let mut passthrough: Vec<OsString> = vec![OsString::from("build")];
    passthrough.extend(args.iter().map(OsString::from));
    crate::core::runner::run_passthrough("bun", &passthrough, verbose)
}

/// Run `bun test` showing only failures. Args are passed as a vector, never via a shell.
pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    if crate::core::runner::is_watch_mode(args) {
        let mut passthrough: Vec<OsString> = vec![OsString::from("test")];
        passthrough.extend(args.iter().map(OsString::from));
        return crate::core::runner::run_passthrough("bun", &passthrough, verbose);
    }

    let mut cmd = resolved_command("bun");
    cmd.arg("test").args(args);
    let display = format!("test {}", args.join(" "));
    crate::core::runner::run_test_cmd(
        cmd,
        "bun",
        display.trim_end(),
        "bun_test",
        crate::core::runner::TestEcosystem::Bun,
        verbose,
    )
}

/// Run `bunx <tool>`. Args are passed as a vector, never via a shell.
///
/// Uses the same light filter as the npx path rather than an errors-only one:
/// bunx hosts arbitrary tools, and for many of them stdout is the whole point.
pub fn run_bunx(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    if crate::core::runner::is_watch_mode(args) {
        let passthrough: Vec<OsString> = args.iter().map(OsString::from).collect();
        return crate::core::runner::run_passthrough("bunx", &passthrough, verbose);
    }

    crate::cmds::js::npm_cmd::exec_with("bunx", args, verbose, skip_env)
}

/// Passthrough for `bun run` and other unfiltered subcommands.
pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("bun", args, verbose)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_bun_install_strips_progress() {
        let raw = include_str!("../../../tests/fixtures/bun_install_raw.txt");
        let out = filter_bun_pkg(raw);
        assert!(!out.contains("Resolving dependencies"), "{out}");
        assert!(!out.contains("Resolved, downloaded"), "{out}");
        assert!(!out.contains("bun install v1.2.20"), "{out}");
        assert!(out.contains("+ chalk@5.6.2"), "{out}");
        assert!(out.contains("4 packages installed"), "{out}");
    }

    #[test]
    fn test_filter_bun_install_token_savings() {
        // Measured against real bun 1.2.20 output rather than a fixture shaped
        // to the filter: what bun prints when piped is a version header, two
        // resolution lines, and one line per package.
        let raw = include_str!("../../../tests/fixtures/bun_install_raw.txt");
        let out = filter_bun_pkg(raw);
        // Bytes, not tokens: bun colorizes even when piped, so what rtk records
        // is measured against the escape sequences the command really emits.
        let savings = 100.0 - (out.len() as f64 / raw.len() as f64 * 100.0);
        assert!(
            savings >= 70.0,
            "bun install filter: expected >=70% byte savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_bun_install_empty_output() {
        let output = "\n\n\n";
        let result = filter_bun_pkg(output);
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_filter_bun_install_strips_ansi() {
        let output = "\x1b[2mResolving dependencies\x1b[0m\n\x1b[32m+\x1b[0m \x1b[1mexpress\x1b[0m\x1b[2m@4.18.2\x1b[0m\n";
        let result = filter_bun_pkg(output);
        assert!(!result.contains("Resolving dependencies"));
        assert!(!result.contains("\x1b["));
        assert!(result.contains("express@4.18.2"));
    }

    #[test]
    fn test_filter_bun_install_preserves_errors() {
        let output = r#"bun install v1.2.20 (6ad208bc)
Resolving dependencies
error: PackageNotFound - "nonexistent-pkg" not found in registry
"#;
        let result = filter_bun_pkg(output);
        assert!(result.contains("error:"));
        assert!(result.contains("nonexistent-pkg"));
    }

    #[test]
    fn test_filter_bun_install_handles_remove() {
        let output = "bun remove v1.2.20 (6ad208bc)\n- express@4.18.2\n1 package removed [7.00ms]\n";
        let result = filter_bun_pkg(output);
        assert!(!result.contains("bun remove v1.2.20"));
        assert!(result.contains("- express@4.18.2"));
        assert!(result.contains("1 package removed"));
    }

    #[test]
    fn test_filter_bun_pm_ls_json() {
        let json = r#"{
            "express": {"version": "4.18.2"},
            "lodash": {"version": "4.17.21"},
            "axios": {"version": "1.6.0"}
        }"#;
        let result = filter_bun_pm_ls_json(json).expect("should parse");
        assert!(result.starts_with("3 deps"));
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines[1], "axios@1.6.0");
        assert_eq!(lines[2], "express@4.18.2");
        assert_eq!(lines[3], "lodash@4.17.21");
    }

    #[test]
    fn test_filter_bun_pm_ls_json_token_savings() {
        // Real `bun pm ls --json` carries resolved URLs and integrity hashes per dep.
        let input = r#"{
            "express": {"version": "4.18.2", "resolved": "https://registry.npmjs.org/express/-/express-4.18.2.tgz", "integrity": "sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            "lodash": {"version": "4.17.21", "resolved": "https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz", "integrity": "sha512-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
            "axios": {"version": "1.6.0", "resolved": "https://registry.npmjs.org/axios/-/axios-1.6.0.tgz", "integrity": "sha512-cccccccccccccccccccccccccccccccccccccccccccc"}
        }"#;
        let output = filter_bun_pm_ls_json(input).expect("should parse");
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Bun pm ls json filter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_bun_pm_ls_json_empty() {
        let result = filter_bun_pm_ls_json("{}");
        assert!(result.is_none());
    }

    #[test]
    fn test_filter_bun_pm_ls_json_invalid() {
        let result = filter_bun_pm_ls_json("not json");
        assert!(result.is_none());
    }

    #[test]
    fn test_filter_bun_pm_ls_selects_by_content_not_flag() {
        // Selection keys on what bun PRINTED, not on the --json flag we
        // passed: tree text must never hit the JSON parser's 500-char
        // truncation fallback.
        let raw = include_str!("../../../tests/fixtures/bun_pm_ls_all_raw.txt");
        let out = filter_bun_pm_ls(raw);
        assert!(out.starts_with("68 deps"), "{out}");
        assert!(!out.contains('\u{251c}'), "tree glyphs must be stripped: {out}");

        let json = r#"{"express": {"version": "4.18.2"}}"#;
        let out = filter_bun_pm_ls(json);
        assert!(out.starts_with("1 deps"), "{out}");

        let err = "error: No package.json was found for directory \"/home/user\"\nnote: Run \"bun init\" to initialize a project";
        let out = filter_bun_pm_ls(err);
        assert!(out.contains("No package.json"), "{out}");
    }

    #[test]
    fn test_filter_bun_pm_ls_text_strips_ansi() {
        let raw = "\x1b[31merror: No package.json was found\x1b[0m\n\x1b[2mnote: Run bun init\x1b[0m";
        let out = filter_bun_pm_ls_text(raw);
        assert!(!out.contains('\x1b'), "{out:?}");
        assert!(out.contains("No package.json"), "{out}");
    }

    #[test]
    fn test_filter_bun_pm_ls_json_rejects_grouped_shape() {
        // Group names are not packages. Without a required `version`, serde
        // accepts this and reports "dependencies"/"devDependencies" as deps.
        let grouped = r#"{"dependencies": {"express": {"version": "4.18.2"}}, "devDependencies": {"vitest": {"version": "1.0.0"}}}"#;
        assert!(filter_bun_pm_ls_json(grouped).is_none());
        // It falls through to the raw text fallback rather than reporting the
        // two group names as a confident dependency list.
        let out = filter_bun_pm_ls(grouped);
        assert!(!out.starts_with("2 deps"), "{out}");
    }

    #[test]
    fn test_filter_bun_pkg_keeps_indentation() {
        let raw = "bun install v1.3.6
error: failed to resolve left-pad
    hint: check the registry
";
        let out = filter_bun_pkg(raw);
        assert!(out.contains("    hint: check the registry"), "{out}");
    }

    #[test]
    fn test_filter_bun_pm_ls_tree_counts_nested_entries() {
        // bun draws levels below the first with a leading pipe; skipping those
        // silently undercounts the tree.
        let raw = include_str!("../../../tests/fixtures/bun_pm_ls_all_raw.txt");
        let out = filter_bun_pm_ls_tree(raw).expect("tree should parse");
        assert!(out.starts_with("68 deps"), "{out}");
        assert!(out.contains("ms@2.1.3"), "{out}");
    }

    #[test]
    fn test_filter_bun_pm_ls_tree_dedups_nested_all() {
        // `bun pm ls --all` can list the same package under several parents.
        let raw = "/home/user/project node_modules\n\u{251c}\u{2500}\u{2500} a@1.0.0\n\u{2502} \u{2514}\u{2500}\u{2500} shared@2.0.0\n\u{2514}\u{2500}\u{2500} b@1.0.0\n  \u{2514}\u{2500}\u{2500} shared@2.0.0";
        let out = filter_bun_pm_ls_tree(raw).expect("should parse");
        assert!(out.starts_with("3 deps"), "{out}");
        assert_eq!(out.matches("shared@2.0.0").count(), 1, "{out}");
    }

    #[test]
    fn test_filter_bun_pm_ls_tree_rejects_non_tree() {
        assert!(filter_bun_pm_ls_tree("error: something broke").is_none());
        assert!(filter_bun_pm_ls_tree("").is_none());
    }

    #[test]
    fn test_filter_bun_pm_ls_text_truncates() {
        let long_output = (0..100)
            .map(|i| format!("pkg-{i}@1.0.0"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = filter_bun_pm_ls_text(&long_output);
        assert!(result.len() <= 520);
    }

    #[test]
    fn test_pkg_argv_passes_specs_verbatim() {
        // Every spec bun itself accepts must reach bun untouched. rtk rejecting
        // chars like ^ ~ : # broke semver ranges and protocol specifiers.
        let specs = [
            "express",
            "@types/node",
            "lodash@^4.17.21",
            "pkg@~1.2.3",
            "@scope/pkg@>=1.0.0 <2.0.0",
            "npm:react@^18",
            "github:user/repo#branch",
            "git+https://github.com/user/repo.git",
            "workspace:*",
            "file:../sibling-pkg",
            // Shell metacharacters are inert: args reach bun as an argv
            // vector, never through a shell, so nothing needs rejecting.
            "pkg;rm -rf /",
        ];
        for spec in specs {
            let argv = pkg_argv("add", &[spec.to_string()]);
            assert_eq!(argv, vec!["add".to_string(), spec.to_string()]);
        }
    }
}
