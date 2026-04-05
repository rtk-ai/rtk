//! Filters Nix command output (nix build, nix develop, nix flake, nix-build, nix-shell, etc.).
//!
//! Nix commands produce extremely verbose output: download progress, hash prefixes,
//! store path copies, and evaluation traces. This filter compresses that noise while
//! preserving errors, warnings, and final build results.

use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

/// Matches nix store paths: /nix/store/<hash>-<name>
static STORE_PATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/nix/store/[a-z0-9]{32}-").unwrap());

/// Matches download/copy progress lines
static PROGRESS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(copying path|copying file|downloading|fetching|unpacking|building derivation|querying|these \d+ paths|these derivations)").unwrap()
});

/// Matches Nix evaluation trace lines
static TRACE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(trace|evaluating file|while evaluating|instantiated)").unwrap()
});

/// Matches "this derivation will be built:" / "these paths will be fetched:" header lines
static PLAN_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(this derivation will be built|these \d+ derivations will be built|these \d+ paths will be fetched|these paths will be fetched|this path will be fetched)").unwrap()
});

/// Matches store path list items (indented /nix/store/... lines)
static STORE_LIST_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s+/nix/store/").unwrap());

/// Matches warning lines
static WARNING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(warning|warn):").unwrap());

/// Matches error lines
static ERROR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^error:").unwrap());

/// Run a `nix` subcommand with filtered output.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    run_tool("nix", args, verbose)
}

/// Run a legacy nix command (nix-build, nix-shell, nix-env) with filtered output.
pub fn run_legacy(tool: &str, args: &[String], verbose: u8) -> Result<i32> {
    run_tool(tool, args, verbose)
}

