//! Filters Nix command output (nix build, nix develop, nix flake, nix-build, nix-shell, etc.).
//!
//! Nix commands produce extremely verbose output: download progress, hash prefixes,
//! store path copies, and evaluation traces. This filter compresses that noise while
//! preserving errors, warnings, and final build results.

use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    /// Matches nix store paths: /nix/store/<hash>-<name>
    static ref STORE_PATH_RE: Regex =
        Regex::new(r"/nix/store/[a-z0-9]{32}-").unwrap();

    /// Matches download/copy progress lines
    static ref PROGRESS_RE: Regex =
        Regex::new(r"(?i)^(copying path|copying file|downloading|fetching|unpacking|building derivation|querying|these \d+ paths|these derivations)").unwrap();

    /// Matches Nix evaluation trace lines
    static ref TRACE_RE: Regex =
        Regex::new(r"^\s*(trace|evaluating file|while evaluating|instantiated)").unwrap();

    /// Matches "this derivation will be built:" / "these paths will be fetched:" header lines
    static ref PLAN_HEADER_RE: Regex =
        Regex::new(r"^(this derivation will be built|these \d+ derivations will be built|these paths will be fetched|this path will be fetched)").unwrap();

    /// Matches store path list items (indented /nix/store/... lines)
    static ref STORE_LIST_RE: Regex =
        Regex::new(r"^\s+/nix/store/").unwrap();

    /// Matches build phase markers
    static ref PHASE_RE: Regex =
        Regex::new(r"^(building|configuring|installing|post-installation|patching|@nix \{)").unwrap();

    /// Matches warning lines
    static ref WARNING_RE: Regex =
        Regex::new(r"(?i)^(warning|warn):").unwrap();

    /// Matches error lines
    static ref ERROR_RE: Regex =
        Regex::new(r"(?i)^(error|ERROR):").unwrap();
}

/// Run a `nix` subcommand with filtered output.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("nix");

    for arg in args {
        cmd.arg(arg);
    }

    let args_display = args.join(" ");

    if verbose > 0 {
        eprintln!("Running: nix {}", args_display);
    }

    runner::run_filtered(
        cmd,
        "nix",
        &args_display,
        filter_nix_output,
        runner::RunOptions::default(),
    )
}

/// Run a legacy nix command (nix-build, nix-shell, nix-env, etc.) with filtered output.
pub fn run_legacy(tool: &str, args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command(tool);

    for arg in args {
        cmd.arg(arg);
    }

    let args_display = args.join(" ");

    if verbose > 0 {
        eprintln!("Running: {} {}", tool, args_display);
    }

    runner::run_filtered(
        cmd,
        tool,
        &args_display,
        filter_nix_output,
        runner::RunOptions::default(),
    )
}

/// Filter nix output: strip download noise, compress store paths, keep errors/warnings/results.
fn filter_nix_output(output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }

    let mut result_lines: Vec<String> = Vec::new();
    let mut download_count: usize = 0;
    let mut copy_count: usize = 0;
    let mut store_list_count: usize = 0;
    let mut in_store_list = false;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Always keep error lines
        if ERROR_RE.is_match(trimmed) {
            flush_counters(
                &mut result_lines,
                &mut download_count,
                &mut copy_count,
                &mut store_list_count,
                &mut in_store_list,
            );
            result_lines.push(trimmed.to_string());
            continue;
        }

        // Always keep warning lines
        if WARNING_RE.is_match(trimmed) {
            flush_counters(
                &mut result_lines,
                &mut download_count,
                &mut copy_count,
                &mut store_list_count,
                &mut in_store_list,
            );
            result_lines.push(trimmed.to_string());
            continue;
        }

        // Plan headers: keep but switch to store list counting mode
        if PLAN_HEADER_RE.is_match(trimmed) {
            flush_counters(
                &mut result_lines,
                &mut download_count,
                &mut copy_count,
                &mut store_list_count,
                &mut in_store_list,
            );
            result_lines.push(trimmed.to_string());
            in_store_list = true;
            continue;
        }

        // Store path list items: count instead of printing
        if STORE_LIST_RE.is_match(line) && in_store_list {
            store_list_count += 1;
            continue;
        }

        // End of store list
        if in_store_list && !STORE_LIST_RE.is_match(line) {
            flush_counters(
                &mut result_lines,
                &mut download_count,
                &mut copy_count,
                &mut store_list_count,
                &mut in_store_list,
            );
        }

        // Download/copy progress: count
        if trimmed.starts_with("copying path") || trimmed.starts_with("copying file") {
            copy_count += 1;
            continue;
        }

        if trimmed.starts_with("downloading '") || trimmed.starts_with("fetching ") {
            download_count += 1;
            continue;
        }

        // Skip evaluation traces
        if TRACE_RE.is_match(trimmed) {
            continue;
        }

        // Skip generic progress lines
        if PROGRESS_RE.is_match(trimmed) {
            continue;
        }

        // Compress store paths in remaining lines (shorten hash)
        let compressed = compress_store_paths(trimmed);
        result_lines.push(compressed);
    }

    // Flush any remaining counters
    flush_counters(
        &mut result_lines,
        &mut download_count,
        &mut copy_count,
        &mut store_list_count,
        &mut in_store_list,
    );

    // Truncate if still too long
    if result_lines.len() > 50 {
        let head: Vec<String> = result_lines[..25].to_vec();
        let tail: Vec<String> = result_lines[result_lines.len() - 20..].to_vec();
        let skipped = result_lines.len() - 45;
        let mut truncated = head;
        truncated.push(format!("... ({} lines omitted)", skipped));
        truncated.extend(tail);
        result_lines = truncated;
    }

    result_lines.join("\n")
}

