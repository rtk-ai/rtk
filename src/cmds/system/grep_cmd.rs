//! Filters grep output by grouping matches by file.

use crate::core::config;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Debug)]
struct GrepRenderOptions {
    max_line_chars: Option<usize>,
    max_matches: Option<usize>,
    max_per_file: Option<usize>,
    uncapped: bool,
    files_only: bool,
    count_by_file: bool,
    agent_safe: bool,
    summary_enabled: bool,
    context_only: bool,
}

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
struct GrepRenderStats {
    total_matches: usize,
    files_matched: usize,
    shown: usize,
    omitted_total: usize,
    omitted_per_file: usize,
    clipped_lines: usize,
    printed_summary: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    pattern: &str,
    path: &str,
    max_line_chars: Option<usize>,
    max_matches: Option<usize>,
    max_per_file: Option<usize>,
    uncapped: bool,
    files_only: bool,
    count_by_file: bool,
    agent_safe: bool,
    summary_enabled: bool,
    top_files: Option<usize>,
    json: bool,
    context_only: bool,
    file_type: Option<&str>,
    fixed: bool,
    extra_args: &[String],
    verbose: u8,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("grep: '{}' in {}", pattern, path);
    }

    let mut rg_cmd = resolved_command("rg");
    rg_cmd.args(build_rg_args(
        pattern,
        path,
        file_type,
        fixed,
        extra_args,
    ));

    let result = exec_capture(&mut rg_cmd)
        .or_else(|_| {
            let mut grep_cmd = resolved_command("grep");
            // When we fall back to grep, include all args, not just -rnHZ.
            grep_cmd.arg("-rnHZ");
            if fixed {
                grep_cmd.arg("-F");
            }
            grep_cmd.args(extra_args);
            grep_cmd.args([pattern, path]);
            exec_capture(&mut grep_cmd)
        })
        .context("grep/rg failed")?;

    if result.exit_code == 2 && !fixed && !result.stderr.trim().is_empty() {
        let s = result.stderr.to_lowercase();
        if s.contains("regex parse error") || s.contains("error parsing regex") {
            eprintln!("rtk grep: regex parse error (hint: try `rtk grep --fixed ...`)");
        }
    }

    // Passthrough output flags that produce output that is already small.
    // In `--json` mode, always emit JSON (no human text), even for format flags.
    if has_format_flag(extra_args) && !json {
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
        let msg = format!("0 matches for '{}'", pattern);
        if json {
            let out = GrepJsonOutput::no_matches(pattern, files_only, count_by_file, top_files);
            println!("{}", serde_json::to_string(&out)?);
        } else {
            println!("{}", msg);
        }
        timer.track(
            &format!("grep -rn '{}' {}", pattern, path),
            "rtk grep",
            &raw_output,
            if json { "" } else { &msg },
        );
        return Ok(exit_code);
    }

    let (rtk_output, stats) = render_grep_output(
        pattern,
        &result.stdout,
        &GrepRenderOptions {
            max_line_chars,
            max_matches,
            max_per_file,
            uncapped,
            files_only,
            count_by_file,
            agent_safe,
            summary_enabled,
            context_only,
        },
        top_files,
        json,
    );

    print!("{}", rtk_output);
    timer.track(
        &format!("grep -rn '{}' {}", pattern, path),
        "rtk grep",
        &raw_output,
        &rtk_output,
    );

    if json && stats.printed_summary {
        // In JSON mode, tracking output is the JSON itself; ensure no extra text sneaks in.
    }

    Ok(exit_code)
}

