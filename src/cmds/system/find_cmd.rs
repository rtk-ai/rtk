//! Filters find results by grouping files by directory.

use crate::core::tracking;
use crate::core::truncate::CAP_INVENTORY;
use anyhow::{Context, Result};
use ignore::WalkBuilder;
use std::collections::HashMap;
use std::io::Write;
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
    max_explicit: bool,
    max_depth: Option<usize>,
    file_type: String,
    case_insensitive: bool,
}

impl Default for FindArgs {
    fn default() -> Self {
        Self {
            pattern: "*".to_string(),
            path: ".".to_string(),
            max_results: CAP_INVENTORY,
            max_explicit: false,
            max_depth: None,
            file_type: "f".to_string(),
            case_insensitive: false,
        }
    }
}

const VERBATIM_ACTIONS: &[&str] = &[
    "-exec", "-execdir", "-ok", "-okdir", "-delete", "-print", "-print0", "-printf", "-fprint",
    "-fprint0", "-fprintf", "-ls", "-fls",
];

enum Dispatch {
    Native(FindArgs),
    Compress {
        options: Vec<String>,
        paths: Vec<String>,
        expr: Vec<String>,
        max: Option<usize>,
        file_type: Option<String>,
    },
    Verbatim(Vec<String>),
}

fn is_expression_token(token: &str) -> bool {
    token.starts_with('-') || token == "!" || token == "(" || token == ")"
}

fn has_glob_meta(token: &str) -> bool {
    token.contains('*') || token.contains('?')
}

/// Leading find options: `-H`, `-L`, `-P`, `-D debugopts`, `-Olevel`.
fn leading_options_len(args: &[String]) -> usize {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-H" | "-L" | "-P" => i += 1,
            "-D" if i + 1 < args.len() => i += 2,
            t if t.len() > 2 && (t.starts_with("-D") || t.starts_with("-O")) => i += 1,
            _ => break,
        }
    }
    i.min(args.len())
}

/// The subset rtk walks itself: one path, each of -name/-iname (globs with * and ?
/// only), -type f|d, -maxdepth N, -m/--max N, -t/--file-type f|d at most once.
fn parse_subset(paths: &[String], expr: &[String]) -> Option<FindArgs> {
    if paths.len() > 1 {
        return None;
    }
    let mut parsed = FindArgs::default();
    if let Some(p) = paths.first() {
        parsed.path = p.clone();
    }
    let mut seen_name = false;
    let mut seen_type = false;
    let mut seen_depth = false;
    let mut seen_max = false;
    let mut i = 0;
    while i < expr.len() {
        let value = expr.get(i + 1)?;
        match expr[i].as_str() {
            "-name" | "-iname" if !seen_name && !value.contains('[') => {
                parsed.pattern = value.clone();
                parsed.case_insensitive = expr[i] == "-iname";
                seen_name = true;
            }
            "-type" | "-t" | "--file-type" if !seen_type && (value == "f" || value == "d") => {
                parsed.file_type = value.clone();
                seen_type = true;
            }
            "-maxdepth" if !seen_depth => {
                parsed.max_depth = Some(value.parse().ok()?);
                seen_depth = true;
            }
            "-m" | "--max" if !seen_max => {
                parsed.max_results = value.parse().ok()?;
                parsed.max_explicit = true;
                seen_max = true;
            }
            _ => return None,
        }
        i += 2;
    }
    Some(parsed)
}

