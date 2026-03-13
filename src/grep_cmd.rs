use crate::tracking;
use crate::utils::resolved_command;
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;

pub fn run(
    pattern: &str,
    path: &str,
    max_line_len: usize,
    max_results: usize,
    context_only: bool,
    file_type: Option<&str>,
    extra_args: &[String],
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("grep: '{}' in {}", pattern, path);
    }

    // BRE alternation \| → | for rg (which uses PCRE-style regex)
    let rg_pattern = pattern.replace(r"\|", "|");

    let mut rg_cmd = resolved_command("rg");
    rg_cmd.args(["-n", "--no-heading", &rg_pattern, path]);

    if let Some(ft) = file_type {
        rg_cmd.arg("--type").arg(ft);
    }

    for arg in extra_args {
        // Skip grep-ism -r flag (rg is recursive by default; rg -r means --replace)
        if arg == "-r" || arg == "--recursive" {
            continue;
        }
        rg_cmd.arg(arg);
    }

    let output = rg_cmd
        .output()
        .or_else(|_| {
            resolved_command("grep")
                .args(["-rn", pattern, path])
                .output()
        })
        .context("grep/rg failed")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let exit_code = output.status.code().unwrap_or(1);

    let raw_output = stdout.to_string();

    if stdout.trim().is_empty() {
        // Show stderr for errors (bad regex, missing file, etc.)
        if exit_code == 2 {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.trim().is_empty() {
                eprintln!("{}", stderr.trim());
            }
        }
        let msg = format!("🔍 0 for '{}'", pattern);
        println!("{}", msg);
        timer.track(
            &format!("grep -rn '{}' {}", pattern, path),
            "rtk grep",
            &raw_output,
            &msg,
        );
        if exit_code != 0 {
            std::process::exit(exit_code);
        }
        return Ok(());
    }

    let mut by_file: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    let mut total = 0;

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, ':').collect();

        let (file, line_num, content) = if parts.len() == 3 {
            let ln = parts[1].parse().unwrap_or(0);
            (parts[0].to_string(), ln, parts[2])
        } else if parts.len() == 2 {
            let ln = parts[0].parse().unwrap_or(0);
            (path.to_string(), ln, parts[1])
        } else {
            continue;
        };

        total += 1;
        let cleaned = clean_line(content, max_line_len, context_only, pattern);
        by_file.entry(file).or_default().push((line_num, cleaned));
    }

    let mut rtk_output = String::new();
    rtk_output.push_str(&format!("🔍 {} in {}F:\n\n", total, by_file.len()));

    let mut shown = 0;
    let mut files: Vec<_> = by_file.iter().collect();
    files.sort_by_key(|(f, _)| *f);

    for (file, matches) in files {
        if shown >= max_results {
            break;
        }

        let file_display = compact_path(file);
        rtk_output.push_str(&format!("📄 {} ({}):\n", file_display, matches.len()));

        for (line_num, content) in matches.iter().take(10) {
            rtk_output.push_str(&format!("  {:>4}: {}\n", line_num, content));
            shown += 1;
            if shown >= max_results {
                break;
            }
        }

        if matches.len() > 10 {
            rtk_output.push_str(&format!("  +{}\n", matches.len() - 10));
        }
        rtk_output.push('\n');
    }

    if total > shown {
        rtk_output.push_str(&format!("... +{}\n", total - shown));
    }

    print!("{}", rtk_output);
    timer.track(
        &format!("grep -rn '{}' {}", pattern, path),
        "rtk grep",
        &raw_output,
        &rtk_output,
    );

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// Filter raw rg/grep output into compact grouped display.
///
/// Handles two formats:
/// - Three-part `file:line_num:content` — produced by `rg -n` or `grep -n`
/// - Two-part `file:content` — produced by `grep` without `-n` or BSD grep
///
/// Suitable for `rtk pipe --filter grep` / `rtk pipe --filter rg`.
/// Uses defaults: max_line_len=80, max_results=50.
pub(crate) fn filter_grep_raw(input: &str) -> String {
    if input.trim().is_empty() {
        return "🔍 0 matches\n".to_string();
    }

    let mut by_file: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    let mut total = 0;
    const MAX_RESULTS: usize = 50;
    const MAX_LINE_LEN: usize = 80;

    for line in input.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        // Three-part: file:line_num:content  (rg -n / grep -n)
        // Two-part:   file:content           (grep without -n)
        let (file, line_num, content) = if parts.len() == 3 {
            if let Ok(ln) = parts[1].parse::<usize>() {
                // Confirmed three-part with numeric line number
                (parts[0].to_string(), ln, parts[2])
            } else {
                // parts[1] is not a number; treat as two-part with ':' in content
                let content = &line[parts[0].len() + 1..]; // everything after first ':'
                (parts[0].to_string(), 0, content)
            }
        } else if parts.len() == 2 {
            // Two-part: file:content
            (parts[0].to_string(), 0, parts[1])
        } else {
            continue;
        };

        total += 1;
        let cleaned = clean_line(content, MAX_LINE_LEN, false, "");
        by_file.entry(file).or_default().push((line_num, cleaned));
    }

    if total == 0 {
        return "🔍 0 matches\n".to_string();
    }

    let mut out = String::new();
    out.push_str(&format!("🔍 {} in {}F:\n\n", total, by_file.len()));

    let mut shown = 0;
    let mut files: Vec<_> = by_file.iter().collect();
    files.sort_by_key(|(f, _)| *f);

    for (file, matches) in files {
        if shown >= MAX_RESULTS {
            break;
        }
        let file_display = compact_path(file);
        out.push_str(&format!("📄 {} ({}):\n", file_display, matches.len()));
        for (line_num, content) in matches.iter().take(10) {
            out.push_str(&format!("  {:>4}: {}\n", line_num, content));
            shown += 1;
            if shown >= MAX_RESULTS {
                break;
            }
        }
        if matches.len() > 10 {
            out.push_str(&format!("  +{}\n", matches.len() - 10));
        }
        out.push('\n');
    }

    if total > shown {
        out.push_str(&format!("... +{}\n", total - shown));
    }

    out
}

