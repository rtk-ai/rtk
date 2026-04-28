//! Proximity-based regex extraction for minified files.
//!
//! `rtk grep` is line-oriented and useless on single-line minified bundles.
//! This module extracts a configurable character window around each regex
//! match instead, with optional secondary-keyword filtering.

use crate::core::tracking;
use anyhow::{Context, Result};
use regex::RegexBuilder;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", "build"];

#[derive(Clone)]
struct Window {
    offset: usize,
    text: String,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    pattern: &str,
    path: &str,
    before: usize,
    after: usize,
    require: &[String],
    ignore_case: bool,
    max_results: usize,
    dedupe: bool,
    verbose: u8,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!(
            "extract: '{}' in {} (before={}, after={}, require={:?})",
            pattern, path, before, after, require
        );
    }

    let re = RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .with_context(|| format!("invalid regex: {pattern}"))?;

    let raw_cmd = format!("extract '{pattern}' {path}");
    let files = collect_files(Path::new(path))?;

    let mut by_file: BTreeMap<String, Vec<Window>> = BTreeMap::new();
    let mut total_matches = 0usize;
    let mut filtered = 0usize;
    let mut emitted = 0usize;
    let mut input_bytes = 0usize;

    'outer: for file in &files {
        let content = match fs::read_to_string(file) {
            Ok(c) => c,
            Err(_) => continue, // binary / unreadable — skip silently
        };
        input_bytes = input_bytes.saturating_add(content.len());

        for m in re.find_iter(&content) {
            total_matches += 1;
            let win = slice_window(&content, m.start(), m.end(), before, after);
            if !passes_require(&win, require, ignore_case) {
                filtered += 1;
                continue;
            }
            by_file
                .entry(file.display().to_string())
                .or_default()
                .push(Window {
                    offset: m.start(),
                    text: win,
                });
            emitted += 1;
            if emitted >= max_results {
                break 'outer;
            }
        }
    }

    let output = format_output(&by_file, total_matches, filtered, emitted, dedupe);
    print!("{output}");

    // estimate_tokens reads only .len(); use a length-proxy to avoid holding
    // every file's bytes in memory simultaneously.
    let raw_proxy: String = " ".repeat(input_bytes);
    timer.track(&raw_cmd, "rtk extract", &raw_proxy, &output);

    Ok(if total_matches == 0 { 1 } else { 0 })
}

fn collect_files(path: &Path) -> Result<Vec<PathBuf>> {
    let meta = fs::metadata(path).with_context(|| format!("cannot stat {}", path.display()))?;
    if meta.is_file() {
        if meta.len() > MAX_FILE_BYTES {
            anyhow::bail!(
                "{} is {} bytes (>5MB cap). Streaming for large files is not yet supported.",
                path.display(),
                meta.len()
            );
        }
        return Ok(vec![path.to_path_buf()]);
    }

    let mut out = Vec::new();
    for entry in WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| !is_skip_dir(e.file_name().to_str()))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
            continue; // silently skip oversize files in dir walks
        }
        out.push(entry.into_path());
    }
    Ok(out)
}

fn is_skip_dir(name: Option<&str>) -> bool {
    match name {
        Some(n) => SKIP_DIRS.contains(&n),
        None => false,
    }
}

/// Char-boundary-safe window extraction.
///
/// Returns a string with `before` chars to the left of the match,
/// the match wrapped in `«…»`, and `after` chars to the right.
/// Newlines inside the window are replaced with `␤` so the output
/// stays one logical line.
fn slice_window(s: &str, start: usize, end: usize, before: usize, after: usize) -> String {
    // Walk char boundaries to find safe slice points.
    let left_byte = back_chars(s, start, before);
    let right_byte = forward_chars(s, end, after);

    let left = &s[left_byte..start];
    let mid = &s[start..end];
    let right = &s[end..right_byte];

    let mut out = String::with_capacity(left.len() + mid.len() + right.len() + 4);
    push_oneline(&mut out, left);
    out.push('«');
    push_oneline(&mut out, mid);
    out.push('»');
    push_oneline(&mut out, right);
    out
}

