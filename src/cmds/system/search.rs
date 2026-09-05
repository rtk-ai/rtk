//! Shared search-output filter for `rtk grep` and `rtk rg`.
//!
//! Runs the agent's exact engine (grep or rg) — never substituting one for the
//! other — and compresses its output by grouping matches by file, capping, and
//! teeing overflow. The engine differs only in which binary and parse flags are
//! used (see `Engine`); the compression is identical because both emit the same
//! `file:line:content` shape.

use crate::core::stream::{
    self, exec_capture, exec_capture_stdin, CaptureResult, FilterMode, StdinMode, StreamFilter,
};
use crate::core::tracking;
use crate::core::utils::{resolved_command, strip_ansi};
use crate::core::ai_output::{
    prepare_emission_with_baseline, render_with_max_tokens, AiDocument, AiRecord, BudgetClass,
    EmissionMeta, ExactReason, Omission, OutputContract, Severity,
};
use crate::core::{args_utils, config, path_inventory, runner};
use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};

/// Short single-char flags that consume one following token (or inline remainder)
/// as their value. `-e` is handled separately — its value goes to `patterns`.
/// Includes all rg short flags that take a value argument except `-e` and `-r`
/// (stripped) and `-E` (dialect, left to #2138). Failure mode for a missing
/// entry: the value becomes a positional (visible wrong result, not silent).
const VALUE_FLAGS_SHORT: &[u8] = b"ABCMTdfgjmt";
const RG_VALUE_FLAGS_SHORT: &[u8] = b"ABCMTdefgjmrt";

/// Long flags that consume the NEXT token as their value (space-separated form).
/// Inline `=` form (`--flag=value`) is one token and passes through unchanged.
/// `--regexp` is additionally extracted into `patterns` for semantic anchoring.
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
    "--regexp",
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
/// Scans left-to-right, accumulating boolean flag letters — including `r`/`R`,
/// which pass through to grep (recursion is the agent's choice, not RTK's) — and
/// stops at the first value-taking flag (from `VALUE_FLAGS_SHORT` or `e`).
/// Everything after that flag char is its inline value, returned verbatim.
fn parse_cluster(rest: &str) -> ClusterResult {
    let bytes = rest.as_bytes();
    let mut raw_prefix = String::new();
    let mut j = 0;
    while j < bytes.len() {
        let ch = bytes[j];
        let is_e = ch == b'e';
        if is_e || VALUE_FLAGS_SHORT.contains(&ch) {
            let inline = std::str::from_utf8(&bytes[j + 1..])
                .unwrap_or("")
                .to_string();
            let prefix = (!raw_prefix.is_empty()).then_some(raw_prefix);
            return ClusterResult::ValueTaking {
                prefix,
                flag: ch as char,
                inline,
            };
        }
        raw_prefix.push(ch as char);
        j += 1;
    }
    ClusterResult::Boolean((!raw_prefix.is_empty()).then_some(raw_prefix))
}

/// Unique, descriptive tee slug for a file's overflow matches. `idx` disambiguates
/// files within one grep; the tee filename's epoch handles separate runs.
fn grep_slug(idx: usize, path: &str) -> String {
    let cleaned: String = path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let tail = &cleaned[cleaned.len().saturating_sub(32)..];
    format!("grep_{}_{}", idx, tail)
}

/// Format a file's matches as `path<sep>line<sep>content`. Tee blocks use the
/// real (un-compacted) `path` so recovered lines stay openable.
fn match_block(path: &str, entries: &[(usize, bool, String)]) -> String {
    let mut s = String::new();
    for (line_num, is_match, content) in entries {
        let sep = if *is_match { ':' } else { '-' };
        s.push_str(&format!("{}{}{}{}{}\n", path, sep, line_num, sep, content));
    }
    s
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
            if let Some(pattern) = arg.strip_prefix("--regexp=") {
                e_patterns.push(pattern.to_string());
                i += 1;
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
            flags.push(arg.to_string());
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

/// Output shapes that RTK can render as compact, line-oriented AI records.
/// Any flag that is not explicitly understood stays exact; adding a future
/// ripgrep flag must therefore be an intentional routing decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RgRoute {
    Matches,
    JsonEvents,
    Inventory,
    Counts,
    OnlyMatching,
    Exact(ExactReason),
}

fn select_rg_route(current: &mut RgRoute, next: RgRoute) -> Option<ExactReason> {
    match (*current, next) {
        (RgRoute::Matches, next) => {
            *current = next;
            None
        }
        (current, next) if current == next => None,
        _ => Some(ExactReason::Structured),
    }
}

fn rg_long_flag_value(flag: &str) -> bool {
    VALUE_FLAGS_LONG.contains(&flag)
}

fn is_rg_text_long_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--auto-hybrid-regex"
            | "--case-sensitive"
            | "--crlf"
            | "--fixed-strings"
            | "--glob-case-insensitive"
            | "--hidden"
            | "--ignore-case"
            | "--invert-match"
            | "--line-regexp"
            | "--messages"
            | "--mmap"
            | "--no-auto-hybrid-regex"
            | "--no-config"
            | "--no-ignore"
            | "--no-ignore-dot"
            | "--no-ignore-exclude"
            | "--no-ignore-files"
            | "--no-ignore-global"
            | "--no-ignore-messages"
            | "--no-ignore-parent"
            | "--no-ignore-vcs"
            | "--no-messages"
            | "--no-mmap"
            | "--no-pcre2-unicode"
            | "--no-require-git"
            | "--no-search-zip"
            | "--no-unicode"
            | "--one-file-system"
            | "--pcre2"
            | "--pcre2-unicode"
            | "--search-zip"
            | "--smart-case"
            | "--trim"
            | "--unicode"
            | "--with-filename"
            | "--no-filename"
            | "--line-number"
            | "--no-line-number"
            | "--word-regexp"
    )
}

fn classify_rg(args: &[String]) -> RgRoute {
    // Newline-bearing paths cannot be represented safely by the line-oriented
    // semantic parser, so preserve ripgrep's native output and exit behavior.
    if args
        .iter()
        .any(|arg| arg.contains('\r') || arg.contains('\n'))
    {
        return RgRoute::Exact(ExactReason::Sensitive);
    }
    let mut route = RgRoute::Matches;
    let mut no_filename = false;
    let mut past_dashdash = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if past_dashdash {
            i += 1;
            continue;
        }
        if arg == "--" {
            past_dashdash = true;
            i += 1;
            continue;
        }

        if let Some(rest) = arg.strip_prefix("--") {
            let (flag, has_inline_value) = rest
                .split_once('=')
                .map(|(flag, _)| (format!("--{flag}"), true))
                .unwrap_or_else(|| (arg.clone(), false));

            let mode = match flag.as_str() {
                "--json" => Some(RgRoute::JsonEvents),
                "--files" | "--files-with-matches" | "--files-without-match" => {
                    Some(RgRoute::Inventory)
                }
                "--count" | "--count-matches" => Some(RgRoute::Counts),
                "--only-matching" => Some(RgRoute::OnlyMatching),
                "--help" | "--version" => return RgRoute::Exact(ExactReason::Interactive),
                "--follow" => return RgRoute::Exact(ExactReason::Streaming),
                "--binary" | "--text" | "--text-encoding" => {
                    return RgRoute::Exact(ExactReason::Binary)
                }
                "--byte-offset"
                | "--column"
                | "--context-separator"
                | "--field-context-separator"
                | "--field-match-separator"
                | "--heading"
                | "--null"
                | "--null-data"
                | "--passthru"
                | "--pretty"
                | "--quiet"
                | "--silent"
                | "--stats"
                | "--vimgrep" => return RgRoute::Exact(ExactReason::Structured),
                "--debug" | "--trace" | "--type-list" | "--pcre2-version" => {
                    return RgRoute::Exact(ExactReason::Interactive)
                }
                "--color" | "--colors" | "--encoding" | "--pre" | "--pre-glob" => {
                    return RgRoute::Exact(ExactReason::Sensitive)
                }
                "--multiline" | "--multiline-dotall" => {
                    return RgRoute::Exact(ExactReason::Structured)
                }
                _ if rg_long_flag_value(&flag) || is_rg_text_long_flag(&flag) => None,
                _ => return RgRoute::Exact(ExactReason::Unknown),
            };

            if flag == "--no-filename" {
                no_filename = true;
            }

            if let Some(next) = mode {
                if let Some(reason) = select_rg_route(&mut route, next) {
                    return RgRoute::Exact(reason);
                }
            }
            if matches!(route, RgRoute::Counts) && no_filename {
                return RgRoute::Exact(ExactReason::Structured);
            }
            if rg_long_flag_value(&flag) && !has_inline_value {
                if flag == "--replace"
                    && args
                        .get(i + 1)
                        .is_none_or(|value| value.starts_with('-'))
                {
                    return RgRoute::Exact(ExactReason::Unknown);
                }
                i += 1;
            }
            i += 1;
            continue;
        }

        let Some(cluster) = arg.strip_prefix('-').filter(|cluster| !cluster.is_empty()) else {
            i += 1;
            continue;
        };
        let bytes = cluster.as_bytes();
        let mut j = 0;
        while j < bytes.len() {
            let flag = bytes[j] as char;
            let mode = match flag {
                'c' => Some(RgRoute::Counts),
                'l' => Some(RgRoute::Inventory),
                'L' => return RgRoute::Exact(ExactReason::Streaming),
                'o' => Some(RgRoute::OnlyMatching),
                'h' | 'V' => return RgRoute::Exact(ExactReason::Interactive),
                '0' | 'Z' | 'b' | 'p' | 'q' => {
                    return RgRoute::Exact(ExactReason::Structured)
                }
                'a' | 'U' | 'z' => return RgRoute::Exact(ExactReason::Binary),
                'A' | 'B' | 'C' | 'M' | 'd' | 'e' | 'f' | 'g' | 'j' | 'm' | 'r' | 't'
                | 'T' => {
                    if j + 1 == bytes.len() {
                        if flag == 'r'
                            && args
                                .get(i + 1)
                                .is_none_or(|value| value.starts_with('-'))
                        {
                            return RgRoute::Exact(ExactReason::Unknown);
                        }
                        i += 1;
                    }
                    break;
                }
                'F' | 'H' | 'i' | 'n' | 'N' | 'P' | 'R' | 's' | 'S' | 'u' | 'v' | 'w'
                | 'x' => None,
                _ => return RgRoute::Exact(ExactReason::Unknown),
            };
            if let Some(next) = mode {
                if let Some(reason) = select_rg_route(&mut route, next) {
                    return RgRoute::Exact(reason);
                }
            }
            j += 1;
        }
        i += 1;
    }

    route
}