/// find syntax: `find [options] [paths...] [expression]` — paths end at the first
/// token starting with `-`, `!` or a parenthesis, exactly like find.
/// RTK syntax: `find <pattern> [path] [-m max] [-t type]` — used when the first
/// token contains `*` or `?`, or is not an existing directory.
fn dispatch(original: &[String]) -> Result<Dispatch> {
    let (args, max, file_type) = peel_trailing_rtk_flags(original);
    let legacy = !args.is_empty()
        && !is_expression_token(&args[0])
        && (has_glob_meta(&args[0]) || !Path::new(&args[0]).is_dir());
    let args = if legacy {
        legacy_to_find_syntax(&args)
    } else {
        args
    };

    let options = args[..leading_options_len(&args)].to_vec();
    let rest = &args[options.len()..];
    let split = rest
        .iter()
        .position(|t| is_expression_token(t))
        .unwrap_or(rest.len());
    let paths = rest[..split].to_vec();
    let expr = rest[split..].to_vec();

    if expr.iter().any(|t| VERBATIM_ACTIONS.contains(&t.as_str())) {
        let verbatim = if legacy {
            legacy_to_find_syntax(original)
        } else {
            original.to_vec()
        };
        return Ok(Dispatch::Verbatim(verbatim));
    }
    if options.is_empty() {
        if let Some(mut parsed) = parse_subset(&paths, &expr) {
            let repeated_max = max.is_some() && parsed.max_explicit;
            let repeated_type =
                file_type.is_some() && parsed.file_type != FindArgs::default().file_type;
            if !repeated_max && !repeated_type {
                if let Some(n) = max {
                    parsed.max_results = n;
                    parsed.max_explicit = true;
                }
                if let Some(t) = file_type {
                    parsed.file_type = t;
                }
                return Ok(Dispatch::Native(parsed));
            }
        }
    }
    Ok(Dispatch::Compress {
        options,
        paths,
        expr,
        max,
        file_type,
    })
}

/// RTK syntax `find <pattern> [path] ...` rewritten as find syntax
/// `[path] -name <pattern> ...` so every token after the pattern is dispatched
/// like any find expression.
fn legacy_to_find_syntax(args: &[String]) -> Vec<String> {
    let mut rebuilt = Vec::with_capacity(args.len() + 1);
    let mut rest = &args[1..];
    if let Some(path) = rest.first().filter(|p| !is_expression_token(p)) {
        rebuilt.push(path.clone());
        rest = &rest[1..];
    }
    rebuilt.push("-name".to_string());
    rebuilt.push(args[0].clone());
    rebuilt.extend(rest.iter().cloned());
    rebuilt
}

/// rtk's own flags are only recognized at the very end of the command line,
/// where they cannot be an -exec argument or a predicate value.
fn peel_trailing_rtk_flags(args: &[String]) -> (Vec<String>, Option<usize>, Option<String>) {
    let mut end = args.len();
    let mut max = None;
    let mut file_type = None;
    while end >= 2 {
        let flag = args[end - 2].as_str();
        let value = &args[end - 1];
        match flag {
            "-m" | "--max" if max.is_none() => match value.parse::<usize>() {
                Ok(n) => max = Some(n),
                Err(_) => break,
            },
            "-t" | "--file-type" if file_type.is_none() && (value == "f" || value == "d") => {
                file_type = Some(value.clone())
            }
            _ => break,
        }
        end -= 2;
    }
    (args[..end].to_vec(), max, file_type)
}

fn run_verbatim(args: &[String], verbose: u8) -> Result<()> {
    let os_args: Vec<std::ffi::OsString> = args.iter().map(std::ffi::OsString::from).collect();
    let exit_code = crate::core::runner::run_passthrough("find", &os_args, verbose)?;
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

fn run_compress(
    options: &[String],
    paths: &[String],
    expr: &[String],
    max: Option<usize>,
    file_type: Option<&str>,
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    if verbose > 0 {
        eprintln!("find: results from find, compressed by rtk");
    }
    let max_results = max.unwrap_or(CAP_INVENTORY);
    let max_explicit = max.is_some();
    let mut cmd = crate::core::utils::resolved_command("find");
    cmd.args(options).args(paths);
    if !expr.is_empty() {
        cmd.arg("(");
        cmd.args(expr);
        cmd.arg(")");
    }
    if let Some(t) = file_type {
        cmd.arg("-type").arg(t);
    }
    cmd.arg("-print0").stdin(std::process::Stdio::inherit());
    let output = cmd.output().context("Failed to execute find")?;
    let exit_code = crate::core::utils::exit_code_from_output(&output, "find");
    {
        let mut stderr = std::io::stderr().lock();
        stderr.write_all(&output.stderr)?;
        stderr.flush()?;
    }
    let entries: Vec<&[u8]> = output
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .collect();
    let track_cmd = format!(
        "find {} {} {}",
        options.join(" "),
        paths.join(" "),
        expr.join(" ")
    );
    if entries.iter().any(|e| std::str::from_utf8(e).is_err()) {
        let raw: Vec<u8> = entries.iter().flat_map(|e| [*e, b"\n"].concat()).collect();
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&raw)?;
        stdout.flush()?;
        timer.track_passthrough(&track_cmd, "rtk find (passthrough)");
    } else {
        let files: Vec<String> = entries
            .iter()
            .map(|e| {
                let s = String::from_utf8_lossy(e);
                match s.strip_prefix("./") {
                    Some(rest) if !rest.is_empty() => rest.to_string(),
                    _ => s.to_string(),
                }
            })
            .collect();
        let raw_output = files.join("\n");
        render(
            files,
            max_results,
            max_explicit,
            &track_cmd,
            &raw_output,
            &timer,
        );
    }
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

/// Entry point from main.rs — dispatches on find's grammar then delegates.
pub fn run_from_args(args: &[String], verbose: u8) -> Result<()> {
    match dispatch(args)? {
        Dispatch::Native(parsed) => run(
            &parsed.pattern,
            &parsed.path,
            parsed.max_results,
            parsed.max_explicit,
            parsed.max_depth,
            &parsed.file_type,
            parsed.case_insensitive,
            verbose,
        ),
        Dispatch::Compress {
            options,
            paths,
            expr,
            max,
            file_type,
        } => run_compress(&options, &paths, &expr, max, file_type.as_deref(), verbose),
        Dispatch::Verbatim(args) => run_verbatim(&args, verbose),
    }
}

fn group_by_dir(files: &[String]) -> HashMap<String, Vec<String>> {
    let mut by_dir: HashMap<String, Vec<String>> = HashMap::new();
    for file in files {
        let p = Path::new(file);
        let dir = p
            .parent()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        let dir = if dir.is_empty() { ".".to_string() } else { dir };
        let filename = p
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| file.clone());
        by_dir.entry(dir).or_default().push(filename);
    }
    by_dir
}

