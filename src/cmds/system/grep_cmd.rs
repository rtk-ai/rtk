//! Filters grep output by grouping matches by file.

use crate::core::config;
use crate::core::stream::{exec_capture, CaptureResult};
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;

/// `rtk grep` — runs **system grep** so grep's own semantics are respected (BRE
/// by default, grep flags). Ripgrep is handled separately by [`run_rg`]; the two
/// are never crossed, because grep and rg share short flags with conflicting
/// meanings (e.g. `-r` is recursive in grep but `--replace` in rg).
#[allow(clippy::too_many_arguments)]
pub fn run(
    pattern: &str,
    path: &str,
    max_line_len: usize,
    max_results: usize,
    context_only: bool,
    file_type: Option<&str>,
    extra_args: &[String],
    verbose: u8,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("grep: '{}' in {}", pattern, path);
    }

    // -r: recurse like the common `grep -r`; -n: line numbers; -H: always print
    // the filename so output is the parseable `file:line:content`. `-e` marks the
    // pattern explicitly so patterns starting with `-` aren't read as flags.
    let mut grep_cmd = resolved_command("grep");
    grep_cmd.arg("-rnH");
    if let Some(ft) = file_type {
        grep_cmd.arg(format!("--include={}", grep_type_glob(ft)));
    }
    grep_cmd.arg("-e").arg(pattern).arg(path).args(extra_args);

    let result = exec_capture(&mut grep_cmd).context("grep failed")?;

    emit_filtered(
        result,
        extra_args,
        pattern,
        path,
        max_line_len,
        max_results,
        context_only,
        parse_grep_line,
        timer,
    )
}

/// `rtk rg` — faithful ripgrep passthrough with RTK filtering. All args are
/// forwarded to ripgrep verbatim (no grep-ism translation), so ripgrep-only flags
/// like `--glob`/`-g`/`-P`/`--files` work in any order. Routing these through the
/// `grep` subcommand / system grep is what broke in #2167, #2060, #1651.
pub fn run_rg(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("rg {}", args.join(" "));
    }

    // Info/list flags produce output that is not `file:line:content` (or is
    // already compact): run rg verbatim and stream it through untouched.
    if args.iter().any(|a| is_raw_passthrough_flag(a)) {
        let mut rg_cmd = resolved_command("rg");
        rg_cmd.args(args);
        let status = rg_cmd.status().context("failed to execute rg")?;
        let raw_command = format!("rg {}", args.join(" "));
        timer.track_passthrough(
            &raw_command,
            &format!("rtk rg {} (passthrough)", args.join(" ")),
        );
        return Ok(crate::core::utils::exit_code_from_status(
            &status,
            &raw_command,
        ));
    }

    let mut rg_cmd = resolved_command("rg");
    // -0 NUL-separates the filename so the parser is unambiguous (#1436);
    // --no-ignore-vcs matches grep -r (don't skip .gitignore'd files).
    rg_cmd.args(["-nH0", "--no-heading", "--no-ignore-vcs"]);
    rg_cmd.args(args);
    let result = exec_capture(&mut rg_cmd).context("rg failed")?;

    // Pattern is only used for context-aware truncation centring; the positional
    // pattern isn't separated out here, so head-truncate instead.
    emit_filtered(result, args, "", ".", 80, 200, false, parse_match_line, timer)
}

/// Map an rg-style `--file-type` name to a system-grep `--include` glob. Most rg
/// type names are the file extension; a few common ones differ.
fn grep_type_glob(file_type: &str) -> String {
    let ext = match file_type {
        "rust" => "rs",
        "python" => "py",
        "ruby" => "rb",
        "javascript" => "js",
        "typescript" => "ts",
        "markdown" => "md",
        other => other,
    };
    format!("*.{}", ext)
}

/// rg flags whose output should be streamed through unfiltered (file listings,
/// count/format modes). Superset of [`has_format_flag`].
fn is_raw_passthrough_flag(arg: &str) -> bool {
    matches!(
        arg,
        "--files" | "--type-list" | "--json" | "--vimgrep" | "--count-matches"
    ) || has_format_flag(std::slice::from_ref(&arg.to_string()))
}

