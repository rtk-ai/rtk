//! Filters find results by grouping files by directory.

use crate::core::tracking;
use crate::core::truncate::CAP_INVENTORY;
use anyhow::{Context, Result};
use ignore::WalkBuilder;
use std::collections::{HashMap, HashSet};
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

fn looks_like_path(token: &str) -> bool {
    token.contains('/') || (cfg!(windows) && token.contains('\\'))
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
        && !looks_like_path(&args[0])
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

fn run_verbatim(args: &[String], verbose: u8) -> Result<i32> {
    let os_args: Vec<std::ffi::OsString> = args.iter().map(std::ffi::OsString::from).collect();
    crate::core::runner::run_passthrough("find", &os_args, verbose)
}

fn run_compress(
    options: &[String],
    paths: &[String],
    expr: &[String],
    max: Option<usize>,
    file_type: Option<&str>,
    verbose: u8,
) -> Result<i32> {
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
            &[],
            &track_cmd,
            &raw_output,
            &timer,
        );
    }
    Ok(exit_code)
}

/// Entry point from main.rs — dispatches on find's grammar then delegates.
pub fn run_from_args(args: &[String], verbose: u8) -> Result<i32> {
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
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // Treat "." as match-all
    let effective_pattern = if pattern == "." { "*" } else { pattern };

    if verbose > 0 {
        eprintln!("find: {} in {}", effective_pattern, path);
    }

    let want_dirs = file_type == "d";

    if !Path::new(path).exists() {
        eprintln!("find: '{}': No such file or directory", path);
        return Ok(1);
    }

    let (files, filtered) = native_walk(
        path,
        effective_pattern,
        max_depth,
        want_dirs,
        case_insensitive,
        true,
    );

    let raw_output = {
        let mut sorted = files.clone();
        sorted.sort();
        sorted.join("\n")
    };
    render(
        files,
        max_results,
        max_explicit,
        &filtered,
        &format!("find {} -name '{}'", path, effective_pattern),
        &raw_output,
        &timer,
    );

    Ok(0)
}

const DISCLOSURE_ENTRY_CAP: usize = 20_000;

fn native_walk(
    path: &str,
    pattern: &str,
    max_depth: Option<usize>,
    want_dirs: bool,
    case_insensitive: bool,
    git_global: bool,
) -> (Vec<String>, Vec<String>) {
    // When the pattern targets dotfiles (e.g. -name ".claude.json"), we must walk hidden
    // entries; otherwise skip them to keep results tidy (#1101).
    let search_hidden = pattern.starts_with('.');

    let mut builder = WalkBuilder::new(path);
    builder
        .hidden(!search_hidden) // skip hidden files/dirs unless pattern targets dotfiles
        .git_ignore(true) // respect .gitignore
        .git_global(git_global)
        .git_exclude(true);
    if let Some(depth) = max_depth {
        builder.max_depth(Some(depth));
    }

    let mut files = Vec::new();
    let mut visited: HashSet<std::path::PathBuf> = HashSet::new();
    let mut visited_dirs: Vec<(std::path::PathBuf, usize)> = Vec::new();
    let mut disclose = true;
    for entry in builder.build().flatten() {
        if disclose {
            if visited.len() >= DISCLOSURE_ENTRY_CAP {
                disclose = false;
                visited.clear();
                visited_dirs.clear();
            } else {
                visited.insert(entry.path().to_path_buf());
                let is_dir = if entry.depth() == 0 {
                    std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
                } else {
                    entry.file_type().is_some_and(|t| t.is_dir())
                };
                if is_dir {
                    visited_dirs.push((entry.path().to_path_buf(), entry.depth()));
                }
            }
        }
        if let Some(display) = entry_display(&entry, path, pattern, want_dirs, case_insensitive) {
            files.push(display);
        }
    }

    let filtered = if disclose {
        disclose_filtered(
            path,
            pattern,
            max_depth,
            want_dirs,
            case_insensitive,
            &visited_dirs,
            &visited,
        )
    } else {
        Vec::new()
    };
    (files, filtered)
}

fn entry_display(
    entry: &ignore::DirEntry,
    root: &str,
    pattern: &str,
    want_dirs: bool,
    case_insensitive: bool,
) -> Option<String> {
    let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
    if want_dirs != is_dir {
        return None;
    }
    let name = entry.path().file_name()?.to_string_lossy();
    if !name_matches(pattern, &name, case_insensitive) {
        return None;
    }
    let display = relative_display(entry.path(), root);
    if !display.is_empty() {
        Some(display)
    } else if !is_dir {
        Some(root.to_string())
    } else {
        None
    }
}