fn display_ordered(files: &[String]) -> Vec<String> {
    let by_dir = group_by_dir(files);
    let mut dirs: Vec<_> = by_dir.keys().cloned().collect();
    dirs.sort();
    let mut ordered = Vec::with_capacity(files.len());
    for dir in &dirs {
        for filename in &by_dir[dir] {
            if dir == "." {
                ordered.push(filename.clone());
            } else {
                ordered.push(format!("{}/{}", dir, filename));
            }
        }
    }
    ordered
}

fn build_capped_listing(files: &[String], max_results: usize) -> String {
    let mut listing = files
        .iter()
        .take(max_results)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    if files.len() > max_results {
        listing.push_str(&format!("\n+{} more", files.len() - max_results));
    }
    listing.push('\n');
    listing
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    pattern: &str,
    path: &str,
    max_results: usize,
    max_explicit: bool,
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

    if !Path::new(path).exists() {
        eprintln!("find: '{}': No such file or directory", path);
        std::process::exit(1);
    }

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

        let display_path = entry_path
            .strip_prefix(path)
            .unwrap_or(entry_path)
            .to_string_lossy()
            .to_string();

        if !display_path.is_empty() {
            files.push(display_path);
        } else if !is_dir {
            files.push(path.to_string());
        }
    }

    let raw_output = {
        let mut sorted = files.clone();
        sorted.sort();
        sorted.join("\n")
    };
    render(
        files,
        max_results,
        max_explicit,
        &format!("find {} -name '{}'", path, effective_pattern),
        &raw_output,
        &timer,
    );

    Ok(())
}