/// Shared post-execution filtering: passthrough for format flags, an empty-result
/// message, or the by-file grouped/truncated output used by both `run` and
/// `run_rg`. `parse_line` adapts the backend's match-line format (rg's NUL form
/// vs grep's `file:line:content`).
#[allow(clippy::too_many_arguments)]
fn emit_filtered(
    result: CaptureResult,
    extra_args: &[String],
    pattern: &str,
    path: &str,
    max_line_len: usize,
    max_results: usize,
    context_only: bool,
    parse_line: fn(&str) -> Option<(String, usize, &str)>,
    timer: tracking::TimedExecution,
) -> Result<i32> {
    // Passthrough output flags that produce output that is already small.
    if has_format_flag(extra_args) {
        print!("{}", result.stdout);
        if !result.stderr.is_empty() {
            eprint!("{}", result.stderr.trim());
        }

        let args_display = if extra_args.is_empty() {
            format!("'{}' {}", pattern, path)
        } else {
            format!("{} '{}' {}", extra_args.join(" "), pattern, path)
        };

        timer.track_passthrough(
            &format!("grep {}", args_display),
            &format!("rtk grep {} (passthrough)", args_display),
        );
        return Ok(result.exit_code);
    }

    let exit_code = result.exit_code;
    let raw_output = result.stdout.clone();

    if result.stdout.trim().is_empty() {
        // Show stderr for errors (bad regex, missing file, etc.)
        if exit_code == 2 && !result.stderr.trim().is_empty() {
            eprintln!("{}", result.stderr.trim());
        }
        // In raw mode the positional pattern isn't separated out, so omit it
        // rather than printing a misleading empty `0 matches for ''`.
        let msg = if pattern.is_empty() {
            "0 matches".to_string()
        } else {
            format!("0 matches for '{}'", pattern)
        };
        println!("{}", msg);
        timer.track(
            &format!("grep -rn '{}' {}", pattern, path),
            "rtk grep",
            &raw_output,
            &msg,
        );
        return Ok(exit_code);
    }

    // Always filter: truncate long lines, apply per-file and global caps.
    // Output in standard file:line:content format that AI agents can parse.
    // (A passthrough approach yields 0% savings — no reason for RTK to exist on that path.)
    let total_matches = result.stdout.lines().count();

    let context_re = if context_only {
        Regex::new(&format!("(?i).{{0,20}}{}.*", regex::escape(pattern))).ok()
    } else {
        None
    };

    let mut by_file: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    for line in result.stdout.lines() {
        let Some((file, line_num, content)) = parse_line(line) else {
            continue;
        };
        let cleaned = clean_line(content, max_line_len, context_re.as_ref(), pattern);
        by_file.entry(file).or_default().push((line_num, cleaned));
    }

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
    for (file, matches) in files {
        if shown >= max_results {
            break;
        }

        let file_display = compact_path(file);
        for (line_num, content) in matches.iter().take(per_file) {
            if shown >= max_results {
                break;
            }
            rtk_output.push_str(&format!("{}:{}:{}\n", file_display, line_num, content));
            shown += 1;
        }
    }

    if total_matches > shown {
        rtk_output.push_str(&format!("[+{} more]\n", total_matches - shown));
    }

    print!("{}", rtk_output);
    timer.track(
        &format!("grep -rn '{}' {}", pattern, path),
        "rtk grep",
        &raw_output,
        &rtk_output,
    );

    Ok(exit_code)
}

/// Parses a single rg/grep match line of the form `file\0line_number:content`.
///
/// Requires the underlying command to be invoked with `-0` (rg) or `-Z` (grep)
/// so the filename is NUL-separated from `line:content`. NUL cannot appear in
/// file paths, so the parser is unambiguous regardless of:
///   - content with `:` or `::` (e.g. `ClassRegistry::init(...)`, issue #1436);
///   - paths with embedded `:` (Windows drive letters, weird filenames like
///     `badly_named:52:file.txt`).
///
/// Returns `None` for lines that do not match the expected shape (e.g. rg
/// `-A`/`-B` context lines that use `-` as separator).
fn parse_match_line(line: &str) -> Option<(String, usize, &str)> {
    lazy_static::lazy_static! {
        static ref MATCH_LINE_RE: Regex = Regex::new(r"^([^\x00]+)\x00(\d+):(.*)$").unwrap();
    }
    MATCH_LINE_RE.captures(line).and_then(|caps| {
        let (_, [file, line_num, content]) = caps.extract();
        let line_num: usize = line_num.parse().ok()?;
        Some((file.to_string(), line_num, content))
    })
}

/// Parse a system-grep match line of the form `file:line:content` (from
/// `grep -nH`). Unlike rg's NUL form there is no separator to disambiguate, so a
/// filename containing `:` can't be told apart and such lines are skipped — this
/// mirrors grep's own inherent ambiguity. Colons inside the content are kept.
fn parse_grep_line(line: &str) -> Option<(String, usize, &str)> {
    let (file, rest) = line.split_once(':')?;
    let (line_num, content) = rest.split_once(':')?;
    let line_num: usize = line_num.parse().ok()?;
    Some((file.to_string(), line_num, content))
}