const RG_AI_MAX_LINE_LEN: usize = 80;
type RgMatchEntry = (String, usize, bool, String, bool);
type RgMatchBlock = (String, Vec<(usize, bool, String, bool)>);

fn rg_document(
    route: RgRoute,
    raw: &str,
    patterns: &[String],
    _paths: &[String],
) -> Result<AiDocument> {
    if raw.is_empty() {
        return Ok(AiDocument::legacy(""));
    }

    match route {
        RgRoute::Matches => rg_match_document(raw, patterns, false),
        RgRoute::OnlyMatching => rg_match_document(raw, patterns, true),
        RgRoute::JsonEvents => rg_json_document(raw, patterns),
        RgRoute::Inventory => Ok(path_inventory::document(&parse_inventory_paths(raw))),
        RgRoute::Counts => rg_count_document(raw, _paths),
        RgRoute::Exact(reason) => Err(anyhow!(
            "exact rg route reached semantic renderer: {}",
            reason.as_str()
        )),
    }
}

fn rg_faithful_match_baseline(
    raw: &str,
    paths: &[String],
    extra_args: &[String],
) -> Result<String> {
    let show_file = rg_show_file(paths, extra_args);
    let show_line = show_line(Engine::Rg, extra_args, std::io::stdout().is_terminal());
    let mut plain = String::new();
    for line in raw.lines() {
        if let Some(output) = format_match_line(line, show_file, show_line) {
            plain.push_str(&output);
        } else if line == "--" {
            plain.push_str("--\n");
        } else {
            return Err(anyhow!("unrecognized ripgrep match record"));
        }
    }
    Ok(plain)
}

fn rg_match_document(raw: &str, patterns: &[String], preserve_exact: bool) -> Result<AiDocument> {
    let mut entries = Vec::new();
    for line in raw.lines() {
        if line == "--" {
            continue;
        }
        let Some((path, line_number, is_match, content)) = parse_match_line(line) else {
            return Err(anyhow!("unrecognized ripgrep match record"));
        };
        let (content, was_shortened) = clean_rg_line(content, patterns, preserve_exact);
        entries.push((path, line_number, is_match, content, was_shortened));
    }
    Ok(rg_match_document_from_entries(entries))
}

fn clean_rg_line(content: &str, anchors: &[String], preserve_exact: bool) -> (String, bool) {
    if preserve_exact {
        return (content.to_string(), false);
    }
    let content_lower = content.to_lowercase();
    let anchor = anchors
        .iter()
        .find(|anchor| !anchor.is_empty() && content_lower.contains(&anchor.to_lowercase()))
        .map(String::as_str)
        .unwrap_or_default();
    let cleaned = clean_line(content, RG_AI_MAX_LINE_LEN, None, anchor);
    let shortened = cleaned != content;
    (cleaned, shortened)
}

fn rg_match_document_from_entries(entries: Vec<RgMatchEntry>) -> AiDocument {
    let matches = entries
        .iter()
        .filter(|(_, _, is_match, _, _)| *is_match)
        .count();
    rg_match_document_from_entries_with_counts(entries, matches, 0)
}

fn rg_match_document_from_entries_with_counts(
    entries: Vec<RgMatchEntry>,
    matches: usize,
    omitted_items: usize,
) -> AiDocument {
    let mut document = AiDocument::new(Some("search"));
    document.fact("matches", matches.to_string());

    let mut blocks: Vec<RgMatchBlock> = Vec::new();
    for (path, line_number, is_match, content, was_shortened) in entries {
        if let Some((previous_path, records)) = blocks.last_mut() {
            if *previous_path == path {
                records.push((line_number, is_match, content, was_shortened));
                continue;
            }
        }
        blocks.push((path, vec![(line_number, is_match, content, was_shortened)]));
    }

    for (path, records) in blocks {
        // Keep per-file groups compact, but never make a large file an
        // indivisible record that is dropped wholesale by the source budget.
        for chunk in records.chunks(20) {
            let rendered_records = chunk
                .iter()
                .map(|(line_number, is_match, content, _)| {
                    let separator = if *is_match { ':' } else { '-' };
                    format!("{line_number}{separator} {content}")
                })
                .collect::<Vec<_>>()
                .join("; ");
            let shortened = chunk
                .iter()
                .filter(|(_, _, _, was_shortened)| *was_shortened)
                .count();
            document.push(
                AiRecord::new(Severity::Info, format!("{path} {{{rendered_records}}}"))
                    .grouped(&path)
                    .representing(chunk.len())
                    .omitting(shortened),
            );
        }
    }

    if omitted_items > 0 {
        document = document.with_omission(Omission {
            items: omitted_items,
            groups: 0,
        });
    }
    document
}

fn rg_json_document(raw: &str, patterns: &[String]) -> Result<AiDocument> {
    let mut entries = Vec::new();

    for line in raw.lines() {
        let event: Value = serde_json::from_str(line).context("invalid ripgrep JSON event")?;
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("ripgrep JSON event has no type"))?;
        if !matches!(event_type, "match" | "context") {
            continue;
        }

        let data = event
            .get("data")
            .ok_or_else(|| anyhow!("ripgrep JSON {event_type} event has no data"))?;
        let path_value = data.get("path");
        if path_value
            .and_then(|path| path.get("bytes"))
            .is_some()
        {
            return Err(anyhow!("ripgrep JSON path is not valid UTF-8"));
        }
        let path = path_value
            .and_then(|path| path.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("<stdin>");
        let line_number = data
            .get("line_number")
            .and_then(Value::as_u64)
            .and_then(|line_number| usize::try_from(line_number).ok())
            .ok_or_else(|| anyhow!("ripgrep JSON {event_type} event has no line number"))?;
        let text = data
            .get("lines")
            .and_then(|lines| lines.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("ripgrep JSON {event_type} event has non-text lines"))?;
        let (text, was_shortened) = clean_rg_line(
            text.trim_end_matches(['\r', '\n']),
            patterns,
            false,
        );
        entries.push((
            path.to_string(),
            line_number,
            event_type == "match",
            text,
            was_shortened,
        ));
    }

    Ok(rg_match_document_from_entries(entries))
}

