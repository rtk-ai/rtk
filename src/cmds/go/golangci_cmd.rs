//! Filters golangci-lint output, grouping issues by rule.

use crate::core::arg_tokenizer::{self, Flag, Grammar, TokenKind, ValueSpec};
use crate::core::args_utils;
use crate::core::config;
use crate::core::runner;
use crate::core::stream::exec_capture;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::{resolved_command, truncate};
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;

fn is_subcommand(name: &str) -> bool {
    matches!(
        name,
        "cache"
            | "completion"
            | "config"
            | "custom"
            | "fmt"
            | "formatters"
            | "help"
            | "linters"
            | "migrate"
            | "run"
            | "version"
    )
}

/// `--flag=v` or `--flag v`: every long flag below.
const VALUE: ValueSpec = ValueSpec::value();

/// Like [`VALUE`], but the short spelling takes a separate value only when it is the whole
/// argument: every short flag below.
const SOLO: ValueSpec = ValueSpec::solo_only();

/// golangci-lint's *global* flags: the value-taking flags accepted before a subcommand. `-c`
/// is `--config`'s shorthand; this list is wider than `--help`'s "Global Flags" section, which
/// omits several of these.
///
/// Solo-only, like every short flag here: Cobra does not let a value-taking shorthand sit
/// inside a cluster (`golangci-lint -vc foo.yml run` is "unknown shorthand flag: 'c' in -c",
/// verified against 2.13.1). Reading one as value-taking there swallowed the next token --
/// `-Egosec run` lost `run`, and with it the subcommand detection this list exists for.
const GLOBAL_FLAGS: &[Flag] = &[
    Flag::long("color").takes(VALUE),
    Flag::pair("c", "config").takes(SOLO),
    Flag::long("cpu-profile-path").takes(VALUE),
    Flag::long("mem-profile-path").takes(VALUE),
    Flag::long("trace-path").takes(VALUE),
];

/// golangci-lint's grammar before a subcommand.
const GLOBAL_GRAMMAR: Grammar = Grammar::posix(&[GLOBAL_FLAGS]);

/// The `run` subcommand's own flags -- a wider list than [`GLOBAL_FLAGS`], which is scoped to
/// the flags valid before a subcommand. Missing an entry here risks a value like
/// `--path-prefix --out-format` tokenizing as its own flag and being misdetected by
/// [`has_output_flag`].
const RUN_FLAGS: &[Flag] = &[
    Flag::long("build-tags").takes(VALUE),
    Flag::pair("j", "concurrency").takes(SOLO),
    Flag::long("default").takes(VALUE),
    Flag::pair("D", "disable").takes(SOLO),
    Flag::pair("E", "enable").takes(SOLO),
    Flag::long("enable-only").takes(VALUE),
    Flag::long("issues-exit-code").takes(VALUE),
    Flag::long("max-issues-per-linter").takes(VALUE),
    Flag::long("max-same-issues").takes(VALUE),
    Flag::long("modules-download-mode").takes(VALUE),
    Flag::long("new-from-merge-base").takes(VALUE),
    Flag::long("new-from-patch").takes(VALUE),
    Flag::long("new-from-rev").takes(VALUE),
    OUT_FORMAT,
    Flag::long("path-mode").takes(VALUE),
    Flag::long("path-prefix").takes(VALUE),
    // Deprecated in 2.x but still value-taking there ("flag needs an argument"), and current in
    // the 1.x installs run_filtered still supports.
    Flag::long("deadline").takes(VALUE),
    // `-e`/`-p` are 1.x's --exclude/--presets; 2.x rejects them outright either way.
    Flag::pair("e", "exclude").takes(SOLO),
    Flag::pair("p", "presets").takes(SOLO),
    Flag::long("skip-dirs").takes(VALUE),
    Flag::long("skip-files").takes(VALUE),
    Flag::long("timeout").takes(VALUE),
];

/// v1-only legacy flag (golangci-lint 2.x's --help no longer lists it, replaced by
/// output.json.path/etc.), kept for v1 installs since [`has_output_flag`] checks for it.
const OUT_FORMAT: Flag = Flag::long("out-format").takes(VALUE);

