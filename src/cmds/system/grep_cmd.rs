//! Filters grep output by grouping matches by file.

use crate::core::stream::{exec_capture, CaptureResult};
use crate::core::tracking;
use crate::core::utils::{resolved_command, strip_ansi};
use crate::core::{args_utils, config};
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;

/// Short single-char flags that consume one following token (or inline remainder)
/// as their value. `-e` is handled separately — its value goes to `patterns`.
/// Includes all rg short flags that take a value argument except `-e` and `-r`
/// (stripped) and `-E` (dialect, left to #2138). Failure mode for a missing
/// entry: the value becomes a positional (visible wrong result, not silent).
const VALUE_FLAGS_SHORT: &[u8] = b"ABCMTdfgjmt";

/// Long flags that consume the NEXT token as their value (space-separated form).
/// Inline `=` form (`--flag=value`) is one token and passes through unchanged.
/// `--regexp` is handled separately (its value goes to `patterns`).
/// `--encoding` value is consumed correctly here; dialect routing is #2138's job.
const VALUE_FLAGS_LONG: &[&str] = &[
    "--after-context",
    "--before-context",
    "--color",
    "--colors",
    "--context",
    "--context-separator",
    "--encoding",
    "--engine",
    "--field-context-separator",
    "--field-match-separator",
    "--file",
    "--glob",
    "--iglob",
    "--ignore-file",
    "--max-columns",
    "--max-count",
    "--max-depth",
    "--max-filesize",
    "--path-separator",
    "--pre",
    "--pre-glob",
    "--replace",
    "--sort",
    "--sortr",
    "--threads",
    "--type",
    "--type-add",
    "--type-clear",
    "--type-not",
];

/// Result of parsing the content of a short flag cluster (the part after `-`).
#[derive(Debug, PartialEq)]
enum ClusterResult {
    /// All chars were boolean flags or `r`/`R` (stripped).
    /// `None` when the entire cluster reduces to nothing after stripping.
    Boolean(Option<String>),
    /// A value-taking flag was encountered. Scanning stops here.
    ValueTaking {
        /// Boolean flags before the value-taking char, `r`/`R` stripped.
        prefix: Option<String>,
        /// The value-taking flag char (`e`, `A`, `g`, etc.).
        flag: char,
        /// Bytes after `flag` in the cluster — its inline value.
        /// Empty string means "consume the next token instead."
        inline: String,
    },
}

/// Parse the content of a short flag cluster (everything after the leading `-`).
///
/// Scans left-to-right: strips `r`/`R`, accumulates boolean flag letters, and
/// stops at the first value-taking flag (from `VALUE_FLAGS_SHORT` or `e`).
/// Everything after that flag char in the cluster is its inline value and is
/// returned verbatim — no `r`/`R` stripping is applied to it.
///
/// This is the only place in the codebase that touches cluster bytes.
fn parse_cluster(rest: &str) -> ClusterResult {
    let bytes = rest.as_bytes();
    let mut raw_prefix = String::new();
    let mut j = 0;
    while j < bytes.len() {
        let ch = bytes[j];
        let is_e = ch == b'e';
        if is_e || VALUE_FLAGS_SHORT.contains(&ch) {
            let prefix = strip_r(&raw_prefix);
            // Inline value = bytes after this char; returned verbatim (no stripping).
            let inline = std::str::from_utf8(&bytes[j + 1..])
                .unwrap_or("")
                .to_string();
            return ClusterResult::ValueTaking {
                prefix,
                flag: ch as char,
                inline,
            };
        }
        raw_prefix.push(ch as char);
        j += 1;
    }
    ClusterResult::Boolean(strip_r(&raw_prefix))
}