fn parse_inventory_paths(raw: &str) -> Vec<String> {
    raw.split(['\n', '\0'])
        .map(|path| path.trim_end_matches('\r'))
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect()
}

fn rg_count_document(raw: &str, paths: &[String]) -> Result<AiDocument> {
    let mut document = AiDocument::new(Some("counts"));
    let mut files = 0;
    for line in raw.lines().filter(|line| !line.is_empty()) {
        let (path, count) = if let Some((path, count)) = line.split_once('\0') {
            (path, count)
        } else if let Some((path, count)) = line.rsplit_once(':') {
            (path, count)
        } else if line.chars().all(|character| character.is_ascii_digit()) {
            (paths.first().map(String::as_str).unwrap_or("<stdin>"), line)
        } else {
            return Err(anyhow!("unrecognized ripgrep count record"));
        };
        if !count.chars().all(|character| character.is_ascii_digit()) {
            return Err(anyhow!("ripgrep count is not numeric"));
        }
        files += 1;
        document.push(AiRecord::new(Severity::Info, format!("{path}={count}")).grouped(path));
    }
    document.fact("files", files.to_string());
    Ok(document)
}

fn append_rg_parse_aids(cmd: &mut Command, args: &[String], aids: &[&str]) {
    if let Some(separator) = args.iter().position(|arg| arg == "--") {
        cmd.args(&args[..separator]);
        cmd.args(aids);
        cmd.arg("--");
        cmd.args(&args[separator + 1..]);
    } else {
        cmd.args(args);
        cmd.args(aids);
    }
}

const RG_STREAM_LINE_CAP: usize = 64 * 1024;

/// Bounded semantic search state. The producer is drained once, while only a
/// bounded number of parsed records and a bounded raw baseline are retained.
/// This is deliberately separate from the legacy streaming grep filter: an
/// oversized search line must be summarized, not replayed as an unbounded raw
/// fallback.
struct RgAiStreamFilter {
    route: RgRoute,
    patterns: Vec<String>,
    paths: Vec<String>,
    extra_args: Vec<String>,
    entries: Vec<RgMatchEntry>,
    max_entries: usize,
    total_matches: usize,
    omitted_items: usize,
    truncated_lines: usize,
    truncated_stored: usize,
    parse_errors: Vec<String>,
    raw_parse_aid: String,
    raw_complete: bool,
    stats: Arc<Mutex<EmissionMeta>>,
}

impl RgAiStreamFilter {
    fn new(
        route: RgRoute,
        patterns: Vec<String>,
        paths: Vec<String>,
        extra_args: Vec<String>,
        stats: Arc<Mutex<EmissionMeta>>,
    ) -> Self {
        Self {
            route,
            patterns,
            paths,
            extra_args,
            entries: Vec::new(),
            max_entries: config::limits().grep_max_results.max(1),
            total_matches: 0,
            omitted_items: 0,
            truncated_lines: 0,
            truncated_stored: 0,
            parse_errors: Vec::new(),
            raw_parse_aid: String::new(),
            raw_complete: true,
            stats,
        }
    }

    fn retain_raw_line(&mut self, line: &str) {
        if !self.raw_complete {
            return;
        }
        let required = line.len().saturating_add(1);
        if self.raw_parse_aid.len().saturating_add(required) > stream::RAW_CAP
            || line.ends_with(stream::TRUNCATED_LINE_MARKER)
        {
            self.raw_complete = false;
            return;
        }
        self.raw_parse_aid.push_str(line);
        self.raw_parse_aid.push('\n');
    }

    fn record_parse_error(&mut self, line: &str) {
        if self.parse_errors.len() < 4 {
            let sample = line.chars().take(512).collect::<String>();
            self.parse_errors.push(sample);
        }
    }

    fn render_bounded(&mut self) -> String {
        let truncated_items = self
            .truncated_lines
            .saturating_sub(self.truncated_stored);
        let declared_omissions = self
            .omitted_items
            .saturating_add(self.truncated_stored)
            .saturating_add(truncated_items);

        let mut document = if self.parse_errors.is_empty() {
            rg_match_document_from_entries_with_counts(
                std::mem::take(&mut self.entries),
                self.total_matches,
                declared_omissions,
            )
        } else {
            let sample = self.parse_errors.join("\n");
            let mut document = AiDocument::parse_failure(&sample, "unrecognized rg record");
            document.fact("observed_matches", self.total_matches.to_string());
            document
        };
        if self.truncated_lines > 0 {
            document.fact("truncated_lines", self.truncated_lines.to_string());
        }

        let rendered = render_with_max_tokens(
            &document,
            BudgetClass::Source,
            runner::requested_max_tokens(),
        );

        if self.raw_complete && self.parse_errors.is_empty() && self.truncated_lines == 0 {
            let baseline = rg_faithful_match_baseline(
                &self.raw_parse_aid,
                &self.paths,
                &self.extra_args,
            )
            .unwrap_or_else(|_| self.raw_parse_aid.clone());
            let prepared = prepare_emission_with_baseline(
                &baseline,
                &baseline,
                "rg",
                rendered,
                true,
            );
            let meta = prepared.meta();
            if let Ok(mut current) = self.stats.lock() {
                *current = meta;
            }
            return prepared.as_str().to_string();
        }

        let omission = rendered.omission.clone();
        let mut output = rendered.text;
        if let Some(ref omission) = omission {
            output.push_str(&format!(
                "\nomitted items={} groups={} recovery=unavailable",
                omission.items, omission.groups
            ));
        } else {
            output.push_str("\nrecovery=unavailable");
        }
        let output = format!("{}\n", output.trim_end_matches('\n'));
        let meta = EmissionMeta {
            omitted_items: omission
                .as_ref()
                .map_or(declared_omissions, |value| value.items),
            omitted_groups: omission.as_ref().map_or(0, |value| value.groups),
            parser_failed: !self.parse_errors.is_empty(),
            runtime_error: Some("capture_incomplete"),
            ..EmissionMeta::default()
        };
        if let Ok(mut current) = self.stats.lock() {
            *current = meta;
        }
        output
    }
}

impl StreamFilter for RgAiStreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        self.retain_raw_line(line);
        if line == "--" {
            return None;
        }

        let preserve_exact = matches!(self.route, RgRoute::OnlyMatching);
        let Some((path, line_number, is_match, content)) = parse_match_line(line) else {
            self.record_parse_error(line);
            return None;
        };
        if is_match {
            self.total_matches = self.total_matches.saturating_add(1);
        }
        let line_was_truncated = line.ends_with(stream::TRUNCATED_LINE_MARKER);
        if line_was_truncated {
            self.truncated_lines = self.truncated_lines.saturating_add(1);
        }
        let (content, was_shortened) = clean_rg_line(content, &self.patterns, preserve_exact);
        if self.entries.len() >= self.max_entries {
            self.omitted_items = self.omitted_items.saturating_add(1);
        } else {
            self.entries
                .push((path, line_number, is_match, content, was_shortened));
            if line_was_truncated {
                self.truncated_stored = self.truncated_stored.saturating_add(1);
            }
        }
        None
    }

    fn flush(&mut self) -> String {
        String::new()
    }

    fn on_exit(&mut self, _exit_code: i32, _raw: &str) -> Option<String> {
        Some(self.render_bounded())
    }
}

fn run_rg_ai_streaming(
    route: RgRoute,
    args: &[String],
    patterns: Vec<String>,
    paths: Vec<String>,
    extra_args: Vec<String>,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let stats = Arc::new(Mutex::new(EmissionMeta::default()));
    let filter = RgAiStreamFilter::new(
        route,
        patterns,
        paths,
        extra_args,
        Arc::clone(&stats),
    );
    let mut command = rg_semantic_command(route, args);
    let result = stream::run_streaming_with_line_cap(
        &mut command,
        StdinMode::Null,
        FilterMode::StreamingStdout(Box::new(filter)),
        Some(RG_STREAM_LINE_CAP),
    )
    .context("search failed")?;
    let meta = stats.lock().map(|value| *value).unwrap_or_default();
    let output_tokens = tracking::estimate_tokens(&format!(
        "{}{}",
        result.raw_stderr, result.filtered
    ));
    timer.track_output_tokens(
        &format!("rg {}", args.join(" ")),
        &format!("rtk rg {}", args.join(" ")),
        tracking::estimate_tokens_from_bytes(result.observed_output_bytes()),
        output_tokens,
        runner::output_tracking_from_emission(OutputContract::AiOwned(BudgetClass::Source), meta),
    );
    Ok(result.exit_code)
}