/// The `run` subcommand's grammar. Every flag valid before `run` stays valid after it, so `run`
/// lists the global table next to its own, the way any subcommand after which its parent's
/// flags stay valid shares them (`src/core/README.md`). Grammars that merely intersect get a
/// table each.
const RUN_GRAMMAR: Grammar = Grammar::posix(&[GLOBAL_FLAGS, OUTPUT_PATH_FLAGS, RUN_FLAGS]);

#[derive(Debug, PartialEq, Eq)]
struct RunInvocation {
    global_args: Vec<String>,
    run_args: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    FilteredRun(RunInvocation),
    Passthrough,
}

#[derive(Debug, Deserialize)]
struct Position {
    #[serde(rename = "Filename")]
    filename: String,
    #[serde(rename = "Line")]
    #[allow(dead_code)]
    line: usize,
    #[serde(rename = "Column")]
    #[allow(dead_code)]
    column: usize,
    #[serde(rename = "Offset", default)]
    #[allow(dead_code)]
    offset: usize,
}

#[derive(Debug, Deserialize)]
struct Issue {
    #[serde(rename = "FromLinter")]
    from_linter: String,
    #[serde(rename = "Text")]
    #[allow(dead_code)]
    text: String,
    #[serde(rename = "Pos")]
    pos: Position,
    #[serde(rename = "SourceLines", default)]
    source_lines: Vec<String>,
    #[serde(rename = "Severity", default)]
    #[allow(dead_code)]
    severity: String,
}

#[derive(Debug, Deserialize)]
struct GolangciOutput {
    #[serde(rename = "Issues")]
    issues: Vec<Issue>,
}

/// Parse major version number from `golangci-lint --version` output.
/// Returns 1 on any failure (safe fallback — v1 behaviour).
pub(crate) fn parse_major_version(version_output: &str) -> u32 {
    // Handles:
    //   "golangci-lint version 1.59.1"
    //   "golangci-lint has version 2.10.0 built with ..."
    //   "golangci-lint has version v1.64.8 built with ..."
    //
    // The `v` prefix varies with how the binary was built, and reading a v2 as a v1
    // would send it `--out-format=json`, a flag v2 removed.
    for word in version_output.split_whitespace() {
        let version = word.strip_prefix('v').unwrap_or(word);
        if let Some(major) = version
            .split('.')
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            && version.contains('.')
        {
            return major;
        }
    }
    1
}

/// Run `golangci-lint --version` and return the major version number.
/// Returns 1 on any failure.
pub(crate) fn detect_major_version() -> u32 {
    let mut cmd = resolved_command("golangci-lint");
    cmd.arg("--version");

    match exec_capture(&mut cmd) {
        Ok(r) => {
            let version_text = if r.stdout.trim().is_empty() {
                &r.stderr
            } else {
                &r.stdout
            };
            parse_major_version(version_text)
        }
        Err(_) => 1,
    }
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let args = &args_utils::restore_double_dash(args);
    match classify_invocation(args) {
        Invocation::FilteredRun(invocation) => run_filtered(args, &invocation, verbose),
        Invocation::Passthrough => run_passthrough(args, verbose),
    }
}

fn run_filtered(original_args: &[String], invocation: &RunInvocation, verbose: u8) -> Result<i32> {
    let version = detect_major_version();

    let filtered_args = build_filtered_args(invocation, version);
    let mut cmd = resolved_command("golangci-lint");
    for arg in &filtered_args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!(
            "Running: {}",
            format_command("golangci-lint", &filtered_args)
        );
    }

    let exit_code = runner::run_filtered(
        cmd,
        "golangci-lint",
        &original_args.join(" "),
        |stdout| {
            // v2 outputs JSON on first line + trailing text; v1 outputs just JSON
            let json_output = if version >= 2 {
                stdout.lines().next().unwrap_or("")
            } else {
                stdout
            };
            filter_golangci_json(json_output, version)
        },
        crate::core::runner::RunOptions::stdout_only(),
    )?;

    // golangci-lint: exit 0 = clean, exit 1 = lint issues found (not an error),
    // exit 2+ = config/build error, None = killed by signal (OOM, SIGKILL)
    Ok(if exit_code == 1 { 0 } else { exit_code })
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
    runner::run_passthrough("golangci-lint", &os_args, verbose)
}

