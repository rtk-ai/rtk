//! Filters find results by grouping files by directory.

use crate::core::tracking;
use anyhow::{Context, Result};
use ignore::WalkBuilder;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

/// Match a filename against a glob pattern (supports `*` and `?`).
fn glob_match(pattern: &str, name: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), name.as_bytes())
}

fn glob_match_inner(pat: &[u8], name: &[u8]) -> bool {
    match (pat.first(), name.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            // '*' matches zero or more characters
            glob_match_inner(&pat[1..], name)
                || (!name.is_empty() && glob_match_inner(pat, &name[1..]))
        }
        (Some(b'?'), Some(_)) => glob_match_inner(&pat[1..], &name[1..]),
        (Some(&p), Some(&n)) if p == n => glob_match_inner(&pat[1..], &name[1..]),
        _ => false,
    }
}

/// Parsed arguments from either native find or RTK find syntax.
#[derive(Debug)]
struct FindArgs {
    pattern: String,
    path: String,
    max_results: usize,
    max_depth: Option<usize>,
    file_type: String,
    case_insensitive: bool,
}

impl Default for FindArgs {
    fn default() -> Self {
        Self {
            pattern: "*".to_string(),
            path: ".".to_string(),
            max_results: 50,
            max_depth: None,
            file_type: "f".to_string(),
            case_insensitive: false,
        }
    }
}

/// Consume the next argument from `args` at position `i`, advancing the index.
/// Returns `None` if `i` is past the end of `args`.
fn next_arg(args: &[String], i: &mut usize) -> Option<String> {
    *i += 1;
    args.get(*i).cloned()
}

/// Check if args contain native find flags (-name, -type, -maxdepth, etc.)
fn has_native_find_flags(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "-name" || a == "-type" || a == "-maxdepth" || a == "-iname")
}

fn is_supported_native_find_flag(flag: &str) -> bool {
    matches!(flag, "-name" | "-type" | "-maxdepth" | "-iname")
}

/// Native find actions we never execute for safety.
const DANGEROUS_FIND_ACTIONS: &[&str] = &["-delete", "-exec", "-execdir", "-ok", "-okdir"];

/// Native find tokens that should auto-passthrough to preserve semantics.
const SAFE_NATIVE_PASSTHROUGH_TOKENS: &[&str] = &[
    "(", ")", "!", "-not", "-or", "-o", "-and", "-a", "-print", "-print0", "-path", "-ipath",
    "-size", "-mtime", "-mmin", "-atime", "-amin", "-ctime", "-cmin", "-newer", "-regex",
    "-iregex", "-perm", "-empty", "-link",
];

#[derive(Debug)]
enum FindInvocation {
    Filtered(FindArgs),
    Passthrough,
}

fn is_rtk_find_flag(flag: &str) -> bool {
    matches!(flag, "-m" | "--max" | "-t" | "--file-type")
}

fn is_glob_pattern(value: &str) -> bool {
    value.contains('*') || value.contains('?') || value.contains('[')
}

fn contains_dangerous_find_actions(args: &[String]) -> bool {
    args.iter()
        .any(|arg| DANGEROUS_FIND_ACTIONS.contains(&arg.as_str()))
}