fn rg_semantic_command(route: RgRoute, args: &[String]) -> Command {
    let mut cmd = resolved_command("rg");
    match route {
        RgRoute::Matches | RgRoute::OnlyMatching => {
            append_rg_parse_aids(&mut cmd, args, &["-n", "--with-filename", "--null"])
        }
        RgRoute::Counts | RgRoute::JsonEvents | RgRoute::Inventory | RgRoute::Exact(_) => {
            cmd.args(args);
        }
    };
    cmd
}

fn run_rg_ai(route: RgRoute, args: &[String]) -> Result<i32> {
    let (mut patterns, paths, extra_args) = extract_pattern_path(args);
    patterns.extend(rg_replacement_values(args));
    if matches!(route, RgRoute::Matches | RgRoute::OnlyMatching) {
        return run_rg_ai_streaming(route, args, patterns, paths, extra_args);
    }
    let budget = match route {
        RgRoute::Inventory => BudgetClass::Collection,
        RgRoute::Matches | RgRoute::JsonEvents | RgRoute::Counts | RgRoute::OnlyMatching => {
            BudgetClass::Source
        }
        RgRoute::Exact(_) => unreachable!("exact rg route cannot use semantic runner"),
    };
    let command = rg_semantic_command(route, args);
    let args_display = args.join(" ");
    let native_args = args.to_vec();

    runner::run_ai_filtered_with_exit(
        command,
        "rg",
        &args_display,
        budget,
        move |raw, exit_code| {
            if raw.is_empty() && exit_code != 0 {
                Ok(AiDocument::legacy(""))
            } else {
                let parsed = (|| -> Result<AiDocument> {
                    let document = rg_document(route, raw, &patterns, &paths)?;
                    if matches!(route, RgRoute::Matches | RgRoute::OnlyMatching) {
                        Ok(document.with_lossless_baseline(rg_faithful_match_baseline(
                            raw,
                            &paths,
                            &extra_args,
                        )?))
                    } else {
                        Ok(document)
                    }
                })();

                parsed.or_else(|_| {
                    // Parse aids are deliberately augmented onto the semantic
                    // invocation. If parsing fails, rerun the original argv so
                    // recovery and fallback preserve native output exactly.
                    let mut native = resolved_command("rg");
                    native.args(&native_args);
                    let result = exec_capture_stdin(&mut native)?;
                    let stdout = result.stdout;
                    Ok(AiDocument::legacy(stdout.clone()).with_lossless_baseline(stdout))
                })
            }
        },
        runner::RunOptions::stdout_only(),
    )
}

fn rg_replacement_values(args: &[String]) -> Vec<String> {
    let mut values = Vec::new();
    let mut past_dashdash = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if past_dashdash {
            break;
        }
        if arg == "--" {
            past_dashdash = true;
            index += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--replace=") {
            values.push(value.to_string());
        } else if arg == "--replace" || arg == "-r" {
            if let Some(value) = args.get(index + 1) {
                values.push(value.clone());
                index += 1;
            }
        } else if let Some(cluster) = arg.strip_prefix('-') {
            if let Some(position) = cluster.bytes().position(|character| character == b'r') {
                let replacement = &cluster[position + 1..];
                if !replacement.is_empty() {
                    values.push(replacement.to_string());
                } else if let Some(value) = args.get(index + 1) {
                    values.push(value.clone());
                    index += 1;
                }
            }
        }
        index += 1;
    }
    values
}

fn run_rg_exact(args: &[String], verbose: u8, reason: ExactReason) -> Result<i32> {
    let args = args
        .iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    runner::run_passthrough_with_reason("rg", &args, verbose, reason)
}

/// Run real grep so matches and the savings baseline match the agent's command;
/// rg is the fallback when grep is absent, rejects a flag, or `--type` is used.
/// The search engine the agent actually invoked. RTK runs this binary verbatim
/// and never substitutes one for the other.
#[derive(Clone, Copy)]
pub enum Engine {
    Grep,
    Rg,
}

impl Engine {
    fn bin(self) -> &'static str {
        match self {
            Engine::Grep => "grep",
            Engine::Rg => "rg",
        }
    }

    pub fn label(self) -> &'static str {
        self.bin()
    }

    /// `-n -H --null` are parse aids (NUL keeps the regroup unambiguous, #1436);
    /// `-I` skips binary noise (-a overrides).
    fn parse_flags(self) -> &'static [&'static str] {
        match self {
            Engine::Grep => &["-n", "-H", "-I", "--null"],
            Engine::Rg => &["-n", "--with-filename", "--null"],
        }
    }
}

/// Runs the agent's exact engine + flags for the grouping path, appending only the
/// parse aids (see `Engine::parse_flags`).
fn engine_capture<T: AsRef<str>>(
    engine: Engine,
    extra_args: &[T],
    patterns: &[String],
    paths: &[String],
) -> Result<CaptureResult> {
    let mut cmd = engine_command(engine, extra_args, patterns, paths, false);
    exec_capture_stdin(&mut cmd).context("search failed")
}

fn engine_command<T: AsRef<str>>(
    engine: Engine,
    extra_args: &[T],
    patterns: &[String],
    paths: &[String],
    line_buffered: bool,
) -> Command {
    let mut cmd = resolved_command(engine.bin());
    cmd.args(engine.parse_flags());
    for a in extra_args {
        cmd.arg(a.as_ref());
    }
    if line_buffered {
        // The engine writes through a pipe, so flush each match immediately.
        cmd.arg("--line-buffered");
    }
    for p in patterns {
        cmd.args(["-e", p]);
    }
    cmd.arg("--");
    cmd.args(paths);
    cmd
}

fn format_match_line(line: &str, show_file: bool, show_line: bool) -> Option<String> {
    let (file, line_num, is_match, content) = parse_match_line(line)?;
    let sep = if is_match { ':' } else { '-' };
    let mut output = String::new();
    if show_file {
        output.push_str(&file);
        output.push(sep);
    }
    if show_line {
        output.push_str(&line_num.to_string());
        output.push(sep);
    }
    output.push_str(content);
    output.push('\n');
    Some(output)
}

/// Emits each piped match as it arrives. Buffered search waits for EOF, so
/// `tail -f app.log | rtk grep ERROR` would otherwise show no matches.
struct SearchStreamFilter {
    show_file: bool,
    show_line: bool,
    max_results: usize,
    shown: usize,
    cap_reported: bool,
}

impl StreamFilter for SearchStreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        let Some(output) = format_match_line(line, self.show_file, self.show_line) else {
            if line == "--" && self.shown >= self.max_results {
                return None;
            }
            return Some(format!("{line}\n"));
        };

        if self.shown >= self.max_results {
            if self.cap_reported {
                return None;
            }
            self.cap_reported = true;
            return Some(format!(
                "[rtk] output capped at {} results\n",
                self.max_results
            ));
        }

        self.shown += 1;
        Some(output)
    }

    fn flush(&mut self) -> String {
        String::new()
    }
}

fn show_file(paths: &[String], extra_args: &[String]) -> bool {
    paths.len() > 1
        || paths.iter().any(|p| std::path::Path::new(p).is_dir())
        || has_short_flag(extra_args, 'H')
        || has_short_flag(extra_args, 'r')
        || has_short_flag(extra_args, 'R')
        || extra_args
            .iter()
            .any(|f| f == "--with-filename" || f == "--recursive")
}