fn push_oneline(out: &mut String, s: &str) {
    for ch in s.chars() {
        if ch == '\n' || ch == '\r' {
            out.push('␤');
        } else {
            out.push(ch);
        }
    }
}

/// Return the byte offset `n_chars` before `start`, snapping to a char boundary.
fn back_chars(s: &str, start: usize, n_chars: usize) -> usize {
    let mut idx = start;
    for _ in 0..n_chars {
        if idx == 0 {
            break;
        }
        idx -= 1;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
    }
    idx
}

/// Return the byte offset `n_chars` after `end`, snapping to a char boundary.
fn forward_chars(s: &str, end: usize, n_chars: usize) -> usize {
    let len = s.len();
    let mut idx = end;
    for _ in 0..n_chars {
        if idx >= len {
            break;
        }
        idx += 1;
        while idx < len && !s.is_char_boundary(idx) {
            idx += 1;
        }
    }
    idx
}

fn passes_require(window: &str, needles: &[String], ignore_case: bool) -> bool {
    if needles.is_empty() {
        return true;
    }
    if ignore_case {
        let lower = window.to_lowercase();
        needles.iter().all(|n| lower.contains(&n.to_lowercase()))
    } else {
        needles.iter().all(|n| window.contains(n.as_str()))
    }
}