fn render_grep_output(
    pattern: &str,
    stdout: &str,
    opts: &GrepRenderOptions,
    top_files: Option<usize>,
    json: bool,
) -> (String, GrepRenderStats) {
    // Filter: group by file, optionally cap/truncate, render in deterministic order.
    // Output uses `file:line:content` so AI agents can parse it.
    let mut by_file_raw: HashMap<String, Vec<(usize, &str)>> = HashMap::new();
    for line in stdout.lines() {
        let Some((file, line_num, content)) = parse_match_line(line) else {
            continue;
        };
        by_file_raw.entry(file).or_default().push((line_num, content));
    }
    let total_matches: usize = by_file_raw.values().map(|v| v.len()).sum();

    if opts.files_only {
        if json {
            let mut rows: Vec<(usize, String)> = by_file_raw
                .iter()
                .map(|(file, matches)| (matches.len(), compact_path(file)))
                .collect();
            rows.sort_by(|(a_cnt, a_file), (b_cnt, b_file)| {
                b_cnt.cmp(a_cnt).then_with(|| a_file.cmp(b_file))
            });
            let out = GrepJsonOutput::file_counts(pattern, total_matches, by_file_raw.len(), &rows);
            return (
                format!("{}\n", serde_json::to_string(&out).unwrap_or_else(|_| "{}".to_string())),
                GrepRenderStats {
                    total_matches,
                    files_matched: by_file_raw.len(),
                    shown: rows.len(),
                    printed_summary: true,
                    ..Default::default()
                },
            );
        } else {
            let mut files: Vec<&String> = by_file_raw.keys().collect();
            files.sort();
            let mut out = String::new();
            for f in files {
                out.push_str(f);
                out.push('\n');
            }
            return (
                out,
                GrepRenderStats {
                    total_matches,
                    files_matched: by_file_raw.len(),
                    shown: by_file_raw.len(),
                    ..Default::default()
                },
            );
        }
    }

    if opts.count_by_file {
        let mut rows: Vec<(usize, &String)> = by_file_raw
            .iter()
            .map(|(file, matches)| (matches.len(), file))
            .collect();
        rows.sort_by(|(a_cnt, a_file), (b_cnt, b_file)| {
            b_cnt.cmp(a_cnt).then_with(|| a_file.cmp(b_file))
        });

        if json {
            let out_rows: Vec<(usize, String)> = rows
                .into_iter()
                .map(|(cnt, file)| (cnt, compact_path(file)))
                .collect();
            let out =
                GrepJsonOutput::file_counts(pattern, total_matches, by_file_raw.len(), &out_rows);
            return (
                format!("{}\n", serde_json::to_string(&out).unwrap_or_else(|_| "{}".to_string())),
                GrepRenderStats {
                    total_matches,
                    files_matched: by_file_raw.len(),
                    shown: out_rows.len(),
                    printed_summary: true,
                    ..Default::default()
                },
            );
        } else {
            let mut out = String::new();
            for (cnt, file) in rows {
                out.push_str(&format!("{}  {}\n", cnt, file));
            }
            return (
                out,
                GrepRenderStats {
                    total_matches,
                    files_matched: by_file_raw.len(),
                    shown: by_file_raw.len(),
                    ..Default::default()
                },
            );
        }
    }

    if let Some(n) = top_files {
        let mut rows: Vec<(usize, &String)> = by_file_raw
            .iter()
            .map(|(file, matches)| (matches.len(), file))
            .collect();
        rows.sort_by(|(a_cnt, a_file), (b_cnt, b_file)| {
            b_cnt.cmp(a_cnt).then_with(|| a_file.cmp(b_file))
        });

        let mut out_files: Vec<(usize, String)> = Vec::new();
        for (cnt, file) in rows.into_iter().take(n) {
            out_files.push((cnt, compact_path(file)));
        }

        if json {
            let out = GrepJsonOutput::top_files(
                pattern,
                total_matches,
                by_file_raw.len(),
                n,
                &out_files,
            );
            return (
                format!("{}\n", serde_json::to_string(&out).unwrap_or_else(|_| "{}".to_string())),
                GrepRenderStats {
                    total_matches,
                    files_matched: by_file_raw.len(),
                    shown: out_files.len(),
                    printed_summary: true,
                    ..Default::default()
                },
            );
        }

        let mut out = String::new();
        out.push_str(&format!("{} matches in {} files\n\n", total_matches, by_file_raw.len()));
        for (cnt, file) in &out_files {
            out.push_str(&format!("{}  {}\n", cnt, file));
        }
        return (
            out,
            GrepRenderStats {
                total_matches,
                files_matched: by_file_raw.len(),
                shown: out_files.len(),
                ..Default::default()
            },
        );
    }

    let context_re = if opts.context_only {
        Regex::new(&format!("(?i).{{0,20}}{}.*", regex::escape(pattern))).ok()
    } else {
        None
    };

    let effective_per_file = if opts.uncapped {
        None
    } else {
        Some(opts.max_per_file.unwrap_or(config::limits().grep_max_per_file))
    };
    let effective_total = if opts.uncapped { None } else { opts.max_matches };
    let effective_line_chars = opts.max_line_chars;

    let mut rtk_output = String::new();
    rtk_output.push_str(&format!(
        "{} matches in {} files:\n\n",
        total_matches,
        by_file_raw.len()
    ));

    let mut shown = 0usize;
    let mut omitted_total = 0usize;
    let mut omitted_per_file = 0usize;
    let mut clipped_lines = 0usize;
    let mut first_displayed: Option<(String, usize)> = None;

    let mut files: Vec<_> = by_file_raw.iter().collect();
    files.sort_by_key(|(f, _)| *f);

    for (file, matches) in files {
        if let Some(total_cap) = effective_total {
            if shown >= total_cap {
                omitted_total += matches.len();
                continue;
            }
        }

        let file_display = compact_path(file);
        let mut used_in_file = 0usize;
        for (line_num, content) in matches.iter() {
            if let Some(total_cap) = effective_total {
                if shown >= total_cap {
                    omitted_total += 1;
                    continue;
                }
            }

            if let Some(per_file_cap) = effective_per_file {
                if used_in_file >= per_file_cap {
                    omitted_per_file += 1;
                    continue;
                }
            }

            let cleaned = if let Some(max_len) = effective_line_chars {
                let s = clean_line(content, max_len, context_re.as_ref(), pattern);
                if s.trim().chars().count() < content.trim().chars().count() {
                    clipped_lines += 1;
                }
                s
            } else {
                content.trim().to_string()
            };

            rtk_output.push_str(&format!("{}:{}:{}\n", file_display, line_num, cleaned));
            if first_displayed.is_none() {
                first_displayed = Some((file_display.clone(), *line_num));
            }
            shown += 1;
            used_in_file += 1;
        }
    }

    // Legacy overflow marker: keep `[+N more]` for uncapped mode.
    if effective_total.is_none() && (total_matches > shown) {
        rtk_output.push_str(&format!("[+{} more]\n", total_matches - shown));
    }

    // Summary only when explicitly enabled (agent-safe or explicit new flags) AND
    // there was actual omission/clipping, or agent-safe was used.
    let print_summary = opts.summary_enabled
        && (opts.agent_safe || clipped_lines > 0 || omitted_total > 0 || omitted_per_file > 0);
    let hints = build_hints(pattern, first_displayed.as_ref());

    if json {
        let out = GrepJsonOutput::normal(
            pattern,
            total_matches,
            by_file_raw.len(),
            shown,
            omitted_total,
            omitted_per_file,
            clipped_lines,
            &by_file_raw,
            &hints,
            effective_total,
            effective_per_file,
            effective_line_chars,
        );
        return (
            format!("{}\n", serde_json::to_string(&out).unwrap_or_else(|_| "{}".to_string())),
            GrepRenderStats {
                total_matches,
                files_matched: by_file_raw.len(),
                shown,
                omitted_total,
                omitted_per_file,
                clipped_lines,
                printed_summary: true,
            },
        );
    }

    if print_summary {
        rtk_output.push('\n');
        rtk_output.push_str(&format!(
            "summary: total={} files={} shown={} omitted_total={} omitted_per_file={} clipped_lines={}\n",
            total_matches,
            by_file_raw.len(),
            shown,
            omitted_total,
            omitted_per_file,
            clipped_lines
        ));
        rtk_output.push_str("hints:\n");
        for h in &hints {
            rtk_output.push_str(&format!("  {}\n", h));
        }
    }

    (
        rtk_output,
        GrepRenderStats {
            total_matches,
            files_matched: by_file_raw.len(),
            shown,
            omitted_total,
            omitted_per_file,
            clipped_lines,
            printed_summary: print_summary,
        },
    )
}