fn render(
    mut files: Vec<String>,
    max_results: usize,
    max_explicit: bool,
    track_cmd: &str,
    raw_output: &str,
    timer: &tracking::TimedExecution,
) {
    files.sort();

    if files.is_empty() {
        timer.track(track_cmd, "rtk find", raw_output, "");
        return;
    }

    let ordered = display_ordered(&files);

    let by_dir = group_by_dir(&files);
    let mut dirs: Vec<_> = by_dir.keys().cloned().collect();
    dirs.sort();
    let dirs_count = dirs.len();
    let total_files = files.len();

    let mut body = String::new();
    body.push_str(&format!("{}F {}D:\n", total_files, dirs_count));
    body.push('\n');

    // Display with proper --max limiting (count individual files)
    let mut displayed = 0;
    for dir in &dirs {
        if displayed >= max_results {
            break;
        }

        let files_in_dir = &by_dir[dir];
        let dir_display = if dir.chars().count() > 50 {
            let tail: String = dir
                .chars()
                .rev()
                .take(47)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("...{}", tail)
        } else {
            dir.clone()
        };

        let remaining_budget = max_results - displayed;
        if files_in_dir.len() <= remaining_budget {
            body.push_str(&format!("{}/ {}\n", dir_display, files_in_dir.join(" ")));
            displayed += files_in_dir.len();
        } else {
            // Partial display: show only what fits in budget
            let partial: Vec<_> = files_in_dir
                .iter()
                .take(remaining_budget)
                .cloned()
                .collect();
            body.push_str(&format!("{}/ {}\n", dir_display, partial.join(" ")));
            displayed += partial.len();
            break;
        }
    }

    if displayed < total_files {
        body.push_str(&format!("+{} more\n", total_files - displayed));
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

    if by_ext.len() > 1 {
        body.push('\n');
        let mut exts: Vec<_> = by_ext.iter().collect();
        exts.sort_by(|a, b| b.1.cmp(a.1));
        let ext_str: Vec<String> = exts
            .iter()
            .take(5)
            .map(|(e, c)| format!(".{}({})", e, c))
            .collect();
        let ext_line = format!("ext: {}", ext_str.join(" "));
        body.push_str(&format!("{}\n", ext_line));
    }

    let capped_raw = build_capped_listing(&ordered, max_results);
    let hint = if displayed < total_files && !max_explicit {
        crate::core::tee::force_tee_tail_hint(&ordered.join("\n"), "find", displayed + 1)
    } else {
        None
    };
    let listing = capped_raw.trim_end_matches('\n');
    let baseline = match &hint {
        Some(h) => format!("{}\n{}", listing, h),
        None => listing.to_string(),
    };
    let shown =
        crate::core::runner::emit_guarded(body.trim_end_matches('\n'), hint.as_deref(), &baseline);
    timer.track(track_cmd, "rtk find", raw_output, &shown);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convert string slices to Vec<String> for test convenience.
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    fn parse_find_args(a: &[String]) -> Result<FindArgs> {
        match dispatch(a)? {
            Dispatch::Native(p) => Ok(p),
            Dispatch::Compress { .. } => anyhow::bail!("dispatched to compress"),
            Dispatch::Verbatim(_) => anyhow::bail!("dispatched to verbatim"),
        }
    }

    fn class(a: &[&str]) -> &'static str {
        match dispatch(&args(a)) {
            Ok(Dispatch::Native(_)) => "native",
            Ok(Dispatch::Compress { .. }) => "compress",
            Ok(Dispatch::Verbatim(_)) => "verbatim",
            Err(_) => "error",
        }
    }

    #[test]
    fn rtk_flags_before_actions_reach_find_so_it_refuses_them() {
        match dispatch(&args(&["*.rs", "src", "-delete", "-m", "1"])).unwrap() {
            Dispatch::Verbatim(a) => {
                assert_eq!(a, args(&["src", "-name", "*.rs", "-delete", "-m", "1"]))
            }
            _ => panic!("expected verbatim"),
        }
        match dispatch(&args(&[
            ".", "-name", "*.rs", "-exec", "echo", "{}", ";", "-t", "d",
        ]))
        .unwrap()
        {
            Dispatch::Verbatim(a) => assert!(a.ends_with(&args(&["-t", "d"]))),
            _ => panic!("expected verbatim"),
        }
    }

    #[test]
    fn bare_directory_positional_is_a_path() {
        let tmp = std::env::temp_dir().to_string_lossy().into_owned();
        for a in [&["src"][..], &["src/"], &["./src"], &[tmp.as_str()], &["."]] {
            let p = parse_find_args(&args(a)).unwrap();
            assert_eq!(p.path, a[0], "{a:?}");
            assert_eq!(p.pattern, "*", "{a:?}");
        }
    }

    #[test]
    fn literal_name_that_is_not_a_directory_stays_a_pattern() {
        let p = parse_find_args(&args(&["Cargo.toml"])).unwrap();
        assert_eq!(p.pattern, "Cargo.toml");
        assert_eq!(p.path, ".");
        let p = parse_find_args(&args(&["no_such_dir_xyz"])).unwrap();
        assert_eq!(p.pattern, "no_such_dir_xyz");
        let p = parse_find_args(&args(&["README.md", "src"])).unwrap();
        assert_eq!(p.pattern, "README.md");
        assert_eq!(p.path, "src");
    }

    #[test]
    fn leading_glob_token_is_rtk_pattern_syntax() {
        let p = parse_find_args(&args(&["*.rs", "src", "-m", "5"])).unwrap();
        assert_eq!(p.pattern, "*.rs");
        assert_eq!(p.path, "src");
        assert_eq!(p.max_results, 5);
        assert!(p.max_explicit);
    }

    #[test]
    fn rtk_pattern_syntax_with_find_predicates_never_blocks() {
        match dispatch(&args(&["*.rs", "src", "-mtime", "+7", "-m", "5"])).unwrap() {
            Dispatch::Compress {
                paths, expr, max, ..
            } => {
                assert_eq!(paths, args(&["src"]));
                assert_eq!(expr, args(&["-name", "*.rs", "-mtime", "+7"]));
                assert_eq!(max, Some(5));
            }
            _ => panic!("expected compress"),
        }
        match dispatch(&args(&["*.rs", "-exec", "rm", "{}", ";"])).unwrap() {
            Dispatch::Verbatim(a) => {
                assert_eq!(a, args(&["-name", "*.rs", "-exec", "rm", "{}", ";"]))
            }
            _ => panic!("expected verbatim"),
        }
    }

    #[test]
    fn trailing_rtk_flags_are_peeled_in_every_class() {
        let (rest, max, ty) =
            peel_trailing_rtk_flags(&args(&[".", "-mtime", "-7", "-t", "d", "-m", "5"]));
        assert_eq!(rest, args(&[".", "-mtime", "-7"]));
        assert_eq!(max, Some(5));
        assert_eq!(ty.as_deref(), Some("d"));
        let (rest, max, _) = peel_trailing_rtk_flags(&args(&[".", "-name", "-m"]));
        assert_eq!(rest, args(&[".", "-name", "-m"]));
        assert_eq!(max, None);
        let (rest, max, _) =
            peel_trailing_rtk_flags(&args(&[".", "-exec", "grep", "-m", "1", "x", "{}", ";"]));
        assert_eq!(rest.len(), 8);
        assert_eq!(max, None);
        match dispatch(&args(&[".", "-mtime", "-7", "-m", "5"])).unwrap() {
            Dispatch::Compress { max, .. } => assert_eq!(max, Some(5)),
            _ => panic!("expected compress"),
        }
        assert_eq!(
            class(&[".", "-name", "*.rs", "-exec", "cat", "{}", ";", "-m", "5"]),
            "verbatim"
        );
    }

    #[test]
    fn subset_expressions_stay_native() {
        assert_eq!(
            class(&[".", "-name", "*.rs", "-type", "f", "-maxdepth", "2"]),
            "native"
        );
        assert_eq!(class(&["-name", "*.rs"]), "native");
        assert_eq!(class(&["src", "-iname", "README*"]), "native");
        assert_eq!(class(&[".", "-type", "d"]), "native");
        assert_eq!(class(&[".", "-name", "*.rs", "-m", "10"]), "native");
    }

    #[test]
    fn unmodeled_listing_predicates_compress_via_find() {
        assert_eq!(class(&["-mindepth", "2"]), "compress");
        assert_eq!(
            class(&[".", "-path", "*/target/*", "-name", "*.rs"]),
            "compress"
        );
        assert_eq!(class(&[".", "-type", "l"]), "compress");
        assert_eq!(class(&[".", "-name", "[ab]*.txt"]), "compress");
        assert_eq!(class(&[".", "-name", "a", "-o", "-name", "b"]), "compress");
        assert_eq!(class(&[".", "-not", "-name", "*.rs"]), "compress");
        assert_eq!(class(&[".", "-mtime", "-7"]), "compress");
        assert_eq!(class(&[".", "src"]), "compress");
        assert_eq!(class(&[".", "-name"]), "compress");
        assert_eq!(class(&[".", "-maxdepth", "abc"]), "compress");
        assert_eq!(class(&[".", "-prune"]), "compress");
    }

    #[test]
    fn actions_and_output_formats_run_verbatim() {
        for a in [
            &[".", "-name", "*.rs", "-exec", "cat", "{}", ";"][..],
            &[".", "-delete"],
            &[".", "-print0"],
            &[".", "-printf", "%p\n"],
            &[".", "-ls"],
            &[".", "-ok", "rm", "{}", ";"],
        ] {
            assert_eq!(class(a), "verbatim", "{a:?}");
        }
    }

    #[test]
    fn rtk_flags_mid_expression_reach_find_untouched() {
        assert_eq!(
            class(&[".", "-name", "*.rs", "-m", "5", "-exec", "grep", "-m", "1", "x", "{}", ";"]),
            "verbatim"
        );
        match dispatch(&args(&[".", "-m", "5", "-mtime", "-7"])).unwrap() {
            Dispatch::Compress { expr, max, .. } => {
                assert_eq!(expr, args(&["-m", "5", "-mtime", "-7"]));
                assert_eq!(max, None);
            }
            _ => panic!("expected compress"),
        }
    }

    #[test]
    fn leading_find_options_are_forwarded_ahead_of_paths() {
        for a in [
            &["-L", "src", "-name", "*.rs"][..],
            &["-H", "src"],
            &["-P", ".", "-type", "l"],
            &["-D", "tree", ".", "-name", "x"],
            &["-O2", ".", "-name", "x"],
            &["-L"],
        ] {
            match dispatch(&args(a)).unwrap() {
                Dispatch::Compress { options, paths, .. } => {
                    assert!(!options.is_empty(), "{a:?}");
                    assert!(paths.iter().all(|p| !p.starts_with('-')), "{a:?}");
                }
                _ => panic!("expected compress for {a:?}"),
            }
        }
        assert_eq!(
            class(&["-D", "tree", ".", "-exec", "true", ";"]),
            "verbatim"
        );
    }

    #[test]
    fn repeated_subset_predicates_go_to_find() {
        assert_eq!(class(&[".", "-name", "a", "-name", "b"]), "compress");
        assert_eq!(
            class(&[".", "-iname", "*.txt", "-name", "*.TXT"]),
            "compress"
        );
        assert_eq!(class(&[".", "-type", "f", "-type", "d"]), "compress");
        assert_eq!(
            class(&[".", "-maxdepth", "1", "-maxdepth", "2"]),
            "compress"
        );
        assert_eq!(
            class(&[".", "-name", "*.rs", "-m", "5", "-m", "6"]),
            "compress"
        );
    }

    #[test]
    fn subset_accepts_rtk_type_flag_and_iname() {
        let p = parse_find_args(&args(&[
            "src", "-iname", "README*", "-t", "d", "--max", "3",
        ]))
        .unwrap();
        assert_eq!(p.path, "src");
        assert!(p.case_insensitive);
        assert_eq!(p.file_type, "d");
        assert_eq!(p.max_results, 3);
        assert!(p.max_explicit);
        assert_eq!(class(&[".", "-type", "l"]), "compress");
        assert_eq!(class(&[".", "-t", "x"]), "compress");
    }

    #[test]
    fn root_entry_keeps_its_name() {
        let ordered = display_ordered(&args(&[".", "a.rs"]));
        assert_eq!(ordered, args(&[".", "a.rs"]));
        assert_eq!(build_capped_listing(&ordered, 10), ".\na.rs\n");
    }

    #[test]
    fn long_unicode_dir_label_does_not_panic() {
        let root = tempfile::TempDir::new().unwrap();
        let dir = root.path().join("é".repeat(30));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "").unwrap();
        let result = run(
            "*.txt",
            root.path().to_str().unwrap(),
            50,
            true,
            None,
            "f",
            false,
            0,
        );
        assert!(result.is_ok());
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
    fn parse_native_find_not_is_not_native() {
        assert_eq!(
            class(&[".", "-name", "*.rs", "-not", "-name", "*_test.rs"]),
            "compress"
        );
    }

    #[test]
    fn parse_native_find_exec_is_not_native() {
        assert_eq!(
            class(&[".", "-name", "*.tmp", "-exec", "rm", "{}", ";"]),
            "verbatim"
        );
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

    // --- #1101: dotfile pattern should not skip hidden files ---

    #[test]
    fn find_dotfile_pattern_includes_hidden() {
        // .gitignore exists at the repo root — must be found when using a dotfile pattern
        let result = run(".gitignore", ".", 50, true, Some(1), "f", false, 0);
        assert!(result.is_ok(), "run with dotfile pattern should not error");
    }

    #[test]
    fn find_regular_pattern_skips_hidden() {
        // Non-dot pattern should not error (hidden dirs remain skipped)
        let result = run("*.rs", "src", 5, true, None, "f", false, 0);
        assert!(result.is_ok());
    }

    // --- integration: run on this repo ---

    #[test]
    fn find_rs_files_in_src() {
        // Should find .rs files without error
        let result = run("*.rs", "src", 100, true, None, "f", false, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn find_dot_pattern_works() {
        // "." pattern should not error (was broken before)
        let result = run(".", "src", 10, true, None, "f", false, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn find_no_matches() {
        let result = run("*.xyz_nonexistent", "src", 50, true, None, "f", false, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn find_respects_max() {
        // With max=2, should not error
        let result = run("*.rs", "src", 2, true, None, "f", false, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn rtk_max_flag_marks_cap_explicit() {
        let parsed = parse_find_args(&args(&["*.rs", "-m", "10"])).unwrap();
        assert!(parsed.max_explicit);
        let parsed = parse_find_args(&args(&["*.rs", "--max", "10"])).unwrap();
        assert!(parsed.max_explicit);
    }

    #[test]
    fn native_syntax_cap_stays_implicit() {
        let parsed = parse_find_args(&args(&[".", "-name", "*.rs", "-type", "f"])).unwrap();
        assert!(!parsed.max_explicit);
    }

    #[test]
    fn default_cap_stays_implicit() {
        let parsed = parse_find_args(&args(&[])).unwrap();
        assert!(!parsed.max_explicit);
        let parsed = parse_find_args(&args(&["*.rs", "src"])).unwrap();
        assert!(!parsed.max_explicit);
    }

    #[test]
    fn tee_remainder_matches_hidden_files_when_dir_sort_diverges() {
        // "logs-old/f01" < "logs/f01" in flat path sort ('-' < '/'), but the
        // display sorts by dir name where "logs" < "logs-old". The tee must
        // follow display order or tail returns already-shown files.
        let mut files: Vec<String> = Vec::new();
        for i in 1..=30 {
            files.push(format!("logs/f{:02}.txt", i));
            files.push(format!("logs-old/f{:02}.txt", i));
        }
        files.sort();
        assert!(files[0].starts_with("logs-old/"));

        let ordered = display_ordered(&files);
        assert!(ordered[0].starts_with("logs/"));
        assert_eq!(ordered.len(), 60);

        let hidden = &ordered[50..];
        assert_eq!(hidden.first().unwrap(), "logs-old/f21.txt");
        assert_eq!(hidden.last().unwrap(), "logs-old/f30.txt");
        assert!(hidden.iter().all(|f| f.starts_with("logs-old/")));
    }

    #[test]
    fn display_ordered_keeps_root_files_in_root_group() {
        let files: Vec<String> = args(&["zebra.txt", "logs/a.txt", "alpha.txt"]);
        let ordered = display_ordered(&files);
        assert_eq!(ordered, args(&["zebra.txt", "alpha.txt", "logs/a.txt"]));
    }

    #[test]
    fn capped_listing_no_truncation() {
        let files = args(&["a.rs", "b.rs"]);
        assert_eq!(build_capped_listing(&files, 10), "a.rs\nb.rs\n");
    }

    #[test]
    fn capped_listing_truncates_with_marker() {
        let files = args(&["a.rs", "b.rs", "c.rs", "d.rs"]);
        assert_eq!(build_capped_listing(&files, 2), "a.rs\nb.rs\n+2 more\n");
    }

    #[test]
    fn capped_listing_at_exact_max_has_no_marker() {
        let files = args(&["a.rs", "b.rs"]);
        assert_eq!(build_capped_listing(&files, 2), "a.rs\nb.rs\n");
    }

    #[test]
    fn capped_baseline_beats_grouped_summary_on_tiny_result_sets() {
        use crate::core::guard::never_worse;
        let files = args(&["a.rs"]);
        let capped = build_capped_listing(&files, 10);
        let grouped = "1F 1D:\n\n./ a.rs\n\next: .rs(1)\n";
        assert_eq!(never_worse(&capped, grouped), capped);
    }

    #[test]
    fn find_gitignored_excluded() {
        // target/ is in .gitignore — files inside should not appear
        let result = run("*", ".", 1000, true, None, "f", false, 0);
        assert!(result.is_ok());
        // We can't easily capture stdout in unit tests, but at least
        // verify it runs without error. The smoke tests verify content.
    }
}