fn format_output(
    by_file: &BTreeMap<String, Vec<Window>>,
    total_matches: usize,
    filtered: usize,
    emitted: usize,
    dedupe: bool,
) -> String {
    let mut out = String::new();

    if emitted == 0 {
        out.push_str(&format!(
            "0 windows shown (of {total_matches} matches, {filtered} filtered)\n"
        ));
        return out;
    }

    for (file, windows) in by_file {
        out.push_str(&format!("{} ({} matches)\n", file, windows.len()));
        if dedupe {
            let mut seen: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
            // Preserve first-occurrence order via Vec while counting via map.
            let mut order: Vec<&str> = Vec::new();
            for w in windows {
                seen.entry(w.text.as_str())
                    .and_modify(|(c, _)| *c += 1)
                    .or_insert_with(|| {
                        order.push(w.text.as_str());
                        (1, w.offset)
                    });
            }
            for text in order {
                let (count, offset) = seen[text];
                if count > 1 {
                    out.push_str(&format!("  @{offset}: {text} (x{count})\n"));
                } else {
                    out.push_str(&format!("  @{offset}: {text}\n"));
                }
            }
        } else {
            for w in windows {
                out.push_str(&format!("  @{}: {}\n", w.offset, w.text));
            }
        }
    }

    let file_count = by_file.len();
    out.push_str(&format!(
        "{} {}, {} {} shown (of {} matches, {} filtered)\n",
        file_count,
        if file_count == 1 { "file" } else { "files" },
        emitted,
        if emitted == 1 { "window" } else { "windows" },
        total_matches,
        filtered,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn slice_window_basic() {
        let s = "AAAAAAAAAA needle BBBBBBBBBB";
        let start = s.find("needle").unwrap();
        let end = start + "needle".len();
        let win = slice_window(s, start, end, 5, 5);
        // 5 chars before the match → "AAAA " (includes the space at idx 10)
        // 5 chars after the match  → " BBBB" (space at idx 17 + four B's)
        assert_eq!(win, "AAAA «needle» BBBB");
    }

    #[test]
    fn slice_window_at_start_no_underflow() {
        let s = "needle here";
        let win = slice_window(s, 0, "needle".len(), 50, 50);
        assert_eq!(win, "«needle» here");
    }

    #[test]
    fn slice_window_at_end_no_overflow() {
        let s = "here needle";
        let start = s.find("needle").unwrap();
        let end = s.len();
        let win = slice_window(s, start, end, 50, 50);
        assert_eq!(win, "here «needle»");
    }

    #[test]
    fn slice_window_multibyte_no_panic() {
        // Japanese text: 3-byte chars. Indexing mid-codepoint would panic.
        let s = "前文ABCDE日本語FGHIJ後文";
        let start = s.find("日本語").unwrap();
        let end = start + "日本語".len();
        let win = slice_window(s, start, end, 4, 4);
        // Should snap to char boundaries; left 4 chars = "ABCDE"[1..]+"" — implementation
        // walks chars not bytes, so we just assert containment + no panic.
        assert!(win.contains("«日本語»"));
        assert!(win.contains("CDE"));
        assert!(win.contains("FGHI"));
    }

    #[test]
    fn slice_window_replaces_newlines() {
        let s = "alpha\nbeta needle gamma\ndelta";
        let start = s.find("needle").unwrap();
        let end = start + "needle".len();
        let win = slice_window(s, start, end, 100, 100);
        assert!(!win.contains('\n'));
        assert!(win.contains('␤'));
        assert!(win.contains("«needle»"));
    }

    #[test]
    fn passes_require_empty_always_true() {
        assert!(passes_require("anything", &[], false));
    }

    #[test]
    fn passes_require_and_logic() {
        let req = vec!["login".to_string(), "Ka".to_string()];
        assert!(passes_require("auth.Ka.login()", &req, false));
        assert!(!passes_require("auth.Ka.logout()", &req, false));
        assert!(!passes_require("auth.login()", &req, false));
    }

    #[test]
    fn passes_require_case_insensitive() {
        let req = vec!["LOGIN".to_string()];
        assert!(passes_require("/api/login", &req, true));
        assert!(!passes_require("/api/login", &req, false));
    }

    #[test]
    fn format_output_groups_by_file_and_summarises() {
        let mut by_file: BTreeMap<String, Vec<Window>> = BTreeMap::new();
        by_file.insert(
            "a.js".into(),
            vec![Window {
                offset: 10,
                text: "...«hit»...".into(),
            }],
        );
        by_file.insert(
            "b.js".into(),
            vec![
                Window {
                    offset: 20,
                    text: "...«hit»...".into(),
                },
                Window {
                    offset: 99,
                    text: "...«hit2»...".into(),
                },
            ],
        );
        let out = format_output(&by_file, 5, 2, 3, true);
        assert!(out.contains("a.js (1 matches)"));
        assert!(out.contains("b.js (2 matches)"));
        assert!(out.contains("@10: ...«hit»..."));
        assert!(out.contains("2 files, 3 windows shown (of 5 matches, 2 filtered)"));
    }

    #[test]
    fn format_output_zero_emitted_explains() {
        let by_file: BTreeMap<String, Vec<Window>> = BTreeMap::new();
        let out = format_output(&by_file, 4, 4, 0, true);
        assert_eq!(out, "0 windows shown (of 4 matches, 4 filtered)\n");
    }

    #[test]
    fn format_output_dedupe_collapses_identical_windows() {
        let mut by_file: BTreeMap<String, Vec<Window>> = BTreeMap::new();
        by_file.insert(
            "f.js".into(),
            vec![
                Window {
                    offset: 1,
                    text: "X".into(),
                },
                Window {
                    offset: 2,
                    text: "X".into(),
                },
                Window {
                    offset: 3,
                    text: "Y".into(),
                },
            ],
        );
        let out = format_output(&by_file, 3, 0, 3, true);
        // "X" appears once with (x2); "Y" appears once
        assert_eq!(out.matches("@1: X (x2)").count(), 1);
        assert_eq!(out.matches("@3: Y").count(), 1);
        assert!(!out.contains("@2: X"));
    }

    #[test]
    fn format_output_no_dedupe_keeps_all() {
        let mut by_file: BTreeMap<String, Vec<Window>> = BTreeMap::new();
        by_file.insert(
            "f.js".into(),
            vec![
                Window {
                    offset: 1,
                    text: "X".into(),
                },
                Window {
                    offset: 2,
                    text: "X".into(),
                },
            ],
        );
        let out = format_output(&by_file, 2, 0, 2, false);
        assert!(out.contains("@1: X"));
        assert!(out.contains("@2: X"));
        assert!(!out.contains("(x2)"));
    }

    #[test]
    fn invalid_regex_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let res = run("(unclosed", dir.path().to_str().unwrap(), 10, 10, &[], false, 100, true, 0);
        assert!(res.is_err(), "invalid regex must surface as Err, not panic");
    }

    #[test]
    fn no_matches_returns_exit_1() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("empty.txt");
        std::fs::write(&f, "no hits here").unwrap();
        let code = run(
            "WILL_NEVER_MATCH_XYZ",
            f.to_str().unwrap(),
            10,
            10,
            &[],
            false,
            100,
            true,
            0,
        )
        .unwrap();
        assert_eq!(code, 1);
    }

    #[test]
    fn matches_returns_exit_0() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t.txt");
        std::fs::write(&f, "found needle here").unwrap();
        let code = run(
            "needle",
            f.to_str().unwrap(),
            5,
            5,
            &[],
            false,
            100,
            true,
            0,
        )
        .unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn end_to_end_minified_savings_above_95pct() {
        // Synthetic 50KB minified blob with one needle.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("bundle.min.js");
        let mut file = std::fs::File::create(&f).unwrap();
        file.write_all("x".repeat(25_000).as_bytes()).unwrap();
        file.write_all(b"fetch('/api/v1/login')").unwrap();
        file.write_all("y".repeat(25_000).as_bytes()).unwrap();
        drop(file);

        // Reproduce the user-facing output via the helper layer (run() prints
        // to stdout which we can't capture in unit tests).
        let content = std::fs::read_to_string(&f).unwrap();
        let re = regex::Regex::new(r"fetch\([^)]{0,300}\)").unwrap();
        let m = re.find(&content).unwrap();
        let win = slice_window(&content, m.start(), m.end(), 80, 80);
        let mut by_file: BTreeMap<String, Vec<Window>> = BTreeMap::new();
        by_file.insert(
            f.display().to_string(),
            vec![Window {
                offset: m.start(),
                text: win,
            }],
        );
        let output = format_output(&by_file, 1, 0, 1, true);

        let input_tokens = count_tokens(&content).max(1);
        let output_tokens = count_tokens(&output).max(1);

        // The whole input is one giant whitespace-free blob → 1 token.
        // Output is small but contains a few words. Compare on bytes too:
        let byte_savings = 100.0 - (output.len() as f64 / content.len() as f64 * 100.0);
        assert!(
            byte_savings >= 95.0,
            "expected ≥95% byte savings, got {:.2}% (in={}, out={})",
            byte_savings,
            content.len(),
            output.len()
        );
        // Token-count savings cross-check (loose): output must not balloon.
        assert!(
            output_tokens < input_tokens * 50,
            "output tokens explosion: in={} out={}",
            input_tokens,
            output_tokens
        );
        // Sanity: window contains the actual hit
        assert!(output.contains("«fetch('/api/v1/login')»"));
    }

    #[test]
    fn collect_files_skips_oversize_in_dir_walk() {
        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("ok.txt");
        std::fs::write(&small, "hi").unwrap();
        let big = dir.path().join("big.bin");
        // 6 MB, just over the cap.
        let buf = vec![b'x'; (MAX_FILE_BYTES + 1024) as usize];
        std::fs::write(&big, &buf).unwrap();

        let files = collect_files(dir.path()).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"ok.txt".to_string()));
        assert!(!names.contains(&"big.bin".to_string()));
    }

    #[test]
    fn collect_files_oversize_single_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.bin");
        let buf = vec![b'x'; (MAX_FILE_BYTES + 1024) as usize];
        std::fs::write(&big, &buf).unwrap();
        let res = collect_files(&big);
        assert!(res.is_err(), "single oversized file must error explicitly");
    }
}