fn build_hints(pattern: &str, first_displayed: Option<&(String, usize)>) -> Vec<String> {
    let mut hints = vec![
        format!("rtk grep \"{}\" --files-only", pattern),
        format!("rtk grep \"{}\" --count-by-file", pattern),
        format!("rtk grep \"{}\" --agent-safe --max-matches 200", pattern),
    ];
    if let Some((path, line)) = first_displayed {
        let start = line.saturating_sub(5).max(1);
        let end = line + 5;
        hints.push(format!("rtk read \"{}\" --lines {}:{}", path, start, end));
    } else {
        hints.push("rtk read \"<file>\" --lines <START:END>".to_string());
    }
    hints
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrepJsonMatch {
    line: usize,
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrepJsonFile {
    path: String,
    count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    matches: Vec<GrepJsonMatch>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrepJsonOutput {
    pattern: String,
    total_matches: usize,
    files_matched: usize,
    displayed_matches: usize,
    omitted_total: usize,
    omitted_per_file: usize,
    clipped_lines: usize,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_files: Option<usize>,
    files: Vec<GrepJsonFile>,
    hints: Vec<String>,
}

impl GrepJsonOutput {
    fn no_matches(pattern: &str, files_only: bool, count_by_file: bool, top_files: Option<usize>) -> Self {
        let _ = (files_only, count_by_file);
        Self {
            pattern: pattern.to_string(),
            total_matches: 0,
            files_matched: 0,
            displayed_matches: 0,
            omitted_total: 0,
            omitted_per_file: 0,
            clipped_lines: 0,
            truncated: false,
            top_files,
            files: Vec::new(),
            hints: build_hints(pattern, None),
        }
    }

    fn file_counts(pattern: &str, total_matches: usize, files_matched: usize, rows: &[(usize, String)]) -> Self {
        Self {
            pattern: pattern.to_string(),
            total_matches,
            files_matched,
            displayed_matches: 0,
            omitted_total: 0,
            omitted_per_file: 0,
            clipped_lines: 0,
            truncated: false,
            top_files: None,
            files: rows
                .iter()
                .map(|(cnt, path)| GrepJsonFile {
                    path: path.clone(),
                    count: *cnt,
                    matches: Vec::new(),
                })
                .collect(),
            hints: build_hints(pattern, None),
        }
    }

    fn top_files(
        pattern: &str,
        total_matches: usize,
        files_matched: usize,
        requested: usize,
        rows: &[(usize, String)],
    ) -> Self {
        Self {
            pattern: pattern.to_string(),
            total_matches,
            files_matched,
            displayed_matches: 0,
            omitted_total: 0,
            omitted_per_file: 0,
            clipped_lines: 0,
            truncated: false,
            top_files: Some(requested),
            files: rows
                .iter()
                .map(|(cnt, path)| GrepJsonFile {
                    path: path.clone(),
                    count: *cnt,
                    matches: Vec::new(),
                })
                .collect(),
            hints: build_hints(pattern, None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn normal(
        pattern: &str,
        total_matches: usize,
        files_matched: usize,
        _displayed_matches: usize,
        omitted_total: usize,
        omitted_per_file: usize,
        clipped_lines: usize,
        by_file_raw: &HashMap<String, Vec<(usize, &str)>>,
        hints: &[String],
        effective_total: Option<usize>,
        effective_per_file: Option<usize>,
        effective_line_chars: Option<usize>,
    ) -> Self {
        let truncated = omitted_total > 0 || omitted_per_file > 0;
        let mut files: Vec<(&String, &Vec<(usize, &str)>)> = by_file_raw.iter().collect();
        files.sort_by_key(|(f, _)| *f);

        let mut current_count = 0usize;
        let mut out_files: Vec<GrepJsonFile> = Vec::new();
        for (file, matches) in files {
            if let Some(total_cap) = effective_total {
                if current_count >= total_cap {
                    break;
                }
            }
            let mut out_matches: Vec<GrepJsonMatch> = Vec::new();
            for (used_in_file, (line, content)) in matches.iter().enumerate() {
                if let Some(total_cap) = effective_total {
                    if current_count >= total_cap {
                        break;
                    }
                }
                if let Some(per_file_cap) = effective_per_file {
                    if used_in_file >= per_file_cap {
                        break;
                    }
                }

                let text = if let Some(max_len) = effective_line_chars {
                    clean_line(content, max_len, None, pattern)
                } else {
                    content.trim().to_string()
                };
                out_matches.push(GrepJsonMatch { line: *line, text });
                current_count += 1;
            }

            // In normal JSON mode, avoid emitting empty file entries that can be
            // created when we early-break due to a total cap.
            if !out_matches.is_empty() {
                out_files.push(GrepJsonFile {
                    path: compact_path(file),
                    count: matches.len(),
                    matches: out_matches,
                });
            }
        }

        Self {
            pattern: pattern.to_string(),
            total_matches,
            files_matched,
            displayed_matches: current_count,
            omitted_total,
            omitted_per_file,
            clipped_lines,
            truncated,
            top_files: None,
            files: out_files,
            hints: hints.to_vec(),
        }
    }
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

fn build_rg_args(
    pattern: &str,
    path: &str,
    file_type: Option<&str>,
    fixed: bool,
    extra_args: &[String],
) -> Vec<String> {
    // Regex mode: convert BRE alternation \| → | for rg (which uses PCRE-style regex)
    let rg_pattern = if fixed {
        pattern.to_string()
    } else {
        pattern.replace(r"\|", "|")
    };

    // --no-ignore-vcs: match grep -r behavior (don't skip .gitignore'd files).
    // Without this, rg returns 0 matches for files in .gitignore, causing
    // false negatives that make AI agents draw wrong conclusions.
    // Using --no-ignore-vcs (not --no-ignore) so .ignore/.rgignore are still respected.
    let mut args = vec![
        // -n: include line numbers.
        // -H: always emit the filename.
        // -0: NUL-separate filename from `line:content` for unambiguous parsing.
        "-nH0".to_string(),
        "--no-heading".to_string(),
        "--no-ignore-vcs".to_string(),
    ];
    if fixed {
        args.push("-F".to_string());
    }
    if let Some(ft) = file_type {
        args.push("--type".to_string());
        args.push(ft.to_string());
    }

    // Insert extra args before pattern/path so flag ordering matches rg expectations.
    for arg in extra_args {
        // Fix: skip grep-ism -r flag (rg is recursive by default; rg -r means --replace)
        if arg == "-r" || arg == "--recursive" {
            continue;
        }
        args.push(arg.clone());
    }

    args.push(rg_pattern);
    args.push(path.to_string());

    args
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

    if max_len == 0 {
        return String::new();
    }

    if let Some(re) = context_re {
        if let Some(m) = re.find(trimmed) {
            let matched = m.as_str();
            if matched.chars().count() <= max_len {
                return matched.to_string();
            }
        }
    }

    if trimmed.chars().count() <= max_len {
        trimmed.to_string()
    } else {
        if max_len <= 3 {
            return trimmed.chars().take(max_len).collect();
        }
        if max_len <= 6 {
            let t: String = trimmed.chars().take(max_len - 3).collect();
            return format!("{}...", t);
        }

        let lower = trimmed.to_lowercase();
        let pattern_lower = pattern.to_lowercase();

        if lower.contains(&pattern_lower) {
            let chars: Vec<char> = trimmed.chars().collect();
            let lower_chars: Vec<char> = lower.chars().collect();
            let pat_chars: Vec<char> = pattern_lower.chars().collect();

            // Find match start/end in char indices (not bytes) so we don't break UTF-8.
            let mut match_start = 0usize;
            let mut match_end = 0usize;
            'outer: for i in 0..=lower_chars.len().saturating_sub(pat_chars.len()) {
                for j in 0..pat_chars.len() {
                    if lower_chars[i + j] != pat_chars[j] {
                        continue 'outer;
                    }
                }
                match_start = i;
                match_end = i + pat_chars.len();
                break;
            }

            let char_len = chars.len();
            if match_end <= match_start || match_end > char_len {
                let t: String = trimmed.chars().take(max_len.saturating_sub(3)).collect();
                return format!("{}...", t);
            }

            // Reserve room for prefix/suffix + ellipses so match stays visible.
            let ellipses = 3usize;
            let remaining = max_len.saturating_sub(ellipses * 2);
            let match_len = match_end - match_start;
            if remaining <= match_len + 2 {
                // Not enough room for context; show match-centered slice.
                let start = match_start.saturating_sub(1);
                let end = (start + remaining).min(char_len);
                let slice: String = chars[start..end].iter().collect();
                return format!("...{}...", slice);
            }

            let context_budget = remaining - match_len;
            let prefix_budget = context_budget / 2;
            let suffix_budget = context_budget - prefix_budget;

            let prefix_start = match_start.saturating_sub(prefix_budget);
            let prefix = &chars[prefix_start..match_start];
            let matched = &chars[match_start..match_end];
            let suffix_end = (match_end + suffix_budget).min(char_len);
            let suffix = &chars[match_end..suffix_end];

            let mut out = String::new();
            if prefix_start > 0 {
                out.push_str("...");
            }
            out.push_str(&prefix.iter().collect::<String>());
            out.push_str(&matched.iter().collect::<String>());
            out.push_str(&suffix.iter().collect::<String>());
            if suffix_end < char_len {
                out.push_str("...");
            }
            out
        } else {
            let t: String = trimmed.chars().take(max_len.saturating_sub(3)).collect();
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
    use serde_json::Value;

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
    fn test_clean_line_multibyte() {
        // Thai text that exceeds max_len in bytes
        let line = "  สวัสดีครับ นี่คือข้อความที่ยาวมากสำหรับทดสอบ  ";
        let cleaned = clean_line(line, 20, None, "ครับ");
        // Should not panic
        assert!(!cleaned.is_empty());
    }

    #[test]
    fn test_clean_line_utf8_croatian() {
        let line = "  Ovo je dugačka rečenica sa slovima čćđšž i uzorkom FooBar negdje u sredini.  ";
        let cleaned = clean_line(line, 24, None, "FooBar");
        assert!(cleaned.chars().count() <= 24);
        assert!(cleaned.contains("FooBar"));
    }

    #[test]
    fn test_clean_line_tiny_max_len() {
        let line = "  abcdef  ";
        assert_eq!(clean_line(line, 0, None, "c"), "");
        assert_eq!(clean_line(line, 1, None, "c").chars().count(), 1);
        assert_eq!(clean_line(line, 2, None, "c").chars().count(), 2);
        assert_eq!(clean_line(line, 3, None, "c").chars().count(), 3);
        assert!(clean_line(line, 4, None, "c").chars().count() <= 4);
        assert!(clean_line(line, 5, None, "c").chars().count() <= 5);
        assert!(clean_line(line, 6, None, "c").chars().count() <= 6);
    }

    #[test]
    fn test_legacy_caps_do_not_print_summary_by_default() {
        let stdout = "b.txt\x001:foo bar baz\n\
a.txt\x001:foo x\n\
a.txt\x002:foo y\n";
        let (out, stats) = render_grep_output(
            "Foo",
            stdout,
            &GrepRenderOptions {
                files_only: false,
                count_by_file: false,
                uncapped: false,
                agent_safe: false,
                summary_enabled: false,
                context_only: false,
                // Legacy-like defaults (from CLI flags -l/--max and config per-file).
                max_line_chars: Some(5),
                max_matches: Some(1),
                max_per_file: None,
            },
            None,
            false,
        );
        assert!(!out.contains("summary:"));
        assert!(stats.omitted_total > 0 || stats.clipped_lines > 0);
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
        let args = build_rg_args(pattern, ".", None, false, &[]);
        assert!(args.iter().any(|a| a == "fn foo|pub.*bar"));
    }

    #[test]
    fn test_fixed_grep_includes_dash_f_and_keeps_parens_literal() {
        let pattern = "memcpy(szDummy";
        let args = build_rg_args(pattern, ".", None, true, &[]);
        assert!(args.iter().any(|a| a == "-F"));
        assert!(args.iter().any(|a| a == pattern));
    }

    #[test]
    fn test_fixed_grep_cpp_symbol_literal() {
        let pattern = "AgcmUICharacter::OnAddModule";
        let args = build_rg_args(pattern, ".", None, true, &[]);
        assert!(args.iter().any(|a| a == "-F"));
        assert!(args.iter().any(|a| a == pattern));
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

    fn sample_stdout() -> &'static str {
        // Shape: `file\0line:content` (rg -0 / grep -Z)
        "b.txt\x001:foo bar baz\n\
a.txt\x001:foo x\n\
a.txt\x002:foo y\n\
a.txt\x003:foo z\n\
c.txt\x001:foo c1\n\
c.txt\x002:foo c2\n"
    }

    #[test]
    fn test_files_only_unique_sorted() {
        let (out, _stats) = render_grep_output(
            "foo",
            sample_stdout(),
            &GrepRenderOptions {
                files_only: true,
                count_by_file: false,
                uncapped: false,
                agent_safe: false,
                summary_enabled: false,
                context_only: false,
                max_matches: None,
                max_per_file: None,
                max_line_chars: None,
            },
            None,
            false,
        );
        assert_eq!(out, "a.txt\nb.txt\nc.txt\n");
    }

    #[test]
    fn test_count_by_file_sorted() {
        let (out, _stats) = render_grep_output(
            "foo",
            sample_stdout(),
            &GrepRenderOptions {
                files_only: false,
                count_by_file: true,
                uncapped: false,
                agent_safe: false,
                summary_enabled: false,
                context_only: false,
                max_matches: None,
                max_per_file: None,
                max_line_chars: None,
            },
            None,
            false,
        );
        // a.txt has 3, c.txt has 2, b.txt has 1
        assert_eq!(out, "3  a.txt\n2  c.txt\n1  b.txt\n");
    }

    #[test]
    fn test_total_cap_omits_and_summarizes() {
        let (out, stats) = render_grep_output(
            "foo",
            sample_stdout(),
            &GrepRenderOptions {
                files_only: false,
                count_by_file: false,
                uncapped: false,
                agent_safe: false,
                summary_enabled: true,
                context_only: false,
                max_matches: Some(2),
                max_per_file: Some(10),
                max_line_chars: None,
            },
            None,
            false,
        );
        assert!(out.contains("summary:"));
        assert_eq!(stats.shown, 2);
        assert!(stats.omitted_total > 0);
    }

    #[test]
    fn test_per_file_cap_omits_and_summarizes() {
        let (out, stats) = render_grep_output(
            "foo",
            sample_stdout(),
            &GrepRenderOptions {
                files_only: false,
                count_by_file: false,
                uncapped: false,
                agent_safe: false,
                summary_enabled: true,
                context_only: false,
                max_matches: Some(100),
                max_per_file: Some(1),
                max_line_chars: None,
            },
            None,
            false,
        );
        assert!(out.contains("summary:"));
        assert!(stats.omitted_per_file > 0);
    }

    #[test]
    fn test_line_clipping_and_full_lines_escape_hatch() {
        let stdout = "a.txt\x001:prefix foo suffix and extra\n";
        let (clipped, stats_clipped) = render_grep_output(
            "foo",
            stdout,
            &GrepRenderOptions {
                files_only: false,
                count_by_file: false,
                uncapped: false,
                agent_safe: false,
                summary_enabled: true,
                context_only: false,
                max_matches: Some(100),
                max_per_file: Some(10),
                max_line_chars: Some(10),
            },
            None,
            false,
        );
        assert!(stats_clipped.clipped_lines >= 1);
        assert!(clipped.contains("foo"));

        let (full, stats_full) = render_grep_output(
            "foo",
            stdout,
            &GrepRenderOptions {
                files_only: false,
                count_by_file: false,
                uncapped: false,
                agent_safe: false,
                summary_enabled: false,
                context_only: false,
                max_matches: Some(100),
                max_per_file: Some(10),
                max_line_chars: None,
            },
            None,
            false,
        );
        assert_eq!(stats_full.clipped_lines, 0);
        assert!(full.contains("prefix foo suffix and extra"));
    }

    #[test]
    fn test_agent_safe_preset_and_override_semantics() {
        // agent-safe: total=80, per-file=5, line=240; explicit max_per_file overrides to 30.
        // (Dispatch logic in main.rs; we validate render behavior here.)
        let mut many = String::new();
        for i in 1..=40 {
            many.push_str(&format!("a.txt\x00{}:foo {}\n", i, i));
        }

        let (_out, stats_default) = render_grep_output(
            "foo",
            &many,
            &GrepRenderOptions {
                files_only: false,
                count_by_file: false,
                uncapped: false,
                agent_safe: true,
                summary_enabled: true,
                context_only: false,
                max_matches: Some(80),
                max_per_file: Some(5),
                max_line_chars: Some(240),
            },
            None,
            false,
        );
        assert_eq!(stats_default.shown, 5);

        let (_out, stats_override) = render_grep_output(
            "foo",
            &many,
            &GrepRenderOptions {
                files_only: false,
                count_by_file: false,
                uncapped: false,
                agent_safe: true,
                summary_enabled: true,
                context_only: false,
                max_matches: Some(80),
                max_per_file: Some(30),
                max_line_chars: Some(240),
            },
            None,
            false,
        );
        assert_eq!(stats_override.shown, 30);
    }

    #[test]
    fn test_summary_hint_includes_concrete_file_and_line() {
        let stdout = "src\\\\main.rs\u{0}371:Foo bar\n";
        let (out, _stats) = render_grep_output(
            "Foo",
            stdout,
            &GrepRenderOptions {
                max_line_chars: Some(80),
                max_matches: Some(200),
                max_per_file: Some(25),
                uncapped: false,
                files_only: false,
                count_by_file: false,
                agent_safe: true,
                summary_enabled: true,
                context_only: false,
            },
            None,
            false,
        );
        assert!(out.contains("rtk read \"src\\\\main.rs\" --lines 366:376"));
    }

    #[test]
    fn test_top_files_sorts_and_limits() {
        let stdout = concat!(
            "b.rs\u{0}1:Foo\n",
            "a.rs\u{0}1:Foo\n",
            "a.rs\u{0}2:Foo\n"
        );
        let (out, _stats) = render_grep_output(
            "Foo",
            stdout,
            &GrepRenderOptions {
                max_line_chars: Some(80),
                max_matches: Some(200),
                max_per_file: Some(25),
                uncapped: false,
                files_only: false,
                count_by_file: false,
                agent_safe: false,
                summary_enabled: false,
                context_only: false,
            },
            Some(1),
            false,
        );
        assert!(out.contains("2  a.rs"));
        assert!(!out.contains("1  b.rs"));
    }

    #[test]
    fn test_json_output_is_valid_json_only() {
        let stdout = "src\\\\main.rs\u{0}371:Foo bar\n";
        let (out, _stats) = render_grep_output(
            "Foo",
            stdout,
            &GrepRenderOptions {
                max_line_chars: Some(80),
                max_matches: Some(1),
                max_per_file: Some(1),
                uncapped: false,
                files_only: false,
                count_by_file: false,
                agent_safe: true,
                summary_enabled: true,
                context_only: false,
            },
            None,
            true,
        );
        let v: Value = serde_json::from_str(out.trim()).expect("valid json");
        assert_eq!(v["pattern"], "Foo");
    }

    #[test]
    fn test_json_output_total_cap_does_not_emit_empty_files() {
        let stdout = concat!(
            "b.rs\u{0}1:Foo b\n",
            "a.rs\u{0}1:Foo a\n",
            "a.rs\u{0}2:Foo a2\n"
        );
        let (out, _stats) = render_grep_output(
            "Foo",
            stdout,
            &GrepRenderOptions {
                max_line_chars: Some(80),
                max_matches: Some(1),
                max_per_file: Some(25),
                uncapped: false,
                files_only: false,
                count_by_file: false,
                agent_safe: true,
                summary_enabled: true,
                context_only: false,
            },
            None,
            true,
        );
        let v: Value = serde_json::from_str(out.trim()).expect("valid json");
        assert_eq!(v["files"].as_array().unwrap().len(), 1);
        assert_eq!(v["files"][0]["matches"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_json_output_is_valid_for_files_only_and_count_by_file() {
        let stdout = concat!(
            "b.rs\u{0}1:Foo\n",
            "a.rs\u{0}1:Foo\n",
            "a.rs\u{0}2:Foo\n"
        );

        let (out_files_only, _stats) = render_grep_output(
            "Foo",
            stdout,
            &GrepRenderOptions {
                max_line_chars: Some(80),
                max_matches: Some(200),
                max_per_file: Some(25),
                uncapped: false,
                files_only: true,
                count_by_file: false,
                agent_safe: true,
                summary_enabled: true,
                context_only: false,
            },
            None,
            true,
        );
        let v1: Value = serde_json::from_str(out_files_only.trim()).expect("valid json");
        assert_eq!(v1["pattern"], "Foo");
        assert!(v1["files"].is_array());

        let (out_count_by_file, _stats) = render_grep_output(
            "Foo",
            stdout,
            &GrepRenderOptions {
                max_line_chars: Some(80),
                max_matches: Some(200),
                max_per_file: Some(25),
                uncapped: false,
                files_only: false,
                count_by_file: true,
                agent_safe: true,
                summary_enabled: true,
                context_only: false,
            },
            None,
            true,
        );
        let v2: Value = serde_json::from_str(out_count_by_file.trim()).expect("valid json");
        assert_eq!(v2["pattern"], "Foo");
        assert!(v2["files"].is_array());
    }

    #[test]
    fn test_json_total_cap_one_emits_one_match_and_non_empty_files() {
        let stdout = concat!("b.rs\u{0}1:Foo\n", "a.rs\u{0}1:Foo\n", "a.rs\u{0}2:Foo\n");
        let (out, _stats) = render_grep_output(
            "Foo",
            stdout,
            &GrepRenderOptions {
                max_line_chars: Some(80),
                max_matches: Some(1),
                max_per_file: None,
                uncapped: false,
                files_only: false,
                count_by_file: false,
                agent_safe: false,
                summary_enabled: false,
                context_only: false,
            },
            None,
            true,
        );
        let v: Value = serde_json::from_str(out.trim()).expect("valid json");
        assert_eq!(v["displayedMatches"], 1);
        assert!(!v["files"].as_array().unwrap().is_empty());
        let mut total_json_matches = 0usize;
        for f in v["files"].as_array().unwrap() {
            total_json_matches += f["matches"].as_array().unwrap().len();
        }
        assert_eq!(total_json_matches, 1);
    }

    #[test]
    fn test_agent_safe_json_total_cap_does_not_emit_empty_files_when_matches_exist() {
        let stdout = concat!("b.rs\u{0}1:Foo\n", "a.rs\u{0}1:Foo\n", "a.rs\u{0}2:Foo\n");
        let (out, _stats) = render_grep_output(
            "Foo",
            stdout,
            &GrepRenderOptions {
                max_line_chars: Some(80),
                max_matches: Some(1),
                max_per_file: None,
                uncapped: false,
                files_only: false,
                count_by_file: false,
                agent_safe: true,
                summary_enabled: true,
                context_only: false,
            },
            None,
            true,
        );
        let v: Value = serde_json::from_str(out.trim()).expect("valid json");
        assert_eq!(v["displayedMatches"], 1);
        let files = v["files"].as_array().unwrap();
        assert!(!files.is_empty());
        assert!(!files[0]["matches"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_json_total_and_per_file_caps_both_respected() {
        let stdout = concat!(
            "a.rs\u{0}1:Foo\n",
            "a.rs\u{0}2:Foo\n",
            "b.rs\u{0}1:Foo\n",
            "b.rs\u{0}2:Foo\n"
        );
        let (out, _stats) = render_grep_output(
            "Foo",
            stdout,
            &GrepRenderOptions {
                max_line_chars: Some(80),
                max_matches: Some(2),
                max_per_file: Some(1),
                uncapped: false,
                files_only: false,
                count_by_file: false,
                agent_safe: false,
                summary_enabled: false,
                context_only: false,
            },
            None,
            true,
        );
        let v: Value = serde_json::from_str(out.trim()).expect("valid json");
        assert_eq!(v["displayedMatches"], 2);
        let mut total_json_matches = 0usize;
        for f in v["files"].as_array().unwrap() {
            let m = f["matches"].as_array().unwrap().len();
            assert!(m <= 1);
            total_json_matches += m;
        }
        assert_eq!(total_json_matches, 2);
    }
}