fn classify_invocation(args: &[String]) -> Invocation {
    match find_subcommand_index(args) {
        Some(idx) if args[idx] == "run" => Invocation::FilteredRun(RunInvocation {
            global_args: args[..idx].to_vec(),
            run_args: args[idx + 1..].to_vec(),
        }),
        _ => Invocation::Passthrough,
    }
}

/// Finds the index in `args` of the first non-flag token, stopping (and returning `None`)
/// entirely at `--` or at a non-flag token that isn't a recognized subcommand — mirroring
/// golangci-lint's own arg parsing just enough to locate `run`, not fully replicate it.
fn find_subcommand_index(args: &[String]) -> Option<usize> {
    let tokens = arg_tokenizer::tokenize_grammar(args, &GLOBAL_GRAMMAR);

    for token in &tokens {
        match token.kind {
            TokenKind::DashDash => return None,
            // A bare "-" (e.g. a stdin placeholder) is unrecognized-flag-like, not a stopping
            // condition -- keep scanning past it, same as any other unrecognized `-`-prefixed
            // token, instead of treating it as "no subcommand found."
            TokenKind::Positional if token.is_free_positional() && token.text != "-" => {
                return is_subcommand(token.text).then_some(token.source_index);
            }
            _ => {}
        }
    }

    None
}

fn build_filtered_args(invocation: &RunInvocation, version: u32) -> Vec<String> {
    let mut args = invocation.global_args.clone();
    args.push("run".to_string());

    if !has_output_flag(&invocation.run_args) {
        if version >= 2 {
            args.push("--output.json.path".to_string());
            args.push("stdout".to_string());
        } else {
            args.push("--out-format=json".to_string());
        }
    }

    args.extend(invocation.run_args.clone());
    args
}

/// The nine `--output.<format>.path` sinks golangci-lint 2.x accepts, enumerated rather than
/// pattern-matched: `starts_with("output.")` would also swallow spellings golangci rejects.
/// One table, listed by [`RUN_GRAMMAR`] and asked about by the collision check, so the two cannot
/// drift apart.
const OUTPUT_PATH_FLAGS: &[Flag] = &[
    Flag::long("output.checkstyle.path").takes(VALUE),
    Flag::long("output.code-climate.path").takes(VALUE),
    Flag::long("output.html.path").takes(VALUE),
    OUTPUT_JSON_PATH,
    Flag::long("output.junit-xml.path").takes(VALUE),
    Flag::long("output.sarif.path").takes(VALUE),
    Flag::long("output.tab.path").takes(VALUE),
    Flag::long("output.teamcity.path").takes(VALUE),
    Flag::long("output.text.path").takes(VALUE),
];

/// The json sink, the one RTK itself injects.
const OUTPUT_JSON_PATH: Flag = Flag::long("output.json.path").takes(VALUE);

/// True when RTK's own `--output.json.path stdout` would collide with what the user asked for:
/// they already configured the json sink (whatever its destination -- golangci takes one path
/// per format), or they directed some other format at stdout, where two reports would
/// interleave and the json parse would choke on whichever landed first. A *file* sink for some
/// other format takes nothing away from stdout, so RTK still injects there or it is left with
/// nothing to parse.
fn has_output_flag(args: &[String]) -> bool {
    let tokens = arg_tokenizer::tokenize_grammar(args, &RUN_GRAMMAR);
    tokens.iter().any(|t| {
        t.is_one_of(&[OUT_FORMAT, OUTPUT_JSON_PATH])
            || (t.is_one_of(OUTPUT_PATH_FLAGS)
                && t.value(&tokens).is_none_or(|dest| dest == "stdout"))
    })
}

fn format_command(base: &str, args: &[String]) -> String {
    if args.is_empty() {
        base.to_string()
    } else {
        format!("{} {}", base, args.join(" "))
    }
}