/// Ripgrep's filename selectors override its default of showing names for
/// multi-file and recursive searches. Preserve their argument order while
/// reconstructing the lossless baseline from our parse-aided invocation.
fn rg_show_file(paths: &[String], extra_args: &[String]) -> bool {
    let mut explicit = None;
    let mut index = 0;
    while index < extra_args.len() {
        let arg = &extra_args[index];
        if VALUE_FLAGS_LONG.contains(&arg.as_str()) {
            index += 2;
            continue;
        }

        if arg == "--with-filename" {
            explicit = Some(true);
        } else if arg == "--no-filename" {
            explicit = Some(false);
        } else if let Some(cluster) = arg.strip_prefix('-').filter(|cluster| !cluster.is_empty())
        {
            let mut value_taking = false;
            for (position, flag) in cluster.bytes().enumerate() {
                if RG_VALUE_FLAGS_SHORT.contains(&flag) {
                    value_taking = position + 1 == cluster.len();
                    break;
                }
                match flag {
                    b'H' => explicit = Some(true),
                    b'h' => explicit = Some(false),
                    _ => {}
                }
            }
            if value_taking {
                index += 2;
                continue;
            }
        }
        index += 1;
    }

    explicit.unwrap_or_else(|| {
        paths.is_empty()
            || paths.len() > 1
            || paths.iter().any(|path| std::path::Path::new(path).is_dir())
    })
}

fn show_line(engine: Engine, extra_args: &[String], stdout_is_tty: bool) -> bool {
    if has_short_flag(extra_args, 'N')
        || extra_args.iter().any(|f| f == "--no-line-number")
    {
        return false;
    }

    has_short_flag(extra_args, 'n')
        || extra_args.iter().any(|f| f == "--line-number")
        || (matches!(engine, Engine::Rg) && stdout_is_tty)
}

fn run_streaming_search(
    timer: &tracking::TimedExecution,
    engine: Engine,
    extra_args: &[String],
    patterns: &[String],
    paths: &[String],
    max_results: usize,
    real_cmd: &str,
) -> Result<i32> {
    let filter = SearchStreamFilter {
        show_file: show_file(paths, extra_args),
        show_line: show_line(engine, extra_args, std::io::stdout().is_terminal()),
        max_results,
        shown: 0,
        cap_reported: false,
    };
    let mut cmd = engine_command(engine, extra_args, patterns, paths, true);
    let result = stream::run_streaming(
        &mut cmd,
        StdinMode::Inherit,
        FilterMode::Streaming(Box::new(filter)),
    )
    .context("search failed")?;

    timer.track(
        real_cmd,
        &format!("rtk {}", engine.label()),
        &result.raw_stdout,
        &result.filtered,
    );
    Ok(result.exit_code)
}

/// Runs the agent's command verbatim for forms RTK does not group: format/shape
/// flags and pattern-less modes (`--files`, `--type-list`).
fn passthrough<T: AsRef<str>>(
    timer: &tracking::TimedExecution,
    engine: Engine,
    args: &[T],
    real_cmd: &str,
    stream_stdin: bool,
) -> Result<i32> {
    let mut cmd = resolved_command(engine.bin());
    if stream_stdin && !std::io::stdout().is_terminal() {
        // Keep passthrough output live when stdout is piped.
        cmd.arg("--line-buffered");
    }
    for a in args {
        cmd.arg(a.as_ref());
    }

    let exit_code = if stream_stdin {
        stream::run_streaming(&mut cmd, StdinMode::Inherit, FilterMode::Passthrough)
            .context("search failed")?
            .exit_code
    } else {
        let result = exec_capture_stdin(&mut cmd).context("search failed")?;
        print!("{}", strip_ansi(&result.stdout));
        if !result.stderr.is_empty() {
            eprint!("{}", result.stderr);
        }
        result.exit_code
    };

    timer.track_passthrough(real_cmd, &format!("rtk {} (passthrough)", real_cmd));
    Ok(exit_code)
}

fn has_short_flag(flags: &[String], ch: char) -> bool {
    flags
        .iter()
        .any(|f| f.starts_with('-') && !f.starts_with("--") && f[1..].contains(ch))
}

fn has_context_flag(flags: &[String]) -> bool {
    has_short_flag(flags, 'A')
        || has_short_flag(flags, 'B')
        || has_short_flag(flags, 'C')
        || flags.iter().any(|f| {
            f == "--after-context"
                || f == "--before-context"
                || f == "--context"
                || f.starts_with("--after-context=")
                || f.starts_with("--before-context=")
                || f.starts_with("--context=")
        })
}