/// Flush accumulated counters into summary lines.
fn flush_counters(
    lines: &mut Vec<String>,
    download_count: &mut usize,
    copy_count: &mut usize,
    store_list_count: &mut usize,
    in_store_list: &mut bool,
) {
    if *store_list_count > 0 {
        lines.push(format!("  ({} store paths)", store_list_count));
        *store_list_count = 0;
    }
    *in_store_list = false;

    if *download_count > 0 {
        lines.push(format!("[downloaded {} paths]", download_count));
        *download_count = 0;
    }
    if *copy_count > 0 {
        lines.push(format!("[copied {} paths]", copy_count));
        *copy_count = 0;
    }
}

/// Shorten /nix/store/<32-char-hash>-name to /nix/store/<7-char>…-name
fn compress_store_paths(line: &str) -> String {
    STORE_PATH_RE
        .replace_all(line, |caps: &regex::Captures<'_>| {
            let matched = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            // /nix/store/ = 11 chars, hash = 32 chars, then -
            if matched.len() >= 18 {
                format!("/nix/store/{}...-", &matched[11..18])
            } else {
                matched.to_string()
            }
        })
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_empty() {
        assert_eq!(filter_nix_output(""), "");
        assert_eq!(filter_nix_output("   "), "");
    }

    #[test]
    fn test_filter_preserves_errors() {
        let input = "error: attribute 'foo' not found\nsome noise\n";
        let output = filter_nix_output(input);
        assert!(output.contains("error: attribute 'foo' not found"));
    }

    #[test]
    fn test_filter_preserves_warnings() {
        let input = "warning: Git tree is dirty\ncopying path /nix/store/abc123\n";
        let output = filter_nix_output(input);
        assert!(output.contains("warning: Git tree is dirty"));
    }

    #[test]
    fn test_filter_compresses_downloads() {
        let input = "\
downloading 'https://cache.nixos.org/nar/abc.nar.xz'
downloading 'https://cache.nixos.org/nar/def.nar.xz'
downloading 'https://cache.nixos.org/nar/ghi.nar.xz'
error: build failed";
        let output = filter_nix_output(input);
        assert!(output.contains("[downloaded 3 paths]"));
        assert!(output.contains("error: build failed"));
        assert!(!output.contains("cache.nixos.org"));
    }

    #[test]
    fn test_filter_compresses_copies() {
        let input = "\
copying path '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-foo' from 'https://cache.nixos.org'
copying path '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa2-bar' from 'https://cache.nixos.org'
copying path '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa3-baz' from 'https://cache.nixos.org'";
        let output = filter_nix_output(input);
        assert!(output.contains("[copied 3 paths]"));
        assert!(!output.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    }

    #[test]
    fn test_filter_compresses_store_paths_inline() {
        let line = "building /nix/store/abcdefghijklmnopqrstuvwxyz012345-hello-2.12.1.drv";
        let output = compress_store_paths(line);
        assert!(output.contains("/nix/store/abcdefg...-"));
        assert!(output.contains("hello-2.12.1.drv"));
    }

    #[test]
    fn test_filter_store_list() {
        let input = "\
these 5 derivations will be built:
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-foo.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa2-bar.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa3-baz.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa4-qux.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa5-quux.drv
building derivation foo";
        let output = filter_nix_output(input);
        assert!(output.contains("these 5 derivations will be built:"));
        assert!(output.contains("(5 store paths)"));
        assert!(!output.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    }

    #[test]
    fn test_token_savings() {
        let input = "\
these 10 derivations will be built:
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-dep-a.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa2-dep-b.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa3-dep-c.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa4-dep-d.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa5-dep-e.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa6-dep-f.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa7-dep-g.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa8-dep-h.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa9-dep-i.drv
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa10-dep-j.drv
copying path '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-foo' from 'https://cache.nixos.org'
copying path '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa2-bar' from 'https://cache.nixos.org'
copying path '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa3-baz' from 'https://cache.nixos.org'
copying path '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa4-qux' from 'https://cache.nixos.org'
copying path '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa5-quux' from 'https://cache.nixos.org'
downloading 'https://cache.nixos.org/nar/abc.nar.xz'
downloading 'https://cache.nixos.org/nar/def.nar.xz'
downloading 'https://cache.nixos.org/nar/ghi.nar.xz'
downloading 'https://cache.nixos.org/nar/jkl.nar.xz'
building /nix/store/abcdefghijklmnopqrstuvwxyz012345-hello-2.12.1.drv
warning: Git tree is dirty
error: builder for '/nix/store/abcdefghijklmnopqrstuvwxyz012345-hello-2.12.1.drv' failed with exit code 1";

        let output = filter_nix_output(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);

        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Nix filter: expected >=60% savings, got {:.1}% (in={}, out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_skips_traces() {
        let input = "\
trace: value is 42
evaluating file '/nix/store/abc-source/default.nix'
while evaluating the attribute 'buildInputs'
error: something broke";
        let output = filter_nix_output(input);
        assert!(!output.contains("trace:"));
        assert!(!output.contains("evaluating file"));
        assert!(output.contains("error: something broke"));
    }
}