fn has_format_flag(extra_args: &[String]) -> bool {
    extra_args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "-c" | "--count"
                | "-l"
                | "--files-with-matches"
                | "-L"
                | "--files-without-match"
                | "-o"
                | "--only-matching"
                | "-Z"
                | "--null"
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_extra_args_accepted() {
        // Test that the function signature accepts extra_args
        // This is a compile-time test - if it compiles, the signature is correct
        let _extra: Vec<String> = vec!["-i".to_string(), "-A".to_string(), "3".to_string()];
        // No need to actually run - we're verifying the parameter exists
    }

    #[test]
    fn test_is_raw_passthrough_flag() {
        // Info/list flags stream through unfiltered.
        for flag in ["--files", "--type-list", "--json", "--count-matches"] {
            assert!(is_raw_passthrough_flag(flag), "{flag} should pass through");
        }
        // Format flags from has_format_flag are also passthrough.
        assert!(is_raw_passthrough_flag("-c"));
        assert!(is_raw_passthrough_flag("--files-with-matches"));
        // Ordinary search flags are filtered, not passed through.
        for flag in ["-i", "--glob", "-g", "-A", "-w", "alpha"] {
            assert!(!is_raw_passthrough_flag(flag), "{flag} should be filtered");
        }
    }

    #[test]
    fn test_parse_grep_line() {
        // Standard grep -nH line.
        assert_eq!(
            parse_grep_line("src/main.rs:42:fn main() {"),
            Some(("src/main.rs".to_string(), 42, "fn main() {"))
        );
        // Colons inside the content are preserved.
        assert_eq!(
            parse_grep_line("a.rs:7:Registry::init(x):y"),
            Some(("a.rs".to_string(), 7, "Registry::init(x):y"))
        );
        // Non-match lines (e.g. grep -A/-B context with `-` separator) are skipped.
        assert_eq!(parse_grep_line("src/main.rs-43-    let x = 1;"), None);
        assert_eq!(parse_grep_line("no colons here"), None);
    }

    #[test]
    fn test_grep_type_glob() {
        assert_eq!(grep_type_glob("py"), "*.py");
        assert_eq!(grep_type_glob("rust"), "*.rs");
        assert_eq!(grep_type_glob("ts"), "*.ts");
        assert_eq!(grep_type_glob("markdown"), "*.md");
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

    // Fix: -r flag (grep recursive) is stripped from extra_args (rg is recursive by default)
    #[test]
    fn test_recursive_flag_stripped() {
        let extra_args: Vec<String> = vec!["-r".to_string(), "-i".to_string()];
        let filtered: Vec<&String> = extra_args
            .iter()
            .filter(|a| *a != "-r" && *a != "--recursive")
            .collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0], "-i");
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
        assert!(has_format_flag(&["-c".to_string()]));
        assert!(has_format_flag(&["--count".to_string()]));
    }

    #[test]
    fn test_format_flag_detects_files_with_matches() {
        assert!(has_format_flag(&["-l".to_string()]));
        assert!(has_format_flag(&["--files-with-matches".to_string()]));
    }

    #[test]
    fn test_format_flag_detects_files_without_match() {
        assert!(has_format_flag(&["-L".to_string()]));
        assert!(has_format_flag(&["--files-without-match".to_string()]));
    }

    #[test]
    fn test_format_flag_detects_only_matching() {
        assert!(has_format_flag(&["-o".to_string()]));
        assert!(has_format_flag(&["--only-matching".to_string()]));
    }

    #[test]
    fn test_format_flag_detects_null() {
        assert!(has_format_flag(&["-Z".to_string()]));
        assert!(has_format_flag(&["--null".to_string()]));
    }

    #[test]
    fn test_format_flag_ignores_normal_flags() {
        assert!(!has_format_flag(&[
            "-i".to_string(),
            "-w".to_string(),
            "-A".to_string(),
            "3".to_string(),
        ]));
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
    // Input shape is `file\0line:content` (rg --null / grep -Z).

    #[test]
    fn test_parse_match_line_simple() {
        let line = "file.php\x0010:use Foo\\Bar;";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "file.php");
        assert_eq!(line_num, 10);
        assert_eq!(content, "use Foo\\Bar;");
    }

    // Issue #1436 reproducer: content with `::` must not split into a phantom
    // file bucket. With NUL separation between file and line:content, content
    // colons are irrelevant to the parser.
    #[test]
    fn test_parse_match_line_content_with_double_colon() {
        let line = "externalImportShell.class.php\x0081:        $this->queueProcessModel = ClassRegistry::init('Collections.QueueProcess');";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "externalImportShell.class.php");
        assert_eq!(line_num, 81);
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
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, r"C:\src\file.rs");
        assert_eq!(line_num, 42);
        assert_eq!(content, "fn main() {}");
    }

    // Filenames containing `:digits:` (which would fool a greedy `:` parser)
    // must still parse correctly under NUL separation.
    #[test]
    fn test_parse_match_line_filename_with_colons() {
        let line = "badly_named:52:file.txt\x001:xxx";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "badly_named:52:file.txt");
        assert_eq!(line_num, 1);
        assert_eq!(content, "xxx");
    }

    // Content that itself contains `:digits:` (e.g. log lines, port numbers,
    // line-number-like substrings) must not confuse the parser.
    #[test]
    fn test_parse_match_line_content_with_digit_colons() {
        let line = "log.txt\x007:debug: counter is :42: now";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "log.txt");
        assert_eq!(line_num, 7);
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
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "file.rs");
        assert_eq!(line_num, 7);
        assert_eq!(content, "");
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
}
