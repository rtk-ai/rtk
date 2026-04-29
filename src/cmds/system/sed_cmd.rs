//! Runs sed and compacts large stdout while preserving native behavior.

use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::{resolved_command, truncate};
use anyhow::Result;

const MAX_UNFILTERED_LINES: usize = 120;
const HEAD_LINES: usize = 60;
const TAIL_LINES: usize = 40;
const MAX_LINE_CHARS: usize = 200;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Running: sed {}", args.join(" "));
    }

    let mut cmd = resolved_command("sed");
    for arg in args {
        cmd.arg(arg);
    }

    let result = exec_capture(&mut cmd)?;
    let filtered_stdout = compact_sed_output(&result.stdout);

    print!("{}", filtered_stdout);
    eprint!("{}", result.stderr);

    let filtered_combined = format!("{}{}", filtered_stdout, result.stderr);
    timer.track(
        &format!("sed {}", args.join(" ")),
        "rtk sed",
        &result.combined(),
        &filtered_combined,
    );

    Ok(result.exit_code)
}

fn compact_sed_output(output: &str) -> String {
    if output.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = output.lines().collect();
    let has_trailing_newline = output.ends_with('\n');
    let has_long_lines = lines
        .iter()
        .any(|line| line.chars().count() > MAX_LINE_CHARS);

    if lines.len() <= MAX_UNFILTERED_LINES && !has_long_lines {
        return output.to_string();
    }

    let mut compacted = Vec::new();
    let head_count = HEAD_LINES.min(lines.len());
    let tail_count = TAIL_LINES.min(lines.len().saturating_sub(head_count));
    let omitted = lines.len().saturating_sub(head_count + tail_count);

    for line in lines.iter().take(head_count) {
        compacted.push(truncate(line, MAX_LINE_CHARS));
    }

    if omitted > 0 {
        compacted.push(format!("... {} lines omitted ...", omitted));
    }

    if tail_count > 0 {
        let tail_start = lines.len() - tail_count;
        for line in &lines[tail_start..] {
            compacted.push(truncate(line, MAX_LINE_CHARS));
        }
    }

    let mut result = compacted.join("\n");
    if has_trailing_newline {
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    #[test]
    fn small_output_is_unchanged() {
        let input = "one\ntwo\nthree\n";
        assert_eq!(compact_sed_output(input), input);
    }

    #[test]
    fn large_output_keeps_head_tail_and_marker() {
        let input = numbered_lines(150);
        let output = compact_sed_output(&input);

        assert!(output.starts_with("line 1\nline 2\n"));
        assert!(output.contains("... 50 lines omitted ..."));
        assert!(output.contains("line 111\n"));
        assert!(output.ends_with("line 150\n"));
        assert!(!output.contains("line 80\n"));
    }

    #[test]
    fn long_lines_are_truncated() {
        let input = format!("{}\nshort\n", "x".repeat(260));
        let output = compact_sed_output(&input);
        let first_line = output.lines().next().unwrap();

        assert_eq!(first_line.chars().count(), MAX_LINE_CHARS);
        assert!(first_line.ends_with("..."));
        assert!(output.ends_with("short\n"));
    }

    #[test]
    fn empty_output_stays_empty() {
        assert_eq!(compact_sed_output(""), "");
    }

    #[test]
    fn large_output_reaches_token_savings_target() {
        let input = numbered_lines(500);
        let output = compact_sed_output(&input);
        let savings =
            100.0 - (count_tokens(&output) as f64 / count_tokens(&input) as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "Expected >=60% savings, got {:.1}%",
            savings
        );
    }

    fn numbered_lines(count: usize) -> String {
        (1..=count)
            .map(|n| format!("line {}", n))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }
}
