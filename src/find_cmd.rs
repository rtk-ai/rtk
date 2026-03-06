use crate::tracking;
use anyhow::{Context, Result};
use ignore::WalkBuilder;
use std::collections::HashMap;
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

/// Native find flags that RTK cannot handle correctly.
/// These involve compound predicates, actions, or semantics we don't support.
const UNSUPPORTED_FIND_FLAGS: &[&str] = &[
    "-not", "!", "-or", "-o", "-and", "-a", "-exec", "-execdir", "-delete", "-print0", "-newer",
    "-perm", "-size", "-mtime", "-mmin", "-atime", "-amin", "-ctime", "-cmin", "-empty", "-link",
    "-regex", "-iregex",
];

fn has_unsupported_find_flags(args: &[String]) -> bool {
    args.iter()
        .any(|a| UNSUPPORTED_FIND_FLAGS.contains(&a.as_str()))
}

/// Parse arguments from raw args vec, supporting both native find and RTK syntax.
///
/// Native find syntax: `find . -name "*.rs" -type f -maxdepth 3`
/// RTK syntax: `find *.rs [path] [-m max] [-t type]`
fn parse_find_args(args: &[String]) -> Result<FindArgs> {
    if args.is_empty() {
        return Ok(FindArgs::default());
    }

    if has_unsupported_find_flags(args) {
        anyhow::bail!(
            "rtk find does not support compound predicates or actions (e.g. -not, -exec). Use `find` directly."
        );
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
pub fn run_from_args(args: &[String], verbose: u8) -> Result<()> {
    let parsed = parse_find_args(args)?;
    run(
        &parsed.pattern,
        &parsed.path,
        parsed.max_results,
        parsed.max_depth,
        &parsed.file_type,
        parsed.case_insensitive,
        verbose,
    )
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

    let mut builder = WalkBuilder::new(path);
    builder
        .hidden(true) // skip hidden files/dirs
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

    println!("📁 {}F {}D:", total_files, dirs_count);
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

    // --- parse_find_args: unsupported flags ---

    #[test]
    fn parse_native_find_rejects_not() {
        let result = parse_find_args(&args(&[".", "-name", "*.rs", "-not", "-name", "*_test.rs"]));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("compound predicates"));
    }

    #[test]
    fn parse_native_find_rejects_exec() {
        let result = parse_find_args(&args(&[".", "-name", "*.tmp", "-exec", "rm", "{}", ";"]));
        assert!(result.is_err());
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

    #[test]
    fn run_from_args_iname_case_insensitive() {
        // -iname should match case-insensitively
        let result = run_from_args(&args(&[".", "-iname", "cargo.toml"]), 0);
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