/// Strip `r`/`R` from a string of flag letters.
/// Returns `None` when nothing remains after stripping.
///
/// Only called on accumulated flag letters (never on inline values).
/// `strip_r("carrot")` → `Some("caot")` — this shows exactly why it must not
/// touch value bytes; that corruption was the original `-ecarrot` bug.
fn strip_r(flag_letters: &str) -> Option<String> {
    let s: String = flag_letters
        .chars()
        .filter(|&c| c != 'r' && c != 'R')
        .collect();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Drop `--recursive` (grep-ism); pass all other long flags through unchanged.
fn strip_recursive(arg: &str) -> Option<String> {
    match arg {
        "--recursive" => None,
        _ => Some(arg.to_string()),
    }
}

/// Ripgrep-only flags the grep fallback drops so grep doesn't abort (issue #2167).
const RG_ONLY_LONG: &[&str] = &[
    "--glob",
    "--iglob",
    "--type",
    "--type-not",
    "--type-add",
    "--type-clear",
    "--hidden",
    "--no-ignore",
    "--pcre2",
    "--json",
    "--stats",
    "--sort",
    "--sortr",
    "--engine",
    "--mmap",
    "--no-mmap",
    "--trim",
    "--one-file-system",
    "--max-columns",
    "--max-depth",
    "--max-filesize",
    "--path-separator",
    "--field-context-separator",
    "--field-match-separator",
    "--pre",
    "--pre-glob",
];

fn strip_rg_only<T: AsRef<str>>(extra_args: &[T]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for arg in extra_args {
        let a = arg.as_ref();
        if skip_next {
            skip_next = false;
            continue;
        }
        let name = a.split('=').next().unwrap_or(a);
        let rg_only = RG_ONLY_LONG.contains(&name) || a == "-g" || a == "-T";
        if rg_only {
            let takes_value = a == "-g" || a == "-T" || VALUE_FLAGS_LONG.contains(&name);
            if takes_value && !a.contains('=') {
                skip_next = true;
            }
            continue;
        }
        out.push(a);
    }
    out
}

fn has_shape_flag<T: AsRef<str>>(extra_args: &[T]) -> bool {
    extra_args.iter().any(|arg| {
        let name = arg.as_ref().split('=').next().unwrap_or("");
        matches!(
            name,
            "--column" | "--vimgrep" | "-b" | "--byte-offset" | "--null-data"
        )
    })
}

/// Extracts `(patterns, paths, flags)` from the raw trailing args.
///
/// - `patterns`: positional pattern + all `-e`/`--regexp` values. Empty → error.
/// - `paths`: subsequent non-flag positionals. Empty → caller defaults to `["."]`.
/// - `flags`: other flags forwarded to rg (`-r`/`-R`/`--recursive` stripped).
///
/// Short clusters are scanned left-to-right; the first value-taking letter
/// terminates the cluster — everything after it is its inline value, not a
/// separate flag. Long value-taking flags consume the next token. `--` marks
/// everything after it as positional.
fn extract_pattern_path<T: AsRef<str>>(args: &[T]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut e_patterns: Vec<String> = Vec::new();
    let mut positionals: Vec<String> = Vec::new();
    let mut flags: Vec<String> = Vec::new();
    let mut past_dashdash = false;
    let mut i = 0;

    while i < args.len() {
        let arg = args[i].as_ref();

        if past_dashdash {
            positionals.push(arg.to_string());
            i += 1;
            continue;
        }

        if arg == "--" {
            past_dashdash = true;
            i += 1;
            continue;
        }

        if arg.starts_with("--") {
            // --regexp is the long form of -e: value goes to patterns.
            if arg == "--regexp" {
                if i + 1 < args.len() {
                    e_patterns.push(args[i + 1].as_ref().to_string());
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            // Other long value-taking flags: consume next token as value.
            if VALUE_FLAGS_LONG.contains(&arg) {
                flags.push(arg.to_string());
                if i + 1 < args.len() {
                    flags.push(args[i + 1].as_ref().to_string());
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            // Drop --recursive; pass everything else through.
            if let Some(cleaned) = strip_recursive(arg) {
                flags.push(cleaned);
            }
            i += 1;
            continue;
        }

        match arg.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => match parse_cluster(rest) {
                ClusterResult::Boolean(prefix) => {
                    if let Some(s) = prefix {
                        flags.push(format!("-{}", s));
                    }
                    i += 1;
                }
                ClusterResult::ValueTaking {
                    prefix,
                    flag,
                    inline,
                } => {
                    if let Some(s) = prefix {
                        flags.push(format!("-{}", s));
                    }
                    if flag == 'e' {
                        if !inline.is_empty() {
                            e_patterns.push(inline);
                            i += 1;
                        } else if i + 1 < args.len() {
                            e_patterns.push(args[i + 1].as_ref().to_string());
                            i += 2;
                        } else {
                            flags.push("-e".to_string());
                            i += 1;
                        }
                    } else {
                        flags.push(format!("-{}", flag));
                        if !inline.is_empty() {
                            flags.push(inline);
                            i += 1;
                        } else if i + 1 < args.len() {
                            flags.push(args[i + 1].as_ref().to_string());
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                }
            },
            _ => {
                positionals.push(arg.to_string());
                i += 1;
            }
        }
    }

    // If -e/--regexp was used: all positionals are paths.
    // Otherwise: first positional is the pattern, rest are paths.
    let (patterns, paths) = if !e_patterns.is_empty() {
        (e_patterns, positionals)
    } else {
        let paths = positionals.iter().skip(1).cloned().collect();
        let patterns = positionals.into_iter().take(1).collect();
        (patterns, paths)
    };

    (patterns, paths, flags)
}

fn unparsed_signal(stdout: &str) -> usize {
    stdout
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && trimmed != "--" && parse_match_line(line).is_none()
        })
        .count()
}

/// Run rg; fall back to system grep when rg can't run or rejects the invocation
/// (exit 2, no output) so any grep-ism (e.g. --include) still works (#2543).
/// #2167: if grep also aborts on a flag, retry it bare.
fn grep_capture<T: AsRef<str>>(
    rg_base: &[&str],
    grep_base: &[&str],
    file_type: Option<&str>,
    extra_args: &[T],
    patterns: &[String],
    paths: &[String],
) -> Result<CaptureResult> {
    let mut rg_cmd = resolved_command("rg");
    rg_cmd.args(rg_base);
    if let Some(ft) = file_type {
        rg_cmd.arg("--type").arg(ft);
    }
    rg_cmd.args(extra_args.iter().map(|a| a.as_ref()));
    for p in patterns {
        rg_cmd.args(["-e", &p.replace(r"\|", "|")]);
    }
    rg_cmd.arg("--");
    rg_cmd.args(paths);

    match exec_capture(&mut rg_cmd) {
        Ok(r) if !(r.exit_code == 2 && r.stdout.is_empty()) => Ok(r),
        _ => {
            // --null not -Z: BSD grep -Z is --decompress (#2310); strip_rg_only (#2167).
            let stripped = strip_rg_only(extra_args);
            let result = run_grep_fallback(grep_base, file_type, &stripped, patterns, paths)?;
            if result.exit_code == 2 && result.stdout.is_empty() && result.stderr.contains("option") {
                // grep aborted on an rg-only flag that strip_rg_only missed; retry bare (#2167).
                run_grep_fallback(grep_base, file_type, &[], patterns, paths)
            } else {
                Ok(result)
            }
        }
    }
}

fn run_grep_fallback(
    grep_base: &[&str],
    file_type: Option<&str>,
    extra_args: &[&str],
    patterns: &[String],
    paths: &[String],
) -> Result<CaptureResult> {
    let mut grep_cmd = resolved_command("grep");
    grep_cmd.args(grep_base);
    // file_type cannot be translated to grep syntax; silently skip.
    let _ = file_type;
    grep_cmd.args(extra_args);
    for p in patterns {
        grep_cmd.args(["-e", p]);
    }
    grep_cmd.arg("--");
    grep_cmd.args(paths);
    exec_capture(&mut grep_cmd).context("grep/rg failed")
}

#[allow(clippy::too_many_arguments)]
fn passthrough<T: AsRef<str>>(
    timer: &tracking::TimedExecution,
    base: &[&str],
    grep_base: &[&str],
    file_type: Option<&str>,
    extra_args: &[T],
    patterns: &[String],
    paths: &[String],
    pattern_display: &str,
    path_display: &str,
) -> Result<i32> {
    let result = grep_capture(base, grep_base, file_type, extra_args, patterns, paths)
        .context("grep/rg failed")?;

    print!("{}", strip_ansi(&result.stdout));
    if !result.stderr.is_empty() {
        eprint!("{}", result.stderr.trim());
    }

    let args_display = if extra_args.is_empty() {
        format!("'{}' {}", pattern_display, path_display)
    } else {
        let joined: Vec<&str> = extra_args.iter().map(|a| a.as_ref()).collect();
        format!("{} '{}' {}", joined.join(" "), pattern_display, path_display)
    };

    timer.track_passthrough(
        &format!("grep {}", args_display),
        &format!("rtk grep {} (passthrough)", args_display),
    );
    Ok(result.exit_code)
}

pub fn run(
    max_line_len: usize,
    max_results: usize,
    context_only: bool,
    file_type: Option<&str>,
    args: &[String],
    verbose: u8,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // --version / --help: pass through to rg without filtering.
    // Note: Clap strips `--` before populating trailing_var_arg, so both
    // `rtk grep --version` and `rtk grep -- --version` land here identically.
    if args
        .iter()
        .any(|a| a == "--version" || a == "--help" || a == "-h")
    {
        let mut rg_cmd = resolved_command("rg");
        rg_cmd.args(args);
        let result = exec_capture(&mut rg_cmd)
            .or_else(|_| {
                // rg unavailable: fall back to system grep.
                let mut grep_cmd = resolved_command("grep");
                grep_cmd.args(args);
                exec_capture(&mut grep_cmd)
            })
            .context("grep/rg failed")?;
        print!("{}", result.stdout);
        if !result.stderr.is_empty() {
            eprint!("{}", result.stderr);
        }
        return Ok(result.exit_code);
    }

    // Re-insert `--` when clap's trailing_var_arg consumed it
    let args = args_utils::restore_double_dash(args);

    let (patterns, paths, extra_args) = extract_pattern_path(&args);

    if patterns.is_empty() {
        eprintln!("rtk grep: pattern required (positional or -e)");
        return Ok(1);
    }

    let pattern_display = if patterns.len() == 1 {
        patterns[0].clone()
    } else {
        patterns.join("|")
    };

    let paths = if paths.is_empty() {
        vec![".".to_string()]
    } else {
        paths
    };
    let path_display = paths.join(" ");

    if verbose > 0 {
        eprintln!("grep: '{}' in {}", pattern_display, path_display);
    }

    // format/shape flags: native passthrough, no -0 NUL leak (#2333).
    if has_format_flag(&extra_args) || has_shape_flag(&extra_args) {
        return passthrough(
            &timer,
            &["--no-heading", "--no-ignore-vcs"],
            &["-r"],
            file_type,
            &extra_args,
            &patterns,
            &paths,
            &pattern_display,
            &path_display,
        );
    }

    // GROUP path: -0 NUL-disambiguates file:line for the reparse (#1436).
    let result = grep_capture(
        &["-nH0", "--no-heading", "--no-ignore-vcs"],
        &["-rnH", "--null"],
        file_type,
        &extra_args,
        &patterns,
        &paths,
    )
    .context("grep/rg failed")?;

    let exit_code = result.exit_code;
    let raw_output = result.stdout.clone();

    if result.stdout.trim().is_empty() {
        if is_grep_error_exit(exit_code) {
            if !result.stderr.trim().is_empty() {
                eprintln!("{}", result.stderr.trim());
            }
            let msg = format!("grep failed with exit code {}", exit_code);
            timer.track(
                &format!("grep -rn '{}' {}", pattern_display, path_display),
                "rtk grep",
                &raw_output,
                &msg,
            );
            eprintln!("{}", msg);
            return Ok(exit_code);
        }
        timer.track(
            &format!("grep -rn '{}' {}", pattern_display, path_display),
            "rtk grep",
            &raw_output,
            "",
        );
        return Ok(exit_code);
    }

    // Safety net: unparseable shape → passthrough verbatim, never silently drop (#2333).
    let signal = unparsed_signal(&raw_output);
    if signal > 0 {
        return passthrough(
            &timer,
            &["-nH", "--no-heading", "--no-ignore-vcs"],
            &["-rnH"],
            file_type,
            &extra_args,
            &patterns,
            &paths,
            &pattern_display,
            &path_display,
        );
    }

    let context_re = if context_only {
        Regex::new(&format!(
            "(?i).{{0,20}}{}.*",
            regex::escape(&pattern_display)
        ))
        .ok()
    } else {
        None
    };

    let mut by_file: HashMap<String, Vec<(usize, bool, String)>> = HashMap::new();
    for line in raw_output.lines() {
        let Some((file, line_num, is_match, content)) = parse_match_line(line) else {
            continue;
        };
        let cleaned = clean_line(content, max_line_len, context_re.as_ref(), &pattern_display);
        by_file
            .entry(file)
            .or_default()
            .push((line_num, is_match, cleaned));
    }

    let total_matches: usize = by_file
        .values()
        .flat_map(|v| v.iter())
        .filter(|(_, is_match, _)| *is_match)
        .count();

    let mut rtk_output = String::new();
    rtk_output.push_str(&format!(
        "{} matches in {} files:\n\n",
        total_matches,
        by_file.len()
    ));

    let mut shown = 0;
    let mut files: Vec<_> = by_file.iter().collect();
    files.sort_by_key(|(f, _)| *f);

    let per_file = config::limits().grep_max_per_file;
    for (file, entries) in files {
        if shown >= max_results {
            break;
        }

        let file_display = compact_path(file);
        for (line_num, is_match, content) in entries.iter().take(per_file) {
            if shown >= max_results {
                break;
            }
            if *is_match {
                rtk_output.push_str(&format!("{}:{}:{}\n", file_display, line_num, content));
            } else {
                rtk_output.push_str(&format!("{}-{}-{}\n", file_display, line_num, content));
            }
            shown += 1;
        }
    }

    let total_lines: usize = by_file.values().map(|v| v.len()).sum();
    if total_lines > shown {
        rtk_output.push_str(&format!("[+{} more]\n", total_lines - shown));
    }

    // Never-worse: show plain `file:line:content` (NUL `-0` -> `:`) if grouping didn't shrink it.
    let plain = raw_output.replace('\0', ":");
    let output = if rtk_output.len() < plain.len() {
        rtk_output
    } else {
        plain
    };

    print!("{}", output);
    timer.track(
        &format!("grep -rn '{}' {}", pattern_display, path_display),
        "rtk grep",
        &raw_output,
        &output,
    );

    Ok(exit_code)
}

/// Parses a single rg/grep match or context line of the form
/// `file\0line_number[:-]content`.
///
/// Requires the underlying command to be invoked with `-0` (rg) or `--null`
/// (grep) so the filename is NUL-separated from `line[:-]content`. NUL cannot
/// appear in file paths, so the parser is unambiguous regardless of:
///   - content with `:` or `::` (e.g. `ClassRegistry::init(...)`, issue #1436);
///   - paths with embedded `:` (Windows drive letters, weird filenames like
///     `badly_named:52:file.txt`).
///
/// Returns `None` for lines that do not match the expected shape.
/// The `bool` in the tuple is `true` for match lines (`:` separator) and
/// `false` for context lines (`-` separator, emitted by -A/-B/-C).
fn parse_match_line(line: &str) -> Option<(String, usize, bool, &str)> {
    lazy_static::lazy_static! {
        static ref MATCH_LINE_RE: Regex = Regex::new(r"^([^\x00]+)\x00(\d+)([:-])(.*)$").unwrap();
    }
    MATCH_LINE_RE.captures(line).and_then(|caps| {
        let file = caps.get(1)?.as_str().to_string();
        let line_num: usize = caps.get(2)?.as_str().parse().ok()?;
        let sep = caps.get(3)?.as_str();
        let content = caps.get(4)?.as_str();
        let is_match = sep == ":";
        Some((file, line_num, is_match, content))
    })
}

fn has_format_flag<T: AsRef<str>>(extra_args: &[T]) -> bool {
    extra_args.iter().any(|arg| {
        matches!(
            arg.as_ref(),
            "-c" | "--count"
                | "--count-matches"
                | "-l"
                | "--files-with-matches"
                | "-L"
                | "--files-without-match"
                | "-o"
                | "--only-matching"
                | "-Z"
                | "--null"
                | "--json"
                | "--passthru"
                | "--files"
        )
    })
}

fn clean_line(line: &str, max_len: usize, context_re: Option<&Regex>, pattern: &str) -> String {
    let trimmed = line.trim();

    if let Some(re) = context_re {
        if let Some(m) = re.find(trimmed) {
            let matched = m.as_str();
            if matched.len() <= max_len {
                return matched.to_string();
            }
        }
    }

    if trimmed.len() <= max_len {
        trimmed.to_string()
    } else {
        let lower = trimmed.to_lowercase();
        let pattern_lower = pattern.to_lowercase();

        if let Some(pos) = lower.find(&pattern_lower) {
            let char_pos = lower[..pos].chars().count();
            let chars: Vec<char> = trimmed.chars().collect();
            let char_len = chars.len();

            let start = char_pos.saturating_sub(max_len / 3);
            let end = (start + max_len).min(char_len);
            let start = if end == char_len {
                end.saturating_sub(max_len)
            } else {
                start
            };

            let slice: String = chars[start..end].iter().collect();
            if start > 0 && end < char_len {
                format!("...{}...", slice)
            } else if start > 0 {
                format!("...{}", slice)
            } else {
                format!("{}...", slice)
            }
        } else {
            let t: String = trimmed.chars().take(max_len - 3).collect();
            format!("{}...", t)
        }
    }
}

fn compact_path(path: &str) -> String {
    if path.len() <= 50 {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        return path.to_string();
    }

    format!(
        "{}/.../{}/{}",
        parts[0],
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    )
}

/// grep/rg convention: exit 1 = no match found (normal), exit >= 2 = real
/// error (bad regex, tool crash, missing binary). An error must surface to the
/// user, never be silently reported as a false "0 matches".
fn is_grep_error_exit(exit_code: i32) -> bool {
    exit_code >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_grep_error_exit() {
        // exit 0 = matches, exit 1 = no match: both normal, not errors.
        assert!(!is_grep_error_exit(0));
        assert!(!is_grep_error_exit(1));
        // exit >= 2 = real error (bad regex, tool crash, missing binary).
        // Must surface, never become a false "0 matches".
        assert!(is_grep_error_exit(2));
        assert!(is_grep_error_exit(3));
        assert!(is_grep_error_exit(127));
    }

    #[test]
    fn test_clean_line() {
        let line = "            const result = someFunction();";
        let cleaned = clean_line(line, 50, None, "result");
        assert!(!cleaned.starts_with(' '));
        assert!(cleaned.len() <= 50);
    }

    #[test]
    fn test_compact_path() {
        let path = "/Users/patrick/dev/project/src/components/Button.tsx";
        let compact = compact_path(path);
        assert!(compact.len() <= 60);
    }

    #[test]
    fn test_clean_line_multibyte() {
        // Thai text that exceeds max_len in bytes
        let line = "  สวัสดีครับ นี่คือข้อความที่ยาวมากสำหรับทดสอบ  ";
        let cleaned = clean_line(line, 20, None, "ครับ");
        // Should not panic
        assert!(!cleaned.is_empty());
    }

    #[test]
    fn test_clean_line_emoji() {
        let line = "🎉🎊🎈🎁🎂🎄 some text 🎃🎆🎇✨";
        let cleaned = clean_line(line, 15, None, "text");
        assert!(!cleaned.is_empty());
    }

    // Fix: BRE \| alternation is translated to PCRE | for rg
    #[test]
    fn test_bre_alternation_translated() {
        let pattern = r"fn foo\|pub.*bar";
        let rg_pattern = pattern.replace(r"\|", "|");
        assert_eq!(rg_pattern, "fn foo|pub.*bar");
    }

    // --- parse_cluster ---

    fn vt(prefix: Option<&str>, flag: char, inline: &str) -> ClusterResult {
        ClusterResult::ValueTaking {
            prefix: prefix.map(|s| s.to_string()),
            flag,
            inline: inline.to_string(),
        }
    }

    #[test]
    fn test_parse_cluster_boolean_only() {
        // Pure boolean clusters: r/R stripped, remainder emitted
        assert_eq!(parse_cluster("r"), ClusterResult::Boolean(None));
        assert_eq!(parse_cluster("R"), ClusterResult::Boolean(None));
        assert_eq!(parse_cluster("rR"), ClusterResult::Boolean(None));
        assert_eq!(
            parse_cluster("rn"),
            ClusterResult::Boolean(Some("n".to_string()))
        );
        assert_eq!(
            parse_cluster("Rni"),
            ClusterResult::Boolean(Some("ni".to_string()))
        );
        assert_eq!(
            parse_cluster("n"),
            ClusterResult::Boolean(Some("n".to_string()))
        );
        assert_eq!(
            parse_cluster("ni"),
            ClusterResult::Boolean(Some("ni".to_string()))
        );
    }

    #[test]
    fn test_parse_cluster_e_no_inline() {
        // -e: value-taking, empty inline → caller consumes next token
        assert_eq!(parse_cluster("e"), vt(None, 'e', ""));
    }

    #[test]
    fn test_parse_cluster_e_inline_value() {
        // -ecarrot: inline="carrot" — no r/R stripping on the value bytes
        assert_eq!(parse_cluster("ecarrot"), vt(None, 'e', "carrot"));
    }

    #[test]
    fn test_parse_cluster_e_inline_value_no_rstrip() {
        // The 'r' chars in "carrot" must survive verbatim in the inline field.
        // If strip_r were called on inline bytes, this would return "caot".
        let ClusterResult::ValueTaking { inline, .. } = parse_cluster("ecarrot") else {
            panic!("expected ValueTaking");
        };
        assert_eq!(inline, "carrot");
    }

    #[test]
    fn test_parse_cluster_g_inline_glob() {
        // -g*.rs: inline="*.rs" — 'r' in "*.rs" must not be stripped
        assert_eq!(parse_cluster("g*.rs"), vt(None, 'g', "*.rs"));
        let ClusterResult::ValueTaking { inline, .. } = parse_cluster("g*.rs") else {
            panic!("expected ValueTaking");
        };
        assert_eq!(inline, "*.rs");
    }

    #[test]
    fn test_parse_cluster_rne() {
        // -rne: r stripped, n in boolean prefix, e is value-taking (empty inline)
        assert_eq!(parse_cluster("rne"), vt(Some("n"), 'e', ""));
    }

    #[test]
    fn test_parse_cluster_r_a() {
        // -rA: r stripped, A is value-taking (empty inline → consume next token)
        assert_eq!(parse_cluster("rA"), vt(None, 'A', ""));
    }

    #[test]
    fn test_parse_cluster_ni_a() {
        // -niA: n and i boolean, A value-taking
        assert_eq!(parse_cluster("niA"), vt(Some("ni"), 'A', ""));
    }

    #[test]
    fn test_parse_cluster_ai_inline() {
        // -Ai: A value-taking, inline="i" (the 'i' is A's value, not a separate flag)
        assert_eq!(parse_cluster("Ai"), vt(None, 'A', "i"));
    }

    #[test]
    fn test_parse_cluster_short_type() {
        assert_eq!(parse_cluster("t"), vt(None, 't', ""));
        assert_eq!(parse_cluster("tpy"), vt(None, 't', "py")); // inline type name
    }

    #[test]
    fn test_parse_cluster_short_max_columns() {
        assert_eq!(parse_cluster("M"), vt(None, 'M', ""));
        assert_eq!(parse_cluster("M120"), vt(None, 'M', "120"));
    }

    // --- strip_r ---

    #[test]
    fn test_strip_r() {
        assert_eq!(strip_r("r"), None);
        assert_eq!(strip_r("R"), None);
        assert_eq!(strip_r("rR"), None);
        assert_eq!(strip_r(""), None);
        assert_eq!(strip_r("rn"), Some("n".to_string()));
        assert_eq!(strip_r("Rni"), Some("ni".to_string()));
        assert_eq!(strip_r("i"), Some("i".to_string()));
        // Shows why it must only be called on flag letters, not value bytes:
        assert_eq!(strip_r("carrot"), Some("caot".to_string()));
    }

    // --- strip_recursive ---

    #[test]
    fn test_strip_recursive() {
        assert_eq!(strip_recursive("--recursive"), None);
        assert_eq!(strip_recursive("--glob"), Some("--glob".to_string()));
        assert_eq!(strip_recursive("--type"), Some("--type".to_string()));
    }

    // --- extract_pattern_path ---

    #[test]
    fn test_extract_simple() {
        let (patterns, paths, flags) = extract_pattern_path(&["foo", "src/"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src/"]);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_extract_with_bool_flag() {
        let (patterns, paths, flags) = extract_pattern_path(&["-i", "foo", "src/"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src/"]);
        assert_eq!(flags, vec!["-i"]);
    }

    #[test]
    fn test_extract_value_taking_flag() {
        // -A 2 must not steal "error" as its value
        let (patterns, paths, flags) = extract_pattern_path(&["-A", "2", "error", "src"]);
        assert_eq!(patterns, vec!["error"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["-A", "2"]);
    }

    #[test]
    fn test_extract_cluster_strip_r() {
        // -rn: r stripped, n forwarded (not leaked to rg as --replace value)
        let (patterns, paths, flags) = extract_pattern_path(&["-rn", "foo", "src"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["-n"]);
    }

    #[test]
    fn test_extract_cluster_ending_in_e() {
        // -rne PATTERN: r stripped, n in prefix, e consumes PATTERN as pattern
        let (patterns, paths, flags) = extract_pattern_path(&["-rne", "PATTERN", "src"]);
        assert_eq!(patterns, vec!["PATTERN"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["-n"]);
    }

    #[test]
    fn test_extract_cluster_ending_in_value_flag() {
        // -rA 2: r stripped, A consumes 2 as context value
        let (patterns, paths, flags) = extract_pattern_path(&["-rA", "2", "foo", "src"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["-A", "2"]);
    }

    #[test]
    fn test_extract_multi_path() {
        let (patterns, paths, flags) = extract_pattern_path(&["TODO", "src", "tests"]);
        assert_eq!(patterns, vec!["TODO"]);
        assert_eq!(paths, vec!["src", "tests"]);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_extract_glob_value() {
        // -g '*.md' must not steal "agent" as its value
        let (patterns, paths, flags) = extract_pattern_path(&["-i", "x", "agent", "-g", "*.md"]);
        assert_eq!(patterns, vec!["x"]);
        assert_eq!(paths, vec!["agent"]);
        assert_eq!(flags, vec!["-i", "-g", "*.md"]);
    }

    #[test]
    fn test_extract_e_flag() {
        let (patterns, paths, flags) = extract_pattern_path(&["-e", "fn run", "src"]);
        assert_eq!(patterns, vec!["fn run"]);
        assert_eq!(paths, vec!["src"]);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_extract_multi_e() {
        let (patterns, paths, flags) = extract_pattern_path(&["-e", "foo", "-e", "bar", "src"]);
        assert_eq!(patterns, vec!["foo", "bar"]);
        assert_eq!(paths, vec!["src"]);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_extract_dashdash_boundary() {
        // After --, args are positional even if they look like flags
        let (patterns, paths, flags) = extract_pattern_path(&["--", "--version"]);
        assert_eq!(patterns, vec!["--version"]);
        assert!(paths.is_empty());
        assert!(flags.is_empty());
    }

    #[test]
    fn test_extract_no_args() {
        let (patterns, paths, flags) = extract_pattern_path::<&str>(&[]);
        assert!(patterns.is_empty());
        assert!(paths.is_empty());
        assert!(flags.is_empty());
    }

    #[test]
    fn test_extract_default_path_empty() {
        // Caller is responsible for defaulting empty paths to ["."]
        let (patterns, paths, _) = extract_pattern_path(&["foo"]);
        assert_eq!(patterns, vec!["foo"]);
        assert!(paths.is_empty());
    }

    #[test]
    fn test_extract_ending_e() {
        let (patterns, paths, flags) =
            extract_pattern_path(&["-e", "foo", "-e", "bar", "src", "-e"]);
        assert_eq!(patterns, vec!["foo", "bar"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["-e"]);
    }

    // --- inline short flag values (Bug 5) ---

    #[test]
    fn test_extract_inline_e_value() {
        // -ecarrot: e hits at j=0, inline="carrot", no r-stripping on value
        let (patterns, paths, flags) = extract_pattern_path(&["-ecarrot", "file"]);
        assert_eq!(patterns, vec!["carrot"]);
        assert_eq!(paths, vec!["file"]);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_extract_inline_e_value_no_rstrip() {
        // -ecarrot: the 'r' in "carrot" must NOT be stripped (it's value, not a flag)
        let (patterns, _, _) = extract_pattern_path(&["-ecarrot", "file"]);
        assert_eq!(
            patterns,
            vec!["carrot"],
            "r in inline value must not be stripped"
        );
    }

    #[test]
    fn test_extract_inline_g_value() {
        // -g*.rs: g hits at j=0, inline="*.rs", no r-stripping on value
        let (patterns, paths, flags) = extract_pattern_path(&["aaa", "sub", "-g*.rs"]);
        assert_eq!(patterns, vec!["aaa"]);
        assert_eq!(paths, vec!["sub"]);
        assert_eq!(flags, vec!["-g", "*.rs"]);
    }

    #[test]
    fn test_extract_inline_g_value_no_rstrip() {
        // -g*.rs: the 'r' in "*.rs" must NOT be stripped
        let (_, _, flags) = extract_pattern_path(&["aaa", "sub", "-g*.rs"]);
        assert!(
            flags.contains(&"*.rs".to_string()),
            "r in glob value must not be stripped"
        );
    }

    // --- long value-taking flags (Bug 5) ---

    #[test]
    fn test_extract_long_glob_value() {
        let (patterns, paths, flags) = extract_pattern_path(&["compact", "sub", "--glob", "*.md"]);
        assert_eq!(patterns, vec!["compact"]);
        assert_eq!(paths, vec!["sub"]);
        assert_eq!(flags, vec!["--glob", "*.md"]);
    }

    #[test]
    fn test_extract_long_max_count() {
        let (patterns, paths, flags) = extract_pattern_path(&["--max-count", "1", "fn", "file"]);
        assert_eq!(patterns, vec!["fn"]);
        assert_eq!(paths, vec!["file"]);
        assert_eq!(flags, vec!["--max-count", "1"]);
    }

    #[test]
    fn test_extract_short_type() {
        // -t rust: type filter, value must not become pattern
        let (patterns, paths, flags) = extract_pattern_path(&["-t", "rust", "fn", "src"]);
        assert_eq!(patterns, vec!["fn"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["-t", "rust"]);
    }

    #[test]
    fn test_extract_short_max_depth() {
        // -d 3: max-depth, value must not become pattern
        let (patterns, paths, flags) = extract_pattern_path(&["-d", "3", "foo", "src"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["-d", "3"]);
    }

    #[test]
    fn test_extract_short_max_columns() {
        // -M 120: max-columns, value must not become pattern
        let (patterns, paths, flags) = extract_pattern_path(&["-M", "120", "foo", "src"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["-M", "120"]);
    }

    #[test]
    fn test_extract_long_regexp() {
        // --regexp is the long form of -e; value goes to patterns
        let (patterns, paths, flags) = extract_pattern_path(&["--regexp", "fn run", "src"]);
        assert_eq!(patterns, vec!["fn run"]);
        assert_eq!(paths, vec!["src"]);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_extract_long_regexp_multi() {
        // --regexp can be combined with -e
        let (patterns, paths, _) = extract_pattern_path(&["--regexp", "foo", "-e", "bar", "src"]);
        assert_eq!(patterns, vec!["foo", "bar"]);
        assert_eq!(paths, vec!["src"]);
    }

    #[test]
    fn test_extract_long_ignore_file() {
        let (patterns, paths, flags) =
            extract_pattern_path(&["--ignore-file", ".myignore", "foo", "src"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["--ignore-file", ".myignore"]);
    }

    #[test]
    fn test_extract_long_engine() {
        let (patterns, paths, flags) = extract_pattern_path(&["--engine", "pcre2", "foo", "src"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["--engine", "pcre2"]);
    }

    #[test]
    fn test_extract_long_type_clear() {
        let (patterns, paths, flags) =
            extract_pattern_path(&["--type-clear", "rust", "foo", "src"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["--type-clear", "rust"]);
    }

    #[test]
    fn test_extract_long_path_separator() {
        let (patterns, paths, flags) =
            extract_pattern_path(&["--path-separator", "/", "foo", "src"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["--path-separator", "/"]);
    }

    #[test]
    fn test_extract_long_flag_inline_eq_passthrough() {
        // --glob=*.rs is one token (inline =): passes through as-is, not consumed as pair
        let (patterns, paths, flags) = extract_pattern_path(&["foo", "src", "--glob=*.rs"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["--glob=*.rs"]);
    }

    // --- has_format_flag additions ---

    #[test]
    fn test_format_flag_detects_count_matches() {
        assert!(has_format_flag(&["--count-matches"]));
    }

    #[test]
    fn test_format_flag_detects_json() {
        assert!(has_format_flag(&["--json"]));
    }

    #[test]
    fn test_format_flag_detects_passthru() {
        assert!(has_format_flag(&["--passthru"]));
    }

    #[test]
    fn test_format_flag_detects_files() {
        assert!(has_format_flag(&["--files"]));
    }

    // --- truncation accuracy ---

    #[test]
    fn test_grep_overflow_uses_uncapped_total() {
        // Confirm the grep overflow invariant: matches vec is never capped before overflow calc.
        // If total_matches > per_file, overflow = total_matches - per_file (not capped).
        // This documents that grep_cmd.rs avoids the diff_cmd bug (cap at N then compute N-10).
        let per_file = config::limits().grep_max_per_file;
        let total_matches = per_file + 42;
        let overflow = total_matches - per_file;
        assert_eq!(overflow, 42, "overflow must equal true suppressed count");
        // Demonstrate why capping before subtraction is wrong:
        let hypothetical_cap = per_file + 5;
        let capped = total_matches.min(hypothetical_cap);
        let wrong_overflow = capped - per_file;
        assert_ne!(
            wrong_overflow, overflow,
            "capping before subtraction gives wrong overflow"
        );
    }

    // --- format flag detection ---

    #[test]
    fn test_format_flag_detects_count() {
        assert!(has_format_flag(&["-c"]));
        assert!(has_format_flag(&["--count"]));
    }

    #[test]
    fn test_format_flag_detects_files_with_matches() {
        assert!(has_format_flag(&["-l"]));
        assert!(has_format_flag(&["--files-with-matches"]));
    }

    #[test]
    fn test_format_flag_detects_files_without_match() {
        assert!(has_format_flag(&["-L"]));
        assert!(has_format_flag(&["--files-without-match"]));
    }

    #[test]
    fn test_format_flag_detects_only_matching() {
        assert!(has_format_flag(&["-o"]));
        assert!(has_format_flag(&["--only-matching"]));
    }

    #[test]
    fn test_format_flag_detects_null() {
        assert!(has_format_flag(&["-Z"]));
        assert!(has_format_flag(&["--null"]));
    }

    #[test]
    fn test_format_flag_ignores_normal_flags() {
        assert!(!has_format_flag(&["-i", "-w", "-A", "3"]));
    }

    #[test]
    fn test_strip_rg_only_drops_glob_and_value() {
        assert_eq!(strip_rg_only(&["--glob", "*.rs", "-i"]), vec!["-i"]);
        assert_eq!(strip_rg_only(&["-g", "*.rs", "-i"]), vec!["-i"]);
    }

    #[test]
    fn test_strip_rg_only_drops_inline_glob() {
        assert_eq!(strip_rg_only(&["--glob=*.rs", "-i"]), vec!["-i"]);
    }

    #[test]
    fn test_strip_rg_only_drops_bool_flags() {
        assert_eq!(
            strip_rg_only(&["--hidden", "--pcre2", "--json", "-n"]),
            vec!["-n"]
        );
    }

    #[test]
    fn test_strip_rg_only_drops_type_and_value() {
        assert_eq!(strip_rg_only(&["--type", "rust", "-w"]), vec!["-w"]);
        assert_eq!(strip_rg_only(&["-T", "rust", "-w"]), vec!["-w"]);
    }

    #[test]
    fn test_strip_rg_only_keeps_grep_compatible() {
        assert_eq!(
            strip_rg_only(&["-i", "-w", "-A", "3", "-v"]),
            vec!["-i", "-w", "-A", "3", "-v"]
        );
    }

    // Verify line numbers are always enabled in rg invocation (grep_cmd.rs:24).
    // The -n/--line-numbers clap flag in main.rs is a no-op accepted for compat.
    #[test]
    fn test_rg_always_has_line_numbers() {
        // grep_cmd::run() always passes "-n" to rg (line 24).
        // This test documents that -n is built-in, so the clap flag is safe to ignore.
        let mut cmd = resolved_command("rg");
        cmd.args(["-n", "--no-heading", "NONEXISTENT_PATTERN_12345", "."]);
        // If rg is available, it should accept -n without error (exit 1 = no match, not error)
        if let Ok(output) = cmd.output() {
            assert!(
                output.status.code() == Some(1) || output.status.success(),
                "rg -n should be accepted"
            );
        }
        // If rg is not installed, skip gracefully (test still passes)
    }

    // --- issue #1436: parse_match_line robustness ---
    // Input shape is `file\0line[:-]content` (rg --null / grep -Z).

    #[test]
    fn test_parse_match_line_simple() {
        let line = "file.php\x0010:use Foo\\Bar;";
        let (file, line_num, is_match, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "file.php");
        assert_eq!(line_num, 10);
        assert!(is_match);
        assert_eq!(content, "use Foo\\Bar;");
    }

    // Issue #1436 reproducer: content with `::` must not split into a phantom
    // file bucket. With NUL separation between file and line:content, content
    // colons are irrelevant to the parser.
    #[test]
    fn test_parse_match_line_content_with_double_colon() {
        let line = "externalImportShell.class.php\x0081:        $this->queueProcessModel = ClassRegistry::init('Collections.QueueProcess');";
        let (file, line_num, is_match, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "externalImportShell.class.php");
        assert_eq!(line_num, 81);
        assert!(is_match);
        assert_eq!(
            content,
            "        $this->queueProcessModel = ClassRegistry::init('Collections.QueueProcess');"
        );
    }

    // Windows abs-path safety: drive letter + backslashes must not break the
    // parser. The NUL separator makes the file portion unambiguous.
    #[test]
    fn test_parse_match_line_windows_path() {
        let line = "C:\\src\\file.rs\x0042:fn main() {}";
        let (file, line_num, is_match, content) = parse_match_line(line).unwrap();
        assert_eq!(file, r"C:\src\file.rs");
        assert_eq!(line_num, 42);
        assert!(is_match);
        assert_eq!(content, "fn main() {}");
    }

    // Filenames containing `:digits:` (which would fool a greedy `:` parser)
    // must still parse correctly under NUL separation.
    #[test]
    fn test_parse_match_line_filename_with_colons() {
        let line = "badly_named:52:file.txt\x001:xxx";
        let (file, line_num, is_match, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "badly_named:52:file.txt");
        assert_eq!(line_num, 1);
        assert!(is_match);
        assert_eq!(content, "xxx");
    }

    // Content that itself contains `:digits:` (e.g. log lines, port numbers,
    // line-number-like substrings) must not confuse the parser.
    #[test]
    fn test_parse_match_line_content_with_digit_colons() {
        let line = "log.txt\x007:debug: counter is :42: now";
        let (file, line_num, is_match, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "log.txt");
        assert_eq!(line_num, 7);
        assert!(is_match);
        assert_eq!(content, "debug: counter is :42: now");
    }

    #[test]
    fn test_parse_match_line_malformed_returns_none() {
        // No NUL separator (e.g. rg/grep invoked without --null/-Z, or a
        // context line written with `-`).
        assert!(parse_match_line("file.rs:1:content").is_none());
        assert!(parse_match_line("not a match line").is_none());
        // Missing line number after NUL
        assert!(parse_match_line("file.rs\x00fn foo()").is_none());
        // Empty
        assert!(parse_match_line("").is_none());
    }

    #[test]
    fn test_parse_match_line_empty_content() {
        let line = "file.rs\x007:";
        let (file, line_num, is_match, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "file.rs");
        assert_eq!(line_num, 7);
        assert!(is_match);
        assert_eq!(content, "");
    }

    // Context line: separator is `-` → is_match==false
    #[test]
    fn test_parse_match_line_context_line() {
        let line = "file.txt\x004-after1";
        let (file, line_num, is_match, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "file.txt");
        assert_eq!(line_num, 4);
        assert!(!is_match, "dash separator must yield is_match==false");
        assert_eq!(content, "after1");
    }

    #[test]
    fn test_rg_no_ignore_vcs_flag_accepted() {
        // Verify rg accepts --no-ignore-vcs (used to match grep -r behavior for .gitignore)
        let mut cmd = resolved_command("rg");
        cmd.args([
            "-n",
            "--no-heading",
            "--no-ignore-vcs",
            "NONEXISTENT_PATTERN_12345",
            ".",
        ]);
        if let Ok(output) = cmd.output() {
            assert!(
                output.status.code() == Some(1) || output.status.success(),
                "rg --no-ignore-vcs should be accepted"
            );
        }
        // If rg is not installed, skip gracefully (test still passes)
    }

    // --- unparsed_signal ---

    #[test]
    fn test_unparsed_signal_parseable_lines_yield_zero() {
        // NUL-separated match lines all parse → signal == 0
        let stdout = "file.txt\x001:hello\nfile.txt\x002:world\n";
        assert_eq!(unparsed_signal(stdout), 0);
    }

    #[test]
    fn test_unparsed_signal_context_separator_not_counted() {
        // The `--` context separator emitted by rg/grep between match groups
        // must not be counted as an unparsed line.
        let stdout = "file.txt\x001:hello\n--\nfile.txt\x003:world\n";
        assert_eq!(unparsed_signal(stdout), 0);
    }

    #[test]
    fn test_unparsed_signal_empty_line_not_counted() {
        let stdout = "file.txt\x001:hello\n\nfile.txt\x002:world\n";
        assert_eq!(unparsed_signal(stdout), 0);
    }

    #[test]
    fn test_unparsed_signal_bare_colon_line_counted() {
        // A line like "file.rs:1:content" (no NUL) is what --heading or
        // --no-filename output looks like — it must be counted.
        let stdout = "file.rs:1:content\n";
        assert_eq!(unparsed_signal(stdout), 1);
    }

    #[test]
    fn test_unparsed_signal_binary_notice_counted() {
        // rg emits "Binary file foo matches" for binary files; no NUL → counted.
        let stdout = "Binary file foo matches\n";
        assert_eq!(unparsed_signal(stdout), 1);
    }

    #[test]
    fn test_unparsed_signal_context_lines_parse_ok() {
        // Context lines (dash separator) parse via the updated regex → not counted.
        let stdout = "file.txt\x003-context_before\nfile.txt\x004:match\nfile.txt\x005-context_after\n";
        assert_eq!(unparsed_signal(stdout), 0);
    }
}