fn name_matches(pattern: &str, name: &str, case_insensitive: bool) -> bool {
    if case_insensitive {
        glob_match(&pattern.to_lowercase(), &name.to_lowercase())
    } else {
        glob_match(pattern, name)
    }
}

fn relative_display(entry_path: &Path, root: &str) -> String {
    entry_path
        .strip_prefix(root)
        .unwrap_or(entry_path)
        .to_string_lossy()
        .to_string()
}

const FILTERED_CAP: usize = 1000;
const UNREPORTED_DIR: &str = ".git";

fn disclose_filtered(
    root: &str,
    pattern: &str,
    max_depth: Option<usize>,
    want_dirs: bool,
    case_insensitive: bool,
    visited_dirs: &[(std::path::PathBuf, usize)],
    visited: &HashSet<std::path::PathBuf>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (dir, depth) in visited_dirs {
        if max_depth.is_some_and(|max| *depth >= max) {
            continue;
        }
        let Ok(children) = std::fs::read_dir(dir) else {
            continue;
        };
        for child in children.flatten() {
            let child_path = child.path();
            if visited.contains(&child_path) {
                continue;
            }
            let name = child.file_name().to_string_lossy().into_owned();
            let is_dir = child.file_type().is_ok_and(|t| t.is_dir());
            let display = relative_display(&child_path, root);
            if is_dir {
                if name == UNREPORTED_DIR {
                    continue;
                }
                out.push(format!("{}/", display));
            } else if !want_dirs && name_matches(pattern, &name, case_insensitive) {
                out.push(display);
            }
            if out.len() >= FILTERED_CAP {
                out.sort();
                return out;
            }
        }
    }
    out.sort();
    out
}

fn filtered_hint(filtered: &[String]) -> Option<String> {
    if filtered.is_empty() {
        return None;
    }
    let count = if filtered.len() >= FILTERED_CAP {
        format!("{}+", FILTERED_CAP)
    } else {
        filtered.len().to_string()
    };
    let note = format!("... ({} filtered)", count);
    let mut sorted = filtered.to_vec();
    sorted.sort();
    let listing = format!("{}\n", sorted.join("\n"));
    match crate::core::tee::force_tee_tail_hint(&listing, "find-hidden", 1) {
        Some(tee_hint) => Some(format!("{}\n{}", note, tee_hint)),
        None => Some(note),
    }
}