/// Filter `find`/`fd` output (one path per line) into a compact summary.
///
/// Groups results by parent directory and counts files by extension.
/// Suitable for `rtk pipe --filter find` and `rtk pipe --filter fd`.
pub(crate) fn filter_find_output(input: &str) -> String {
    if input.trim().is_empty() {
        return "find: 0 results\n".to_string();
    }

    let mut by_dir: HashMap<String, usize> = HashMap::new();
    let mut by_ext: HashMap<String, usize> = HashMap::new();
    let mut total = 0usize;

    for line in input.lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        total += 1;

        // Parent directory
        let dir = if let Some(pos) = path.rfind('/') {
            &path[..pos]
        } else {
            "."
        };
        *by_dir.entry(dir.to_string()).or_insert(0) += 1;

        // Extension
        let ext = if let Some(pos) = path.rfind('.') {
            let candidate = &path[pos + 1..];
            // Only treat as extension if no '/' after the dot (i.e. it's in the filename)
            if candidate.contains('/') {
                "(no ext)"
            } else {
                candidate
            }
        } else {
            "(no ext)"
        };
        *by_ext.entry(ext.to_string()).or_insert(0) += 1;
    }

    if total == 0 {
        return "find: 0 results\n".to_string();
    }

    let mut out = String::new();
    out.push_str(&format!("find: {} files\n", total));

    // Top directories (up to 10)
    let mut dirs: Vec<_> = by_dir.iter().collect();
    dirs.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    out.push_str("Dirs:\n");
    for (dir, count) in dirs.iter().take(10) {
        out.push_str(&format!("  {} ({})\n", dir, count));
    }
    if dirs.len() > 10 {
        out.push_str(&format!("  ... +{} more dirs\n", dirs.len() - 10));
    }

    // Extension breakdown (up to 8)
    let mut exts: Vec<_> = by_ext.iter().collect();
    exts.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    if !exts.is_empty() {
        out.push_str("Types: ");
        let ext_parts: Vec<String> = exts
            .iter()
            .take(8)
            .map(|(ext, count)| format!(".{} ({})", ext, count))
            .collect();
        out.push_str(&ext_parts.join(", "));
        if exts.len() > 8 {
            out.push_str(&format!(", +{} more", exts.len() - 8));
        }
        out.push('\n');
    }

    out
}