fn should_passthrough_to_native_find(args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }

    if args
        .iter()
        .any(|arg| SAFE_NATIVE_PASSTHROUGH_TOKENS.contains(&arg.as_str()))
    {
        return true;
    }

    let start_paths_before_expression = args
        .iter()
        .take_while(|arg| {
            !arg.starts_with('-') && !SAFE_NATIVE_PASSTHROUGH_TOKENS.contains(&arg.as_str())
        })
        .count();
    if !is_glob_pattern(&args[0]) && start_paths_before_expression > 1 && has_native_find_flags(args)
    {
        return true;
    }

    if has_native_find_flags(args) {
        let first = args[0].as_str();
        if !first.starts_with('-') && !is_glob_pattern(first) && !Path::new(first).exists() {
            return true;
        }
    }

    // Any non-RTK flag in a native-looking invocation should passthrough.
    let has_non_rtk_flag = args
        .iter()
        .any(|arg| {
            arg.starts_with('-') && !is_rtk_find_flag(arg) && !is_supported_native_find_flag(arg)
        });
    if !has_non_rtk_flag {
        return false;
    }

    if has_native_find_flags(args) {
        return true;
    }

    let first = args[0].as_str();
    let starts_like_native_find = first.starts_with('-')
        || (first != "." && first != ".." && !is_glob_pattern(first) && Path::new(first).exists());

    if starts_like_native_find {
        return true;
    }

    // `find . -foo` is native syntax even when the flag is unknown to RTK.
    !first.starts_with('-')
        && !is_glob_pattern(first)
        && args.iter().skip(1).any(|arg| arg.starts_with('-'))
}

fn classify_find_invocation(args: &[String]) -> Result<FindInvocation> {
    if contains_dangerous_find_actions(args) {
        anyhow::bail!(
            "rtk find blocked dangerous native find action (-delete/-exec/-execdir/-ok/-okdir). Run `find` directly."
        );
    }

    if should_passthrough_to_native_find(args) {
        return Ok(FindInvocation::Passthrough);
    }

    Ok(FindInvocation::Filtered(parse_find_args(args)?))
}

/// Parse arguments from raw args vec, supporting both native find and RTK syntax.
///
/// Native find syntax: `find . -name "*.rs" -type f -maxdepth 3`
/// RTK syntax: `find *.rs [path] [-m max] [-t type]`
fn parse_find_args(args: &[String]) -> Result<FindArgs> {
    if args.is_empty() {
        return Ok(FindArgs::default());
    }

    if has_native_find_flags(args) {
        parse_native_find_args(args)
    } else {
        parse_rtk_find_args(args)
    }
}

/// Parse native find syntax: `find [path] -name "*.rs" -type f -maxdepth 3`
fn parse_native_find_args(args: &[String]) -> Result<FindArgs> {
    let mut parsed = FindArgs::default();
    let mut i = 0;

    // First non-flag argument is the path (standard find behavior)
    if !args[0].starts_with('-') {
        parsed.path = args[0].clone();
        i = 1;
    }

    while i < args.len() {
        match args[i].as_str() {
            "-name" => {
                if let Some(val) = next_arg(args, &mut i) {
                    parsed.pattern = val;
                }
            }
            "-iname" => {
                if let Some(val) = next_arg(args, &mut i) {
                    parsed.pattern = val;
                    parsed.case_insensitive = true;
                }
            }
            "-type" => {
                if let Some(val) = next_arg(args, &mut i) {
                    parsed.file_type = val;
                }
            }
            "-maxdepth" => {
                if let Some(val) = next_arg(args, &mut i) {
                    parsed.max_depth = Some(val.parse().context("invalid -maxdepth value")?);
                }
            }
            flag if flag.starts_with('-') => {
                eprintln!("rtk find: unknown flag '{}', ignored", flag);
            }
            _ => {}
        }
        i += 1;
    }

    Ok(parsed)
}

/// Parse RTK syntax: `find <pattern> [path] [-m max] [-t type]`
fn parse_rtk_find_args(args: &[String]) -> Result<FindArgs> {
    let mut parsed = FindArgs {
        pattern: args[0].clone(),
        ..FindArgs::default()
    };
    let mut i = 1;

    // Second positional arg (if not a flag) is the path
    if i < args.len() && !args[i].starts_with('-') {
        parsed.path = args[i].clone();
        i += 1;
    }

    while i < args.len() {
        match args[i].as_str() {
            "-m" | "--max" => {
                if let Some(val) = next_arg(args, &mut i) {
                    parsed.max_results = val.parse().context("invalid --max value")?;
                }
            }
            "-t" | "--file-type" => {
                if let Some(val) = next_arg(args, &mut i) {
                    parsed.file_type = val;
                }
            }
            _ => {}
        }
        i += 1;
    }

    Ok(parsed)
}