fn run_tool(tool: &str, args: &[String], verbose: u8) -> Result<i32> {
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
    let mut counters = Counters::new();

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Always keep error lines (with store paths compressed)
        if ERROR_RE.is_match(trimmed) {
            counters.flush(&mut result_lines);
            result_lines.push(compress_store_paths(trimmed));
            continue;
        }

        // Always keep warning lines (with store paths compressed)
        if WARNING_RE.is_match(trimmed) {
            counters.flush(&mut result_lines);
            result_lines.push(compress_store_paths(trimmed));
            continue;
        }

        // Plan headers: keep but switch to store list counting mode
        if PLAN_HEADER_RE.is_match(trimmed) {
            counters.flush(&mut result_lines);
            result_lines.push(trimmed.to_string());
            counters.in_store_list = true;
            continue;
        }

        // Store path list items: count instead of printing
        if STORE_LIST_RE.is_match(line) && counters.in_store_list {
            counters.store_list += 1;
            continue;
        }

        // End of store list
        if counters.in_store_list && !STORE_LIST_RE.is_match(line) {
            counters.flush(&mut result_lines);
        }

        // Download/copy progress: count
        if trimmed.starts_with("copying path") || trimmed.starts_with("copying file") {
            counters.copies += 1;
            continue;
        }

        if trimmed.starts_with("downloading '") || trimmed.starts_with("fetching ") {
            counters.downloads += 1;
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
    counters.flush(&mut result_lines);

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

/// Accumulated progress counters, flushed into summary lines when interrupted.
struct Counters {
    downloads: usize,
    copies: usize,
    store_list: usize,
    in_store_list: bool,
}

impl Counters {
    fn new() -> Self {
        Self {
            downloads: 0,
            copies: 0,
            store_list: 0,
            in_store_list: false,
        }
    }

    fn flush(&mut self, lines: &mut Vec<String>) {
        if self.store_list > 0 {
            lines.push(format!("  ({} paths)", self.store_list));
            self.store_list = 0;
        }
        self.in_store_list = false;

        if self.downloads > 0 {
            lines.push(format!("[downloaded {}]", self.downloads));
            self.downloads = 0;
        }
        if self.copies > 0 {
            lines.push(format!("[copied {}]", self.copies));
            self.copies = 0;
        }
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
        assert!(output.contains("[downloaded 3]"));
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
        assert!(output.contains("[copied 3]"));
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
        assert!(output.contains("(5 paths)"));
        assert!(!output.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    }

    #[test]
    fn test_filter_fetch_plan_list() {
        // Real nix fetch plans carry size annotations; the header must be kept
        // and its store paths counted instead of dropped by progress filtering.
        let input = "\
these 3 paths will be fetched (12.40 MiB download, 31.55 MiB unpacked):
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-python3-3.12.4
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa2-setuptools
  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa3-pip";
        let output = filter_nix_output(input);
        assert!(output.contains("these 3 paths will be fetched"));
        assert!(output.contains("(3 paths)"));
        assert!(!output.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    }

    #[test]
    fn test_error_lines_get_hash_compressed() {
        let input = "error: builder for '/nix/store/abcdefghijklmnopqrstuvwxyz012345-hello-2.12.1.drv' failed with exit code 1";
        let output = filter_nix_output(input);
        assert!(output.contains("error: builder for '/nix/store/abcdefg...-hello-2.12.1.drv'"));
        assert!(!output.contains("abcdefghijklmnopqrstuvwxyz012345"));
    }

    #[test]
    fn test_warning_lines_get_hash_compressed() {
        let input = "warning: cannot talk to '/nix/store/abcdefghijklmnopqrstuvwxyz012345-bad': No such file";
        let output = filter_nix_output(input);
        assert!(output.contains("/nix/store/abcdefg...-bad"));
        assert!(!output.contains("abcdefghijklmnopqrstuvwxyz012345"));
    }

    #[test]
    fn test_severity_matching_is_case_insensitive() {
        let input = "Error: attribute 'foo' missing\nWARNING: disk nearly full\n";
        let output = filter_nix_output(input);
        assert!(output.contains("Error: attribute 'foo' missing"));
        assert!(output.contains("WARNING: disk nearly full"));
    }

    #[test]
    fn test_flush_order_downloads_before_copies() {
        let input = "\
downloading 'https://cache.nixos.org/nar/a.nar.xz'
copying path '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-x' from 'https://cache.nixos.org'
error: build failed";
        let output = filter_nix_output(input);
        let dl = output.find("[downloaded 1]").expect("download summary");
        let cp = output.find("[copied 1]").expect("copy summary");
        let err = output.find("error: build failed").expect("error line");
        assert!(
            dl < cp && cp < err,
            "expected downloads, copies, then error; got:\n{output}"
        );
    }

    #[test]
    fn test_no_truncation_at_threshold() {
        // Exactly 50 surviving lines must pass through untruncated.
        let mut input = String::new();
        for i in 0..50 {
            input.push_str(&format!("building /nix/store/{i:032}-pkg-{i}.drv\n"));
        }
        let output = filter_nix_output(&input);
        assert_eq!(output.lines().count(), 50);
        assert!(!output.contains("lines omitted"));
    }

    #[test]
    fn test_truncates_very_long_output() {
        // 60 surviving lines -> 25 head + omission marker + 20 tail.
        let mut input = String::new();
        for i in 0..60 {
            input.push_str(&format!("building /nix/store/{i:032}-pkg-{i}.drv\n"));
        }
        let output = filter_nix_output(&input);
        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 46);
        assert_eq!(out_lines[25], "... (15 lines omitted)");
        assert!(output.ends_with("pkg-59.drv"), "tail must keep final line");
    }

    #[test]
    fn test_token_savings() {
        // Representative failed `nix build` log: eval traces, build plan, fetch plan,
        // cache transfer progress with size annotations, warning, and error tail.
        let input = "\
evaluating file '/home/user/proj/flake.nix'
trace: Flattening flake input 'nixpkgs'
trace: checking outdated outputs
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
these 4 paths will be fetched (12.40 MiB download, 31.55 MiB unpacked):
  /nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb1-python3-3.12.4
  /nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb2-setuptools
  /nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb3-pip
  /nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb4-wheel
copying path '/nix/store/ccccccccccccccccccccccccccccccc1-foo' from 'https://cache.nixos.org'
copying path '/nix/store/ccccccccccccccccccccccccccccccc2-bar' from 'https://cache.nixos.org'
copying path '/nix/store/ccccccccccccccccccccccccccccccc3-baz' from 'https://cache.nixos.org'
copying path '/nix/store/ccccccccccccccccccccccccccccccc4-qux' from 'https://cache.nixos.org'
copying path '/nix/store/ccccccccccccccccccccccccccccccc5-quux' from 'https://cache.nixos.org'
downloading 'https://cache.nixos.org/nar/abc.nar.xz'... 8.50 MiB / 24.30 MiB
downloading 'https://cache.nixos.org/nar/def.nar.xz'... 3.10 MiB / 9.80 MiB
downloading 'https://cache.nixos.org/nar/ghi.nar.xz'... 0.40 MiB / 1.20 MiB
downloading 'https://cache.nixos.org/nar/jkl.nar.xz'... 5.20 MiB / 11.70 MiB
unpacking 'https://cache.nixos.org/nar/jkl.nar.xz'...
building '/nix/store/dddddddddddddddddddddddddddddddd1-hello-2.12.1.drv'...
warning: Git tree is dirty
error: builder for '/nix/store/dddddddddddddddddddddddddddddddd1-hello-2.12.1.drv^out' failed with exit code 1;
       make: *** [Makefile:42: hello.o] Error 1";

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