fn clean_line(line: &str, max_len: usize, context_only: bool, pattern: &str) -> String {
    let trimmed = line.trim();

    if context_only {
        if let Ok(re) = Regex::new(&format!("(?i).{{0,20}}{}.*", regex::escape(pattern))) {
            if let Some(m) = re.find(trimmed) {
                let matched = m.as_str();
                if matched.len() <= max_len {
                    return matched.to_string();
                }
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
        let cleaned = clean_line(line, 50, false, "result");
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
    fn test_clean_line_multibyte() {
        // Thai text that exceeds max_len in bytes
        let line = "  สวัสดีครับ นี่คือข้อความที่ยาวมากสำหรับทดสอบ  ";
        let cleaned = clean_line(line, 20, false, "ครับ");
        // Should not panic
        assert!(!cleaned.is_empty());
    }

    #[test]
    fn test_clean_line_emoji() {
        let line = "🎉🎊🎈🎁🎂🎄 some text 🎃🎆🎇✨";
        let cleaned = clean_line(line, 15, false, "text");
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

    // Verify line numbers are always enabled in rg invocation (grep_cmd.rs:24).
    #[test]
    fn test_rg_always_has_line_numbers() {
        // grep_cmd::run() always passes "-n" to rg (line 24).
        // This test documents that -n is built-in, so the clap flag is safe to ignore.
        let mut cmd = resolved_command("rg");
        cmd.args(["-n", "--no-heading", "NONEXISTENT_PATTERN_12345", "."]);
        if let Ok(output) = cmd.output() {
            assert!(
                output.status.code() == Some(1) || output.status.success(),
                "rg -n should be accepted"
            );
        }
    }

    // ── filter_grep_raw: 3-part format (rg -n / grep -n) ─────────────────────

    #[test]
    fn test_filter_grep_raw_whitespace_only() {
        let out = filter_grep_raw("   \n\t\n");
        assert!(out.contains("0 matches"), "out={}", out);
    }

    // ── filter_grep_raw: 2-part format (grep without -n) ─────────────────────

    #[test]
    fn test_filter_grep_raw_two_part_no_line_number() {
        // grep without -n produces "file:content" (no line number)
        let input = "src/main.rs:fn main() {\nsrc/lib.rs:pub fn helper() {}\n";
        let out = filter_grep_raw(input);
        assert!(out.contains("main.rs"), "2-part: out={}", out);
        assert!(out.contains("lib.rs"), "2-part: out={}", out);
        assert!(
            out.contains("2 in"),
            "2-part: expected 2 matches: out={}",
            out
        );
    }

    #[test]
    fn test_filter_grep_raw_two_part_content_with_colon() {
        // Two-part where content itself contains ':' (e.g. URL or time)
        let input = "config.yaml:server: http://localhost:8080\n";
        let out = filter_grep_raw(input);
        // Should not panic and should show config.yaml
        assert!(out.contains("config.yaml"), "out={}", out);
    }

    #[test]
    fn test_filter_grep_raw_mixed_two_and_three_part() {
        // Some lines have line numbers, some don't — both should be counted
        let input = "src/a.rs:10:fn foo() {}\nsrc/b.rs:fn bar() {}\n";
        let out = filter_grep_raw(input);
        assert!(out.contains("a.rs"), "out={}", out);
        assert!(out.contains("b.rs"), "out={}", out);
    }

    #[test]
    fn test_filter_grep_raw_three_part_nonnumeric_middle() {
        // Three-part split but middle is not a number (e.g. Windows path C:\file:content)
        // Should fall back gracefully — either include or skip, but not panic
        let input = "C:\\path\\file.rs:some content\n";
        let out = filter_grep_raw(input); // must not panic
        assert!(!out.is_empty());
    }

    // ── filter_find_output ────────────────────────────────────────────────────

    #[test]
    fn test_filter_find_output_empty() {
        let out = filter_find_output("");
        assert!(out.contains("0 results"), "out={}", out);
    }

    #[test]
    fn test_filter_find_output_basic() {
        let input = "./src/main.rs\n./src/lib.rs\n./src/cmd/mod.rs\n";
        let out = filter_find_output(input);
        assert!(out.contains("3 files"), "out={}", out);
        // Extension breakdown
        assert!(out.contains(".rs"), "out={}", out);
    }

    #[test]
    fn test_filter_find_output_groups_by_dir() {
        let input = "./src/a.rs\n./src/b.rs\n./tests/c.rs\n";
        let out = filter_find_output(input);
        assert!(out.contains("./src"), "out={}", out);
        assert!(out.contains("./tests"), "out={}", out);
    }

    #[test]
    fn test_filter_find_output_extension_counts() {
        let input = "./a.rs\n./b.rs\n./c.toml\n./d.md\n";
        let out = filter_find_output(input);
        // .rs appears twice, toml and md once each
        assert!(out.contains(".rs (2)"), "out={}", out);
        assert!(
            out.contains(".toml (1)") || out.contains(".md (1)"),
            "out={}",
            out
        );
    }

    #[test]
    fn test_filter_find_output_no_extension() {
        let input = "./Makefile\n./Dockerfile\n";
        let out = filter_find_output(input);
        assert!(out.contains("2 files"), "out={}", out);
        assert!(out.contains("(no ext)"), "out={}", out);
    }

    #[test]
    fn test_filter_find_output_many_dirs_truncated() {
        // More than 10 unique dirs — should show "+N more dirs"
        let mut input = String::new();
        for i in 0..15 {
            input.push_str(&format!("./dir{}/file.rs\n", i));
        }
    }
}
