//! Compact filter for `soffice`/`libreoffice` headless conversion.
//!
//! `soffice --headless --convert-to <fmt> ...` emits startup/profile noise
//! (javaldx warnings on Java-less hosts, macOS `Task policy` messages, safe-mode
//! banners) around a single line that actually matters:
//!
//! ```text
//! convert /in/deck.pptx as a Impress presentation -> /out/deck.pdf using filter : impress_pdf_Export
//! ```
//!
//! This keeps only that conversion result line and any genuine `Error:` line,
//! dropping the surrounding banner. soffice writes both the noise and its errors
//! to stderr and exits 0 even on failure, so we filter the combined stream (not
//! stdout only) to avoid hiding a failure. If nothing meaningful is matched, the
//! raw output is kept verbatim so unexpected output is never swallowed.
//!
//! Savings scale with how noisy the host is: on a Java-less headless Linux/WSL2
//! run (the reported case) the javaldx banner pushes this well past 60%; on a
//! quiet macOS run with few extra lines it is smaller. The `never_worse` guard in
//! the runner guarantees the filtered output is never larger than the raw output.
use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("soffice");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: soffice {}", args.join(" "));
    }

    // Filter the combined stream: soffice puts both banner noise and errors on
    // stderr, so stdout-only filtering would drop the strippable noise and, worse,
    // hide an error (soffice exits 0 even when conversion fails).
    runner::run_filtered(
        cmd,
        "soffice",
        &args.join(" "),
        filter_soffice_output,
        RunOptions::default(),
    )
}

/// Keep the `convert ... -> ...` result line(s) and any genuine `Error:` line;
/// drop startup/profile banner noise. Falls back to the raw output when nothing
/// is matched so unexpected output is never hidden.
fn filter_soffice_output(raw: &str) -> String {
    let kept: Vec<&str> = raw
        .lines()
        .map(str::trim_end)
        .filter(|line| is_meaningful(line))
        .collect();

    if kept.is_empty() {
        return raw.trim_end().to_string();
    }
    kept.join("\n")
}

/// A line worth keeping: the conversion result, or a genuine error.
///
/// Errors are matched by an `Error:` prefix (soffice's format), deliberately not
/// by substrings like "failed"/"could not", which also appear in the javaldx
/// banner noise we want to drop (`Warning: failed to read path from javaldx`,
/// `javaldx: Could not find a Java Runtime Environment!`).
fn is_meaningful(line: &str) -> bool {
    let l = line.trim();
    if l.is_empty() {
        return false;
    }
    if l.starts_with("convert ") && l.contains("->") {
        return true;
    }
    l.to_ascii_lowercase().starts_with("error")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    const CONVERT_LINE: &str = "convert /home/u/decks/report.pptx as a Impress presentation -> /home/u/out/report.pdf using filter : impress_pdf_Export";

    #[test]
    fn test_keeps_convert_line_drops_noise() {
        let raw = format!(
            "javaldx: Could not find a Java Runtime Environment!\n\
             Warning: failed to read path from javaldx\n\
             LibreOffice - Safe Mode disabled.\n\
             {CONVERT_LINE}\n"
        );
        let out = filter_soffice_output(&raw);
        assert_eq!(out, CONVERT_LINE);
    }

    #[test]
    fn test_macos_task_policy_noise_dropped() {
        // Real macOS LibreOffice 26.2.4 fresh-profile output.
        let raw = format!(
            "2026-07-21 18:02:28.275 soffice[20533:1531297] Task policy set failed: 4 ((os/kern) invalid argument)\n\
             {CONVERT_LINE}\n"
        );
        let out = filter_soffice_output(&raw);
        assert_eq!(out, CONVERT_LINE);
    }

    #[test]
    fn test_error_is_surfaced() {
        // soffice exits 0 on a load failure and prints this to stderr; it must
        // not be filtered away.
        let raw = "Error: source file could not be loaded\n";
        let out = filter_soffice_output(raw);
        assert_eq!(out, "Error: source file could not be loaded");
    }

    #[test]
    fn test_unexpected_output_falls_back_to_raw() {
        let raw = "some unfamiliar soffice diagnostic\nwith no result line\n";
        let out = filter_soffice_output(raw);
        assert_eq!(out, "some unfamiliar soffice diagnostic\nwith no result line");
    }

    #[test]
    fn test_empty() {
        assert_eq!(filter_soffice_output(""), "");
    }

    #[test]
    fn test_savings_on_javaless_run() {
        // Representative Java-less headless Linux/WSL2 run (the reported case).
        let raw = format!(
            "javaldx: Could not find a Java Runtime Environment!\n\
             Warning: failed to read path from javaldx\n\
             LibreOffice - Safe Mode disabled.\n\
             {CONVERT_LINE}\n"
        );
        let out = filter_soffice_output(&raw);
        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(&raw) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60% savings, got {savings:.1}%");
    }
}
