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
use crate::core::{args_utils, config};
use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::io::{IsTerminal, Read, Write};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

/// Short single-char flags that consume one following token (or inline remainder)
/// as their value. `-e` is handled separately — its value goes to `patterns`.
/// Includes all rg short flags that take a value argument except `-e` and `-r`
/// (stripped) and `-E` (dialect, left to #2138). Failure mode for a missing
/// entry: the value becomes a positional (visible wrong result, not silent).
const VALUE_FLAGS_SHORT: &[u8] = b"ABCMTdfgjmt";

/// grep value flags used when deciding whether a following RTK-looking token
/// is actually native grep data.
const GREP_VALUE_FLAGS_SHORT: &[u8] = b"ABCDdfm";

/// Long flags that consume the NEXT token as their value (space-separated form).
/// Inline `=` form (`--flag=value`) is one token and passes through unchanged.
/// `--regexp` is handled separately (its value goes to `patterns`).
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
    "--sort",
    "--sortr",
    "--threads",
    "--type",
    "--type-add",
    "--type-clear",
    "--type-not",
];

const GREP_VALUE_FLAGS_LONG: &[&str] = &[
    "--after-context",
    "--before-context",
    "--context",
    "--binary-files",
    "--devices",
    "--directories",
    "--regexp",
    "--file",
    "--max-count",
    "--exclude",
    "--exclude-from",
    "--exclude-dir",
    "--include",
    "--label",
    "--group-separator",
    "--color",
    "--colour",
    "--encoding",
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
fn parse_cluster_with_values(rest: &str, value_flags: &[u8]) -> ClusterResult {
    let bytes = rest.as_bytes();
    let mut raw_prefix = String::new();
    let mut j = 0;
    while j < bytes.len() {
        let ch = bytes[j];
        let is_e = ch == b'e';
        if is_e || value_flags.contains(&ch) {
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
fn extract_pattern_path_with<T: AsRef<str>>(
    args: &[T],
    value_flags_short: &[u8],
    value_flags_long: &[&str],
) -> (Vec<String>, Vec<String>, Vec<String>) {
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
            if let Some(value) = arg.strip_prefix("--regexp=") {
                e_patterns.push(value.to_string());
                i += 1;
                continue;
            }
            // Other long value-taking flags: consume next token as value.
            if value_flags_long.contains(&arg) {
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
            Some(rest) if !rest.is_empty() => match parse_cluster_with_values(rest, value_flags_short) {
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

fn extract_pattern_path<T: AsRef<str>>(args: &[T]) -> (Vec<String>, Vec<String>, Vec<String>) {
    extract_pattern_path_with(args, VALUE_FLAGS_SHORT, VALUE_FLAGS_LONG)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GrepNativeArgs {
    argv: Vec<String>,
    operand: Vec<bool>,
    missing_operand: Option<String>,
}

impl GrepNativeArgs {
    fn is_flag(&self, index: usize) -> bool {
        !self.operand.get(index).copied().unwrap_or(false)
    }
}

fn extract_grep_pattern_path<T: AsRef<str>>(args: &[T]) -> (Vec<String>, Vec<String>, GrepNativeArgs) {
    let args = args.iter().map(|arg| arg.as_ref().to_string()).collect::<Vec<_>>();
    let native_operand = grep_native_operand_roles(&args);
    let mut e_patterns = Vec::new();
    let mut positionals = Vec::new();
    let mut native = GrepNativeArgs {
        argv: Vec::new(),
        operand: Vec::new(),
        missing_operand: None,
    };
    let mut past_dashdash = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if past_dashdash {
            positionals.push(arg.clone());
            i += 1;
            continue;
        }
        if arg == "--" {
            past_dashdash = true;
            i += 1;
            continue;
        }
        if arg == "--regexp" {
            if let Some(pattern) = args.get(i + 1) {
                e_patterns.push(pattern.clone());
                i += 2;
            } else {
                native.argv.push(arg.clone());
                native.operand.push(false);
                native.missing_operand = Some(arg.clone());
                i += 1;
            }
            continue;
        }
        if let Some(pattern) = arg.strip_prefix("--regexp=") {
            e_patterns.push(pattern.to_string());
            i += 1;
            continue;
        }
        if arg.starts_with("--") {
            native.argv.push(arg.clone());
            native.operand.push(false);
            let (name, has_inline_value) = arg
                .split_once('=')
                .map_or((arg.as_str(), false), |(name, _)| (name, true));
            if !has_inline_value && GREP_VALUE_FLAGS_LONG.contains(&name) {
                if i + 1 < args.len() {
                    native.argv.push(args[i + 1].clone());
                    native.operand.push(true);
                    i += 2;
                } else {
                    if !matches!(name, "--color" | "--colour") {
                        native.missing_operand = Some(name.to_string());
                    }
                    i += 1;
                }
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(rest) = arg.strip_prefix('-').filter(|rest| !rest.is_empty()) {
            match parse_grep_cluster(rest) {
                ClusterResult::Boolean(_) => {
                    native.argv.push(arg.clone());
                    native.operand.push(false);
                    i += 1;
                }
                ClusterResult::ValueTaking { flag: 'e', inline, .. } => {
                    if !inline.is_empty() {
                        e_patterns.push(inline);
                        i += 1;
                    } else if let Some(pattern) = args.get(i + 1) {
                        e_patterns.push(pattern.clone());
                        i += 2;
                    } else {
                        native.argv.push(arg.clone());
                        native.operand.push(false);
                        native.missing_operand = Some("-e".to_string());
                        i += 1;
                    }
                }
                ClusterResult::ValueTaking { flag, inline, .. } => {
                    native.argv.push(arg.clone());
                    native.operand.push(false);
                    if inline.is_empty() && i + 1 < args.len() {
                        native.argv.push(args[i + 1].clone());
                        native.operand.push(true);
                        i += 2;
                    } else {
                        if inline.is_empty() {
                            native.missing_operand = Some(format!("-{flag}"));
                        }
                        i += 1;
                    }
                }
            }
            continue;
        }
        debug_assert!(!native_operand[i]);
        positionals.push(arg.clone());
        i += 1;
    }

    let (patterns, paths) = if e_patterns.is_empty() {
        (
            positionals.iter().take(1).cloned().collect(),
            positionals.into_iter().skip(1).collect(),
        )
    } else {
        (e_patterns, positionals)
    };
    (patterns, paths, native)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParserMode {
    Nul,
    OrdinaryWindows,
}

#[cfg(test)]
fn parse_cluster(rest: &str) -> ClusterResult {
    parse_cluster_with_values(rest, VALUE_FLAGS_SHORT)
}

fn parse_grep_cluster(rest: &str) -> ClusterResult {
    parse_cluster_with_values(rest, GREP_VALUE_FLAGS_SHORT)
}

fn grep_native_operand_roles(args: &[String]) -> Vec<bool> {
    let mut roles = vec![false; args.len()];
    let mut past_dashdash = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if past_dashdash {
            roles[i] = true;
            i += 1;
            continue;
        }
        if arg == "--" {
            past_dashdash = true;
            i += 1;
            continue;
        }
        if arg.starts_with("--") {
            let (name, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(name, value)| (name, Some(value)));
            if inline.is_none()
                && GREP_VALUE_FLAGS_LONG.contains(&name)
                && i + 1 < args.len()
            {
                roles[i + 1] = true;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(rest) = arg.strip_prefix('-').filter(|rest| !rest.is_empty()) {
            if let ClusterResult::ValueTaking { inline, .. } = parse_grep_cluster(rest) {
                if inline.is_empty() && i + 1 < args.len() {
                    roles[i + 1] = true;
                    i += 2;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    roles
}

fn parser_mode_for(
    engine: Engine,
    long_null_supported: bool,
    short_null_supported: bool,
    windows: bool,
) -> Option<ParserMode> {
    match engine {
        Engine::Rg => Some(ParserMode::Nul),
        Engine::Grep if long_null_supported || short_null_supported => {
            Some(ParserMode::Nul)
        }
        // Ordinary grep output is ambiguous on POSIX: a filename may contain ':'.
        Engine::Grep if windows => Some(ParserMode::OrdinaryWindows),
        Engine::Grep => None,
    }
}

fn parser_mode(engine: Engine) -> Option<ParserMode> {
    parser_mode_for(
        engine,
        *GREP_NULL_SUPPORTED,
        *GREP_SHORT_NULL_SUPPORTED,
        cfg!(windows),
    )
}

const STRUCTURED_PARSER_UNAVAILABLE: &str =
    "structured grep output is unavailable because this grep lacks NUL-safe filename output (--null or -Z)";
const STRUCTURED_CONTEXT_UNAVAILABLE: &str =
    "structured context output requires NUL-safe grep filename output (--null or -Z)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StructuredParserPolicy {
    Continue(ParserMode),
    JsonError,
    HumanError,
}

fn structured_parser_policy(parser_mode: Option<ParserMode>, json: bool) -> StructuredParserPolicy {
    match parser_mode {
        Some(mode) => StructuredParserPolicy::Continue(mode),
        None if json => StructuredParserPolicy::JsonError,
        None => StructuredParserPolicy::HumanError,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StructuredContextPolicy {
    Continue,
    JsonError,
    HumanError,
}

fn structured_context_policy(
    parser_mode: ParserMode,
    has_context: bool,
    json: bool,
) -> StructuredContextPolicy {
    if parser_mode != ParserMode::OrdinaryWindows || !has_context {
        StructuredContextPolicy::Continue
    } else if json {
        StructuredContextPolicy::JsonError
    } else {
        StructuredContextPolicy::HumanError
    }
}

fn unparsed_signal(stdout: &str, mode: ParserMode) -> usize {
    stdout
        .lines()
        .filter(|line| {
            let clean = strip_ansi(line);
            let trimmed = clean.trim();
            !trimmed.is_empty()
                && trimmed != "--"
                && parse_match_line(&clean, mode).is_none()
        })
        .count()
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
    fn parse_flags(self) -> Vec<&'static str> {
        match self {
            Engine::Grep => {
                let mut flags = vec!["-n", "-H", "-I"];
                if *GREP_NULL_SUPPORTED {
                    flags.push("--null");
                } else if *GREP_SHORT_NULL_SUPPORTED {
                    flags.push("-Z");
                }
                flags
            }
            Engine::Rg => vec!["-n", "--with-filename", "--null"],
        }
    }
}

static GREP_NULL_SUPPORTED: LazyLock<bool> = LazyLock::new(|| {
    let mut cmd = resolved_command("grep");
    cmd.args(["--null", "--help"]);
    exec_capture(&mut cmd)
        .map(|result| result.exit_code == 0)
        .unwrap_or(false)
});

static GREP_SHORT_NULL_SUPPORTED: LazyLock<bool> = LazyLock::new(|| {
    let mut cmd = resolved_command("grep");
    cmd.args(["-Z", "--help"]);
    exec_capture(&mut cmd)
        .map(|result| result.exit_code == 0)
        .unwrap_or(false)
});

static GREP_LINE_BUFFERED_SUPPORTED: LazyLock<bool> = LazyLock::new(|| {
    let mut cmd = resolved_command("grep");
    cmd.args(["--line-buffered", "--help"]);
    exec_capture(&mut cmd)
        .map(|result| result.exit_code == 0)
        .unwrap_or(false)
});

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
    for flag in engine.parse_flags() {
        cmd.arg(flag);
    }
    for a in extra_args {
        cmd.arg(a.as_ref());
    }
    if line_buffered
        && (matches!(engine, Engine::Rg) || *GREP_LINE_BUFFERED_SUPPORTED)
    {
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

fn format_match_line(
    line: &str,
    show_file: bool,
    show_line: bool,
    mode: ParserMode,
) -> Option<String> {
    let line = strip_ansi(line);
    let (file, line_num, is_match, content) = parse_match_line(&line, mode)?;
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
    parser_mode: ParserMode,
}

impl StreamFilter for SearchStreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        let Some(output) = format_match_line(line, self.show_file, self.show_line, self.parser_mode) else {
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

fn grep_show_file(paths: &[String], extra_args: &GrepNativeArgs) -> bool {
    paths.len() > 1
        || paths.iter().any(|p| std::path::Path::new(p).is_dir())
        || grep_has_short_flag(extra_args, 'H')
        || grep_has_short_flag(extra_args, 'r')
        || grep_has_short_flag(extra_args, 'R')
        || grep_flag_tokens(extra_args)
            .any(|(_, f)| f == "--with-filename" || f == "--recursive")
}

fn show_line(extra_args: &[String]) -> bool {
    (has_short_flag(extra_args, 'n')
        || extra_args.iter().any(|f| f == "--line-number"))
        && !has_short_flag(extra_args, 'N')
        && !extra_args.iter().any(|f| f == "--no-line-number")
}

fn grep_show_line(extra_args: &GrepNativeArgs) -> bool {
    (grep_has_short_flag(extra_args, 'n')
        || grep_flag_tokens(extra_args).any(|(_, f)| f == "--line-number"))
        && !grep_has_short_flag(extra_args, 'N')
        && !grep_flag_tokens(extra_args).any(|(_, f)| f == "--no-line-number")
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
    let Some(parser_mode) = parser_mode(engine) else {
        return Err(anyhow::anyhow!(
            "structured grep parsing unavailable without NUL-safe engine output"
        ));
    };
    let filter = SearchStreamFilter {
        show_file: show_file(paths, extra_args),
        show_line: show_line(extra_args),
        max_results,
        shown: 0,
        cap_reported: false,
        parser_mode,
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

fn grep_flag_tokens(args: &GrepNativeArgs) -> impl Iterator<Item = (usize, &str)> {
    args.argv
        .iter()
        .enumerate()
        .filter(move |(index, _)| args.is_flag(*index))
        .map(|(index, flag)| (index, flag.as_str()))
}

fn grep_has_short_flag(args: &GrepNativeArgs, ch: char) -> bool {
    grep_flag_tokens(args).any(|(_, flag)| {
        flag.starts_with('-') && !flag.starts_with("--") && flag[1..].contains(ch)
    })
}

fn grep_has_context_flag(args: &GrepNativeArgs) -> bool {
    grep_has_short_flag(args, 'A')
        || grep_has_short_flag(args, 'B')
        || grep_has_short_flag(args, 'C')
        || grep_flag_tokens(args).any(|(_, f)| {
            f == "--after-context"
                || f == "--before-context"
                || f == "--context"
                || f.starts_with("--after-context=")
                || f.starts_with("--before-context=")
                || f.starts_with("--context=")
        })
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

struct BoundedCaptureResult {
    result: CaptureResult,
    truncated: bool,
}

struct BoundedText {
    text: String,
    truncated: bool,
}

fn append_bounded_text(output: &mut String, text: &str, cap: usize, truncated: &mut bool) {
    if *truncated {
        return;
    }
    for ch in text.chars() {
        if ch == '\n' && output.ends_with('\r') {
            output.pop();
        }
        let width = ch.len_utf8();
        let fits = output.len().saturating_add(width) <= cap;
        if fits {
            output.push(ch);
        } else {
            *truncated = true;
            break;
        }
    }
}

/// Read a stream with fixed-size input and output bounds.
/// Invalid UTF-8 becomes U+FFFD; CRLF is normalized to LF like `lines()`.
fn capture_stream_bounded<R: Read>(mut reader: R, cap: usize) -> std::io::Result<BoundedText> {
    const CHUNK_SIZE: usize = 8192;
    let mut output = String::with_capacity(cap);
    let mut chunk = [0u8; CHUNK_SIZE];
    let mut pending = Vec::with_capacity(3);
    let mut truncated = false;

    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(&chunk[..read]);
        let mut offset = 0;
        loop {
            match std::str::from_utf8(&pending[offset..]) {
                Ok(valid) => {
                    append_bounded_text(&mut output, valid, cap, &mut truncated);
                    offset = pending.len();
                    break;
                }
                Err(error) => {
                    let valid_end = offset + error.valid_up_to();
                    append_bounded_text(
                        &mut output,
                        std::str::from_utf8(&pending[offset..valid_end]).unwrap_or_default(),
                        cap,
                        &mut truncated,
                    );
                    if let Some(error_len) = error.error_len() {
                        append_bounded_text(&mut output, "\u{FFFD}", cap, &mut truncated);
                        offset = valid_end + error_len;
                    } else {
                        pending.drain(..valid_end);
                        break;
                    }
                }
            }
        }
        if offset == pending.len() {
            pending.clear();
        } else if offset > 0 {
            pending.drain(..offset);
        }
    }

    if !pending.is_empty() {
        append_bounded_text(&mut output, "\u{FFFD}", cap, &mut truncated);
    }
    if !output.is_empty() && !output.ends_with('\n') {
        append_bounded_text(&mut output, "\n", cap, &mut truncated);
    }

    Ok(BoundedText {
        text: output,
        truncated,
    })
}

fn engine_capture_bounded<T: AsRef<str>>(
    engine: Engine,
    extra_args: &[T],
    patterns: &[String],
    paths: &[String],
) -> Result<BoundedCaptureResult> {
    const CAP: usize = stream::RAW_CAP;
    let mut cmd = engine_command(engine, extra_args, patterns, paths, false);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().context("Failed to spawn process")?;
    let stdout = child.stdout.take().context("No child stdout handle")?;
    let stderr = child.stderr.take().context("No child stderr handle")?;
    let stderr_thread = std::thread::spawn(move || {
        capture_stream_bounded(stderr, CAP)
    });
    let stdout_capture = capture_stream_bounded(stdout, CAP);
    let stderr = stderr_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stderr capture thread panicked"))?
        .context("Failed to capture stderr")?;
    let stdout_capture = stdout_capture.context("Failed to capture stdout")?;
    let status = child.wait().context("Failed to wait for child")?;
    Ok(BoundedCaptureResult {
        result: CaptureResult {
            stdout: stdout_capture.text,
            stderr: stderr.text,
            exit_code: stream::status_to_exit_code(status),
        },
        truncated: stdout_capture.truncated || stderr.truncated,
    })
}

#[allow(clippy::too_many_arguments)]
fn format_stream_row(
    file: &str,
    line_num: usize,
    is_match: bool,
    content: &str,
    show_file: bool,
    show_line: bool,
    max_line_chars: Option<usize>,
    pattern: &str,
    context_only: bool,
    clipped_lines: &mut usize,
) -> String {
    let source = if context_only {
        context_only_excerpt(content, pattern)
    } else {
        content.trim().to_string()
    };
    let (text, clipped) = clipped_text(&source, max_line_chars, pattern);
    *clipped_lines += usize::from(clipped);
    let sep = if is_match { ':' } else { '-' };
    let mut output = String::new();
    if show_file {
        output.push_str(file);
        output.push(sep);
    }
    if show_line {
        output.push_str(&line_num.to_string());
        output.push(sep);
    }
    output.push_str(&text);
    output.push('\n');
    output
}

fn has_forced_color(flags: &GrepNativeArgs) -> bool {
    grep_flag_tokens(flags).any(|(index, flag)| {
        if flag == "--color=always" || flag == "--colour=always" {
            return true;
        }
        if flag != "--color" && flag != "--colour" {
            return false;
        }
        flags
            .argv
            .get(index + 1)
            .filter(|_| flags.operand.get(index + 1) == Some(&true))
            .is_some_and(|value| value == "always")
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum GrepMode {
    #[default]
    Matches,
    FilesOnly,
    CountByFile,
    TopFiles(usize),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct GrepRtkOptions {
    mode: GrepMode,
    context_only: bool,
    all: bool,
    full_lines: bool,
    agent_safe: bool,
    json: bool,
    max_matches: Option<usize>,
    max_per_file: Option<usize>,
    max_line_chars: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GrepEffectiveOptions {
    mode: GrepMode,
    all: bool,
    full_lines: bool,
    agent_safe: bool,
    json: bool,
    context_only: bool,
    max_matches: Option<usize>,
    max_per_file: Option<usize>,
    max_line_chars: Option<usize>,
    summary: bool,
}

#[derive(Debug, Serialize)]
struct GrepJsonMatch {
    line: usize,
    kind: &'static str,
    text: String,
}

#[derive(Debug, Serialize)]
struct GrepJsonFile {
    path: String,
    true_match_count: usize,
    displayed_count: usize,
    omitted_count: usize,
    displayed_context_count: usize,
    omitted_context_count: usize,
    matches: Vec<GrepJsonMatch>,
}

#[derive(Debug, Serialize)]
struct GrepJsonDocument {
    schema: &'static str,
    mode: &'static str,
    engine: &'static str,
    patterns: Vec<String>,
    searched_paths: Vec<String>,
    total_match_count: usize,
    matched_file_count: usize,
    displayed_match_count: usize,
    omitted_match_count: usize,
    clipped_line_count: usize,
    displayed_context_row_count: usize,
    omitted_context_row_count: usize,
    files: Vec<GrepJsonFile>,
    recovery_hints: Vec<String>,
    recovery_argv: Vec<Vec<String>>,
    error: Option<String>,
}

#[derive(Clone, Debug)]
struct RenderedFile {
    path: String,
    true_match_count: usize,
    displayed_count: usize,
    omitted_count: usize,
    displayed_context_count: usize,
    omitted_context_count: usize,
    rows: Vec<(usize, bool, String)>,
}

fn parse_bool_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn env_bool_value(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|value| parse_bool_value(&value))
}

fn config_agent_safe() -> bool {
    config::Config::load()
        .map(|c| c.agent.is_some_and(|agent| agent.safe_mode))
        .unwrap_or(false)
}

fn parse_grep_number(name: &str, value: &str) -> Result<usize> {
    value
        .parse::<usize>()
        .with_context(|| format!("invalid {} value {:?}; expected a non-negative integer", name, value))
}

fn take_grep_value(args: &[String], i: usize, name: &str) -> Result<(String, usize)> {
    let Some(value) = args.get(i + 1) else {
        return Err(anyhow::anyhow!("missing value for {}", name));
    };
    Ok((value.clone(), i + 2))
}

fn parse_grep_rtk_options(args: &[String]) -> Result<(GrepRtkOptions, Vec<String>)> {
    let mut options = GrepRtkOptions::default();
    let mut forwarded = Vec::with_capacity(args.len());
    let native_operands = grep_native_operand_roles(args);
    let mut i = 0;
    let mut past_dashdash = false;

    while i < args.len() {
        let arg = &args[i];
        if native_operands[i] {
            forwarded.push(arg.clone());
            i += 1;
            continue;
        }
        if past_dashdash {
            forwarded.push(arg.clone());
            i += 1;
            continue;
        }
        if arg == "--" {
            past_dashdash = true;
            forwarded.push(arg.clone());
            i += 1;
            continue;
        }

        let (name, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(name, value)| (name, Some(value)));
        match name {
            "--files-only" if inline.is_none() => {
                if options.mode != GrepMode::Matches {
                    return Err(anyhow::anyhow!("--files-only conflicts with another result mode"));
                }
                options.mode = GrepMode::FilesOnly;
                i += 1;
            }
            "--count-by-file" if inline.is_none() => {
                if options.mode != GrepMode::Matches {
                    return Err(anyhow::anyhow!("--count-by-file conflicts with another result mode"));
                }
                options.mode = GrepMode::CountByFile;
                i += 1;
            }
            "--top-files" => {
                let (value, next) = if let Some(value) = inline {
                    (value.to_string(), i + 1)
                } else {
                    take_grep_value(args, i, "--top-files")?
                };
                if options.mode != GrepMode::Matches {
                    return Err(anyhow::anyhow!("--top-files conflicts with another result mode"));
                }
                options.mode = GrepMode::TopFiles(parse_grep_number("--top-files", &value)?);
                i = next;
            }
            "--all" if inline.is_none() => {
                options.all = true;
                i += 1;
            }
            "--full-lines" if inline.is_none() => {
                options.full_lines = true;
                i += 1;
            }
            "--agent-safe" if inline.is_none() => {
                options.agent_safe = true;
                i += 1;
            }
            "--context-only" if inline.is_none() => {
                options.context_only = true;
                i += 1;
            }
            "--json" if inline.is_none() => {
                options.json = true;
                i += 1;
            }
            "--max-matches" => {
                let (value, next) = if let Some(value) = inline {
                    (value.to_string(), i + 1)
                } else {
                    take_grep_value(args, i, "--max-matches")?
                };
                options.max_matches = Some(parse_grep_number("--max-matches", &value)?);
                i = next;
            }
            "--max-per-file" => {
                let (value, next) = if let Some(value) = inline {
                    (value.to_string(), i + 1)
                } else {
                    take_grep_value(args, i, "--max-per-file")?
                };
                options.max_per_file = Some(parse_grep_number("--max-per-file", &value)?);
                i = next;
            }
            "--max-line-chars" => {
                let (value, next) = if let Some(value) = inline {
                    (value.to_string(), i + 1)
                } else {
                    take_grep_value(args, i, "--max-line-chars")?
                };
                options.max_line_chars = Some(parse_grep_number("--max-line-chars", &value)?);
                i = next;
            }
            _ => {
                forwarded.push(arg.clone());
                i += 1;
            }
        }
    }

    if options.mode != GrepMode::Matches && options.all {
        // Structured modes already use true uncapped totals; --all is harmless.
    }
    if options.full_lines && options.max_line_chars.is_some() {
        return Err(anyhow::anyhow!("--full-lines conflicts with --max-line-chars"));
    }
    if options.all && (options.max_matches.is_some() || options.max_per_file.is_some()) {
        return Err(anyhow::anyhow!("--all conflicts with explicit match caps"));
    }
    Ok((options, forwarded))
}

fn json_requested_before_dashdash(args: &[String]) -> bool {
    let native_operands = grep_native_operand_roles(args);
    args.iter()
        .enumerate()
        .any(|(i, arg)| arg == "--json" && !native_operands[i])
}

fn effective_grep_options(
    legacy_max_len: usize,
    legacy_max: usize,
    context_only: bool,
    options: &GrepRtkOptions,
) -> GrepEffectiveOptions {
    let agent_safe = options.agent_safe
        || env_bool_value("RTK_AGENT_SAFE").unwrap_or_else(config_agent_safe);
    let safe_max_matches = agent_safe.then_some(80);
    let safe_max_per_file = agent_safe.then_some(5);
    let safe_max_line_chars = agent_safe.then_some(240);
    let max_matches = if options.all {
        None
    } else {
        options.max_matches.or(safe_max_matches).or(Some(legacy_max))
    };
    let max_per_file = if options.all {
        None
    } else {
        options.max_per_file
            .or(safe_max_per_file)
            .or_else(|| Some(config::limits().grep_max_per_file))
    };
    let max_line_chars = if options.full_lines {
        None
    } else {
        options
            .max_line_chars
            .or(safe_max_line_chars)
            .or(Some(legacy_max_len))
    };
    GrepEffectiveOptions {
        mode: options.mode,
        all: options.all,
        full_lines: options.full_lines,
        agent_safe,
        json: options.json,
        context_only,
        max_matches,
        max_per_file,
        max_line_chars,
        summary: agent_safe
            || options.max_matches.is_some()
            || options.max_per_file.is_some()
            || options.max_line_chars.is_some(),
    }
}

struct AgentSafeStreamFilter {
    show_file: bool,
    show_line: bool,
    max_results: Option<usize>,
    max_per_file: Option<usize>,
    max_line_chars: Option<usize>,
    pattern: String,
    context_only: bool,
    shown: usize,
    total_matches: usize,
    omitted_per_file: usize,
    clipped_lines: usize,
    shown_by_file: HashMap<String, usize>,
    parser_mode: ParserMode,
    context: ContextSpec,
    pending_context: std::collections::VecDeque<(String, usize, String)>,
    active_file: Option<String>,
    after_remaining: usize,
    total_context: usize,
    displayed_context: usize,
    last_output_file: Option<String>,
    last_output_line: Option<usize>,
    emit_summary: bool,
    parse_failed: bool,
}

impl AgentSafeStreamFilter {
    fn append_row(
        &mut self,
        output: &mut String,
        file: &str,
        line_num: usize,
        is_match: bool,
        content: &str,
    ) {
        if (self.context.before > 0 || self.context.after > 0)
            && self.last_output_file.as_deref() == Some(file)
            && self
                .last_output_line
                .is_some_and(|previous| line_num > previous.saturating_add(1))
        {
            output.push_str("--\n");
        }
        output.push_str(&format_stream_row(
            file,
            line_num,
            is_match,
            content,
            self.show_file,
            self.show_line,
            self.max_line_chars,
            &self.pattern,
            self.context_only,
            &mut self.clipped_lines,
        ));
        self.last_output_file = Some(file.to_string());
        self.last_output_line = Some(line_num);
    }

}

impl StreamFilter for AgentSafeStreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        let line = strip_ansi(line);
        let Some((file, line_num, is_match, content)) =
            parse_match_line(&line, self.parser_mode)
        else {
            if line != "--" {
                self.parse_failed = true;
            }
            return None;
        };
        if is_match {
            if self.active_file.as_deref() != Some(file.as_str()) {
                self.pending_context.clear();
                self.after_remaining = 0;
            }
            self.total_matches += 1;
            let file_shown = self.shown_by_file.get(&file).copied().unwrap_or_default();
            if self.max_results.is_some_and(|cap| self.shown >= cap)
                || self.max_per_file.is_some_and(|cap| file_shown >= cap)
            {
                self.omitted_per_file += 1;
                self.pending_context.clear();
                self.after_remaining = 0;
                return None;
            }
            let mut output = String::new();
            let pending = std::mem::take(&mut self.pending_context);
            for (pending_file, pending_line, pending_text) in pending {
                self.append_row(&mut output, &pending_file, pending_line, false, &pending_text);
                self.displayed_context += 1;
            }
            self.shown_by_file.insert(file.clone(), file_shown + 1);
            self.shown += 1;
            self.append_row(&mut output, &file, line_num, true, content);
            self.after_remaining = self.context.after;
            self.active_file = Some(file);
            return Some(output);
        }
        self.total_context += 1;
        if self.after_remaining > 0 && self.active_file.as_deref() == Some(file.as_str()) {
            self.after_remaining -= 1;
            self.displayed_context += 1;
            let mut output = String::new();
            self.append_row(&mut output, &file, line_num, false, content);
            return Some(output);
        }
        if self.context.before > 0 {
            if self.pending_context.len() == self.context.before {
                self.pending_context.pop_front();
            }
            self.pending_context
                .push_back((file, line_num, content.to_string()));
        }
        None
    }

    fn flush(&mut self) -> String {
        if self.parse_failed || !self.emit_summary {
            return String::new();
        }
        format!(
            "summary: total={} shown={} omitted_total={} omitted_per_file={} context_shown={} context_omitted={} clipped_lines={}\n",
            self.total_matches,
            self.shown,
            self.total_matches.saturating_sub(self.shown),
            self.omitted_per_file,
            self.displayed_context,
            self.total_context.saturating_sub(self.displayed_context),
            self.clipped_lines
        )
    }
}

const LIVE_STREAM_CHUNK: usize = 8192;

struct LiveRowDecoder {
    line: Vec<u8>,
    cap: usize,
    overlong: bool,
    parse_failed: bool,
}

impl LiveRowDecoder {
    fn new(cap: usize) -> Self {
        Self {
            line: Vec::with_capacity(cap.min(LIVE_STREAM_CHUNK)),
            cap,
            overlong: false,
            parse_failed: false,
        }
    }

    fn finish_row<F>(&mut self, emit: &mut F) -> std::io::Result<()>
    where
        F: FnMut(&str) -> std::io::Result<()>,
    {
        if self.overlong {
            self.overlong = false;
            self.line.clear();
            return Ok(());
        }
        if self.line.last() == Some(&b'\r') {
            self.line.pop();
        }
        let row = std::mem::take(&mut self.line);
        match String::from_utf8(row) {
            Ok(row) => emit(&row)?,
            Err(_) => self.parse_failed = true,
        }
        self.line = Vec::with_capacity(self.cap.min(LIVE_STREAM_CHUNK));
        Ok(())
    }

    fn feed<F>(&mut self, bytes: &[u8], mut emit: F) -> std::io::Result<()>
    where
        F: FnMut(&str) -> std::io::Result<()>,
    {
        for &byte in bytes {
            if byte == b'\n' {
                self.finish_row(&mut emit)?;
            } else if !self.overlong {
                if self.line.len() < self.cap {
                    self.line.push(byte);
                } else {
                    self.overlong = true;
                    self.parse_failed = true;
                    self.line.clear();
                }
            }
        }
        Ok(())
    }

    fn finish<F>(&mut self, mut emit: F) -> std::io::Result<()>
    where
        F: FnMut(&str) -> std::io::Result<()>,
    {
        if self.overlong || !self.line.is_empty() {
            self.finish_row(&mut emit)?;
        }
        Ok(())
    }
}

fn drain_reader<R: Read>(reader: &mut R) {
    let mut chunk = [0u8; LIVE_STREAM_CHUNK];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn drain_stderr_to<R: Read, W: Write>(mut reader: R, output: &mut W) -> std::io::Result<()> {
    let mut chunk = [0u8; LIVE_STREAM_CHUNK];
    let mut output_open = true;
    let mut first_error = None;
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                first_error = Some(error);
                break;
            }
        };
        if read == 0 {
            break;
        }
        if output_open {
            match output.write_all(&chunk[..read]) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                    output_open = false;
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                    output_open = false;
                }
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn drain_stderr<R: Read>(reader: R) -> std::io::Result<()> {
    let stderr = std::io::stderr();
    let mut output = stderr.lock();
    drain_stderr_to(reader, &mut output)
}

#[allow(clippy::too_many_arguments)]
fn emit_live_row<W: Write>(
    line: &str,
    filter: &mut AgentSafeStreamFilter,
    raw_stdout: &mut String,
    raw_truncated: &mut bool,
    filtered: &mut String,
    filtered_truncated: &mut bool,
    output: &mut W,
    output_open: &mut bool,
) -> std::io::Result<()> {
    append_bounded_text(raw_stdout, line, stream::RAW_CAP, raw_truncated);
    append_bounded_text(raw_stdout, "\n", stream::RAW_CAP, raw_truncated);
    if let Some(row) = filter.feed_line(line) {
        append_bounded_text(filtered, &row, stream::RAW_CAP, filtered_truncated);
        if *output_open {
            match output.write_all(row.as_bytes()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                    *output_open = false;
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

fn exit_code_after_parse_failure(native: i32) -> i32 {
    if native == 0 { 2 } else { native }
}

fn run_agent_safe_streaming_search(
    timer: &tracking::TimedExecution,
    engine: Engine,
    extra_args: &GrepNativeArgs,
    patterns: &[String],
    paths: &[String],
    options: GrepEffectiveOptions,
    real_cmd: &str,
) -> Result<i32> {
    let Some(parser_mode) = parser_mode(engine) else {
        return Err(anyhow::anyhow!(
            "structured grep parsing unavailable without NUL-safe engine output"
        ));
    };
    let mut filter = AgentSafeStreamFilter {
        show_file: grep_show_file(paths, extra_args),
        show_line: grep_show_line(extra_args),
        max_results: options.max_matches,
        max_per_file: options.max_per_file,
        max_line_chars: options.max_line_chars,
        pattern: patterns.join("|"),
        context_only: options.context_only,
        shown: 0,
        total_matches: 0,
        omitted_per_file: 0,
        clipped_lines: 0,
        shown_by_file: HashMap::new(),
        parser_mode,
        context: context_spec(extra_args),
        pending_context: std::collections::VecDeque::new(),
        active_file: None,
        after_remaining: 0,
        total_context: 0,
        displayed_context: 0,
        last_output_file: None,
        last_output_line: None,
        emit_summary: options.summary,
        parse_failed: false,
    };
    let mut cmd = engine_command(engine, &extra_args.argv, patterns, paths, true);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().context("search failed")?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if stdout.is_none() || stderr.is_none() {
        let stderr_thread = stderr.map(|stderr| std::thread::spawn(move || drain_stderr(stderr)));
        if let Some(mut stdout) = stdout {
            drain_reader(&mut stdout);
        }
        let wait_result = child.wait();
        let stderr_result = stderr_thread.map(|thread| thread.join());
        let _ = wait_result.context("Failed to wait for child")?;
        if let Some(result) = stderr_result {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error.into()),
                Err(_) => return Err(anyhow::anyhow!("stderr reader thread panicked")),
            }
        }
        return Err(anyhow::anyhow!("child pipe handle unavailable"));
    }
    let mut stdout = stdout.expect("checked stdout handle");
    let stderr = stderr.expect("checked stderr handle");
    let stderr_thread = std::thread::spawn(move || drain_stderr(stderr));
    let mut raw_stdout = String::new();
    let mut raw_truncated = false;
    let mut filtered = String::new();
    let mut filtered_truncated = false;
    let stdout_handle = std::io::stdout();
    let mut out = stdout_handle.lock();
    let mut decoder = LiveRowDecoder::new(stream::RAW_CAP);
    let mut chunk = [0u8; LIVE_STREAM_CHUNK];
    let mut output_open = true;
    let mut stdout_error = None;
    loop {
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                if let Err(error) = decoder.feed(&chunk[..read], |line| {
                    emit_live_row(
                        line,
                        &mut filter,
                        &mut raw_stdout,
                        &mut raw_truncated,
                        &mut filtered,
                        &mut filtered_truncated,
                        &mut out,
                        &mut output_open,
                    )
                }) {
                    stdout_error = Some(error);
                    drain_reader(&mut stdout);
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                stdout_error = Some(error);
                break;
            }
        }
    }
    if stdout_error.is_none() {
        if let Err(error) = decoder.finish(|line| {
            emit_live_row(
                line,
                &mut filter,
                &mut raw_stdout,
                &mut raw_truncated,
                &mut filtered,
                &mut filtered_truncated,
                &mut out,
                &mut output_open,
            )
        }) {
            stdout_error = Some(error);
        }
    }
    if decoder.parse_failed {
        filter.parse_failed = true;
    }
    let tail = filter.flush();
    append_bounded_text(&mut filtered, &tail, stream::RAW_CAP, &mut filtered_truncated);
    if output_open {
        match out.write_all(tail.as_bytes()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
            }
            Err(error) => {
                stdout_error.get_or_insert(error);
            }
        }
    }
    drop(out);
    let stderr_result = stderr_thread.join();
    let status = child.wait().context("Failed to wait for child")?;
    match stderr_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error.into()),
        Err(_) => return Err(anyhow::anyhow!("stderr reader thread panicked")),
    }
    if let Some(error) = stdout_error {
        return Err(error.into());
    }
    let mut exit_code = stream::status_to_exit_code(status);
    if filter.parse_failed || decoder.parse_failed {
        eprintln!("grep output could not be parsed safely; structured output suppressed");
        exit_code = exit_code_after_parse_failure(exit_code);
    }
    timer.track(
        real_cmd,
        &format!("rtk {}", engine.label()),
        &raw_stdout,
        &filtered,
    );
    Ok(exit_code)
}

fn json_mode(mode: GrepMode) -> &'static str {
    match mode {
        GrepMode::Matches => "matches",
        GrepMode::FilesOnly => "files-only",
        GrepMode::CountByFile => "count-by-file",
        GrepMode::TopFiles(_) => "top-files",
    }
}

fn json_error(mode: GrepMode, patterns: Vec<String>, paths: Vec<String>, message: String) -> Result<()> {
    let document = GrepJsonDocument {
        schema: "rtk.grep.v1",
        mode: json_mode(mode),
        engine: "grep",
        patterns,
        searched_paths: paths,
        total_match_count: 0,
        matched_file_count: 0,
        displayed_match_count: 0,
        omitted_match_count: 0,
        clipped_line_count: 0,
        displayed_context_row_count: 0,
        omitted_context_row_count: 0,
        files: Vec::new(),
        recovery_hints: Vec::new(),
        recovery_argv: Vec::new(),
        error: Some(message),
    };
    println!("{}", serde_json::to_string(&document)?);
    Ok(())
}

fn recovery_argv(forwarded: &[String]) -> Vec<Vec<String>> {
    let mut argv = vec!["grep".to_string()];
    argv.extend(forwarded.iter().cloned());
    vec![argv]
}

fn grep_recovery_hints(first: Option<(&str, usize)>) -> Vec<String> {
    let mut hints = vec![
        "recovery_argv preserves original grep flags, patterns, and paths".to_string(),
        "rerun with --files-only or --count-by-file for a smaller result".to_string(),
    ];
    if let Some((path, line)) = first {
        hints.push(format!(
            "inspect {} lines {}:{} (use your shell's native argument quoting)",
            path,
            line.saturating_sub(5).max(1),
            line.saturating_add(5)
        ));
    }
    hints
}

fn clipped_text(text: &str, max_chars: Option<usize>, pattern: &str) -> (String, bool) {
    let trimmed = text.trim();
    let Some(max_chars) = max_chars else {
        return (trimmed.to_string(), false);
    };
    if trimmed.chars().count() <= max_chars {
        return (trimmed.to_string(), false);
    }
    if max_chars <= 3 {
        return (trimmed.chars().take(max_chars).collect(), true);
    }
    if max_chars <= 6 {
        let mut result: String = trimmed.chars().take(max_chars - 3).collect();
        result.push_str("...");
        return (result, true);
    }
    let chars: Vec<char> = trimmed.chars().collect();
    // ponytail: scan original scalar boundaries; avoids mapping expanded
    // lowercase scalars (for example `İ` -> `i` plus combining dot) back into
    // the original string. Upgrade to a folded-search index only if profiling
    // proves this bounded display path hot.
    let start = find_case_insensitive_scalar_span(&chars, pattern)
        .map(|(start, _)| start)
        .unwrap_or(0);
    let budget = max_chars.saturating_sub(6);
    let slice_start = start.saturating_sub(budget / 2);
    let slice_end = (slice_start + budget).min(chars.len());
    let mut result = String::new();
    if slice_start > 0 {
        result.push_str("...");
    }
    result.extend(chars[slice_start..slice_end].iter());
    if slice_end < chars.len() {
        result.push_str("...");
    }
    (result, true)
}

fn find_case_insensitive_scalar_span(chars: &[char], pattern: &str) -> Option<(usize, usize)> {
    let needle = pattern.to_lowercase();
    if needle.is_empty() {
        return Some((0, 0));
    }
    for start in 0..chars.len() {
        for end in (start + 1)..=chars.len() {
            let candidate: String = chars[start..end].iter().collect::<String>().to_lowercase();
            if candidate == needle {
                return Some((start, end));
            }
            if !needle.starts_with(&candidate) {
                break;
            }
        }
    }
    None
}

fn render_row_text(
    content: &str,
    options: &GrepEffectiveOptions,
    pattern: &str,
) -> (String, bool) {
    let base = if options.context_only {
        context_only_excerpt(content, pattern)
    } else {
        content.trim().to_string()
    };
    clipped_text(&base, options.max_line_chars, pattern)
}

fn context_only_excerpt(content: &str, pattern: &str) -> String {
    let trimmed = content.trim();
    if pattern.is_empty() {
        return trimmed.to_string();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    let Some((start, end)) = find_case_insensitive_scalar_span(&chars, pattern) else {
        return trimmed.to_string();
    };
    let left = start.saturating_sub(20);
    let right = end.saturating_add(20).min(chars.len());
    chars[left..right].iter().collect()
}

#[allow(clippy::too_many_arguments)]
fn render_grep_json(
    mode: GrepMode,
    patterns: &[String],
    paths: &[String],
    files: &[RenderedFile],
    matched_file_count: usize,
    total_matches: usize,
    displayed_matches: usize,
    omitted_matches: usize,
    clipped_lines: usize,
    hints: Vec<String>,
    recovery_argv: Vec<Vec<String>>,
    error: Option<String>,
) -> Result<String> {
    let displayed_context_row_count = files.iter().map(|file| file.displayed_context_count).sum();
    let omitted_context_row_count = files.iter().map(|file| file.omitted_context_count).sum();
    let document = GrepJsonDocument {
        schema: "rtk.grep.v1",
        mode: json_mode(mode),
        engine: "grep",
        patterns: patterns.to_vec(),
        searched_paths: paths.to_vec(),
        total_match_count: total_matches,
        matched_file_count,
        displayed_match_count: displayed_matches,
        omitted_match_count: omitted_matches,
        clipped_line_count: clipped_lines,
        displayed_context_row_count,
        omitted_context_row_count,
        files: files
            .iter()
            .map(|file| GrepJsonFile {
                path: file.path.clone(),
                true_match_count: file.true_match_count,
                displayed_count: file.displayed_count,
                omitted_count: file.omitted_count,
                displayed_context_count: file.displayed_context_count,
                omitted_context_count: file.omitted_context_count,
                matches: file
                    .rows
                    .iter()
                    .map(|(line, is_match, text)| GrepJsonMatch {
                        line: *line,
                        kind: if *is_match { "match" } else { "context" },
                        text: text.clone(),
                    })
                    .collect(),
            })
            .collect(),
        recovery_hints: hints,
        recovery_argv,
        error,
    };
    Ok(format!("{}\n", serde_json::to_string(&document)?))
}

#[derive(Clone, Copy, Debug, Default)]
struct ContextSpec {
    before: usize,
    after: usize,
}

fn context_spec(flags: &GrepNativeArgs) -> ContextSpec {
    let mut spec = ContextSpec::default();
    let mut i = 0;
    while i < flags.argv.len() {
        if !flags.is_flag(i) {
            i += 1;
            continue;
        }
        let flag = &flags.argv[i];
        if let Some(rest) = flag.strip_prefix('-').filter(|rest| !rest.starts_with('-')) {
            if let ClusterResult::ValueTaking { flag, inline, .. } = parse_grep_cluster(rest) {
                if matches!(flag, 'A' | 'B' | 'C') {
                    let value = if inline.is_empty() {
                        flags
                            .argv
                            .get(i + 1)
                            .filter(|_| !flags.is_flag(i + 1))
                            .map(String::as_str)
                    } else {
                        Some(inline.as_str())
                    };
                    if let Some(value) = value.and_then(|value| value.parse::<usize>().ok()) {
                        match flag {
                            'A' => spec.after = value,
                            'B' => spec.before = value,
                            'C' => {
                                spec.before = value;
                                spec.after = value;
                            }
                            _ => unreachable!(),
                        }
                    }
                    if inline.is_empty() {
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
            }
        }
        let (name, inline) = flag
            .split_once('=')
            .map_or((flag.as_str(), None), |(name, value)| (name, Some(value)));
        let value = inline.or_else(|| {
            if i + 1 < flags.argv.len() && !flags.is_flag(i + 1) {
                Some(flags.argv[i + 1].as_str())
            } else {
                None
            }
        });
        let parsed = value.and_then(|value| value.parse::<usize>().ok());
        match name {
            "-A" | "--after-context" => {
                if let Some(value) = parsed {
                    spec.after = value;
                    if inline.is_none() {
                        i += 1;
                    }
                }
            }
            "-B" | "--before-context" => {
                if let Some(value) = parsed {
                    spec.before = value;
                    if inline.is_none() {
                        i += 1;
                    }
                }
            }
            "-C" | "--context" => {
                if let Some(value) = parsed {
                    spec.before = value;
                    spec.after = value;
                    if inline.is_none() {
                        i += 1;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    spec
}

fn selected_context_rows(
    rows: &[(usize, bool, String)],
    selected_matches: &std::collections::HashSet<usize>,
    spec: ContextSpec,
) -> (std::collections::HashSet<usize>, usize) {
    let mut selected = selected_matches.clone();
    let mut omitted_context = 0;
    for (index, (line, is_match, _)) in rows.iter().enumerate() {
        if *is_match {
            continue;
        }
        let relevant = selected_matches.iter().any(|match_index| {
            let match_line = rows[*match_index].0;
            let between = |left: usize, right: usize| {
                left < right
                    && rows[left + 1..right]
                        .iter()
                        .any(|(_, is_match, _)| *is_match)
            };
            (*line < match_line
                && match_line.saturating_sub(*line) <= spec.before
                && !between(index, *match_index))
                || (*line > match_line
                    && line.saturating_sub(match_line) <= spec.after
                    && !between(*match_index, index))
        });
        if relevant {
            selected.insert(index);
        } else if spec.before > 0 || spec.after > 0 {
            omitted_context += 1;
        }
    }
    (selected, omitted_context)
}

fn render_grep_structured(
    options: GrepEffectiveOptions,
    patterns: &[String],
    raw_output: &str,
    parser_mode: ParserMode,
    extra_args: &GrepNativeArgs,
) -> (String, usize, usize, usize, usize, Vec<RenderedFile>) {
    let pattern = patterns.join("|");
    let context = context_spec(extra_args);
    let mut raw_by_file: BTreeMap<String, Vec<(usize, bool, String)>> = BTreeMap::new();
    for line in raw_output.lines() {
        let line = strip_ansi(line);
        if let Some((file, line_num, is_match, content)) = parse_match_line(&line, parser_mode) {
            raw_by_file
                .entry(file)
                .or_default()
                .push((line_num, is_match, content.to_string()));
        }
    }
    let total_matches: usize = raw_by_file
        .values()
        .map(|rows| rows.iter().filter(|(_, is_match, _)| *is_match).count())
        .sum();
    let matched_file_count = raw_by_file
        .values()
        .filter(|rows| rows.iter().any(|(_, is_match, _)| *is_match))
        .count();

    if options.mode == GrepMode::FilesOnly || options.mode == GrepMode::CountByFile || matches!(options.mode, GrepMode::TopFiles(_)) {
        let mut ranked: Vec<(String, usize)> = raw_by_file
            .iter()
            .map(|(path, rows)| {
                (
                    path.clone(),
                    rows.iter().filter(|(_, is_match, _)| *is_match).count(),
                )
            })
            .filter(|(_, count)| *count > 0)
            .collect();
        ranked.sort_by(|(path_a, count_a), (path_b, count_b)| {
            count_b.cmp(count_a).then_with(|| path_a.cmp(path_b))
        });
        if options.mode == GrepMode::FilesOnly {
            ranked.sort_by(|(path_a, _), (path_b, _)| path_a.cmp(path_b));
        }
        let selected_len = match options.mode {
            GrepMode::TopFiles(n) => n.min(ranked.len()),
            _ => ranked.len(),
        };
        let selected = &ranked[..selected_len];
        let selected_matches: usize = selected.iter().map(|(_, count)| *count).sum();
        let rendered: Vec<RenderedFile> = selected
            .iter()
            .map(|(path, count)| RenderedFile {
                path: path.clone(),
                true_match_count: *count,
                displayed_count: *count,
                displayed_context_count: 0,
                omitted_context_count: 0,
                omitted_count: 0,
                rows: Vec::new(),
            })
            .collect();
        return (
            if options.mode == GrepMode::FilesOnly {
                rendered.iter().map(|file| format!("{}\n", file.path)).collect()
            } else {
                let mut output = String::new();
                if matches!(options.mode, GrepMode::TopFiles(_)) {
                    output.push_str(&format!("{} matches in {} files\n\n", total_matches, matched_file_count));
                }
                for file in &rendered {
                    output.push_str(&format!("{}  {}\n", file.true_match_count, file.path));
                }
                output
            },
            total_matches,
            selected_matches,
            0,
            matched_file_count,
            rendered,
        );
    }

    let mut shown_matches = 0usize;
    let mut clipped_lines = 0usize;
    let mut rendered = Vec::new();
    for (path, rows) in raw_by_file {
        let true_match_count = rows.iter().filter(|(_, is_match, _)| *is_match).count();
        if true_match_count == 0 {
            continue;
        }
        let per_file_cap = options.max_per_file;
        let mut file_shown = 0usize;
        let mut selected_match_lines = Vec::new();
        for (index, (line, is_match, _)) in rows.iter().enumerate() {
            if !*is_match {
                continue;
            }
            if options.max_matches.is_some_and(|cap| shown_matches >= cap) {
                continue;
            }
            if per_file_cap.is_some_and(|cap| file_shown >= cap) {
                continue;
            }
            shown_matches += 1;
            file_shown += 1;
            selected_match_lines.push((index, *line));
        }
        if selected_match_lines.is_empty() {
            let omitted_context_count = if context.before == 0 && context.after == 0 {
                0
            } else {
                rows.iter().filter(|(_, is_match, _)| !*is_match).count()
            };
            rendered.push(RenderedFile {
                path,
                true_match_count,
                displayed_count: 0,
                omitted_count: true_match_count,
                displayed_context_count: 0,
                omitted_context_count,
                rows: Vec::new(),
            });
            continue;
        }
        let selected_indices: std::collections::HashSet<usize> =
            selected_match_lines.iter().map(|(index, _)| *index).collect();
        let (selected_indices, _) = selected_context_rows(&rows, &selected_indices, context);
        let total_context = rows.iter().filter(|(_, is_match, _)| !*is_match).count();
        let displayed_context = selected_indices
            .iter()
            .filter(|index| !rows[**index].1)
            .count();
        let mut output_rows = Vec::new();
        for (index, (line, is_match, content)) in rows.into_iter().enumerate() {
            if !selected_indices.contains(&index) {
                continue;
            }
            let (text, clipped) = render_row_text(&content, &options, &pattern);
            clipped_lines += usize::from(clipped);
            output_rows.push((line, is_match, text));
        }
        rendered.push(RenderedFile {
            path: path.clone(),
            true_match_count,
            displayed_count: file_shown,
            omitted_count: true_match_count.saturating_sub(file_shown),
            displayed_context_count: displayed_context,
            omitted_context_count: if context.before == 0 && context.after == 0 {
                0
            } else {
                total_context.saturating_sub(displayed_context)
            },
            rows: output_rows,
        });
    }
    let mut output = String::new();
    for file in &rendered {
        let mut previous_line: Option<usize> = None;
        for (line, is_match, text) in &file.rows {
            if (context.before > 0 || context.after > 0)
                && previous_line.is_some_and(|previous| *line > previous.saturating_add(1))
            {
                output.push_str("--\n");
            }
            output.push_str(&format!(
                "{}{}{}{}{}\n",
                file.path,
                if *is_match { ':' } else { '-' },
                line,
                if *is_match { ':' } else { '-' },
                text
            ));
            previous_line = Some(*line);
        }
    }
    (output, total_matches, shown_matches, clipped_lines, matched_file_count, rendered)
}

pub fn run_grep(
    legacy_max_len: usize,
    legacy_max: usize,
    context_only: bool,
    args: &[String],
    verbose: u8,
) -> Result<i32> {
    let restored = args_utils::restore_double_dash(args);
    let requested_json = json_requested_before_dashdash(&restored);
    let (parsed, forwarded) = match parse_grep_rtk_options(&restored) {
        Ok(parsed) => parsed,
        Err(error) if requested_json => {
            json_error(
                GrepMode::Matches,
                Vec::new(),
                Vec::new(),
                error.to_string(),
            )?;
            return Ok(2);
        }
        Err(error) => return Err(error),
    };
    let requested_context_only = context_only || parsed.context_only;
    let legacy_args = if parsed.context_only {
        &forwarded
    } else {
        &restored
    };
    let effective = effective_grep_options(
        legacy_max_len,
        legacy_max,
        requested_context_only,
        &parsed,
    );
    let has_new_mode = parsed.mode != GrepMode::Matches
        || parsed.all
        || parsed.full_lines
        || parsed.agent_safe
        || parsed.json
        || parsed.context_only
        || parsed.max_matches.is_some()
        || parsed.max_per_file.is_some()
        || parsed.max_line_chars.is_some()
        || env_bool_value("RTK_AGENT_SAFE").unwrap_or_else(config_agent_safe);

    if !has_new_mode {
        return run(
            Engine::Grep,
            legacy_max_len,
            legacy_max,
            requested_context_only,
            legacy_args,
            verbose,
        );
    }

    let (patterns, paths, extra_args) = extract_grep_pattern_path(&forwarded);
    if let Some(message) = missing_grep_operand_error(&extra_args) {
        if effective.json {
            json_error(effective.mode, patterns, paths, message)?;
            return Ok(2);
        }
        return Err(anyhow::anyhow!(message));
    }
    let structured_non_matches = effective.mode != GrepMode::Matches;
    let incompatible = if structured_non_matches
        && (parsed.max_matches.is_some()
            || parsed.max_per_file.is_some()
            || parsed.max_line_chars.is_some())
    {
        Some("result modes cannot combine with match caps or line clipping")
    } else if structured_non_matches && effective.all {
        Some("--all is only meaningful for normal match output")
    } else if structured_non_matches && (effective.full_lines || effective.context_only) {
        Some("files-only/count-by-file/top-files cannot combine with line or context output")
    } else if structured_non_matches && grep_has_context_flag(&extra_args) {
        Some("files-only/count-by-file/top-files cannot combine with native context flags")
    } else if has_forced_color(&extra_args) {
        Some("structured grep modes cannot combine with --color=always")
    } else {
        None
    };
    if let Some(message) = incompatible {
        if effective.json {
            json_error(effective.mode, patterns, paths, message.to_string())?;
            return Ok(2);
        }
        return Err(anyhow::anyhow!(message));
    }
    if grep_has_format_flag(&extra_args) {
        let message = "RTK structured grep modes cannot combine with native shape-changing flags";
        if effective.json {
            json_error(effective.mode, patterns, paths, message.to_string())?;
            return Ok(2);
        }
        return Err(anyhow::anyhow!(message));
    }
    if patterns.is_empty() {
        if effective.json {
            json_error(effective.mode, patterns, paths, "missing grep pattern".to_string())?;
            return Ok(2);
        }
        if forwarded
            .iter()
            .any(|arg| matches!(arg.as_str(), "--version" | "--help" | "-h"))
        {
            return run(
                Engine::Grep,
                legacy_max_len,
                legacy_max,
                requested_context_only,
                legacy_args,
                verbose,
            );
        }
        return Err(anyhow::anyhow!("missing grep pattern"));
    }
    let parser_mode = match structured_parser_policy(parser_mode(Engine::Grep), effective.json) {
        StructuredParserPolicy::Continue(mode) => mode,
        StructuredParserPolicy::JsonError => {
            json_error(
                effective.mode,
                patterns,
                paths,
                STRUCTURED_PARSER_UNAVAILABLE.to_string(),
            )?;
            return Ok(2);
        }
        StructuredParserPolicy::HumanError => {
            return Err(anyhow::anyhow!(STRUCTURED_PARSER_UNAVAILABLE));
        }
    };
    match structured_context_policy(
        parser_mode,
        grep_has_context_flag(&extra_args),
        effective.json,
    ) {
        StructuredContextPolicy::Continue => {}
        StructuredContextPolicy::JsonError => {
            let message = STRUCTURED_CONTEXT_UNAVAILABLE;
            json_error(effective.mode, patterns, paths, message.to_string())?;
            return Ok(2);
        }
        StructuredContextPolicy::HumanError => {
            return Err(anyhow::anyhow!(STRUCTURED_CONTEXT_UNAVAILABLE));
        }
    }
    if let Some(message) = invalid_grep_label(parser_mode, &extra_args) {
        if effective.json {
            json_error(effective.mode, patterns, paths, message.to_string())?;
            return Ok(2);
        }
        return Err(anyhow::anyhow!(message));
    }

    let reads_piped_stdin = !std::io::stdin().is_terminal()
        && (paths.is_empty() || paths.iter().any(|path| path == "-"));
    if reads_piped_stdin
        && !effective.json
        && effective.mode == GrepMode::Matches
    {
        let timer = tracking::TimedExecution::start();
        let real_cmd = format!("grep {}", forwarded.join(" "));
        if effective.context_only
            || effective.agent_safe
            || parsed.max_matches.is_some()
            || parsed.max_per_file.is_some()
            || parsed.max_line_chars.is_some()
        {
            return run_agent_safe_streaming_search(
                &timer,
                Engine::Grep,
                &extra_args,
                &patterns,
                &paths,
                effective,
                &real_cmd,
            );
        }
        return run_streaming_search(
            &timer,
            Engine::Grep,
            &extra_args.argv,
            &patterns,
            &paths,
            effective.max_matches.unwrap_or(usize::MAX),
            &real_cmd,
        );
    }

    if verbose > 0 {
        eprintln!("grep: '{}' in {}", patterns.join("|"), paths.join(" "));
    }
    let bounded = engine_capture_bounded(Engine::Grep, &extra_args.argv, &patterns, &paths)?;
    let capture_error = bounded
        .truncated
        .then(|| "structured grep capture exceeded 10 MiB; result is partial".to_string());
    let result = bounded.result;
    if !result.stderr.is_empty() {
        eprint!("{}", result.stderr);
    }
    if let Some(error) = capture_error.clone() {
        if !effective.json {
            return Err(anyhow::anyhow!(error));
        }
    }
    if unparsed_signal(&result.stdout, parser_mode) > 0 {
        let message = "grep output could not be parsed safely";
        if effective.json {
            json_error(
                effective.mode,
                patterns,
                paths,
                capture_error.clone().unwrap_or_else(|| message.to_string()),
            )?;
            return Ok(if result.exit_code == 0 { 2 } else { result.exit_code });
        }
        return Err(anyhow::anyhow!(message));
    }
    if result.exit_code != 0 && result.stdout.trim().is_empty() {
        if effective.json {
            if result.exit_code == 1 {
                let output = render_grep_json(
                    effective.mode,
                    &patterns,
                    &paths,
                    &[],
                    0,
                    0,
                    0,
                    0,
                    0,
                    Vec::new(),
                    Vec::new(),
                    None,
                )?;
                print!("{}", output);
            } else {
                json_error(
                    effective.mode,
                    patterns,
                    paths,
                    capture_error
                        .clone()
                        .or_else(|| Some(result.stderr.trim().to_string()))
                        .unwrap_or_else(|| "grep execution failed".to_string()),
                )?;
            }
        }
        return Ok(result.exit_code);
    }

    let (human, total, displayed, clipped, matched_file_count, files) = render_grep_structured(
        effective,
        &patterns,
        &result.stdout,
        parser_mode,
        &extra_args,
    );
    let omitted = total.saturating_sub(displayed);
    let first = files
        .iter()
        .flat_map(|file| file.rows.iter().map(|(line, _, _)| (file.path.as_str(), *line)))
        .next();
    let hints = if effective.agent_safe && (omitted > 0 || clipped > 0) {
        grep_recovery_hints(first)
    } else {
        Vec::new()
    };
    if effective.json {
        print!(
            "{}",
            render_grep_json(
                effective.mode,
                &patterns,
                &paths,
                &files,
                matched_file_count,
                total,
                displayed,
                omitted,
                clipped,
                hints,
                recovery_argv(&forwarded),
                capture_error.or_else(|| {
                    if result.exit_code <= 1 {
                        None
                    } else {
                        Some(if result.stderr.trim().is_empty() {
                            format!("grep exited with status {}", result.exit_code)
                        } else {
                            result.stderr.trim().to_string()
                        })
                    }
                }),
            )?
        );
    } else {
        print!("{}", human);
        if effective.mode == GrepMode::Matches && effective.summary {
            let omitted_per_file: usize = files.iter().map(|file| file.omitted_count).sum();
            println!(
                "summary: total={} files={} shown={} omitted_total={} omitted_per_file={} clipped_lines={}",
                total,
                files.len(),
                displayed,
                omitted,
                omitted_per_file,
                clipped
            );
            for file in files.iter().filter(|file| file.omitted_count > 0) {
                println!("omitted: {}={}", file.path, file.omitted_count);
            }
            for hint in hints {
                println!("hint: {}", hint);
            }
        }
    }
    Ok(result.exit_code)
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
    if args
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

    let Some(parser_mode) = parser_mode(engine) else {
        return passthrough(&timer, engine, &args, &real_cmd, false);
    };

    let result = engine_capture(engine, &extra_args, &patterns, &paths)?;

    let exit_code = result.exit_code;
    let raw_output = result.stdout.clone();

    // Unparseable shape re-runs verbatim below (with its own stderr), so handle it
    // before surfacing this run's stderr (#2333).
    if unparsed_signal(&raw_output, parser_mode) > 0 {
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
        let Some((file, line_num, is_match, content)) = parse_match_line(line, parser_mode) else {
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
    let show_line = show_line(&extra_args);

    // Faithful baseline: exactly what the real command prints, full content.
    let mut plain = String::new();
    for line in raw_output.lines() {
        let Some(output) = format_match_line(line, show_file, show_line, parser_mode) else {
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
/// NUL mode requires the underlying command to be invoked with `-0` (rg) or
/// `--null` (grep), so the filename is NUL-separated from `line[:-]content`.
/// NUL cannot appear in file paths, so that parser is unambiguous regardless of:
///   - content with `:` or `::` (e.g. `ClassRegistry::init(...)`, issue #1436);
///   - paths with embedded `:` (Windows drive letters, weird filenames like
///     `badly_named:52:file.txt`).
///
/// Returns `None` for lines that do not match the expected shape.
/// The `bool` in the tuple is `true` for match lines (`:` separator) and
/// `false` for NUL-mode context lines (`-` separator, emitted by -A/-B/-C).
/// OrdinaryWindows intentionally parses match records only: ordinary context
/// rows are ambiguous with valid Windows paths and are rejected.
fn parse_match_line(line: &str, mode: ParserMode) -> Option<(String, usize, bool, &str)> {
    static MATCH_LINE_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([^\x00]+)\x00(\d+)([:-])(.*)$").unwrap());

    if mode == ParserMode::Nul {
        if let Some(parsed) = MATCH_LINE_RE.captures(line).and_then(|caps| {
        let file = caps.get(1)?.as_str().to_string();
        let line_num: usize = caps.get(2)?.as_str().parse().ok()?;
        let sep = caps.get(3)?.as_str();
        let content = caps.get(4)?.as_str();
        let is_match = sep == ":";
        Some((file, line_num, is_match, content))
        }) {
            return Some(parsed);
        }
    }

    if mode != ParserMode::OrdinaryWindows {
        return None;
    }

    // Only Windows fallback: POSIX filenames may contain ':' and make this
    // shape ambiguous. NUL output remains preferred where supported.
    let bytes = line.as_bytes();
    for (index, delimiter) in bytes.iter().enumerate() {
        if *delimiter != b':' {
            continue;
        }
        let digits_start = index + 1;
        let mut digits_end = digits_start;
        while digits_end < bytes.len() && bytes[digits_end].is_ascii_digit() {
            digits_end += 1;
        }
        if digits_end == digits_start
            || digits_end >= bytes.len()
            || bytes[digits_end] != *delimiter
            || index == 0
        {
            continue;
        }
        let file = &line[..index];
        if !is_defensible_windows_path(file) {
            continue;
        }
        let line_num = line[digits_start..digits_end].parse().ok()?;
        return Some((
            file.to_string(),
            line_num,
            true,
            &line[digits_end + 1..],
        ));
    }
    None
}

fn is_defensible_windows_path(file: &str) -> bool {
    let drive_path = is_windows_drive_path(file);
    if drive_path {
        return !file[2..].contains(':');
    }

    // Relative, bare, and pseudo paths cannot contain a colon on Windows.
    // They need not contain a slash or extension (`Makefile`, `(standard input)`).
    !file.is_empty() && !file.contains(':')
}

fn is_windows_drive_path(file: &str) -> bool {
    let bytes = file.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn grep_has_format_flag(extra_args: &GrepNativeArgs) -> bool {
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
        "--no-filename",
        "--no-line-number",
        "--initial-tab",
        "--group-separator",
    ];
    grep_flag_tokens(extra_args).any(|(_, arg)| {
        if arg.starts_with("--") {
            LONG.contains(&arg.split('=').next().unwrap_or(arg))
        } else if let Some(letters) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            letters.chars().any(|ch| {
                matches!(
                    ch,
                    'c' | 'l' | 'L' | 'o' | 'q' | 'b' | 'Z' | 'z' | 'h' | 'N' | 'T'
                )
            })
        } else {
            false
        }
    })
}

fn invalid_grep_label(parser_mode: ParserMode, args: &GrepNativeArgs) -> Option<&'static str> {
    for (index, flag) in grep_flag_tokens(args) {
        let value = if flag == "--label" {
            args.argv
                .get(index + 1)
                .filter(|_| !args.is_flag(index + 1))
                .map(String::as_str)
        } else {
            flag.strip_prefix("--label=")
        };
        let Some(value) = value else {
            continue;
        };
        let valid = !value.is_empty()
            && !value.contains(['\r', '\n'])
            && (parser_mode == ParserMode::Nul || is_defensible_windows_path(value));
        if !valid {
            return Some("structured grep --label is ambiguous or invalid for this parser");
        }
    }
    None
}

fn missing_grep_operand_error(args: &GrepNativeArgs) -> Option<String> {
    args.missing_operand
        .as_ref()
        .map(|option| format!("missing native grep operand for {option}"))
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
            if matched.chars().count() <= max_len {
                return matched.to_string();
            }
        }
    }

    clipped_text(trimmed, Some(max_len), pattern).0
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
    fn bounded_capture_is_fixed_chunk_and_utf8_safe() {
        let exact = capture_stream_bounded(std::io::Cursor::new(b"a\n"), 2).unwrap();
        assert_eq!(exact.text, "a\n");
        assert!(!exact.truncated);

        let over = capture_stream_bounded(std::io::Cursor::new(b"a\nb"), 2).unwrap();
        assert_eq!(over.text.len(), 2);
        assert!(over.truncated);

        let long_line = capture_stream_bounded(std::io::Cursor::new(b"abcdef"), 5).unwrap();
        assert_eq!(long_line.text, "abcde");
        assert!(long_line.truncated);

        let many_lines = capture_stream_bounded(std::io::Cursor::new(b"ab\ncd\nef\n"), 5).unwrap();
        assert_eq!(many_lines.text, "ab\ncd");
        assert!(many_lines.truncated);

        let missing_newline = capture_stream_bounded(std::io::Cursor::new(b"line"), 5).unwrap();
        assert_eq!(missing_newline.text, "line\n");
        assert!(!missing_newline.truncated);

        let invalid = capture_stream_bounded(std::io::Cursor::new(b"ok\xff\n"), 10).unwrap();
        assert_eq!(invalid.text, "ok�\n");
        assert!(!invalid.truncated);

        let crlf = capture_stream_bounded(std::io::Cursor::new(b"\r\n"), 1).unwrap();
        assert_eq!(crlf.text, "\n");
        assert!(!crlf.truncated);

        let newline = capture_stream_bounded(std::io::Cursor::new(b"\n"), 5).unwrap();
        assert_eq!(newline.text, "\n");
        assert!(!newline.truncated);
    }

    struct ChunkedReader {
        bytes: Vec<u8>,
        offset: usize,
        chunk: usize,
    }

    impl std::io::Read for ChunkedReader {
        fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
            if self.offset == self.bytes.len() {
                return Ok(0);
            }
            let count = self
                .chunk
                .min(target.len())
                .min(self.bytes.len() - self.offset);
            target[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
            self.offset += count;
            Ok(count)
        }
    }

    #[test]
    fn bounded_capture_normalizes_crlf_across_chunks() {
        let reader = ChunkedReader {
            bytes: b"a\r\n".to_vec(),
            offset: 0,
            chunk: 2,
        };
        let result = capture_stream_bounded(reader, 2).unwrap();
        assert_eq!(result.text, "a\n");
        assert!(!result.truncated);
    }

    #[test]
    fn live_row_decoder_is_bounded_and_split_safe() {
        let mut decoder = LiveRowDecoder::new(8);
        let mut rows = Vec::new();
        decoder
            .feed(b"ok\xe2", |row| {
                rows.push(row.to_string());
                Ok(())
            })
            .unwrap();
        assert!(decoder.line.len() <= 8);
        decoder
            .feed(b"\x82\xac\r", |row| {
                rows.push(row.to_string());
                Ok(())
            })
            .unwrap();
        decoder
            .feed(b"\nlonger-than-cap\nok\n", |row| {
                rows.push(row.to_string());
                Ok(())
            })
            .unwrap();
        decoder.finish(|row| {
            rows.push(row.to_string());
            Ok(())
        }).unwrap();
        assert_eq!(rows, vec!["ok€", "ok"]);
        assert!(decoder.parse_failed);
        assert!(decoder.line.len() <= 8);
    }

    #[test]
    fn live_row_decoder_handles_eof_and_invalid_utf8_without_raw_bypass() {
        let mut decoder = LiveRowDecoder::new(8);
        let mut rows = Vec::new();
        decoder
            .feed(b"bad\xff\nlast", |row| {
                rows.push(row.to_string());
                Ok(())
            })
            .unwrap();
        decoder.finish(|row| {
            rows.push(row.to_string());
            Ok(())
        }).unwrap();
        assert_eq!(rows, vec!["last"]);
        assert!(decoder.parse_failed);
    }

    #[test]
    fn parse_failure_exit_preserves_native_nonzero() {
        assert_eq!(exit_code_after_parse_failure(0), 2);
        assert_eq!(exit_code_after_parse_failure(1), 1);
        assert_eq!(exit_code_after_parse_failure(2), 2);
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
            parser_mode: ParserMode::Nul,
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
            parser_mode: ParserMode::Nul,
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
        assert!(!show_line(&flags(&[])));
        assert!(!show_line(&flags(&["-i"])));
        assert!(!show_line(&flags(&["-r"])));
        assert!(!show_line(&flags(&["-A", "3"])));
    }

    #[test]
    fn show_line_honours_n_in_every_spelling() {
        assert!(show_line(&flags(&["-n"])));
        assert!(show_line(&flags(&["--line-number"])));
        assert!(show_line(&flags(&["-rn"])));
        assert!(show_line(&flags(&["-in"])));
    }

    #[test]
    fn show_line_is_off_when_explicitly_negated() {
        assert!(!show_line(&flags(&["-N"])));
        assert!(!show_line(&flags(&["--no-line-number"])));
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
        assert!(!show_line(&flags(&[])));
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
        let (file, line_num, is_match, content) = parse_match_line(line, ParserMode::Nul).unwrap();
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
        let (file, line_num, is_match, content) = parse_match_line(line, ParserMode::Nul).unwrap();
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
        let (file, line_num, is_match, content) = parse_match_line(line, ParserMode::Nul).unwrap();
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
        let (file, line_num, is_match, content) = parse_match_line(line, ParserMode::Nul).unwrap();
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
        let (file, line_num, is_match, content) = parse_match_line(line, ParserMode::Nul).unwrap();
        assert_eq!(file, "log.txt");
        assert_eq!(line_num, 7);
        assert!(is_match);
        assert_eq!(content, "debug: counter is :42: now");
    }

    #[test]
    fn test_parse_match_line_malformed_returns_none() {
        // Ordinary grep fallback shape is parseable when NUL output is unavailable.
        assert!(parse_match_line("file.rs:1:content", ParserMode::OrdinaryWindows).is_some());
        assert!(parse_match_line("not a match line", ParserMode::Nul).is_none());
        // Missing line number after NUL
        assert!(parse_match_line("file.rs\x00fn foo()", ParserMode::Nul).is_none());
        // Empty
        assert!(parse_match_line("", ParserMode::Nul).is_none());
    }

    #[test]
    fn ordinary_parser_allows_colons_in_windows_content() {
        for (line, expected_line_num, expected_content) in [
            (r"C:\src\main.rs:42:Foo", 42, "Foo"),
            (r"C:\src\main.rs:42:https://example.com", 42, "https://example.com"),
            (r"C:\src\lib.rs:18:ClassRegistry::init()", 18, "ClassRegistry::init()"),
            (r####"C:\src\data.json:7:{"key":"value"}"####, 7, r####"{"key":"value"}"####),
            (r"C:\src\time.txt:3:12:45", 3, "12:45"),
            ("Makefile:42:time:12:45", 42, "time:12:45"),
            ("README:7:error:404:retry", 7, "error:404:retry"),
            ("custom label:1:x:2:y", 1, "x:2:y"),
            (r####"file.json:3:{"time":"12:45"}"####, 3, r####"{"time":"12:45"}"####),
            ("file.txt:9:prefix:1:middle:2:suffix", 9, "prefix:1:middle:2:suffix"),
            ("(standard input):1:http://localhost:8080", 1, "http://localhost:8080"),
            (r"C:\src\foo-12-bar.rs:42:hit", 42, "hit"),
            (r"C:\src\foo-12-bar.rs:42:https://example.com", 42, "https://example.com"),
            (r"C:\src\foo-12-bar.rs:42:text-9-more", 42, "text-9-more"),
            (r"C:\src\dir-12-part\file.rs:42:hit", 42, "hit"),
            (r"src\foo-12-bar.rs:42:hit", 42, "hit"),
            (r"src/foo-12-bar.rs:42:hit", 42, "hit"),
            (r"file-12-name.rs:42:hit", 42, "hit"),
            (r"file.rs:9:Foo", 9, "Foo"),
        ] {
            let (_, line_num, is_match, content) =
                parse_match_line(line, ParserMode::OrdinaryWindows).unwrap();
            assert_eq!(line_num, expected_line_num);
            assert!(is_match);
            assert_eq!(content, expected_content);
        }
    }

    #[test]
    fn ordinary_parser_accepts_bare_and_pseudo_paths() {
        for (line, expected_line, expected_content) in [
            ("Makefile:42:hit", 42, "hit"),
            ("Dockerfile:42:hit", 42, "hit"),
            ("LICENSE:42:hit", 42, "hit"),
            ("README:42:hit", 42, "hit"),
            ("(standard input):1:hit", 1, "hit"),
            ("custom label:1:hit", 1, "hit"),
            ("žuti dokument:2:hit", 2, "hit"),
            ("file name:3:https://example.com", 3, "https://example.com"),
        ] {
            let (file, line_num, is_match, content) =
                parse_match_line(line, ParserMode::OrdinaryWindows).unwrap();
            assert_eq!(file, line.split_once(':').unwrap().0);
            assert_eq!(line_num, expected_line);
            assert!(is_match);
            assert_eq!(content, expected_content);
        }
    }

    #[test]
    fn ordinary_windows_dash_filename_cannot_become_context_row() {
        let (file, line_num, is_match, content) = parse_match_line(
            r"C:\src\foo-12-bar.rs:42:hit",
            ParserMode::OrdinaryWindows,
        )
        .unwrap();
        assert_eq!(file, r"C:\src\foo-12-bar.rs");
        assert_eq!(line_num, 42);
        assert!(is_match);
        assert_eq!(content, "hit");
    }

    #[test]
    fn ordinary_parser_preserves_context_separator_and_rejects_ambiguous_paths() {
        for line in [
            r"C:\src\file.rs-9-context:with:colons",
            r"C:\src\foo-12-bar.rs-42-context",
            r"context-9-content-9-more",
        ] {
            assert!(parse_match_line(line, ParserMode::OrdinaryWindows).is_none(), "{line}");
        }

        for line in [
            r"C:\dir:bad\file.rs:9:Foo",
            "unix:name.rs:99:Foo",
            "malformed output",
        ] {
            assert!(parse_match_line(line, ParserMode::OrdinaryWindows).is_none(), "{line}");
        }
    }

    #[test]
    fn test_parse_match_line_empty_content() {
        let line = "file.rs\x007:";
        let (file, line_num, is_match, content) = parse_match_line(line, ParserMode::Nul).unwrap();
        assert_eq!(file, "file.rs");
        assert_eq!(line_num, 7);
        assert!(is_match);
        assert_eq!(content, "");
    }

    // Context line: separator is `-` → is_match==false
    #[test]
    fn test_parse_match_line_context_line() {
        let line = "file.txt\x004-after1";
        let (file, line_num, is_match, content) = parse_match_line(line, ParserMode::Nul).unwrap();
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
        assert_eq!(unparsed_signal(stdout, ParserMode::Nul), 0);
    }

    #[test]
    fn test_unparsed_signal_context_separator_not_counted() {
        // The `--` context separator emitted by rg/grep between match groups
        // must not be counted as an unparsed line.
        let stdout = "file.txt\x001:hello\n--\nfile.txt\x003:world\n";
        assert_eq!(unparsed_signal(stdout, ParserMode::Nul), 0);
    }

    #[test]
    fn test_unparsed_signal_empty_line_not_counted() {
        let stdout = "file.txt\x001:hello\n\nfile.txt\x002:world\n";
        assert_eq!(unparsed_signal(stdout, ParserMode::Nul), 0);
    }

    #[test]
    fn test_unparsed_signal_bare_colon_line_parseable() {
        // Windows-only fallback shape is parseable without NUL.
        let stdout = "file.rs:1:content\n";
        assert_eq!(unparsed_signal(stdout, ParserMode::OrdinaryWindows), 0);
    }

    #[test]
    fn unparsed_signal_accepts_windows_content_colons() {
        let stdout = concat!(
            "C:\\src\\main.rs:42:https://example.com\n",
            "C:\\src\\lib.rs:18:ClassRegistry::init()\n",
            "C:\\src\\time.txt:3:12:45\n",
            "C:\\src\\file.rs-9-context:with:colons\n",
        );
        assert_eq!(unparsed_signal(stdout, ParserMode::OrdinaryWindows), 1);
        assert_eq!(
            unparsed_signal("weird:12:name.rs:99:Foo\n", ParserMode::OrdinaryWindows),
            0
        );
    }

    #[test]
    fn test_unparsed_signal_binary_notice_counted() {
        // rg emits "Binary file foo matches" for binary files; no NUL → counted.
        let stdout = "Binary file foo matches\n";
        assert_eq!(unparsed_signal(stdout, ParserMode::Nul), 1);
    }

    #[test]
    fn test_unparsed_signal_context_lines_parse_ok() {
        // Context lines (dash separator) parse via the updated regex → not counted.
        let stdout = "file.txt\x003-context_before\nfile.txt\x004:match\nfile.txt\x005-context_after\n";
        assert_eq!(unparsed_signal(stdout, ParserMode::Nul), 0);
    }

    #[test]
    fn parser_capability_selection_is_engine_and_platform_specific() {
        assert_eq!(
            parser_mode_for(Engine::Grep, true, false, false),
            Some(ParserMode::Nul)
        );
        assert_eq!(
            parser_mode_for(Engine::Grep, false, true, false),
            Some(ParserMode::Nul)
        );
        assert_eq!(
            parser_mode_for(Engine::Grep, false, false, true),
            Some(ParserMode::OrdinaryWindows)
        );
        assert_eq!(parser_mode_for(Engine::Grep, false, false, false), None);
        assert_eq!(
            parser_mode_for(Engine::Rg, false, false, false),
            Some(ParserMode::Nul)
        );
    }

    #[test]
    fn unavailable_structured_parser_fails_closed_without_passthrough() {
        assert_eq!(
            structured_parser_policy(None, true),
            StructuredParserPolicy::JsonError
        );
        assert_eq!(
            structured_parser_policy(None, false),
            StructuredParserPolicy::HumanError
        );
        assert_eq!(
            structured_parser_policy(Some(ParserMode::Nul), true),
            StructuredParserPolicy::Continue(ParserMode::Nul)
        );
        assert_eq!(
            structured_parser_policy(Some(ParserMode::OrdinaryWindows), false),
            StructuredParserPolicy::Continue(ParserMode::OrdinaryWindows)
        );
        assert!(STRUCTURED_PARSER_UNAVAILABLE.contains("--null or -Z"));
    }

    #[test]
    fn ordinary_windows_context_requires_nul_output() {
        assert_eq!(
            structured_context_policy(ParserMode::OrdinaryWindows, true, true),
            StructuredContextPolicy::JsonError
        );
        assert_eq!(
            structured_context_policy(ParserMode::OrdinaryWindows, true, false),
            StructuredContextPolicy::HumanError
        );
        assert_eq!(
            structured_context_policy(ParserMode::OrdinaryWindows, false, true),
            StructuredContextPolicy::Continue
        );
        assert_eq!(
            structured_context_policy(ParserMode::Nul, true, true),
            StructuredContextPolicy::Continue
        );
        assert!(STRUCTURED_CONTEXT_UNAVAILABLE.contains("--null or -Z"));
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

    #[test]
    fn grep_rtk_flags_parse_anywhere_before_dashdash() {
        let args = [
            "--agent-safe",
            "Foo",
            "src",
            "--json",
            "--max-per-file",
            "30",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        let (options, forwarded) = parse_grep_rtk_options(&args).unwrap();
        assert!(options.agent_safe);
        assert!(options.json);
        assert_eq!(options.max_per_file, Some(30));
        assert_eq!(forwarded, vec!["Foo", "src"]);
    }

    #[test]
    fn grep_rtk_flags_after_dashdash_are_literals() {
        let args = ["Foo", "--", "--agent-safe", "literal.rs"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        let (options, forwarded) = parse_grep_rtk_options(&args).unwrap();
        assert_eq!(options, GrepRtkOptions::default());
        assert_eq!(forwarded, args);
    }

    #[test]
    fn context_only_is_rtk_option_before_or_after_path() {
        for args in [
            vec!["--context-only", "Foo", "src"],
            vec!["Foo", "--context-only", "src"],
            vec!["Foo", "src", "--context-only"],
        ] {
            let args = args.into_iter().map(String::from).collect::<Vec<_>>();
            let (options, forwarded) = parse_grep_rtk_options(&args).unwrap();
            assert!(options.context_only);
            assert_eq!(forwarded, vec!["Foo", "src"]);
        }
    }

    #[test]
    fn grep_rtk_numeric_values_fail_closed() {
        let args = vec!["Foo".to_string(), "--max-matches".to_string(), "bad".to_string()];
        assert!(parse_grep_rtk_options(&args).is_err());
    }

    #[test]
    fn grep_native_operands_keep_rtk_names_literal() {
        let cases = [
            (vec!["-e", "--json", "README.md"], false),
            (vec!["--regexp", "--agent-safe", "src"], false),
            (vec!["-f", "--files-only", "src"], false),
            (vec!["--file", "--top-files", "src"], false),
            (vec!["--label", "--json", "pattern"], false),
            (vec!["-A", "--json", "needle", "file"], false),
        ];
        for (raw, _) in cases {
            let args = raw.into_iter().map(String::from).collect::<Vec<_>>();
            let (options, forwarded) = parse_grep_rtk_options(&args).unwrap();
            assert_eq!(options, GrepRtkOptions::default());
            assert_eq!(forwarded, args);
            assert!(!json_requested_before_dashdash(&forwarded));
        }
    }

    #[test]
    fn grep_native_operand_then_rtk_option_scans_after_operand() {
        let args = ["-e", "--json", "--agent-safe", "file"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        let (options, forwarded) = parse_grep_rtk_options(&args).unwrap();
        assert!(options.agent_safe);
        assert!(!options.json);
        assert_eq!(forwarded, vec!["-e", "--json", "file"]);
        assert!(!json_requested_before_dashdash(&args));

        let args = ["--regexp=--json", "file"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        let (options, forwarded) = parse_grep_rtk_options(&args).unwrap();
        assert!(!options.json);
        assert_eq!(forwarded, args);
        let (patterns, paths, _) = extract_pattern_path(&forwarded);
        assert_eq!(patterns, vec!["--json"]);
        assert_eq!(paths, vec!["file"]);

        let args = ["--file", "--agent-safe", "pattern"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        let (patterns, paths, flags) = extract_grep_pattern_path(&args);
        assert_eq!(patterns, vec!["pattern"]);
        assert!(paths.is_empty());
        assert_eq!(flags.argv, vec!["--file", "--agent-safe"]);
        assert_eq!(flags.operand, vec![false, true]);

        let args = ["pattern", "--", "--json"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        assert!(!json_requested_before_dashdash(&args));
    }

    #[test]
    fn rg_extraction_keeps_upstream_value_roles() {
        let (patterns, paths, flags) = extract_pattern_path(&["--include", "needle", "src"]);
        assert_eq!(patterns, vec!["needle"]);
        assert_eq!(paths, vec!["src"]);
        assert_eq!(flags, vec!["--include"]);
    }

    #[test]
    fn grep_native_roles_survive_extraction_and_policy_scans() {
        let cases = [
            (
                vec!["--label", "--json", "pattern", "--agent-safe"],
                false,
                false,
                false,
                false,
            ),
            (
                vec!["--file", "-q", "pattern", "--agent-safe"],
                false,
                false,
                false,
                false,
            ),
            (
                vec!["--label", "-A", "pattern", "--agent-safe"],
                false,
                false,
                false,
                false,
            ),
            (
                vec!["--exclude", "--color=always", "pattern", "--agent-safe"],
                false,
                false,
                false,
                false,
            ),
            (
                vec!["--label", "-N", "pattern", "--context-only"],
                false,
                false,
                false,
                false,
            ),
        ];
        for (raw, json, context, color, shape) in cases {
            let args = raw.into_iter().map(String::from).collect::<Vec<_>>();
            let (options, forwarded) = parse_grep_rtk_options(&args).unwrap();
            let (_, _, native) = extract_grep_pattern_path(&forwarded);
            assert_eq!(options.json, json);
            assert_eq!(grep_has_context_flag(&native), context);
            assert_eq!(has_forced_color(&native), color);
            assert_eq!(grep_has_format_flag(&native), shape);
        }

        let args = ["--label", "value", "-A", "2", "pattern", "--agent-safe"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        let (options, forwarded) = parse_grep_rtk_options(&args).unwrap();
        let (_, _, native) = extract_grep_pattern_path(&forwarded);
        assert!(options.agent_safe);
        assert!(grep_has_context_flag(&native));
        assert_eq!(context_spec(&native).after, 2);

        let (_, _, native) = extract_grep_pattern_path(&["-A2", "pattern"]);
        assert_eq!(context_spec(&native).after, 2);
        let (_, _, native) = extract_grep_pattern_path(&["--context=3", "pattern"]);
        assert_eq!(context_spec(&native).before, 3);
        assert_eq!(context_spec(&native).after, 3);
    }

    #[test]
    fn grep_shape_flags_reject_only_actual_flags() {
        for flag in [
            "-h",
            "--no-filename",
            "-N",
            "--no-line-number",
            "-T",
            "--initial-tab",
            "--group-separator",
            "--group-separator=SEP",
        ] {
            let (_, _, native) = extract_grep_pattern_path(&[flag, "pattern"]);
            assert!(grep_has_format_flag(&native), "{flag}");
        }
        for (flag, operand_flag) in [("--label", "-h"), ("--file", "--group-separator=SEP")] {
            let (_, _, native) = extract_grep_pattern_path(&[flag, operand_flag, "pattern"]);
            assert!(!grep_has_format_flag(&native), "{flag} {operand_flag}");
        }
    }

    #[test]
    fn grep_forced_color_uses_aligned_native_roles() {
        let extract = |raw: &[&str]| {
            let args = raw.iter().map(|arg| (*arg).to_string()).collect::<Vec<_>>();
            let (_, forwarded) = parse_grep_rtk_options(&args).unwrap();
            let (_, _, native) = extract_grep_pattern_path(&forwarded);
            native
        };

        for raw in [
            &["--color=always", "pattern"][..],
            &["--colour=always", "pattern"][..],
            &["--color", "always", "pattern"][..],
            &["--colour", "always", "pattern"][..],
        ] {
            assert!(has_forced_color(&extract(raw)));
        }
        for raw in [
            &["--color=auto", "pattern"][..],
            &["--color", "auto", "pattern"][..],
            &["--color", "never", "pattern"][..],
            &["--exclude", "--color=always", "pattern", "--agent-safe"][..],
            &["--exclude", "always", "pattern", "--agent-safe"][..],
            &["--label", "--color=always", "pattern", "--agent-safe"][..],
            &["--label", "--color", "pattern", "--agent-safe"][..],
            &["--file", "always", "pattern", "--agent-safe"][..],
        ] {
            assert!(!has_forced_color(&extract(raw)));
        }

        let native = extract(&[
            "--label",
            "value",
            "--color",
            "always",
            "pattern",
            "--agent-safe",
        ]);
        assert_eq!(native.argv, ["--label", "value", "--color", "always"]);
        assert_eq!(native.operand, [false, true, false, true]);
        assert!(has_forced_color(&native));

        let native = extract(&["--label", "--json", "pattern", "--agent-safe"]);
        assert_eq!(native.argv, ["--label", "--json"]);
        assert_eq!(native.operand, [false, true]);
        assert!(!has_forced_color(&native));
    }

    #[test]
    fn grep_missing_required_native_operands_leave_no_injected_pattern() {
        for option in [
            "--after-context",
            "--before-context",
            "--context",
            "--binary-files",
            "--devices",
            "--directories",
            "--regexp",
            "--file",
            "--max-count",
            "--exclude",
            "--exclude-from",
            "--exclude-dir",
            "--include",
            "--label",
            "--group-separator",
            "--encoding",
            "-A",
            "-B",
            "-C",
            "-D",
            "-d",
            "-e",
            "-f",
            "-m",
        ] {
            let args = vec![option.to_string()];
            let (_, forwarded) = parse_grep_rtk_options(&args).unwrap();
            let (patterns, _, native) = extract_grep_pattern_path(&forwarded);
            assert!(patterns.is_empty(), "unexpected pattern for {option}");
            assert_eq!(native.missing_operand.as_deref(), Some(option));
            assert_eq!(
                missing_grep_operand_error(&native).as_deref(),
                Some(format!("missing native grep operand for {option}").as_str())
            );
        }

        for option in [
            "--after-context",
            "--before-context",
            "--context",
            "--binary-files",
            "--devices",
            "--directories",
            "--regexp",
            "--file",
            "--max-count",
            "--exclude",
            "--exclude-from",
            "--exclude-dir",
            "--include",
            "--label",
            "--group-separator",
            "--encoding",
            "-A",
            "-B",
            "-C",
            "-D",
            "-d",
            "-e",
            "-f",
            "-m",
        ] {
            let args = ["--agent-safe", "pattern", option]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>();
            let (_, forwarded) = parse_grep_rtk_options(&args).unwrap();
            let (patterns, _, native) = extract_grep_pattern_path(&forwarded);
            assert_eq!(patterns, vec!["pattern"]);
            assert_eq!(native.missing_operand.as_deref(), Some(option));
            assert_eq!(native.argv.last().map(String::as_str), Some(option));
        }
    }

    #[test]
    fn grep_missing_operand_controls_and_rtklike_operands() {
        for raw in [
            ["pattern", "--json", "--label"],
            ["pattern", "--json", "-A"],
            ["pattern", "--agent-safe", "--label"],
            ["pattern", "--context-only", "-e"],
        ] {
            let args = raw.into_iter().map(String::from).collect::<Vec<_>>();
            let (options, forwarded) = parse_grep_rtk_options(&args).unwrap();
            let (patterns, _, native) = extract_grep_pattern_path(&forwarded);
            assert_eq!(patterns, vec!["pattern"]);
            assert!(native.missing_operand.is_some());
            assert!(options.json || options.agent_safe || options.context_only);
        }

        for raw in [
            vec!["pattern", "--agent-safe", "--label", "value"],
            vec!["pattern", "--agent-safe", "--label=value"],
            vec!["pattern", "--agent-safe", "-A", "2"],
            vec!["pattern", "--agent-safe", "-A2"],
            vec!["--label", "--agent-safe", "pattern"],
        ] {
            let args = raw.into_iter().map(String::from).collect::<Vec<_>>();
            let (_, forwarded) = parse_grep_rtk_options(&args).unwrap();
            let (patterns, _, native) = extract_grep_pattern_path(&forwarded);
            assert!(!patterns.is_empty());
            assert_eq!(native.missing_operand, None);
        }

        let args = ["-e", "--json", "file", "--agent-safe"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        let (options, forwarded) = parse_grep_rtk_options(&args).unwrap();
        let (patterns, paths, native) = extract_grep_pattern_path(&forwarded);
        assert!(!options.json);
        assert!(options.agent_safe);
        assert_eq!(patterns, vec!["--json"]);
        assert_eq!(paths, vec!["file"]);
        assert_eq!(native.missing_operand, None);
    }

    #[test]
    fn run_grep_rejects_missing_operand_before_child() {
        let args = ["--agent-safe", "pattern", "--label"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        let error = run_grep(240, 80, false, &args, 0).unwrap_err();
        assert_eq!(error.to_string(), "missing native grep operand for --label");
    }

    #[test]
    fn ordinary_windows_labels_are_validated_without_affecting_nul() {
        for label in ["custom label", "(standard input)"] {
            let (_, _, native) = extract_grep_pattern_path(&["--label", label, "pattern"]);
            assert_eq!(invalid_grep_label(ParserMode::OrdinaryWindows, &native), None);
        }
        let (_, _, native) = extract_grep_pattern_path(&["--label=custom label", "pattern"]);
        assert_eq!(invalid_grep_label(ParserMode::OrdinaryWindows, &native), None);
        for label in ["foo:12:bar", "", "bad\rlabel", "bad\nlabel"] {
            let (_, _, native) = extract_grep_pattern_path(&["--label", label, "pattern"]);
            assert!(invalid_grep_label(ParserMode::OrdinaryWindows, &native).is_some());
            assert_eq!(
                invalid_grep_label(ParserMode::Nul, &native).is_some(),
                label.is_empty() || label.contains(['\r', '\n'])
            );
        }
        let (_, _, native) = extract_grep_pattern_path(&["--label=foo:12:bar", "pattern"]);
        assert!(invalid_grep_label(ParserMode::OrdinaryWindows, &native).is_some());
        let (_, _, native) = extract_grep_pattern_path(&["--file", "foo:12:bar", "pattern"]);
        assert_eq!(invalid_grep_label(ParserMode::OrdinaryWindows, &native), None);
    }

    #[test]
    fn stderr_sink_error_drains_input_before_returning() {
        struct ErrorWriter;
        impl Write for ErrorWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("sink"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let input = vec![b'x'; LIVE_STREAM_CHUNK * 2 + 1];
        let input_len = input.len();
        let mut reader = ChunkedReader {
            bytes: input,
            offset: 0,
            chunk: LIVE_STREAM_CHUNK / 2,
        };
        let mut writer = ErrorWriter;
        assert!(drain_stderr_to(&mut reader, &mut writer).is_err());
        assert_eq!(reader.offset, input_len);
    }

    #[test]
    fn context_only_stream_is_excerpt_without_unrequested_summary() {
        let mut filter = AgentSafeStreamFilter {
            show_file: false,
            show_line: true,
            max_results: None,
            max_per_file: None,
            max_line_chars: None,
            pattern: "needle".to_string(),
            context_only: true,
            shown: 0,
            total_matches: 0,
            omitted_per_file: 0,
            clipped_lines: 0,
            shown_by_file: HashMap::new(),
            parser_mode: ParserMode::OrdinaryWindows,
            context: ContextSpec::default(),
            pending_context: std::collections::VecDeque::new(),
            active_file: None,
            after_remaining: 0,
            total_context: 0,
            displayed_context: 0,
            last_output_file: None,
            last_output_line: None,
            emit_summary: false,
            parse_failed: false,
        };
        let output = filter
            .feed_line("(standard input):1:prefix needle suffix with more text")
            .unwrap();
        assert!(output.contains("needle"));
        assert!(!output.contains("prefix needle suffix with more text"));
        assert!(filter.flush().is_empty());
        assert!(!filter.parse_failed);
    }

    #[test]
    fn ordinary_stdin_stream_filter_caps_without_raw_bypass() {
        let mut filter = AgentSafeStreamFilter {
            show_file: false,
            show_line: true,
            max_results: Some(1),
            max_per_file: Some(5),
            max_line_chars: Some(80),
            pattern: "hit".to_string(),
            context_only: false,
            shown: 0,
            total_matches: 0,
            omitted_per_file: 0,
            clipped_lines: 0,
            shown_by_file: HashMap::new(),
            parser_mode: ParserMode::OrdinaryWindows,
            context: ContextSpec::default(),
            pending_context: std::collections::VecDeque::new(),
            active_file: None,
            after_remaining: 0,
            total_context: 0,
            displayed_context: 0,
            last_output_file: None,
            last_output_line: None,
            emit_summary: true,
            parse_failed: false,
        };
        assert!(filter.feed_line("(standard input):1:hit").is_some());
        assert!(filter.feed_line("(standard input):2:hit").is_none());
        let summary = filter.flush();
        assert!(summary.contains("total=2 shown=1 omitted_total=1"));
        assert!(!summary.contains("hit\n"));
        assert!(!filter.parse_failed);
    }

    #[test]
    fn agent_safe_stream_suppresses_unparsed_rows_and_reports_once() {
        let mut filter = AgentSafeStreamFilter {
            show_file: false,
            show_line: true,
            max_results: Some(1),
            max_per_file: Some(1),
            max_line_chars: Some(80),
            pattern: "Foo".to_string(),
            context_only: false,
            shown: 0,
            total_matches: 0,
            omitted_per_file: 0,
            clipped_lines: 0,
            shown_by_file: HashMap::new(),
            parser_mode: ParserMode::Nul,
            context: ContextSpec::default(),
            pending_context: std::collections::VecDeque::new(),
            active_file: None,
            after_remaining: 0,
            total_context: 0,
            displayed_context: 0,
            last_output_file: None,
            last_output_line: None,
            emit_summary: true,
            parse_failed: false,
        };
        assert!(filter.feed_line("malformed stdout").is_none());
        assert!(filter.feed_line("another malformed stdout").is_none());
        assert!(filter.parse_failed);
        assert!(filter.flush().is_empty());
        assert!(filter.feed_line("stdin\x001:Foo").is_some());
    }

    #[test]
    fn bounded_capture_stays_prefix_stable_when_scalar_hits_cap() {
        let one = capture_stream_bounded(std::io::Cursor::new(b"a"), 1).unwrap();
        assert_eq!(one.text, "a");
        assert!(one.truncated);

        let two = capture_stream_bounded(std::io::Cursor::new(b"ab"), 2).unwrap();
        assert_eq!(two.text, "ab");
        assert!(two.truncated);

        let utf8 = capture_stream_bounded("😀".as_bytes(), 4).unwrap();
        assert_eq!(utf8.text, "😀");
        assert!(utf8.truncated);
        assert!(utf8.text.len() <= 4);
    }

    #[test]
    fn grep_agent_safe_truth_values_are_conservative() {
        for value in ["1", "true", "TRUE", "yes", "on"] {
            assert!(parse_bool_value(value));
        }
        for value in ["0", "false", "no", "off", "", "unknown", "y"] {
            assert!(!parse_bool_value(value));
        }
    }

    #[test]
    fn grep_structured_renderer_keeps_true_counts_and_paths() {
        let path = r#"C:\work dir\a"b.rs"#;
        let raw = format!(
            "{path}\x001:Foo long line 😀\n{path}\x002-context\n{path}\x003:Foo second\n"
        );
        let options = GrepEffectiveOptions {
            mode: GrepMode::Matches,
            all: false,
            full_lines: false,
            agent_safe: true,
            json: true,
            context_only: false,
            max_matches: Some(10),
            max_per_file: Some(1),
            max_line_chars: Some(5),
            summary: true,
        };
        let (human, total, displayed, clipped, _, files) =
            render_grep_structured(
                options,
                &["Foo".to_string()],
                &raw,
                ParserMode::Nul,
                &GrepNativeArgs {
                    argv: Vec::new(),
                    operand: Vec::new(),
                    missing_operand: None,
                },
            );
        assert!(human.contains(path));
        assert_eq!(total, 2);
        assert_eq!(displayed, 1);
        assert_eq!(clipped, 1);
        assert_eq!(files[0].true_match_count, 2);
        assert_eq!(files[0].omitted_count, 1);
        let json = render_grep_json(
            GrepMode::Matches,
            &["Foo".to_string()],
            &[path.to_string()],
            &files,
            1,
            total,
            displayed,
            total - displayed,
            clipped,
            Vec::new(),
            Vec::new(),
            None,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["files"][0]["path"], path);
        assert_eq!(value["files"][0]["true_match_count"], 2);
        assert_eq!(value["omitted_match_count"], 1);
    }

    #[test]
    fn grep_clip_bounds_use_unicode_scalar_values() {
        for limit in 0..=10 {
            let (text, clipped) = clipped_text("😀😀😀😀😀😀", Some(limit), "😀");
            assert_eq!(clipped, limit < 6);
            assert!(text.chars().count() <= limit, "limit={limit}, text={text:?}");
        }
    }

    #[test]
    fn grep_clip_does_not_map_expanded_lowercase_indices() {
        for limit in 0..=10 {
            let text = format!("{}needle", "İ".repeat(20));
            let (clipped, _) = clipped_text(&text, Some(limit), "needle");
            assert!(clipped.chars().count() <= limit, "limit={limit}, text={clipped:?}");
        }
        for text in ["ß".repeat(20), "Σσς".repeat(20), "é".repeat(20), "😀".repeat(20)] {
            for limit in [0, 1, 2, 3, 4, 5, 6, usize::MAX] {
                let (clipped, _) = clipped_text(&text, Some(limit), "");
                assert!(clipped.chars().count() <= limit);
            }
        }
    }

    #[test]
    fn ordinary_parser_treats_later_digit_colons_as_content() {
        let (file, line, is_match, content) =
            parse_match_line("weird:12:name.rs:99:Foo", ParserMode::OrdinaryWindows).unwrap();
        assert_eq!(file, "weird");
        assert_eq!(line, 12);
        assert!(is_match);
        assert_eq!(content, "name.rs:99:Foo");
        assert!(parse_match_line(
            r"C:\src\file.rs:99:Foo",
            ParserMode::OrdinaryWindows
        )
        .is_some());
        assert!(parse_match_line("unix:name.rs:99:Foo", ParserMode::Nul).is_none());
    }

    #[test]
    fn context_selection_does_not_attach_omitted_match_context() {
        let rows = vec![
            (1, true, "first".to_string()),
            (2, false, "before omitted".to_string()),
            (3, true, "second".to_string()),
            (4, false, "after second".to_string()),
        ];
        let selected = [0usize].into_iter().collect();
        let (rows, _) = selected_context_rows(
            &rows,
            &selected,
            ContextSpec {
                before: 1,
                after: 1,
            },
        );
        assert!(rows.contains(&0));
        assert!(rows.contains(&1));
        assert!(!rows.contains(&2));
        assert!(!rows.contains(&3));
    }

    #[test]
    fn agent_safe_stream_reports_true_omissions_at_flush() {
        let mut filter = AgentSafeStreamFilter {
            show_file: false,
            show_line: true,
            max_results: Some(1),
            max_per_file: Some(10),
            max_line_chars: Some(80),
            pattern: "Foo".to_string(),
            context_only: false,
            shown: 0,
            total_matches: 0,
            omitted_per_file: 0,
            clipped_lines: 0,
            shown_by_file: HashMap::new(),
            parser_mode: ParserMode::Nul,
            context: ContextSpec::default(),
            pending_context: std::collections::VecDeque::new(),
            active_file: None,
            after_remaining: 0,
            total_context: 0,
            displayed_context: 0,
            last_output_file: None,
            last_output_line: None,
            emit_summary: true,
            parse_failed: false,
        };
        assert!(filter.feed_line("stdin\x001:Foo").is_some());
        assert!(filter.feed_line("stdin\x002:Foo").is_none());
        assert!(filter.flush().contains("total=2 shown=1 omitted_total=1"));
    }
}