/// Filter golangci-lint JSON output - group by linter and file
pub(crate) fn filter_golangci_json(output: &str, version: u32) -> String {
    let result: Result<GolangciOutput, _> = serde_json::from_str(output);

    let golangci_output = match result {
        Ok(o) => o,
        Err(e) => {
            return format!(
                "golangci-lint (JSON parse failed: {})\n{}",
                e,
                truncate(output, config::limits().passthrough_max_chars)
            );
        }
    };

    let issues = golangci_output.issues;

    if issues.is_empty() {
        return "golangci-lint: No issues found".to_string();
    }

    let total_issues = issues.len();

    // Count unique files
    let unique_files: std::collections::HashSet<_> =
        issues.iter().map(|i| &i.pos.filename).collect();
    let total_files = unique_files.len();

    // Group by linter
    let mut by_linter: HashMap<String, usize> = HashMap::new();
    for issue in &issues {
        *by_linter.entry(issue.from_linter.clone()).or_insert(0) += 1;
    }

    // Group by file
    let mut by_file: HashMap<&str, usize> = HashMap::new();
    for issue in &issues {
        *by_file.entry(issue.pos.filename.as_str()).or_insert(0) += 1;
    }

    let mut file_counts: Vec<_> = by_file.iter().collect();
    file_counts.sort_by(|a, b| b.1.cmp(a.1));

    // Build output
    let mut result = String::new();
    result.push_str(&format!(
        "golangci-lint: {} issues in {} files\n",
        total_issues, total_files
    ));

    // Show top linters
    let mut linter_counts: Vec<_> = by_linter.iter().collect();
    linter_counts.sort_by(|a, b| b.1.cmp(a.1));

    if !linter_counts.is_empty() {
        result.push_str("Top linters:\n");
        for (linter, count) in linter_counts.iter().take(10) {
            result.push_str(&format!("  {} ({}x)\n", linter, count));
        }
        result.push('\n');
    }

    // Show top files
    const MAX_GOLANGCI_FILES: usize = CAP_WARNINGS;
    result.push_str("Top files:\n");
    for (file, count) in file_counts.iter().take(MAX_GOLANGCI_FILES) {
        let short_path = compact_path(file);
        result.push_str(&format!("  {} ({} issues)\n", short_path, count));

        // Show top 3 linters in this file
        let mut file_linters: HashMap<String, Vec<&Issue>> = HashMap::new();
        for issue in issues.iter().filter(|i| i.pos.filename.as_str() == **file) {
            file_linters
                .entry(issue.from_linter.clone())
                .or_default()
                .push(issue);
        }

        let mut file_linter_counts: Vec<_> = file_linters.iter().collect();
        file_linter_counts.sort_by_key(|b| std::cmp::Reverse(b.1.len()));

        for (linter, linter_issues) in file_linter_counts.iter().take(3) {
            result.push_str(&format!("    {} ({})\n", linter, linter_issues.len()));

            // v2 only: show first source line for this linter-file group
            if version >= 2
                && let Some(first_issue) = linter_issues.first()
                && let Some(source_line) = first_issue.source_lines.first()
            {
                let trimmed = source_line.trim();
                let display = match trimmed.char_indices().nth(80) {
                    Some((i, _)) => &trimmed[..i],
                    None => trimmed,
                };
                result.push_str(&format!("      → {}\n", display));
            }
        }
    }

    if file_counts.len() > MAX_GOLANGCI_FILES {
        result.push_str(&format!(
            "\n... +{} more files\n",
            file_counts.len() - MAX_GOLANGCI_FILES
        ));
    }

    result.trim().to_string()
}

