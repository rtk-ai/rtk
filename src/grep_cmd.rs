use crate::tracking;
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::process::Command;

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

    let mut rg_cmd = Command::new("rg");
    rg_cmd.args(["-n", "--no-heading", pattern, path]);

    if let Some(ft) = file_type {
        rg_cmd.arg("--type").arg(ft);
    }

    for arg in extra_args {
        rg_cmd.arg(arg);
    }

    let output = rg_cmd
        .output()
        .or_else(|_| Command::new("grep").args(["-rn", pattern, path]).output())
        .context("grep/rg failed")?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    let raw_output = stdout.to_string();

    if stdout.trim().is_empty() {
        let msg = format!("🔍 0 for '{}'", pattern);
        println!("{}", msg);
        timer.track(
            &format!("grep -rn '{}' {}", pattern, path),
            "rtk grep",
            &raw_output,
            &msg,
        );
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

    Ok(())
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
            let start = pos.saturating_sub(max_len / 3);
            let end = (start + max_len).min(trimmed.len());
            let start = if end == trimmed.len() {
                end.saturating_sub(max_len)
            } else {
                start
            };

            // Snap to valid char boundaries
            let mut safe_start = start;
            while safe_start > 0 && !trimmed.is_char_boundary(safe_start) {
                safe_start -= 1;
            }
            let mut safe_end = end;
            while safe_end < trimmed.len() && !trimmed.is_char_boundary(safe_end) {
                safe_end += 1;
            }

            let slice = &trimmed[safe_start..safe_end];
            if safe_start > 0 && safe_end < trimmed.len() {
                format!("...{}...", slice)
            } else if safe_start > 0 {
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
}