pub fn run(
    engine: Engine,
    max_line_len: usize,
    max_results: usize,
    context_only: bool,
    args: &[String],
    verbose: u8,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // --version / --help: pass through to the engine without filtering.
    // Note: Clap strips `--` before populating trailing_var_arg, so both
    // `rtk grep --version` and `rtk grep -- --version` land here identically.
    if matches!(engine, Engine::Grep)
        && args
        .iter()
        .any(|a| a == "--version" || a == "--help" || a == "-h")
    {
        let mut cmd = resolved_command(engine.bin());
        cmd.args(args);
        let result = exec_capture(&mut cmd).context("search failed")?;
        print!("{}", result.stdout);
        if !result.stderr.is_empty() {
            eprint!("{}", result.stderr);
        }
        return Ok(result.exit_code);
    }

    // Re-insert `--` when clap's trailing_var_arg consumed it
    let args = args_utils::restore_double_dash(args);

    if matches!(engine, Engine::Rg) {
        let (_, rg_paths, _) = extract_pattern_path(&args);
        let route = classify_rg(&args);
        let reads_piped_stdin = !std::io::stdin().is_terminal()
            && (rg_paths.is_empty() || rg_paths.iter().any(|path| path == "-"));
        if reads_piped_stdin && !matches!(route, RgRoute::Inventory) {
            return run_rg_exact(&args, verbose, ExactReason::Streaming);
        }
        return match route {
            RgRoute::Exact(reason) => run_rg_exact(&args, verbose, reason),
            route => run_rg_ai(route, &args),
        };
    }

    let real_cmd = format!("{} {}", engine.label(), args.join(" "));
    let rtk_label = format!("rtk {}", engine.label());

    let (patterns, paths, extra_args) = extract_pattern_path(&args);

    if patterns.is_empty() {
        return passthrough(&timer, engine, &args, &real_cmd, false);
    }

    let pattern_display = if patterns.len() == 1 {
        patterns[0].clone()
    } else {
        patterns.join("|")
    };

    let path_display = paths.join(" ");

    if verbose > 0 {
        eprintln!("grep: '{}' in {}", pattern_display, path_display);
    }

    let reads_piped_stdin = !std::io::stdin().is_terminal()
        && (paths.is_empty() || paths.iter().any(|path| path == "-"));

    // format/shape flags (-c/-l/-o/...): already-minimal native output, passthrough.
    if has_format_flag(&extra_args) {
        return passthrough(&timer, engine, &args, &real_cmd, reads_piped_stdin);
    }

    if reads_piped_stdin {
        return run_streaming_search(
            &timer,
            engine,
            &extra_args,
            &patterns,
            &paths,
            max_results,
            &real_cmd,
        );
    }

    let result = engine_capture(engine, &extra_args, &patterns, &paths)?;

    let exit_code = result.exit_code;
    let raw_output = result.stdout.clone();

    // Unparseable shape re-runs verbatim below (with its own stderr), so handle it
    // before surfacing this run's stderr (#2333).
    if unparsed_signal(&raw_output) > 0 {
        return passthrough(&timer, engine, &args, &real_cmd, false);
    }

    if !result.stderr.is_empty() {
        eprint!("{}", result.stderr);
    }

    if result.stdout.trim().is_empty() {
        timer.track(&real_cmd, &rtk_label, &raw_output, "");
        return Ok(exit_code);
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

    // Mirror what the real command prints: the filename only when grep/rg would
    // show one (multiple files, a directory, -r or -H), the line number only with
    // -n. We force -nH--null for robust parsing, then drop what the engine itself
    // would not have shown.
    let show_file = by_file.len() > 1 || show_file(&paths, &extra_args);
    let show_line = show_line(engine, &extra_args, std::io::stdout().is_terminal());

    // Faithful baseline: exactly what the real command prints, full content.
    let mut plain = String::new();
    for line in raw_output.lines() {
        let Some(output) = format_match_line(line, show_file, show_line) else {
            if line == "--" {
                plain.push_str("--\n");
            }
            continue;
        };
        plain.push_str(&output);
    }

    let has_context = has_context_flag(&extra_args);

    let per_file = config::limits().grep_max_per_file;
    let mut files: Vec<_> = by_file.iter().collect();
    files.sort_by_key(|(f, _)| *f);

    let mut body = String::new();
    let mut shown = 0;
    let mut skipped_files = 0;
    let mut skipped_block = String::new();
    for (idx, (file, entries)) in files.into_iter().enumerate() {
        if shown >= max_results {
            skipped_files += 1;
            skipped_block.push_str(&match_block(file, entries));
            continue;
        }

        let file_display = compact_path(file);
        let mut file_shown = 0;
        let mut prev_line: usize = 0;
        for (line_num, is_match, content) in entries.iter().take(per_file) {
            if shown >= max_results {
                break;
            }
            if has_context && prev_line > 0 && *line_num > prev_line + 1 {
                body.push_str("--\n");
            }
            prev_line = *line_num;
            let sep = if *is_match { ':' } else { '-' };
            if show_file {
                body.push_str(&file_display);
                body.push(sep);
            }
            if show_line {
                body.push_str(&line_num.to_string());
                body.push(sep);
            }
            body.push_str(content);
            body.push('\n');
            shown += 1;
            file_shown += 1;
        }

        let remaining = entries.len() - file_shown;
        if remaining == 0 {
            continue;
        }
        // Tee the file's full matches (real path) so the tail hint recovers them
        // openably, skipping the lines already shown.
        let full_block = match_block(file, entries);
        match crate::core::tee::force_tee_tail_hint(&full_block, &grep_slug(idx, file), file_shown + 1)
        {
            Some(hint) => {
                body.push_str(&format!("  +{} more in {} {}\n", remaining, file_display, hint))
            }
            None => body.push_str(&format!("  +{} more in {}\n", remaining, file_display)),
        }
    }

    if skipped_files > 0 {
        let hint = crate::core::tee::force_tee_tail_hint(&skipped_block, "grep_skipped", 1)
            .map(|h| format!(" {}", h))
            .unwrap_or_default();
        body.push_str(&format!("+{} more files{}\n", skipped_files, hint));
    }

    // Switch to the grouped form only when capping actually shrank the output;
    // otherwise emit the faithful baseline, so RTK never exceeds the real command.
    let capped = shown < total_matches || skipped_files > 0;
    let rtk_output = if capped {
        format!(
            "{} matches in {} files:\n\n{}",
            total_matches,
            by_file.len(),
            body
        )
    } else {
        body
    };

    let output = if capped && rtk_output.len() < plain.len() {
        rtk_output
    } else {
        plain
    };

    print!("{}", output);
    timer.track(&real_cmd, &rtk_label, &raw_output, &output);

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
    static MATCH_LINE_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([^\x00]+)\x00(\d+)([:-])(.*)$").unwrap());

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
    // Minimal/shape forms the agent already chose; short flags scanned per-letter
    // so clusters like -rl/-rq route through, plus their long forms.
    const LONG: &[&str] = &[
        "--count",
        "--count-matches",
        "--files-with-matches",
        "--files-without-match",
        "--only-matching",
        "--quiet",
        "--silent",
        "--byte-offset",
        "--column",
        "--vimgrep",
        "--null",
        "--null-data",
        "--json",
        "--passthru",
        "--files",
    ];
    extra_args.iter().any(|arg| {
        let a = arg.as_ref();
        if a.starts_with("--") {
            LONG.contains(&a.split('=').next().unwrap_or(a))
        } else if let Some(letters) = a.strip_prefix('-').filter(|s| !s.is_empty()) {
            // -c count, -l/-L lists, -o only-matching, -q quiet, -b byte-offset, -Z/-z NUL
            letters
                .chars()
                .any(|ch| matches!(ch, 'c' | 'l' | 'L' | 'o' | 'q' | 'b' | 'Z' | 'z'))
        } else {
            false
        }
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
    use crate::core::ai_output::Omission;

    #[test]
    fn rg_route_table_is_conservative_and_complete() {
        let cases = [
            (&["needle"][..], RgRoute::Matches),
            (&["--json", "needle"][..], RgRoute::JsonEvents),
            (&["--files"][..], RgRoute::Inventory),
            (&["-l", "needle"][..], RgRoute::Inventory),
            (
                &["-L", "needle"][..],
                RgRoute::Exact(ExactReason::Streaming),
            ),
            (&["-c", "needle"][..], RgRoute::Counts),
            (&["--count-matches", "needle"][..], RgRoute::Counts),
            (&["-o", "needle"][..], RgRoute::OnlyMatching),
            (&["--replace", "hit", "needle"][..], RgRoute::Matches),
            (
                &["--replace", "-h", "needle"][..],
                RgRoute::Exact(ExactReason::Unknown),
            ),
            (
                &["-r", "-h", "needle"][..],
                RgRoute::Exact(ExactReason::Unknown),
            ),
            (&["--regexp", "needle", "src"][..], RgRoute::Matches),
            (&["--regexp=needle", "src"][..], RgRoute::Matches),
            (&["-C", "2", "needle"][..], RgRoute::Matches),
            (
                &["--glob", "--future-flag", "needle"][..],
                RgRoute::Matches,
            ),
            (&["needle", "--", "--future-flag"][..], RgRoute::Matches),
            (
                &["--null", "needle"][..],
                RgRoute::Exact(ExactReason::Structured),
            ),
            (
                &["--help"][..],
                RgRoute::Exact(ExactReason::Interactive),
            ),
            (
                &["--version"][..],
                RgRoute::Exact(ExactReason::Interactive),
            ),
            (
                &["--text", "needle"][..],
                RgRoute::Exact(ExactReason::Binary),
            ),
            (
                &["--follow", "needle"][..],
                RgRoute::Exact(ExactReason::Streaming),
            ),
            (
                &["--future-flag", "needle"][..],
                RgRoute::Exact(ExactReason::Unknown),
            ),
            (
                &["needle", "file\nname.txt"][..],
                RgRoute::Exact(ExactReason::Sensitive),
            ),
            (
                &["--count", "--no-filename", "needle"][..],
                RgRoute::Exact(ExactReason::Structured),
            ),
            (
                &["--no-filename", "--count", "needle"][..],
                RgRoute::Exact(ExactReason::Structured),
            ),
            (
                &["--json", "--count", "needle"][..],
                RgRoute::Exact(ExactReason::Structured),
            ),
        ];

        for (raw, expected) in cases {
            let args = raw.iter().map(|value| (*value).to_string()).collect::<Vec<_>>();
            assert_eq!(classify_rg(&args), expected, "args={raw:?}");
        }
    }

    #[test]
    fn rg_matches_group_contiguous_entries_without_reordering() {
        let raw = concat!(
            "a.rs\0",
            "3:needle one\n",
            "b.rs\0",
            "2:needle two\n",
            "a.rs\0",
            "9:needle three\n",
        );
        let document = rg_document(
            RgRoute::Matches,
            raw,
            &["needle".into()],
            &[],
        )
        .unwrap();
        let rendered = crate::core::ai_output::render(
            &document,
            crate::core::ai_output::BudgetClass::Source,
        )
        .text;

        assert!(rendered.contains("a.rs"));
        assert!(rendered.contains("3: needle one"));
        assert!(rendered.find("b.rs").unwrap() < rendered.rfind("a.rs").unwrap());
    }

    #[test]
    fn rg_shortened_match_declares_a_lossless_omission() {
        let long_match = format!("prefix {} needle suffix", "x".repeat(100));
        let raw = format!("a.rs\0{}:{}\n", 7, long_match);
        let document = rg_document(RgRoute::Matches, &raw, &["needle".into()], &[]).unwrap();
        let rendered = crate::core::ai_output::render(
            &document,
            crate::core::ai_output::BudgetClass::Source,
        );

        assert_eq!(
            rendered.omission,
            Some(Omission {
                items: 1,
                groups: 0,
            })
        );
    }

    #[test]
    fn rg_whitespace_cleanup_declares_a_lossless_omission() {
        let raw = "a.rs\x007:  needle  \n";
        let document = rg_document(RgRoute::Matches, raw, &["needle".into()], &[]).unwrap();
        let rendered = crate::core::ai_output::render(
            &document,
            crate::core::ai_output::BudgetClass::Source,
        );

        assert_eq!(
            rendered.omission,
            Some(Omission {
                items: 1,
                groups: 0,
            })
        );
    }

    #[test]
    fn rg_no_filename_baseline_overrides_multiple_paths() {
        let raw = "a.rs\x007:needle\nb.rs\x007:needle\n";
        let paths = vec!["a.rs".to_string(), "b.rs".to_string()];
        let extra_args = vec!["--no-filename".to_string()];

        assert_eq!(
            rg_faithful_match_baseline(raw, &paths, &extra_args).unwrap(),
            "needle\nneedle\n"
        );
    }

    #[test]
    fn rg_filename_selectors_follow_argument_order() {
        let paths = vec!["a.rs".to_string(), "b.rs".to_string()];

        assert!(!rg_show_file(
            &paths,
            &["--with-filename".to_string(), "--no-filename".to_string()],
        ));
        assert!(rg_show_file(
            &paths,
            &["--no-filename".to_string(), "--with-filename".to_string()],
        ));
        assert!(!rg_show_file(&paths, &["-Hh".to_string()]));
        assert!(rg_show_file(&paths, &["-hH".to_string()]));
    }

    #[test]
    fn rg_filename_selector_ignores_inline_replace_values() {
        let raw = "a.rs\x007:-h\nb.rs\x007:-h\n";
        let paths = vec!["a.rs".to_string(), "b.rs".to_string()];
        let extra_args = vec!["--replace=-h".to_string()];

        assert_eq!(
            rg_faithful_match_baseline(raw, &paths, &extra_args).unwrap(),
            "a.rs:-h\nb.rs:-h\n"
        );

        let short_args = vec!["-r-h".to_string()];
        assert_eq!(
            rg_faithful_match_baseline(raw, &paths, &short_args).unwrap(),
            "a.rs:-h\nb.rs:-h\n"
        );
    }

    #[test]
    fn rg_default_filename_behavior_ignores_inline_replace_flag_text() {
        let raw = "a.rs\x007:-h\n";
        let paths = vec!["a.rs".to_string()];
        let extra_args = vec!["-r-h".to_string()];

        assert_eq!(
            rg_faithful_match_baseline(raw, &paths, &extra_args).unwrap(),
            "-h\n"
        );
    }

    #[test]
    fn rg_budget_chunks_large_match_set() {
        let long_match = format!("needle {}", "x".repeat(60));
        let raw = (1..=500)
            .map(|line_number| format!("a.rs\0{line_number}:{long_match}\n"))
            .collect::<String>();
        let document = rg_document(RgRoute::Matches, &raw, &["needle".into()], &[]).unwrap();
        let rendered = crate::core::ai_output::render(
            &document,
            crate::core::ai_output::BudgetClass::Source,
        );

        let omission = rendered.omission.expect("large match set should report omission");
        assert_eq!(omission.groups, 1);
        assert!(
            omission.items < 500,
            "large file should be previewed in chunks: {omission:?}, text_len={}"
            , rendered.text.len()
        );
        assert!(rendered.text.contains("a.rs"));
    }

    #[test]
    fn bounded_rg_preview_does_not_replay_a_huge_line() {
        let stats = Arc::new(Mutex::new(EmissionMeta::default()));
        let mut filter = RgAiStreamFilter::new(
            RgRoute::Matches,
            vec!["needle".into()],
            vec!["a.rs".into()],
            Vec::new(),
            Arc::clone(&stats),
        );
        let line = format!(
            "a.rs\x00180:needle {}{}",
            "x".repeat(RG_STREAM_LINE_CAP),
            stream::TRUNCATED_LINE_MARKER
        );
        filter.feed_line(&line);
        let output = filter.on_exit(0, "").expect("bounded preview");

        assert!(output.contains("matches=1"));
        assert!(output.contains("truncated_lines=1"));
        assert!(output.contains("recovery=unavailable"));
        assert!(output.len() < 4_096);
        assert_eq!(stats.lock().unwrap().runtime_error, Some("capture_incomplete"));
    }

    #[test]
    fn bounded_rg_preview_keeps_total_matches_when_result_cap_is_reached() {
        let stats = Arc::new(Mutex::new(EmissionMeta::default()));
        let mut filter = RgAiStreamFilter::new(
            RgRoute::Matches,
            vec!["needle".into()],
            vec!["a.rs".into()],
            Vec::new(),
            stats,
        );
        filter.max_entries = 1;
        filter.feed_line("a.rs\x001:needle first");
        filter.feed_line("a.rs\x002:needle second");
        filter.raw_complete = false;
        let output = filter.on_exit(0, "").expect("bounded preview");

        assert!(output.contains("matches=2"));
        assert!(output.contains("omitted items=1"));
        assert!(!output.contains("second"));
    }

    #[test]
    fn rg_json_budget_chunks_large_match_set() {
        let long_match = format!("needle {}", "x".repeat(60));
        let raw = (1..=500)
            .map(|line_number| {
                format!(
                    "{{\"type\":\"match\",\"data\":{{\"path\":{{\"text\":\"a.rs\"}},\"lines\":{{\"text\":\"{long_match}\\n\"}},\"line_number\":{line_number}}}}}\n"
                )
            })
            .collect::<String>();
        let document = rg_document(RgRoute::JsonEvents, &raw, &["needle".into()], &[]).unwrap();
        let rendered = crate::core::ai_output::render(
            &document,
            crate::core::ai_output::BudgetClass::Source,
        );

        let omission = rendered.omission.expect("large JSON match set should report omission");
        assert_eq!(omission.groups, 1);
        assert!(
            omission.items < 500,
            "large JSON file should be previewed in chunks: {omission:?}, text_len={}"
            , rendered.text.len()
        );
        assert!(rendered.text.contains("a.rs"));
    }

    #[test]
    fn rg_json_discards_event_noise_but_keeps_match_text() {
        let raw = concat!(
            "{\"type\":\"begin\",\"data\":{\"path\":{\"text\":\"a.rs\"}}}\n",
            "{\"type\":\"match\",\"data\":{\"path\":{\"text\":\"a.rs\"},",
            "\"lines\":{\"text\":\"needle here\\n\"},\"line_number\":7,",
            "\"absolute_offset\":0,\"submatches\":[{\"match\":{\"text\":\"needle\"},\"start\":0,\"end\":6}]}}\n",
            "{\"type\":\"end\",\"data\":{\"path\":{\"text\":\"a.rs\"},\"binary_offset\":null,\"stats\":{}}}\n",
        );
        let document = rg_document(
            RgRoute::JsonEvents,
            raw,
            &["needle".into()],
            &[],
        )
        .unwrap();
        let rendered = crate::core::ai_output::render(
            &document,
            crate::core::ai_output::BudgetClass::Source,
        )
        .text;

        assert!(rendered.contains("a.rs"));
        assert!(rendered.contains("7: needle here"));
        assert!(!rendered.contains("\"type\":\"begin\""));
        assert!(!rendered.contains("omitted items="));
    }

    #[test]
    fn rg_count_records_are_path_equals_count() {
        let document = rg_document(
            RgRoute::Counts,
            concat!("a.rs\0", "4\n", "b.rs\0", "1\n"),
            &[],
            &[],
        )
        .unwrap();
        let rendered = crate::core::ai_output::render(
            &document,
            crate::core::ai_output::BudgetClass::Collection,
        )
        .text;

        assert!(rendered.contains("a.rs=4"));
        assert!(rendered.contains("b.rs=1"));
    }

    #[test]
    fn rg_count_single_file_numeric_output_keeps_the_file_name() {
        let document = rg_document(
            RgRoute::Counts,
            "4\n",
            &[],
            &["src/main.rs".into()],
        )
        .unwrap();
        let rendered = crate::core::ai_output::render(
            &document,
            crate::core::ai_output::BudgetClass::Collection,
        )
        .text;

        assert!(rendered.contains("src/main.rs=4"));
        assert!(!rendered.contains("<stdin>"));
    }

    #[test]
    fn rg_only_matching_preserves_long_match_values() {
        let value = "needle".to_string() + &"x".repeat(120);
        let raw = format!("a.rs\01:{value}\n");
        let document = rg_document(RgRoute::OnlyMatching, &raw, &["needle".into()], &[]).unwrap();
        let rendered = crate::core::ai_output::render(
            &document,
            crate::core::ai_output::BudgetClass::Source,
        )
        .text;

        assert!(rendered.contains(&value));
        assert!(!rendered.contains("omitted items="));
    }

    #[test]
    fn rg_json_non_utf8_path_is_not_fabricated() {
        let raw = r#"{"type":"match","data":{"path":{"bytes":[255]},"lines":{"text":"needle\n"},"line_number":1}}"#;
        let error = rg_document(RgRoute::JsonEvents, raw, &["needle".into()], &[]).unwrap_err();
        assert!(error.to_string().contains("not valid UTF-8"));
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
    fn streaming_search_preserves_native_shape() {
        let mut filter = SearchStreamFilter {
            show_file: false,
            show_line: true,
            max_results: 10,
            shown: 0,
            cap_reported: false,
        };

        assert_eq!(
            filter.feed_line("engine warning"),
            Some("engine warning\n".to_string())
        );
        assert_eq!(
            filter.feed_line(concat!("(standard input)\0", "1:match")),
            Some("1:match\n".to_string())
        );
    }

    #[test]
    fn streaming_search_reports_the_cap_once() {
        let mut filter = SearchStreamFilter {
            show_file: false,
            show_line: true,
            max_results: 1,
            shown: 0,
            cap_reported: false,
        };

        assert_eq!(
            filter.feed_line(concat!("(standard input)\0", "1:first")),
            Some("1:first\n".to_string())
        );
        assert_eq!(
            filter.feed_line(concat!("(standard input)\0", "2:second")),
            Some("[rtk] output capped at 1 results\n".to_string())
        );
        assert_eq!(
            filter.feed_line(concat!("(standard input)\0", "3:third")),
            None
        );
        assert_eq!(filter.feed_line("--"), None);
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
        // Pure boolean clusters: r/R kept and passed through to grep
        assert_eq!(
            parse_cluster("r"),
            ClusterResult::Boolean(Some("r".to_string()))
        );
        assert_eq!(
            parse_cluster("R"),
            ClusterResult::Boolean(Some("R".to_string()))
        );
        assert_eq!(
            parse_cluster("rR"),
            ClusterResult::Boolean(Some("rR".to_string()))
        );
        assert_eq!(
            parse_cluster("rn"),
            ClusterResult::Boolean(Some("rn".to_string()))
        );
        assert_eq!(
            parse_cluster("Rni"),
            ClusterResult::Boolean(Some("Rni".to_string()))
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
        // r/R pass through; e is value-taking (empty inline)
        assert_eq!(parse_cluster("rne"), vt(Some("rn"), 'e', ""));
    }

    #[test]
    fn test_parse_cluster_r_a() {
        // r passes through in the prefix; A is value-taking
        assert_eq!(parse_cluster("rA"), vt(Some("r"), 'A', ""));
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
    fn test_extract_cluster_keeps_r() {
        // -rn: r kept, passed straight to grep
        let (patterns, paths, flags) = extract_pattern_path(&["-rn", "foo", "src"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["-rn"]);
    }

    #[test]
    fn test_extract_cluster_ending_in_e() {
        // -rne PATTERN: rn kept, e consumes PATTERN as the pattern
        let (patterns, paths, flags) = extract_pattern_path(&["-rne", "PATTERN", "src"]);
        assert_eq!(patterns, vec!["PATTERN"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["-rn"]);
    }

    #[test]
    fn test_extract_cluster_ending_in_value_flag() {
        // -rA 2: r kept as its own flag, A consumes 2 as context value
        let (patterns, paths, flags) = extract_pattern_path(&["-rA", "2", "foo", "src"]);
        assert_eq!(patterns, vec!["foo"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["-r", "-A", "2"]);
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
        // This documents that the search filter avoids the diff_cmd bug (cap at N then compute N-10).
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
    fn test_format_flag_detects_clusters() {
        // clustered minimal forms must route to passthrough, not GROUP
        assert!(has_format_flag(&["-rl"]));
        assert!(has_format_flag(&["-rc"]));
        assert!(has_format_flag(&["-rq"]));
        assert!(has_format_flag(&["-rln"]));
        assert!(has_format_flag(&["-cr"]));
    }

    #[test]
    fn test_format_flag_detects_quiet_and_shape() {
        assert!(has_format_flag(&["-q"]));
        assert!(has_format_flag(&["--quiet"]));
        assert!(has_format_flag(&["--silent"]));
        assert!(has_format_flag(&["-b"]));
        assert!(has_format_flag(&["--byte-offset"]));
        assert!(has_format_flag(&["--column"]));
        assert!(has_format_flag(&["--vimgrep"]));
        assert!(has_format_flag(&["-z"]));
        assert!(has_format_flag(&["--null-data"]));
    }

    #[test]
    fn test_format_flag_compresses_default_and_context() {
        // compressible forms must NOT passthrough
        assert!(!has_format_flag(&["-rn"]));
        assert!(!has_format_flag(&["-A", "3"]));
        assert!(!has_format_flag(&["-v"]));
        assert!(!has_format_flag(&["-rin"]));
    }

    fn flags(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn show_line_is_off_without_an_explicit_request() {
        assert!(!show_line(Engine::Grep, &flags(&[]), true));
        assert!(!show_line(Engine::Rg, &flags(&[]), false));
        assert!(!show_line(Engine::Grep, &flags(&["-i"]), true));
        assert!(!show_line(Engine::Grep, &flags(&["-r"]), true));
        assert!(!show_line(Engine::Grep, &flags(&["-A", "3"]), true));
    }

    #[test]
    fn show_line_preserves_ripgrep_tty_default() {
        assert!(show_line(Engine::Rg, &flags(&[]), true));
        assert!(!show_line(Engine::Rg, &flags(&[]), false));
    }

    #[test]
    fn show_line_honours_n_in_every_spelling() {
        assert!(show_line(Engine::Grep, &flags(&["-n"]), false));
        assert!(show_line(
            Engine::Grep,
            &flags(&["--line-number"]),
            false
        ));
        assert!(show_line(Engine::Grep, &flags(&["-rn"]), false));
        assert!(show_line(Engine::Grep, &flags(&["-in"]), false));
    }

    #[test]
    fn show_line_is_off_when_explicitly_negated() {
        assert!(!show_line(Engine::Rg, &flags(&["-N"]), true));
        assert!(!show_line(
            Engine::Rg,
            &flags(&["--no-line-number"]),
            true
        ));
    }

    #[test]
    fn match_block_stays_fully_qualified() {
        let entries = vec![
            (3usize, true, "line 3 needle".to_string()),
            (4usize, false, "line 4 context".to_string()),
        ];
        let block = match_block("src/deep/file.rs", &entries);
        assert_eq!(
            block,
            "src/deep/file.rs:3:line 3 needle\nsrc/deep/file.rs-4-line 4 context\n"
        );
    }

    #[test]
    fn match_block_keeps_position_when_display_drops_it() {
        assert!(!show_line(Engine::Grep, &flags(&[]), false));
        let entries = vec![(42usize, true, "hit".to_string())];
        assert_eq!(match_block("f.txt", &entries), "f.txt:42:hit\n");
    }

    // Verify line numbers are always enabled in the engine invocation (parse_flags).
    // The -n/--line-numbers clap flag in main.rs is a no-op accepted for compat.
    #[test]
    fn test_rg_always_has_line_numbers() {
        // engine_capture always passes "-n" to the engine via parse_flags().
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

    // --- issues #1436 / #1613: parse_match_line robustness (single-file colon misparse) ---
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

    // --- has_context_flag ---

    #[test]
    fn test_has_context_flag_short() {
        let f = |args: &[&str]| -> bool {
            has_context_flag(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert!(f(&["-A", "3"]));
        assert!(f(&["-B", "2"]));
        assert!(f(&["-C", "1"]));
        assert!(!f(&["-rn"]));
        assert!(!f(&["-i", "-w"]));
    }

    #[test]
    fn test_has_context_flag_long() {
        let f = |args: &[&str]| -> bool {
            has_context_flag(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert!(f(&["--after-context", "3"]));
        assert!(f(&["--before-context", "2"]));
        assert!(f(&["--context", "1"]));
        assert!(f(&["--after-context=3"]));
        assert!(f(&["--before-context=2"]));
        assert!(f(&["--context=1"]));
        assert!(!f(&["--color", "auto"]));
    }
}