/// Compact file path (remove common prefixes)
fn compact_path(path: &str) -> String {
    let path = path.replace('\\', "/");

    if let Some(pos) = path.rfind("/pkg/") {
        format!("pkg/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/cmd/") {
        format!("cmd/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/internal/") {
        format!("internal/{}", &path[pos + 10..])
    } else if let Some(pos) = path.rfind('/') {
        path[pos + 1..].to_string()
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::arg_tokenizer::assert_takes_value_table;
    use crate::core::tracking::estimate_tokens;

    #[test]
    fn test_filter_golangci_no_issues() {
        let output = r#"{"Issues":[]}"#;
        let result = filter_golangci_json(output, 1);
        assert!(result.contains("golangci-lint"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_filter_golangci_with_issues() {
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5}
    },
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Pos": {"Filename": "main.go", "Line": 50, "Column": 10}
    },
    {
      "FromLinter": "gosimple",
      "Text": "Should use strings.Contains",
      "Pos": {"Filename": "utils.go", "Line": 15, "Column": 2}
    }
  ]
}"#;

        let result = filter_golangci_json(output, 1);
        assert!(result.contains("3 issues"));
        assert!(result.contains("2 files"));
        assert!(result.contains("errcheck"));
        assert!(result.contains("gosimple"));
        assert!(result.contains("main.go"));
        assert!(result.contains("utils.go"));
    }

    #[test]
    fn test_compact_path() {
        assert_eq!(
            compact_path("/Users/foo/project/pkg/handler/server.go"),
            "pkg/handler/server.go"
        );
        assert_eq!(
            compact_path("/home/user/app/cmd/main/main.go"),
            "cmd/main/main.go"
        );
        assert_eq!(
            compact_path("/project/internal/config/loader.go"),
            "internal/config/loader.go"
        );
        assert_eq!(compact_path("relative/file.go"), "file.go");
    }

    #[test]
    fn test_parse_version_v1_format() {
        assert_eq!(parse_major_version("golangci-lint version 1.59.1"), 1);
    }

    #[test]
    fn test_parse_version_v2_format() {
        assert_eq!(
            parse_major_version(
                "golangci-lint has version 2.10.0 built with go1.26.0 from 95dcb68a on 2026-02-17T13:05:51Z"
            ),
            2
        );
    }

    /// A `go install` build of v1 prints the version `v`-prefixed while the same
    /// build of v2 does not, so neither spelling may decide the major version.
    #[test]
    fn test_parse_version_tolerates_v_prefix() {
        assert_eq!(
            parse_major_version(
                "golangci-lint has version v1.64.8 built with go1.27.0 from (unknown)"
            ),
            1
        );
        assert_eq!(
            parse_major_version(
                "golangci-lint has version v2.13.2 built with go1.27.0 from (unknown)"
            ),
            2
        );
    }

    #[test]
    fn test_parse_version_empty_returns_1() {
        assert_eq!(parse_major_version(""), 1);
    }

    #[test]
    fn test_parse_version_malformed_returns_1() {
        assert_eq!(parse_major_version("not a version string"), 1);
    }

    #[test]
    fn test_classify_invocation_run_uses_filtered_path() {
        assert_eq!(
            classify_invocation(&["run".into(), "./...".into()]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec![],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_with_global_flag_value_uses_filtered_path() {
        assert_eq!(
            classify_invocation(&[
                "--color".into(),
                "never".into(),
                "run".into(),
                "./...".into(),
            ]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec!["--color".into(), "never".into()],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_with_short_global_flag_uses_filtered_path() {
        assert_eq!(
            classify_invocation(&["-v".into(), "run".into(), "./...".into()]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec!["-v".into()],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_with_short_flag_separate_value_uses_filtered_path() {
        // -c takes a separate-token value; its value ("foo.yml") must not be mistaken for
        // the subcommand.
        assert_eq!(
            classify_invocation(&["-c".into(), "foo.yml".into(), "run".into(), "./...".into(),]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec!["-c".into(), "foo.yml".into()],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_dashdash_before_subcommand_is_passthrough() {
        // `--` ends option parsing before any subcommand was found; golangci-lint's own
        // parser gets to decide what follows, not rtk's filter.
        assert_eq!(
            classify_invocation(&["--".into(), "run".into()]),
            Invocation::Passthrough
        );
    }

    #[test]
    fn test_classify_invocation_bare_dash_before_subcommand_uses_filtered_path() {
        // Regression: a bare "-" (e.g. a stdin placeholder) is unrecognized-flag-like, not a
        // stopping condition -- must not be mistaken for "no subcommand found."
        assert_eq!(
            classify_invocation(&["-".into(), "run".into(), "./...".into()]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec!["-".into()],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_with_inline_value_flag_uses_filtered_path() {
        assert_eq!(
            classify_invocation(&["--color=never".into(), "run".into(), "./...".into()]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec!["--color=never".into()],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_with_inline_config_flag_uses_filtered_path() {
        assert_eq!(
            classify_invocation(&["--config=foo.yml".into(), "run".into(), "./...".into()]),
            Invocation::FilteredRun(RunInvocation {
                global_args: vec!["--config=foo.yml".into()],
                run_args: vec!["./...".into()],
            })
        );
    }

    #[test]
    fn test_classify_invocation_bare_command_is_passthrough() {
        assert_eq!(classify_invocation(&[]), Invocation::Passthrough);
    }

    #[test]
    fn test_classify_invocation_version_flag_is_passthrough() {
        assert_eq!(
            classify_invocation(&["--version".into()]),
            Invocation::Passthrough
        );
    }

    #[test]
    fn test_classify_invocation_version_subcommand_is_passthrough() {
        assert_eq!(
            classify_invocation(&["version".into()]),
            Invocation::Passthrough
        );
    }

    #[test]
    fn test_build_filtered_args_does_not_duplicate_run() {
        let invocation = RunInvocation {
            global_args: vec![],
            run_args: vec!["./...".into()],
        };

        assert_eq!(
            build_filtered_args(&invocation, 2),
            vec!["run", "--output.json.path", "stdout", "./..."]
        );
    }

    #[test]
    fn test_has_output_flag_ignores_positional_after_dashdash() {
        // Regression: a positional argument literally named "--out-format" after `--` (e.g.
        // a package path to lint) must not be mistaken for the real flag.
        assert!(!has_output_flag(&[
            "./...".to_string(),
            "--".to_string(),
            "--out-format".to_string(),
        ]));
        assert!(has_output_flag(&["--out-format=json".to_string()]));
        assert!(has_output_flag(&["--output.json.path".to_string()]));
    }

    #[test]
    fn test_has_output_flag_sees_every_output_destination() {
        // golangci-lint 2.x has one --output.<format>.path per format; RTK injecting a second
        // sink makes it write two reports to stdout and the JSON parse then fails.
        for flag in [
            "--output.text.path",
            "--output.tab.path",
            "--output.sarif.path",
            "--output.checkstyle.path",
            "--output.code-climate.path",
            "--output.html.path",
            "--output.junit-xml.path",
            "--output.teamcity.path",
            "--output.json.path",
        ] {
            assert!(
                has_output_flag(&[flag.to_string(), "stdout".to_string()]),
                "{flag} is a user-chosen destination"
            );
        }
        assert!(!has_output_flag(&["--tests".to_string()]));
    }

    #[test]
    fn test_run_level_value_flags_swallow_their_own_values() {
        // `-e '--out-format'` is 1.x's --exclude pattern, not an output flag.
        assert!(!has_output_flag(&[
            "-e".to_string(),
            "--out-format".to_string()
        ]));
        assert!(!has_output_flag(&[
            "--concurrency".to_string(),
            "--output.json.path".to_string()
        ]));
    }

    #[test]
    fn test_has_output_flag_does_not_misread_a_run_level_flags_value() {
        // --path-prefix's separate-token value must not be misdetected as the real output flag.
        assert!(!has_output_flag(&[
            "--path-prefix".to_string(),
            "--output.json.path".to_string(),
        ]));
        assert!(!has_output_flag(&[
            "--disable".to_string(),
            "errcheck".to_string(),
        ]));
        assert!(!has_output_flag(&[
            "--timeout".to_string(),
            "30s".to_string()
        ]));
    }

    #[test]
    fn test_clustered_short_flag_does_not_swallow_the_subcommand() {
        // Cobra rejects a value-taking shorthand inside a cluster ("unknown shorthand flag:
        // 'c' in -c", golangci-lint 2.13.1), so reading `-Egosec`'s trailing `c` as --config
        // and eating `run` lost the subcommand and with it all filtering.
        let args: Vec<String> = ["-Egosec", "run"].iter().map(|a| a.to_string()).collect();
        assert_eq!(find_subcommand_index(&args), Some(1));

        // Solo, it still takes its value -- that spelling golangci-lint does accept.
        let args: Vec<String> = ["-c", "cfg.yml", "run"]
            .iter()
            .map(|a| a.to_string())
            .collect();
        assert_eq!(find_subcommand_index(&args), Some(2));

        // And the attached spelling is untouched by the solo-only rule.
        let args: Vec<String> = ["-cCfg.yml", "run"].iter().map(|a| a.to_string()).collect();
        assert_eq!(find_subcommand_index(&args), Some(1));
    }

    #[test]
    fn test_golangci_run_grammar_links_out_format_separate_token_value() {
        // --out-format is a v1-only legacy flag; its value must still link, not tokenize as an
        // unlinked Positional.
        let args = vec!["--out-format".to_string(), "json".to_string()];
        let tokens = arg_tokenizer::tokenize_grammar(&args, &RUN_GRAMMAR);
        assert!(
            tokens[0].linked.is_some(),
            "\"json\" must link to --out-format"
        );
    }

    #[test]
    fn test_filter_golangci_v2_fields_parse_cleanly() {
        // v2 JSON includes Severity, SourceLines, Offset — must not panic
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Severity": "error",
      "SourceLines": ["    if err := foo(); err != nil {"],
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5, "Offset": 1024}
    }
  ]
}"#;
        let result = filter_golangci_json(output, 2);
        assert!(result.contains("errcheck"));
        assert!(result.contains("main.go"));
    }

    #[test]
    fn test_filter_v2_shows_source_lines() {
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Severity": "error",
      "SourceLines": ["    if err := foo(); err != nil {"],
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5, "Offset": 0}
    }
  ]
}"#;
        let result = filter_golangci_json(output, 2);
        assert!(
            result.contains("→"),
            "v2 should show source line with → prefix"
        );
        assert!(result.contains("if err := foo()"));
    }

    #[test]
    fn test_filter_v1_does_not_show_source_lines() {
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Severity": "error",
      "SourceLines": ["    if err := foo(); err != nil {"],
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5, "Offset": 0}
    }
  ]
}"#;
        let result = filter_golangci_json(output, 1);
        assert!(!result.contains("→"), "v1 should not show source lines");
    }

    #[test]
    fn test_filter_v2_empty_source_lines_graceful() {
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Severity": "",
      "SourceLines": [],
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5, "Offset": 0}
    }
  ]
}"#;
        let result = filter_golangci_json(output, 2);
        assert!(result.contains("errcheck"));
        assert!(
            !result.contains("→"),
            "no source line to show, should degrade gracefully"
        );
    }

    #[test]
    fn test_filter_v2_source_line_truncated_to_80_chars() {
        let long_line = "x".repeat(120);
        let output = format!(
            r#"{{
  "Issues": [
    {{
      "FromLinter": "lll",
      "Text": "line too long",
      "Severity": "",
      "SourceLines": ["{}"],
      "Pos": {{"Filename": "main.go", "Line": 1, "Column": 1, "Offset": 0}}
    }}
  ]
}}"#,
            long_line
        );
        let result = filter_golangci_json(&output, 2);
        // Content truncated at 80 chars; prefix "      → " = 10 bytes (6 spaces + 3-byte arrow + space)
        // Total line max = 80 + 10 = 90 bytes
        for line in result.lines() {
            if line.trim_start().starts_with('→') {
                assert!(line.len() <= 90, "source line too long: {}", line.len());
            }
        }
    }

    #[test]
    fn test_filter_v2_source_line_truncated_non_ascii() {
        // Japanese characters are 3 bytes each; 30 chars = 90 bytes > 80 bytes naive slice would panic
        let long_line = "日".repeat(30); // 30 chars, 90 bytes
        let output = format!(
            r#"{{
  "Issues": [
    {{
      "FromLinter": "lll",
      "Text": "line too long",
      "Severity": "",
      "SourceLines": ["{}"],
      "Pos": {{"Filename": "main.go", "Line": 1, "Column": 1, "Offset": 0}}
    }}
  ]
}}"#,
            long_line
        );
        // Should not panic and output should be ≤ 80 chars
        let result = filter_golangci_json(&output, 2);
        for line in result.lines() {
            if line.trim_start().starts_with('→') {
                let content = line.trim_start().trim_start_matches('→').trim();
                assert!(
                    content.chars().count() <= 80,
                    "content chars: {}",
                    content.chars().count()
                );
            }
        }
    }

    #[test]
    fn test_filter_real_v2_clean_json() {
        let raw = include_str!("../../../tests/fixtures/golangci_v2_clean_raw.json");
        assert_eq!(
            filter_golangci_json(raw.lines().next().unwrap_or(""), 2),
            "golangci-lint: No issues found"
        );
    }

    #[test]
    fn test_filter_real_v2_issues_json() {
        let raw = include_str!("../../../tests/fixtures/golangci_v2_issues_raw.json");
        let filtered = filter_golangci_json(raw.lines().next().unwrap_or(""), 2);

        assert!(filtered.contains("golangci-lint: 6 issues in 1 files"));
        assert!(filtered.contains("errcheck (3x)"));
        assert!(filtered.contains("ineffassign (3x)"));
        assert!(filtered.contains("main.go"));
    }

    #[test]
    fn test_real_v2_issues_token_savings() {
        let raw = include_str!("../../../tests/fixtures/golangci_v2_issues_raw.json");
        // v2 puts the JSON on the first line and may print text after it; measure
        // against the slice the filter is actually handed, not the whole file.
        let json = raw.lines().next().unwrap_or("");
        let filtered = filter_golangci_json(json, 2);
        // golangci-lint emits its JSON on a single line, so whitespace-word counting
        // measures indentation rather than content. Use the estimator RTK bills with.
        let raw_tokens = estimate_tokens(json) as f64;
        let filtered_tokens = estimate_tokens(&filtered) as f64;
        let savings = 100.0 - (filtered_tokens / raw_tokens * 100.0);

        assert!(
            savings >= 60.0,
            "expected >=60% savings, got {:.1}%",
            savings
        );
    }

    /// The filter always has something to say about its input. Whether that is worth
    /// printing is the guard's call, not an empty return here.
    #[test]
    fn test_filter_never_silent_on_go127_load_error() {
        let stderr = include_str!("../../../tests/fixtures/golangci_v1_go127_error_stderr.txt");

        for (label, input) in [
            ("empty stdout", ""),
            ("whitespace", "   \n  "),
            ("error text on stdout", stderr),
            ("truncated json", "{\"Issues\": ["),
        ] {
            for version in [1, 2] {
                let filtered = filter_golangci_json(input, version);
                assert!(
                    !filtered.trim().is_empty(),
                    "filter went silent on {label} (v{version})"
                );
            }
        }
    }

    #[test]
    fn test_global_grammar_matches_its_table() {
        assert_takes_value_table(
            &GLOBAL_GRAMMAR,
            &[
                (
                    TokenKind::Long,
                    &[
                        "color",
                        "config",
                        "cpu-profile-path",
                        "mem-profile-path",
                        "trace-path",
                    ],
                    Some(ValueSpec::value()),
                ),
                (TokenKind::Short, &["c"], Some(ValueSpec::solo_only())),
            ],
        );
    }

    #[test]
    fn test_run_grammar_matches_its_table() {
        assert_takes_value_table(
            &RUN_GRAMMAR,
            &[
                (
                    TokenKind::Long,
                    &[
                        // Global flags, valid after `run` too.
                        "color",
                        "config",
                        "cpu-profile-path",
                        "mem-profile-path",
                        "trace-path",
                        // Output sinks.
                        "output.checkstyle.path",
                        "output.code-climate.path",
                        "output.html.path",
                        "output.json.path",
                        "output.junit-xml.path",
                        "output.sarif.path",
                        "output.tab.path",
                        "output.teamcity.path",
                        "output.text.path",
                        // `run`'s own.
                        "build-tags",
                        "concurrency",
                        "default",
                        "disable",
                        "enable",
                        "enable-only",
                        "issues-exit-code",
                        "max-issues-per-linter",
                        "max-same-issues",
                        "modules-download-mode",
                        "new-from-merge-base",
                        "new-from-patch",
                        "new-from-rev",
                        "out-format",
                        "path-mode",
                        "path-prefix",
                        "deadline",
                        "exclude",
                        "presets",
                        "skip-dirs",
                        "skip-files",
                        "timeout",
                    ],
                    Some(ValueSpec::value()),
                ),
                (
                    TokenKind::Short,
                    &["c", "D", "E", "e", "j", "p"],
                    Some(ValueSpec::solo_only()),
                ),
            ],
        );
    }
}