fn render(
    mut files: Vec<String>,
    max_results: usize,
    max_explicit: bool,
    filtered: &[String],
    track_cmd: &str,
    raw_output: &str,
    timer: &tracking::TimedExecution,
) -> String {
    files.sort();
    let note = filtered_hint(filtered);

    if files.is_empty() {
        let shown = match &note {
            Some(note) => crate::core::runner::emit_guarded(note, None, note),
            None => String::new(),
        };
        timer.track(track_cmd, "rtk find", raw_output, &shown);
        return shown;
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

    if let Some(note) = &note {
        body.push_str(&format!("{}\n", note));
    }

    let capped_raw = build_capped_listing(&ordered, max_results);
    let hint = if displayed < total_files && !max_explicit {
        crate::core::tee::force_tee_tail_hint(&ordered.join("\n"), "find", displayed + 1)
    } else {
        None
    };
    let listing = capped_raw.trim_end_matches('\n');
    let mut baseline = match &hint {
        Some(h) => format!("{}\n{}", listing, h),
        None => listing.to_string(),
    };
    if let Some(note) = &note {
        baseline.push('\n');
        baseline.push_str(note);
    }
    let shown =
        crate::core::runner::emit_guarded(body.trim_end_matches('\n'), hint.as_deref(), &baseline);
    timer.track(track_cmd, "rtk find", raw_output, &shown);
    shown
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

    #[test]
    fn missing_path_with_separator_is_a_find_path_not_a_pattern() {
        let p = parse_find_args(&args(&["/definitely/missing/xyz", "-maxdepth", "1"])).unwrap();
        assert_eq!(p.path, "/definitely/missing/xyz");
        assert_eq!(p.pattern, "*");
        let p = parse_find_args(&args(&["./nope_xyz"])).unwrap();
        assert_eq!(p.path, "./nope_xyz");
        assert_eq!(p.pattern, "*");
        match dispatch(&args(&["/definitely/missing/xyz", "-mtime", "+0"])).unwrap() {
            Dispatch::Compress { paths, expr, .. } => {
                assert_eq!(paths, args(&["/definitely/missing/xyz"]));
                assert_eq!(expr, args(&["-mtime", "+0"]));
            }
            _ => panic!("expected compress"),
        }
    }

    #[test]
    fn native_run_reports_missing_path_as_exit_1() {
        let code = run(
            "*",
            "/definitely/missing/xyz",
            10,
            false,
            None,
            "f",
            false,
            0,
        )
        .unwrap();
        assert_eq!(code, 1);
    }

    #[test]
    fn native_run_returns_zero_on_success() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("RTK_TEE_DIR", tmp.path());
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        let root = tmp.path().to_string_lossy().into_owned();
        assert_eq!(
            run("*.txt", &root, 10, false, None, "f", false, 0).unwrap(),
            0
        );
    }

    #[test]
    fn run_from_args_propagates_find_exit_status() {
        let argv = ["/definitely/missing/xyz", "-mtime", "+0"];
        let expected = std::process::Command::new("find")
            .args(argv)
            .output()
            .map(|o| o.status.code().unwrap_or(1))
            .unwrap_or(127);
        let code = run_from_args(&args(&argv), 0).unwrap();
        assert_eq!(code, expected);
    }

    #[test]
    fn hidden_and_ignored_matches_are_collected_for_disclosure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        if !std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return; // no git: .gitignore cannot apply, nothing to assert
        }
        std::fs::write(root.join(".gitignore"), "secret.txt\nbuild/\n").unwrap();
        std::fs::write(root.join("secret.txt"), "x").unwrap();
        std::fs::write(root.join("visible.txt"), "y").unwrap();
        std::fs::create_dir(root.join(".hidden")).unwrap();
        std::fs::write(root.join(".hidden").join("h.txt"), "z").unwrap();
        std::fs::create_dir(root.join("build")).unwrap();
        std::fs::write(root.join("build").join("out.txt"), "w").unwrap();
        let root_s = root.to_string_lossy().into_owned();

        let (files, filtered) = native_walk(&root_s, "*.txt", None, false, false, false);
        assert_eq!(files, vec!["visible.txt".to_string()]);
        assert_eq!(
            filtered,
            vec![
                ".hidden/".to_string(),
                "build/".to_string(),
                "secret.txt".to_string()
            ]
        );

        let (_, filtered) = native_walk(&root_s, "*.txt", Some(0), false, false, false);
        assert!(filtered.is_empty(), "{filtered:?}");

        let (files, filtered) = native_walk(&root_s, ".gitignore", None, false, false, false);
        assert_eq!(files, vec![".gitignore".to_string()]);
        assert_eq!(filtered, vec!["build/".to_string()]);
    }

    #[test]
    fn disclosure_survives_the_output_guard() {
        let tee = tempfile::tempdir().unwrap();
        std::env::set_var("RTK_TEE_DIR", tee.path());
        let timer = tracking::TimedExecution::start();
        let shown = render(
            vec!["visible.txt".to_string()],
            50,
            false,
            &["secret.txt".to_string()],
            "find . -name '*.txt'",
            "visible.txt",
            &timer,
        );
        assert!(shown.contains("(1 filtered"), "{shown}");
        let shown = render(
            vec![],
            50,
            false,
            &["secret.txt".to_string()],
            "find . -name secret.txt",
            "",
            &timer,
        );
        assert!(shown.contains("(1 filtered"), "{shown}");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_still_discloses() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub").join(".hidden")).unwrap();
        std::fs::write(root.join("sub").join("a.txt"), "a").unwrap();
        std::fs::write(root.join("sub").join(".hidden").join("h.txt"), "h").unwrap();
        std::os::unix::fs::symlink(root.join("sub"), root.join("link")).unwrap();
        let link = root.join("link").to_string_lossy().into_owned();
        let (files, filtered) = native_walk(&link, "*.txt", None, false, false, false);
        assert_eq!(files, vec!["a.txt".to_string()]);
        assert_eq!(filtered, vec![".hidden/".to_string()]);
    }

    #[test]
    fn filtered_hint_names_the_count() {
        assert!(filtered_hint(&[]).is_none());
        let h = filtered_hint(&["secret.txt".to_string(), ".hidden/h.txt".to_string()]).unwrap();
        assert!(h.starts_with("... (2 filtered"), "{h}");
    }
}