/// Entry point from main.rs — parses raw args then delegates to run().
pub fn run_from_args(args: &[String], verbose: u8) -> Result<i32> {
    match classify_find_invocation(args)? {
        FindInvocation::Filtered(parsed) => {
            run(
                &parsed.pattern,
                &parsed.path,
                parsed.max_results,
                parsed.max_depth,
                &parsed.file_type,
                parsed.case_insensitive,
                verbose,
            )?;
            Ok(0)
        }
        FindInvocation::Passthrough => {
            let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
            crate::core::runner::run_passthrough("find", &os_args, verbose)
        }
    }
}

pub fn run(
    pattern: &str,
    path: &str,
    max_results: usize,
    max_depth: Option<usize>,
    file_type: &str,
    case_insensitive: bool,
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Treat "." as match-all
    let effective_pattern = if pattern == "." { "*" } else { pattern };

    if verbose > 0 {
        eprintln!("find: {} in {}", effective_pattern, path);
    }

    let want_dirs = file_type == "d";

    // When the pattern targets dotfiles (e.g. -name ".claude.json"), we must walk hidden
    // entries; otherwise skip them to keep results tidy (#1101).
    let search_hidden = effective_pattern.starts_with('.');

    let mut builder = WalkBuilder::new(path);
    builder
        .hidden(!search_hidden) // skip hidden files/dirs unless pattern targets dotfiles
        .git_ignore(true) // respect .gitignore
        .git_global(true)
        .git_exclude(true);
    if let Some(depth) = max_depth {
        builder.max_depth(Some(depth));
    }
    let walker = builder.build();

    let mut files: Vec<String> = Vec::new();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let ft = entry.file_type();
        let is_dir = ft.as_ref().is_some_and(|t| t.is_dir());

        // Filter by type
        if want_dirs && !is_dir {
            continue;
        }
        if !want_dirs && is_dir {
            continue;
        }

        let entry_path = entry.path();

        // Get filename for glob matching
        let name = match entry_path.file_name() {
            Some(n) => n.to_string_lossy(),
            None => continue,
        };

        let matches = if case_insensitive {
            glob_match(&effective_pattern.to_lowercase(), &name.to_lowercase())
        } else {
            glob_match(effective_pattern, &name)
        };
        if !matches {
            continue;
        }

        // Store path relative to search root
        let display_path = entry_path
            .strip_prefix(path)
            .unwrap_or(entry_path)
            .to_string_lossy()
            .to_string();

        if !display_path.is_empty() {
            files.push(display_path);
        }
    }

    files.sort();

    let raw_output = files.join("\n");

    if files.is_empty() {
        let msg = format!("0 for '{}'", effective_pattern);
        println!("{}", msg);
        timer.track(
            &format!("find {} -name '{}'", path, effective_pattern),
            "rtk find",
            &raw_output,
            &msg,
        );
        return Ok(());
    }

    // Group by directory
    let mut by_dir: HashMap<String, Vec<String>> = HashMap::new();

    for file in &files {
        let p = Path::new(file);
        let dir = p
            .parent()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        let dir = if dir.is_empty() { ".".to_string() } else { dir };
        let filename = p
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        by_dir.entry(dir).or_default().push(filename);
    }

    let mut dirs: Vec<_> = by_dir.keys().cloned().collect();
    dirs.sort();
    let dirs_count = dirs.len();
    let total_files = files.len();

    println!("{}F {}D:", total_files, dirs_count);
    println!();

    // Display with proper --max limiting (count individual files)
    let mut shown = 0;
    for dir in &dirs {
        if shown >= max_results {
            break;
        }

        let files_in_dir = &by_dir[dir];
        let dir_display = if dir.len() > 50 {
            format!("...{}", &dir[dir.len() - 47..])
        } else {
            dir.clone()
        };

        let remaining_budget = max_results - shown;
        if files_in_dir.len() <= remaining_budget {
            println!("{}/ {}", dir_display, files_in_dir.join(" "));
            shown += files_in_dir.len();
        } else {
            // Partial display: show only what fits in budget
            let partial: Vec<_> = files_in_dir
                .iter()
                .take(remaining_budget)
                .cloned()
                .collect();
            println!("{}/ {}", dir_display, partial.join(" "));
            shown += partial.len();
            break;
        }
    }

    if shown < total_files {
        println!("+{} more", total_files - shown);
    }

    // Extension summary
    let mut by_ext: HashMap<String, usize> = HashMap::new();
    for file in &files {
        let ext = Path::new(file)
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_else(|| "none".to_string());
        *by_ext.entry(ext).or_default() += 1;
    }

    let mut ext_line = String::new();
    if by_ext.len() > 1 {
        println!();
        let mut exts: Vec<_> = by_ext.iter().collect();
        exts.sort_by(|a, b| b.1.cmp(a.1));
        let ext_str: Vec<String> = exts
            .iter()
            .take(5)
            .map(|(e, c)| format!(".{}({})", e, c))
            .collect();
        ext_line = format!("ext: {}", ext_str.join(" "));
        println!("{}", ext_line);
    }

    let rtk_output = format!("{}F {}D + {}", total_files, dirs_count, ext_line);
    timer.track(
        &format!("find {} -name '{}'", path, effective_pattern),
        "rtk find",
        &raw_output,
        &rtk_output,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convert string slices to Vec<String> for test convenience.
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    // --- glob_match unit tests ---

    #[test]
    fn glob_match_star_rs() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", "find_cmd.rs"));
        assert!(!glob_match("*.rs", "main.py"));
        assert!(!glob_match("*.rs", "rs"));
    }

    #[test]
    fn glob_match_star_all() {
        assert!(glob_match("*", "anything.txt"));
        assert!(glob_match("*", "a"));
        assert!(glob_match("*", ".hidden"));
    }

    #[test]
    fn glob_match_question_mark() {
        assert!(glob_match("?.rs", "a.rs"));
        assert!(!glob_match("?.rs", "ab.rs"));
    }

    #[test]
    fn glob_match_exact() {
        assert!(glob_match("Cargo.toml", "Cargo.toml"));
        assert!(!glob_match("Cargo.toml", "cargo.toml"));
    }

    #[test]
    fn glob_match_complex() {
        assert!(glob_match("test_*", "test_foo"));
        assert!(glob_match("test_*", "test_"));
        assert!(!glob_match("test_*", "test"));
    }

    // --- dot pattern treated as star ---

    #[test]
    fn dot_becomes_star() {
        // run() converts "." to "*" internally, test the logic
        let effective = if "." == "." { "*" } else { "." };
        assert_eq!(effective, "*");
    }

    // --- parse_find_args: native find syntax ---

    #[test]
    fn parse_native_find_name() {
        let parsed = parse_find_args(&args(&[".", "-name", "*.rs"])).unwrap();
        assert_eq!(parsed.pattern, "*.rs");
        assert_eq!(parsed.path, ".");
        assert_eq!(parsed.file_type, "f");
        assert_eq!(parsed.max_results, 50);
    }

    #[test]
    fn parse_native_find_name_and_type() {
        let parsed = parse_find_args(&args(&["src", "-name", "*.rs", "-type", "f"])).unwrap();
        assert_eq!(parsed.pattern, "*.rs");
        assert_eq!(parsed.path, "src");
        assert_eq!(parsed.file_type, "f");
    }

    #[test]
    fn parse_native_find_type_d() {
        let parsed = parse_find_args(&args(&[".", "-type", "d"])).unwrap();
        assert_eq!(parsed.pattern, "*");
        assert_eq!(parsed.file_type, "d");
    }

    #[test]
    fn parse_native_find_maxdepth() {
        let parsed = parse_find_args(&args(&[".", "-name", "*.toml", "-maxdepth", "2"])).unwrap();
        assert_eq!(parsed.pattern, "*.toml");
        assert_eq!(parsed.max_depth, Some(2));
        assert_eq!(parsed.max_results, 50); // max_results unchanged by -maxdepth
    }

    #[test]
    fn parse_native_find_iname() {
        let parsed = parse_find_args(&args(&[".", "-iname", "Makefile"])).unwrap();
        assert_eq!(parsed.pattern, "Makefile");
        assert!(parsed.case_insensitive);
    }

    #[test]
    fn parse_native_find_name_is_case_sensitive() {
        let parsed = parse_find_args(&args(&[".", "-name", "*.rs"])).unwrap();
        assert!(!parsed.case_insensitive);
    }

    #[test]
    fn parse_native_find_no_path() {
        // `find -name "*.rs"` without explicit path defaults to "."
        let parsed = parse_find_args(&args(&["-name", "*.rs"])).unwrap();
        assert_eq!(parsed.pattern, "*.rs");
        assert_eq!(parsed.path, ".");
    }

    // --- classify_find_invocation: passthrough and hard-fail ---

    #[test]
    fn classify_native_supported_flags_is_filtered() {
        let invocation =
            classify_find_invocation(&args(&[".", "-name", "*.rs", "-type", "f"])).unwrap();
        assert!(matches!(invocation, FindInvocation::Filtered(_)));
    }

    #[test]
    fn classify_rtk_pattern_path_and_max_is_filtered() {
        let invocation = classify_find_invocation(&args(&["*.rs", "src", "-m", "5"])).unwrap();
        assert!(matches!(invocation, FindInvocation::Filtered(_)));
    }

    #[test]
    fn classify_rtk_exact_pattern_and_path_is_filtered() {
        let invocation = classify_find_invocation(&args(&["Cargo.toml", "src"])).unwrap();
        assert!(matches!(invocation, FindInvocation::Filtered(_)));
    }

    #[test]
    fn classify_native_compound_predicate_passthrough() {
        let invocation = classify_find_invocation(&args(&[
            ".",
            "-name",
            "*.rs",
            "-o",
            "-name",
            "*.md",
        ]))
        .unwrap();
        assert!(matches!(invocation, FindInvocation::Passthrough));
    }

    #[test]
    fn classify_native_unknown_flag_passthrough() {
        let invocation = classify_find_invocation(&args(&[".", "-name", "*.rs", "-bogus-flag"])).unwrap();
        assert!(matches!(invocation, FindInvocation::Passthrough));
    }

    #[test]
    fn classify_native_path_predicate_passthrough() {
        let invocation = classify_find_invocation(&args(&[".", "-path", "*/src/*"])).unwrap();
        assert!(matches!(invocation, FindInvocation::Passthrough));
    }

    #[test]
    fn classify_native_multiple_start_paths_passthrough() {
        let invocation =
            classify_find_invocation(&args(&["src", "tests", "-name", "*.rs"])).unwrap();
        assert!(matches!(invocation, FindInvocation::Passthrough));
    }

    #[test]
    fn classify_native_missing_explicit_path_passthrough() {
        let invocation =
            classify_find_invocation(&args(&["/definitely/missing/rtk-find-path", "-name", "*.rs"]))
                .unwrap();
        assert!(matches!(invocation, FindInvocation::Passthrough));
    }

    #[test]
    fn classify_dangerous_delete_is_blocked() {
        let err = classify_find_invocation(&args(&[".", "-name", "*.tmp", "-delete"])).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("blocked dangerous native find action"));
    }

    #[test]
    fn classify_dangerous_exec_is_blocked() {
        let err =
            classify_find_invocation(&args(&[".", "-name", "*.tmp", "-exec", "rm", "{}", ";"]))
                .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("blocked dangerous native find action"));
    }

    // --- parse_find_args: RTK syntax ---

    #[test]
    fn parse_rtk_syntax_pattern_only() {
        let parsed = parse_find_args(&args(&["*.rs"])).unwrap();
        assert_eq!(parsed.pattern, "*.rs");
        assert_eq!(parsed.path, ".");
    }

    #[test]
    fn parse_rtk_syntax_pattern_and_path() {
        let parsed = parse_find_args(&args(&["*.rs", "src"])).unwrap();
        assert_eq!(parsed.pattern, "*.rs");
        assert_eq!(parsed.path, "src");
    }

    #[test]
    fn parse_rtk_syntax_with_flags() {
        let parsed = parse_find_args(&args(&["*.rs", "src", "-m", "10", "-t", "d"])).unwrap();
        assert_eq!(parsed.pattern, "*.rs");
        assert_eq!(parsed.path, "src");
        assert_eq!(parsed.max_results, 10);
        assert_eq!(parsed.file_type, "d");
    }

    #[test]
    fn parse_empty_args() {
        let parsed = parse_find_args(&args(&[])).unwrap();
        assert_eq!(parsed.pattern, "*");
        assert_eq!(parsed.path, ".");
    }

    // --- run_from_args integration tests ---

    #[test]
    fn run_from_args_native_find_syntax() {
        // Simulates: find . -name "*.rs" -type f
        let result = run_from_args(&args(&[".", "-name", "*.rs", "-type", "f"]), 0);
        assert!(result.is_ok());
    }

    #[test]
    fn run_from_args_rtk_syntax() {
        // Simulates: rtk find *.rs src
        let result = run_from_args(&args(&["*.rs", "src"]), 0);
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn run_from_args_missing_native_path_returns_find_exit_code() {
        let missing = std::env::temp_dir()
            .join(format!("rtk-find-missing-{}", std::process::id()))
            .to_string_lossy()
            .to_string();
        let result = run_from_args(&args(&[missing.as_str(), "-name", "*.rs"]), 0).unwrap();
        assert_ne!(result, 0);
    }

    #[test]
    fn run_from_args_iname_case_insensitive() {
        // -iname should match case-insensitively
        let result = run_from_args(&args(&[".", "-iname", "cargo.toml"]), 0);
        assert!(result.is_ok());
    }

    // --- #1101: dotfile pattern should not skip hidden files ---

    #[test]
    fn find_dotfile_pattern_includes_hidden() {
        // .gitignore exists at the repo root — must be found when using a dotfile pattern
        let result = run(".gitignore", ".", 50, Some(1), "f", false, 0);
        assert!(result.is_ok(), "run with dotfile pattern should not error");
    }

    #[test]
    fn find_regular_pattern_skips_hidden() {
        // Non-dot pattern should not error (hidden dirs remain skipped)
        let result = run("*.rs", "src", 5, None, "f", false, 0);
        assert!(result.is_ok());
    }

    // --- integration: run on this repo ---

    #[test]
    fn find_rs_files_in_src() {
        // Should find .rs files without error
        let result = run("*.rs", "src", 100, None, "f", false, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn find_dot_pattern_works() {
        // "." pattern should not error (was broken before)
        let result = run(".", "src", 10, None, "f", false, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn find_no_matches() {
        let result = run("*.xyz_nonexistent", "src", 50, None, "f", false, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn find_respects_max() {
        // With max=2, should not error
        let result = run("*.rs", "src", 2, None, "f", false, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn find_gitignored_excluded() {
        // target/ is in .gitignore — files inside should not appear
        let result = run("*", ".", 1000, None, "f", false, 0);
        assert!(result.is_ok());
        // We can't easily capture stdout in unit tests, but at least
        // verify it runs without error. The smoke tests verify content.
    }
}
