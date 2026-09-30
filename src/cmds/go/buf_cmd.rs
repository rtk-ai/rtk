//! Filters buf (Protobuf toolchain) output: lint/build/breaking diagnostics grouped by rule,
//! `format -d` diffs summarised per file, `generate` failures trimmed. Reached as `rtk buf`
//! and, through `go_cmd`, as `rtk go tool buf`.

use crate::core::arg_tokenizer::{self, Dialect, Token, TokenKind, ValueSpec};
use crate::core::args_utils;
use crate::core::guard::never_worse;
use crate::core::runner;
use crate::core::tee;
use crate::core::truncate::{self, CAP_ERRORS, CAP_LIST, CAP_WARNINGS};
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::process::Command;
use std::sync::LazyLock;

const MAX_GROUPS: usize = CAP_ERRORS;
// One line per location across up to MAX_GROUPS groups: the full warnings cap per group floods.
const MAX_LOCATIONS: usize = truncate::reduced(CAP_WARNINGS, 5);
const MAX_FORMAT_FILES: usize = CAP_LIST;
const MAX_GENERATE_LINES: usize = CAP_ERRORS;
/// Stack lines kept after `panic:`: the goroutine header, the panicking frame, its location.
const KEPT_STACK_LINES: usize = 3;

/// A line of a Go runtime stack dump: `goroutine N [state]:`, a `pkg.func(args)` frame, its
/// tab-indented location, `created by …`, the elision marker, or a blank separator.
static GO_STACK_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:goroutine \d+ \[[^\]]*\]:|\t.*|created by .*|[\w./*()\[\]-]+\(.*\)|\.\.\.additional frames elided\.\.\.|)$")
        .unwrap()
});

/// The compile error a missing import produces; every `cannot find` after it is its fallout.
const ROOT_CAUSE_MESSAGE: &str = "imported file does not exist";

/// buf's exit code when it reported at least one diagnostic.
const EXIT_DIAGNOSTICS: i32 = 100;

const FORMAT_TEE_LABEL: &str = "buf-format";

/// Backtick- or double-quote-delimited spans: the identifier that varies between otherwise
/// identical compile errors.
static QUOTED_SPAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"`[^`]*`|"[^"]*""#).unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BufBin {
    Direct,
    GoTool,
}

impl BufBin {
    fn command(self) -> Command {
        match self {
            BufBin::Direct => resolved_command("buf"),
            BufBin::GoTool => {
                let mut cmd = resolved_command("go");
                cmd.args(["tool", "buf"]);
                cmd
            }
        }
    }

    fn tool_name(self) -> &'static str {
        match self {
            BufBin::Direct => "buf",
            BufBin::GoTool => "go tool buf",
        }
    }

    /// The program `runner::run_passthrough` resolves, and the words placed before the user's.
    fn passthrough_prefix(self) -> (&'static str, &'static [&'static str]) {
        match self {
            BufBin::Direct => ("buf", &[]),
            BufBin::GoTool => ("go", &["tool", "buf"]),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagSub {
    Lint,
    Build,
    Breaking,
}

impl DiagSub {
    fn name(self) -> &'static str {
        match self {
            DiagSub::Lint => "lint",
            DiagSub::Build => "build",
            DiagSub::Breaking => "breaking",
        }
    }

    fn tee_label(self) -> String {
        format!("buf-{}", self.name())
    }

    fn takes_value(self) -> fn(TokenKind, &str) -> Option<ValueSpec> {
        match self {
            DiagSub::Lint => lint_takes_value,
            DiagSub::Build => build_takes_value,
            DiagSub::Breaking => breaking_takes_value,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    Diagnostics { sub: DiagSub, inject_at: usize },
    FormatDiff,
    Generate,
    Passthrough,
}

// Grammars transcribed from `buf <sub> --help` (buf 1.73.0). One per subcommand; each one
// composes the shared input flags, a strict subset of every subcommand here, the same way
// golangci's `run_takes_value` composes its globals.

/// buf's global value-taking flags. Every one is also accepted after a subcommand.
fn global_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    (kind == TokenKind::Long && matches!(name, "log-format" | "timeout")).then(ValueSpec::value)
}

fn input_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    global_takes_value(kind, name).or_else(|| {
        (kind == TokenKind::Long
            && matches!(name, "config" | "error-format" | "exclude-path" | "path"))
        .then(ValueSpec::value)
    })
}

fn lint_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    input_takes_value(kind, name)
}

fn build_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    input_takes_value(kind, name).or_else(|| match kind {
        TokenKind::Long => matches!(name, "output" | "type").then(ValueSpec::value),
        TokenKind::Short => (name == "o").then(ValueSpec::solo_only),
        _ => None,
    })
}

fn breaking_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    input_takes_value(kind, name).or_else(|| {
        (kind == TokenKind::Long && matches!(name, "against" | "against-config"))
            .then(ValueSpec::value)
    })
}

fn format_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    input_takes_value(kind, name).or_else(|| match kind {
        TokenKind::Long => (name == "output").then(ValueSpec::value),
        TokenKind::Short => (name == "o").then(ValueSpec::solo_only),
        _ => None,
    })
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let args = args_utils::restore_double_dash(args);
    run_with(BufBin::Direct, &args, verbose)
}

/// Entry for both executables. `args` must already have `--` restored.
pub(crate) fn run_with(bin: BufBin, args: &[String], verbose: u8) -> Result<i32> {
    match classify(args) {
        Invocation::Diagnostics { sub, inject_at } => {
            run_diagnostics(bin, sub, args, inject_at, verbose)
        }
        Invocation::FormatDiff => run_format_diff(bin, args, verbose),
        Invocation::Generate => run_generate(bin, args, verbose),
        Invocation::Passthrough => run_passthrough(bin, args, verbose),
    }
}

fn run_passthrough(bin: BufBin, args: &[String], verbose: u8) -> Result<i32> {
    let (program, prefix) = bin.passthrough_prefix();
    let os_args: Vec<OsString> = prefix
        .iter()
        .map(OsString::from)
        .chain(args.iter().map(OsString::from))
        .collect();
    runner::run_passthrough(program, &os_args, verbose)
}

fn classify(args: &[String]) -> Invocation {
    let Some(sub_index) = find_subcommand_index(args) else {
        return Invocation::Passthrough;
    };
    // `buf -h lint` prints lint's help: a global help flag before the subcommand counts too.
    let globals =
        arg_tokenizer::tokenize_grammar(&args[..sub_index], &global_takes_value, Dialect::Posix);
    if wants_help(&globals) {
        return Invocation::Passthrough;
    }
    let sub_args = &args[sub_index + 1..];
    match args[sub_index].as_str() {
        "lint" => classify_diagnostics(DiagSub::Lint, sub_index, sub_args),
        "build" => classify_diagnostics(DiagSub::Build, sub_index, sub_args),
        "breaking" => classify_diagnostics(DiagSub::Breaking, sub_index, sub_args),
        "format" => classify_format(sub_args),
        "generate" => classify_generate(sub_args),
        _ => Invocation::Passthrough,
    }
}

/// Index of the first free positional (the subcommand), `None` at `--` or when there is none.
fn find_subcommand_index(args: &[String]) -> Option<usize> {
    let tokens = arg_tokenizer::tokenize_grammar(args, &global_takes_value, Dialect::Posix);
    for token in &tokens {
        match token.kind {
            TokenKind::DashDash => return None,
            TokenKind::Positional if token.is_free_positional() => {
                return Some(token.source_index);
            }
            _ => {}
        }
    }
    None
}

fn classify_diagnostics(sub: DiagSub, sub_index: usize, sub_args: &[String]) -> Invocation {
    let takes_value = sub.takes_value();
    let tokens = arg_tokenizer::tokenize_grammar(sub_args, &takes_value, Dialect::Posix);
    let own = arg_tokenizer::before_dashdash(&tokens);
    // A user-chosen report format (github-actions, junit, …) is theirs to read.
    if wants_help(own)
        || arg_tokenizer::has_flag(own, Dialect::Posix, "error-format")
        || (sub == DiagSub::Build && writes_image_to_stdout(own, &tokens))
    {
        return Invocation::Passthrough;
    }
    Invocation::Diagnostics {
        sub,
        inject_at: sub_index + 1 + arg_tokenizer::injection_point(&tokens, sub_args.len()),
    }
}

/// Only `-d`/`--diff` is filtered: `-w` prints nothing and bare `format` prints the formatted
/// source the user asked for.
fn classify_format(sub_args: &[String]) -> Invocation {
    let tokens = arg_tokenizer::tokenize_grammar(sub_args, &format_takes_value, Dialect::Posix);
    let own = arg_tokenizer::before_dashdash(&tokens);
    let diff = arg_tokenizer::has_flag(own, Dialect::Posix, "diff")
        || own
            .iter()
            .any(|t| t.kind == TokenKind::Short && t.text == "d");
    if diff && !wants_help(own) {
        Invocation::FormatDiff
    } else {
        Invocation::Passthrough
    }
}

fn wants_help(own: &[Token<'_>]) -> bool {
    arg_tokenizer::has_flag(own, Dialect::Posix, "help")
        || own
            .iter()
            .any(|t| t.kind == TokenKind::Short && t.text == "h")
}

const STDOUT_DESTINATIONS: &[&str] = &["-", "/dev/stdout", "/dev/fd/1"];

/// `buf build -o -` (or `/dev/stdout`, either with `#format=…`) writes the image to
/// stdout, where it is the user's output rather than diagnostics. Capturing it as text would
/// corrupt the binary image.
fn writes_image_to_stdout(own: &[Token<'_>], tokens: &[Token<'_>]) -> bool {
    own.iter().any(|t| {
        let is_long = t.kind == TokenKind::Long && t.text == "output";
        let is_short = t.kind == TokenKind::Short && t.text == "o";
        if !is_long && !is_short {
            return false;
        }
        t.value(tokens).is_some_and(|value| {
            // pflag accepts `-o=VALUE`, which the tokenizer attaches as `=VALUE`.
            let value = if is_short {
                value.strip_prefix('=').unwrap_or(value)
            } else {
                value
            };
            let destination = value.split('#').next().unwrap_or(value);
            STDOUT_DESTINATIONS.contains(&destination)
        })
    })
}

fn with_json_errors(args: &[String], inject_at: usize) -> Vec<String> {
    let mut out = args.to_vec();
    out.insert(inject_at, "--error-format=json".to_string());
    out
}

fn run_diagnostics(
    bin: BufBin,
    sub: DiagSub,
    args: &[String],
    inject_at: usize,
    verbose: u8,
) -> Result<i32> {
    let filtered_args = with_json_errors(args, inject_at);
    let mut cmd = bin.command();
    cmd.args(&filtered_args);
    if verbose > 1 {
        eprintln!("Running: {} {}", bin.tool_name(), filtered_args.join(" "));
    }
    let tee_label = sub.tee_label();
    // `buf build` reports its diagnostics on stderr, lint and breaking on stdout
    // (buf 1.73.0), so build filters the combined stream and the others keep stderr apart.
    let opts = match sub {
        DiagSub::Build => runner::RunOptions::with_tee(&tee_label),
        DiagSub::Lint | DiagSub::Breaking => runner::RunOptions::stdout_only().tee(&tee_label),
    };
    runner::run_filtered_with_exit(
        cmd,
        bin.tool_name(),
        &args.join(" "),
        move |output, exit_code| {
            if verbose > 2 {
                eprintln!("{output}");
            }
            let filtered = filter_buf_diagnostics(sub.name(), output);
            if warns_not_json(output, &filtered, exit_code) {
                eprintln!("rtk: filter warning: buf output is not JSON, showing it unchanged");
            }
            if verbose > 0 {
                eprintln!(
                    "rtk buf {}: {} lines in, {} lines out",
                    sub.name(),
                    output.lines().count(),
                    filtered.lines().count()
                );
            }
            with_exit_zero_hint(sub, output, filtered, exit_code, tee::force_tee_hint)
        },
        opts,
    )
}

/// Only warn when diagnostics were expected — buf exited 100 or printed a
/// JSON-shaped line — yet nothing parsed. A config error or a `WARN` log line on build's
/// combined stream is plain text, not a sign buf ignored `--error-format`.
fn warns_not_json(output: &str, filtered: &str, exit_code: i32) -> bool {
    filtered == output
        && !output.trim().is_empty()
        && (exit_code == EXIT_DIAGNOSTICS
            || output.lines().any(|l| l.trim_start().starts_with('{')))
}

/// Lint, build and breaking exit 100 whenever they report a diagnostic, and on a
/// non-zero exit the runner's tee already stores the raw output. So this hint rarely fires: only
/// for a capped listing on exit 0. `store` is `tee::force_tee_hint` outside tests.
fn with_exit_zero_hint(
    sub: DiagSub,
    output: &str,
    filtered: String,
    exit_code: i32,
    store: impl FnOnce(&str, &str) -> Option<String>,
) -> String {
    if exit_code != 0 || filtered == output {
        return filtered;
    }
    let full = render_diagnostics(
        sub.name(),
        &parse_diagnostics(output),
        usize::MAX,
        usize::MAX,
    );
    if full == filtered {
        return filtered;
    }
    match store(&full, &sub.tee_label()) {
        Some(hint) => format!("{filtered}\n{hint}"),
        None => filtered,
    }
}

#[derive(Debug, Deserialize)]
struct Diag {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    start_line: u32,
    #[serde(rename = "type")]
    kind: String,
    message: String,
}

struct Parsed<'a> {
    diags: Vec<Diag>,
    unparsed: Vec<&'a str>,
}

fn parse_diagnostics(output: &str) -> Parsed<'_> {
    let mut diags = Vec::new();
    let mut unparsed = Vec::new();
    for line in output.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<Diag>(line) {
            Ok(d) => diags.push(d),
            // Kept verbatim under `unparsed:`, never dropped.
            Err(_) => unparsed.push(line),
        }
    }
    Parsed { diags, unparsed }
}

/// Group order: the missing import first, then its COMPILE fallout, then rule violations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    RootCause,
    Compile,
    Rule,
}

struct Group<'d> {
    title: String,
    /// The masked message a COMPILE group is keyed on; `None` for a rule group.
    message: Option<String>,
    rank: Rank,
    items: Vec<&'d Diag>,
}

fn normalize_compile_message(message: &str) -> String {
    QUOTED_SPAN
        .replace_all(message, |c: &regex::Captures| {
            let quote = &c[0][..1];
            format!("{quote}…{quote}")
        })
        .into_owned()
}

fn group_diagnostics(diags: &[Diag]) -> Vec<Group<'_>> {
    let mut by_title: HashMap<String, Group<'_>> = HashMap::new();
    for d in diags {
        // One missing import cascades into thousands of COMPILE errors that differ
        // only in the identifier, so COMPILE groups on the masked message, not the type.
        let (title, message, rank) = if d.kind == "COMPILE" {
            let masked = normalize_compile_message(&d.message);
            let rank = if masked == ROOT_CAUSE_MESSAGE {
                Rank::RootCause
            } else {
                Rank::Compile
            };
            (format!("COMPILE {masked}"), Some(masked), rank)
        } else {
            (d.kind.clone(), None, Rank::Rule)
        };
        by_title
            .entry(title.clone())
            .or_insert_with(|| Group {
                title,
                message,
                rank,
                items: Vec::new(),
            })
            .items
            .push(d);
    }
    let mut groups: Vec<_> = by_title.into_values().collect();
    groups.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then(b.items.len().cmp(&a.items.len()))
            .then_with(|| a.title.cmp(&b.title))
    });
    groups
}

fn group_header(group: &Group<'_>) -> String {
    if group.rank == Rank::RootCause {
        format!("{} ({}x, root cause?)", group.title, group.items.len())
    } else {
        format!("{} ({}x)", group.title, group.items.len())
    }
}

fn location_line(d: &Diag, group: &Group<'_>) -> String {
    // Breaking's file/package deletions carry no path; the file is in the message.
    let Some(path) = d.path.as_deref().filter(|p| !p.is_empty()) else {
        return d.message.clone();
    };
    if group.message.as_deref() == Some(d.message.as_str()) {
        format!("{path}:{}", d.start_line)
    } else {
        format!("{path}:{} {}", d.start_line, d.message)
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn render_diagnostics(
    sub: &str,
    parsed: &Parsed<'_>,
    max_groups: usize,
    max_locations: usize,
) -> String {
    let groups = group_diagnostics(&parsed.diags);
    let files: HashSet<&str> = parsed
        .diags
        .iter()
        .filter_map(|d| d.path.as_deref())
        .filter(|p| !p.is_empty())
        .collect();
    let mut out = format!(
        "buf {sub}: {} in {} ({})\n",
        plural(parsed.diags.len(), "issue"),
        plural(files.len(), "file"),
        plural(groups.len(), "group")
    );
    for group in groups.iter().take(max_groups) {
        out.push_str(&group_header(group));
        out.push('\n');
        for d in group.items.iter().take(max_locations) {
            out.push_str("  ");
            out.push_str(&location_line(d, group));
            out.push('\n');
        }
        if group.items.len() > max_locations {
            out.push_str(&format!(
                "  … +{} more\n",
                group.items.len() - max_locations
            ));
        }
    }
    if groups.len() > max_groups {
        out.push_str(&format!("… +{} more groups\n", groups.len() - max_groups));
    }
    if !parsed.unparsed.is_empty() {
        out.push_str("unparsed:\n");
        for line in &parsed.unparsed {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// Group buf's `--error-format=json` diagnostics by rule. Output without a single JSON line (an
/// older buf, or one that ignored the flag) is returned unchanged.
fn filter_buf_diagnostics(sub: &str, output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }
    let parsed = parse_diagnostics(output);
    if parsed.diags.is_empty() {
        return output.to_string();
    }
    render_diagnostics(sub, &parsed, MAX_GROUPS, MAX_LOCATIONS)
}

fn run_format_diff(bin: BufBin, args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = bin.command();
    cmd.args(args);
    if verbose > 1 {
        eprintln!("Running: {} {}", bin.tool_name(), args.join(" "));
    }
    runner::run_filtered_with_exit(
        cmd,
        bin.tool_name(),
        &args.join(" "),
        move |stdout, exit_code| {
            if verbose > 2 {
                eprintln!("{stdout}");
            }
            let filtered = filter_buf_format_diff(stdout);
            if verbose > 0 {
                eprintln!(
                    "rtk buf format: {} diff lines summarised",
                    stdout.lines().count()
                );
            }
            with_format_hint(stdout, filtered, exit_code, tee::force_tee_hint)
        },
        runner::RunOptions::stdout_only().tee(FORMAT_TEE_LABEL),
    )
}

/// The summary drops every hunk, so the full diff is made recallable — unless the
/// runner stores it anyway (non-zero exit with `--exit-code`), or the summary is no smaller and
/// `never_worse` will print the raw diff instead. `store` is `tee::force_tee_hint` outside tests.
fn with_format_hint(
    stdout: &str,
    filtered: String,
    exit_code: i32,
    store: impl FnOnce(&str, &str) -> Option<String>,
) -> String {
    if exit_code != 0 || filtered == stdout || never_worse(stdout, &filtered) == stdout {
        return filtered;
    }
    match store(stdout, FORMAT_TEE_LABEL) {
        Some(hint) => format!("{filtered}\n{hint}"),
        None => filtered,
    }
}

struct FileChange {
    path: String,
    added: usize,
    removed: usize,
}

fn parse_format_diff(diff: &str) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
    let mut headers_left = 0;
    for line in diff.lines() {
        if line.starts_with("diff ") {
            headers_left = 2;
            continue;
        }
        if headers_left > 0 {
            headers_left -= 1;
            if let Some(rest) = line.strip_prefix("+++ ") {
                let path = rest.split('\t').next().unwrap_or(rest);
                files.push(FileChange {
                    path: path.to_string(),
                    added: 0,
                    removed: 0,
                });
            }
            continue;
        }
        let Some(current) = files.last_mut() else {
            continue;
        };
        if line.starts_with('+') {
            current.added += 1;
        } else if line.starts_with('-') {
            current.removed += 1;
        }
    }
    files
}

fn filter_buf_format_diff(diff: &str) -> String {
    if diff.trim().is_empty() {
        return String::new();
    }
    let files = parse_format_diff(diff);
    if files.is_empty() {
        return diff.to_string();
    }
    let mut out = format!("buf format: {} would change\n", plural(files.len(), "file"));
    for f in files.iter().take(MAX_FORMAT_FILES) {
        out.push_str(&format!("  {} (+{} -{})\n", f.path, f.added, f.removed));
    }
    if files.len() > MAX_FORMAT_FILES {
        out.push_str(&format!(
            "  … +{} more files\n",
            files.len() - MAX_FORMAT_FILES
        ));
    }
    out.trim_end().to_string()
}

fn generate_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    input_takes_value(kind, name).or_else(|| match kind {
        TokenKind::Long => {
            matches!(name, "exclude-type" | "output" | "template" | "type").then(ValueSpec::value)
        }
        TokenKind::Short => (name == "o").then(ValueSpec::solo_only),
        _ => None,
    })
}

fn classify_generate(sub_args: &[String]) -> Invocation {
    let tokens = arg_tokenizer::tokenize_grammar(sub_args, &generate_takes_value, Dialect::Posix);
    let own = arg_tokenizer::before_dashdash(&tokens);
    // A user-chosen report format (junit, …) is theirs to read, as for diagnostics.
    if wants_help(own) || arg_tokenizer::has_flag(own, Dialect::Posix, "error-format") {
        Invocation::Passthrough
    } else {
        Invocation::Generate
    }
}

fn run_generate(bin: BufBin, args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = bin.command();
    cmd.args(args);
    if verbose > 1 {
        eprintln!("Running: {} {}", bin.tool_name(), args.join(" "));
    }
    runner::run_filtered_with_exit(
        cmd,
        bin.tool_name(),
        &args.join(" "),
        move |output, exit_code| {
            if verbose > 2 {
                eprintln!("{output}");
            }
            filter_buf_generate(output, exit_code)
        },
        // Combined: a failing plugin reports on stderr.
        runner::RunOptions::with_tee("buf-generate"),
    )
}

fn push_collapsed(lines: &mut Vec<(String, usize)>, line: &str) {
    match lines.last_mut() {
        Some((last, count)) if last == line => *count += 1,
        _ => lines.push((line.to_string(), 1)),
    }
}

/// Trim a failed `buf generate`: keep `Failure:`/plugin lines, collapse repeats, cut a plugin's Go
/// panic dump after its first frame. Success is left as is.
fn filter_buf_generate(output: &str, exit_code: i32) -> String {
    if exit_code == 0 {
        return output.to_string();
    }
    let mut lines: Vec<(String, usize)> = Vec::new();
    let mut in_stack = false;
    let mut kept_stack = 0;
    let mut omitted = 0;
    for line in output.lines() {
        if in_stack && GO_STACK_LINE.is_match(line) {
            if line.is_empty() {
                continue;
            }
            if kept_stack < KEPT_STACK_LINES {
                push_collapsed(&mut lines, line);
                kept_stack += 1;
            } else {
                omitted += 1;
            }
            continue;
        }
        if in_stack && omitted > 0 {
            push_collapsed(&mut lines, &format!("… ({omitted} stack lines omitted)"));
        }
        in_stack = line.starts_with("panic:");
        kept_stack = 0;
        omitted = 0;
        push_collapsed(&mut lines, line);
    }
    if in_stack && omitted > 0 {
        push_collapsed(&mut lines, &format!("… ({omitted} stack lines omitted)"));
    }
    let rendered: Vec<String> = lines
        .into_iter()
        .map(|(line, count)| {
            if count > 1 {
                format!("{line} (x{count})")
            } else {
                line
            }
        })
        .collect();
    if rendered.len() <= MAX_GENERATE_LINES {
        return rendered.join("\n");
    }
    // buf prints its `Failure:` verdict last, after any plugin chatter; the cap
    // must never hide it (or a plugin's `panic:` header).
    let (head, tail) = rendered.split_at(MAX_GENERATE_LINES);
    let verdicts: Vec<&String> = tail
        .iter()
        .filter(|line| is_generate_verdict(line))
        .collect();
    let mut out = head.to_vec();
    out.push(format!("… +{} more lines", tail.len() - verdicts.len()));
    out.extend(verdicts.into_iter().cloned());
    out.join("\n")
}

fn is_generate_verdict(line: &str) -> bool {
    line.starts_with("Failure:") || line.starts_with("panic:")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn assert_savings(name: &str, input: &str, output: &str) {
        let savings = 100.0 - (count_tokens(output) as f64 / count_tokens(input) as f64 * 100.0);
        eprintln!("{name}: {savings:.1}% bash output reduction");
        assert!(
            savings >= 60.0,
            "{name}: expected >=60% reduction, got {savings:.1}%"
        );
    }

    fn diag(path: &str, line: u32, kind: &str, message: &str) -> String {
        format!(
            r#"{{"path":"{path}","start_line":{line},"start_column":1,"end_line":{line},"end_column":2,"type":"{kind}","message":"{message}"}}"#
        )
    }

    #[test]
    fn classifies_diagnostics_subcommands() {
        assert_eq!(
            classify(&s(&["lint"])),
            Invocation::Diagnostics {
                sub: DiagSub::Lint,
                inject_at: 1
            }
        );
        assert_eq!(
            classify(&s(&["build", "-o", "out.binpb"])),
            Invocation::Diagnostics {
                sub: DiagSub::Build,
                inject_at: 3
            }
        );
        assert_eq!(
            classify(&s(&["breaking", "--against", ".git#ref=HEAD"])),
            Invocation::Diagnostics {
                sub: DiagSub::Breaking,
                inject_at: 3
            }
        );
    }

    #[test]
    fn global_flags_before_the_subcommand_are_skipped() {
        assert_eq!(
            classify(&s(&["--debug", "lint", "--path", "x"])),
            Invocation::Diagnostics {
                sub: DiagSub::Lint,
                inject_at: 4
            }
        );
        assert_eq!(
            classify(&s(&["--log-format", "json", "lint"])),
            Invocation::Diagnostics {
                sub: DiagSub::Lint,
                inject_at: 3
            }
        );
    }

    #[test]
    fn user_chosen_error_format_passes_through() {
        assert_eq!(
            classify(&s(&["lint", "--error-format", "json"])),
            Invocation::Passthrough
        );
        assert_eq!(
            classify(&s(&["lint", "--error-format=json"])),
            Invocation::Passthrough
        );
        assert_eq!(
            classify(&s(&[
                "breaking",
                "--error-format=github-actions",
                "--against",
                "x"
            ])),
            Invocation::Passthrough
        );
    }

    #[test]
    fn a_flag_value_is_not_mistaken_for_error_format() {
        // `--path` takes a value, so the next token is its value, not a flag.
        assert_eq!(
            classify(&s(&["lint", "--path", "--error-format"])),
            Invocation::Diagnostics {
                sub: DiagSub::Lint,
                inject_at: 3
            }
        );
    }

    #[test]
    fn flags_after_dashdash_are_not_buf_flags_and_injection_stays_before_it() {
        assert_eq!(
            classify(&s(&["lint", "--", "--error-format=json", "--help"])),
            Invocation::Diagnostics {
                sub: DiagSub::Lint,
                inject_at: 1
            }
        );
        assert_eq!(
            with_json_errors(&s(&["lint", "--", "x"]), 1),
            s(&["lint", "--error-format=json", "--", "x"])
        );
    }

    #[test]
    fn help_version_and_other_subcommands_pass_through() {
        for args in [
            &["lint", "--help"][..],
            &["lint", "-h"],
            &["--version"],
            &["dep", "update"],
            &["format", "-w"],
            &[],
        ] {
            assert_eq!(classify(&s(args)), Invocation::Passthrough, "{args:?}");
        }
    }

    #[test]
    fn help_before_the_subcommand_passes_through() {
        for args in [
            &["-h", "lint"][..],
            &["--help", "lint"],
            &["--debug", "--help", "format", "-d"],
            &["-h", "generate"],
        ] {
            assert_eq!(classify(&s(args)), Invocation::Passthrough, "{args:?}");
        }
    }

    #[test]
    fn not_json_warning_only_when_diagnostics_were_expected() {
        // Config error on stderr (build filters the combined stream): not a JSON problem.
        let config_error = "Failure: decode buf.yaml: unknown field\n";
        assert!(!warns_not_json(config_error, config_error, 1));
        let log = "WARN\tsomething deprecated\n";
        assert!(!warns_not_json(log, log, 0));
        // Violations reported (exit 100) but nothing parsed: buf ignored the flag.
        let text = "a.proto:1:1:imported file does not exist\n";
        assert!(warns_not_json(text, text, 100));
        let broken = "{\"path\":\"a.proto\"\n";
        assert!(warns_not_json(broken, broken, 1));
        assert!(!warns_not_json(text, "buf lint: 1 issue", 100));
    }

    fn never_store(_: &str, _: &str) -> Option<String> {
        panic!("nothing should be stored")
    }

    #[test]
    fn format_hint_stores_the_full_diff() {
        let big = include_str!("../../../tests/fixtures/buf_format_diff_raw.txt");
        let summary = filter_buf_format_diff(big);
        let stored = std::cell::RefCell::new(None);
        let out = with_format_hint(big, summary.clone(), 0, |content, slug| {
            stored.replace(Some((content.to_string(), slug.to_string())));
            Some("[full output: rtk recall abc]".to_string())
        });
        assert_eq!(out, format!("{summary}\n[full output: rtk recall abc]"));
        assert_eq!(
            stored.into_inner(),
            Some((big.to_string(), FORMAT_TEE_LABEL.to_string()))
        );
    }

    #[test]
    fn format_hint_skipped_when_it_would_not_be_shown_or_would_duplicate() {
        let tiny = "diff -u a b\n--- a\n+++ b\n+x\n";
        let filtered = filter_buf_format_diff(tiny);
        // never_worse will print the raw diff instead, so storing the diff is churn.
        assert_eq!(
            with_format_hint(tiny, filtered.clone(), 0, never_store),
            filtered
        );
        // Non-zero exit: the runner's tee owns recovery.
        let big = include_str!("../../../tests/fixtures/buf_format_diff_raw.txt");
        let summary = filter_buf_format_diff(big);
        assert_eq!(
            with_format_hint(big, summary.clone(), 1, never_store),
            summary
        );
    }

    #[test]
    fn build_writing_the_image_to_stdout_passes_through() {
        assert_eq!(classify(&s(&["build", "-o", "-"])), Invocation::Passthrough);
        assert_eq!(
            classify(&s(&["build", "--output=-#format=json"])),
            Invocation::Passthrough
        );
        for args in [
            &["build", "-o=-"][..],
            &["build", "-o", "/dev/stdout"],
            &["build", "--output=/dev/stdout"],
            &["build", "-o", "/dev/fd/1#format=json"],
        ] {
            assert_eq!(classify(&s(args)), Invocation::Passthrough, "{args:?}");
        }
    }

    #[test]
    fn groups_by_rule_with_compile_root_cause_first() {
        let input = [
            diag("a/v1/a.proto", 3, "COMPILE", "imported file does not exist"),
            diag("a/v1/a.proto", 5, "COMPILE", "cannot find `Foo` in this scope"),
            diag("a/v1/b.proto", 7, "COMPILE", "cannot find `Bar` in this scope"),
            diag(
                "a/v1/b.proto",
                9,
                "FIELD_LOWER_SNAKE_CASE",
                r#"Field name \"userId\" should be lower_snake_case, such as \"user_id\"."#,
            ),
            r#"{"start_line":1,"start_column":1,"end_line":1,"end_column":1,"type":"FILE_NO_DELETE","message":"Previously present file \"a/v1/c.proto\" was deleted."}"#.to_string(),
        ]
        .join("\n");

        assert_eq!(
            filter_buf_diagnostics("lint", &input),
            r#"buf lint: 5 issues in 2 files (4 groups)
COMPILE imported file does not exist (1x, root cause?)
  a/v1/a.proto:3
COMPILE cannot find `…` in this scope (2x)
  a/v1/a.proto:5 cannot find `Foo` in this scope
  a/v1/b.proto:7 cannot find `Bar` in this scope
FIELD_LOWER_SNAKE_CASE (1x)
  a/v1/b.proto:9 Field name "userId" should be lower_snake_case, such as "user_id".
FILE_NO_DELETE (1x)
  Previously present file "a/v1/c.proto" was deleted."#
        );
    }

    #[test]
    fn caps_locations_per_group() {
        let input: Vec<String> = (1..=7)
            .map(|i| diag("a.proto", i, "RPC_REQUEST_STANDARD_NAME", &format!("m{i}")))
            .collect();
        let out = filter_buf_diagnostics("lint", &input.join("\n"));
        assert_eq!(
            out,
            "buf lint: 7 issues in 1 file (1 group)\nRPC_REQUEST_STANDARD_NAME (7x)\n  a.proto:1 m1\n  a.proto:2 m2\n  a.proto:3 m3\n  a.proto:4 m4\n  a.proto:5 m5\n  … +2 more"
        );
    }

    #[test]
    fn caps_groups() {
        let input: Vec<String> = (1..=22)
            .map(|i| diag("a.proto", i, &format!("RULE_{i:02}"), "m"))
            .collect();
        let out = filter_buf_diagnostics("lint", &input.join("\n"));
        assert!(out.contains("RULE_20 (1x)"));
        assert!(!out.contains("RULE_21 (1x)"));
        assert!(out.ends_with("… +2 more groups"), "{out}");
    }

    #[test]
    fn unparsed_lines_are_kept_verbatim() {
        let input = format!("{}\nnot json at all", diag("a.proto", 1, "R", "m"));
        let out = filter_buf_diagnostics("lint", &input);
        assert!(out.ends_with("unparsed:\n  not json at all"), "{out}");
    }

    #[test]
    fn output_with_no_json_line_is_returned_unchanged() {
        let text = "a.proto:1:1:imported file does not exist\n";
        assert_eq!(filter_buf_diagnostics("lint", text), text);
    }

    #[test]
    fn empty_stdout_stays_empty() {
        // Config errors leave stdout empty; the runner forwards stderr whole on failure.
        assert_eq!(filter_buf_diagnostics("lint", ""), "");
        assert_eq!(filter_buf_diagnostics("lint", "\n\n"), "");
    }

    #[test]
    fn crlf_ndjson_parses() {
        let input = format!(
            "{}\r\n{}\r\n",
            diag("a.proto", 1, "R", "m"),
            diag("b.proto", 2, "R", "n")
        );
        assert_eq!(
            filter_buf_diagnostics("lint", &input),
            "buf lint: 2 issues in 2 files (1 group)\nR (2x)\n  a.proto:1 m\n  b.proto:2 n"
        );
    }

    #[test]
    fn format_diff_is_classified() {
        assert_eq!(classify(&s(&["format", "-d"])), Invocation::FormatDiff);
        assert_eq!(
            classify(&s(&["format", "--diff", "--exit-code"])),
            Invocation::FormatDiff
        );
        assert_eq!(classify(&s(&["format", "-dw"])), Invocation::FormatDiff);
        assert_eq!(
            classify(&s(&["format", "-d", "--help"])),
            Invocation::Passthrough
        );
        assert_eq!(
            classify(&s(&["format", "--path", "-d"])),
            Invocation::Passthrough
        );
    }

    #[test]
    fn format_diff_summarises_per_file() {
        let diff = "diff -u a.proto.orig a.proto\n--- a.proto.orig\t2026-09-24 13:50:04\n+++ a.proto\t2026-09-24 13:50:04\n@@ -1 +1,5 @@\n-syntax=\"proto3\";package x;message   A{int32 a=1;}\n+syntax = \"proto3\";\n+package x;\n+message A {\n+  int32 a = 1;\n+}\n";
        assert_eq!(
            filter_buf_format_diff(diff),
            "buf format: 1 file would change\n  a.proto (+5 -1)"
        );
    }

    #[test]
    fn format_diff_empty_and_unrecognised() {
        assert_eq!(filter_buf_format_diff(""), "");
        assert_eq!(
            filter_buf_format_diff("something else\n"),
            "something else\n"
        );
    }

    #[test]
    fn format_diff_fixture() {
        let input = include_str!("../../../tests/fixtures/buf_format_diff_raw.txt");
        let out = filter_buf_format_diff(input);
        assert!(
            out.starts_with("buf format: 25 files would change\n"),
            "{out}"
        );
        assert!(out.ends_with("  … +5 more files"), "{out}");
        assert_savings("buf format -d", input, &out);
    }

    #[test]
    fn generate_is_classified() {
        assert_eq!(classify(&s(&["generate"])), Invocation::Generate);
        assert_eq!(
            classify(&s(&["generate", "--template", "t.yaml"])),
            Invocation::Generate
        );
        assert_eq!(
            classify(&s(&["generate", "--help"])),
            Invocation::Passthrough
        );
        assert_eq!(
            classify(&s(&["generate", "--error-format=junit"])),
            Invocation::Passthrough
        );
        assert_eq!(
            classify(&s(&["generate", "--", "--error-format=junit"])),
            Invocation::Generate
        );
    }

    #[test]
    fn generate_success_is_unchanged() {
        assert_eq!(filter_buf_generate("", 0), "");
        assert_eq!(filter_buf_generate("note\n", 0), "note\n");
    }

    #[test]
    fn generate_trims_a_plugin_panic() {
        let raw = "panic: protoc-gen-panic: boom\n\ngoroutine 1 [running]:\nmain.deep(0x0)\n\t/tmp/p/main.go:5 +0x2c\nmain.deep(0x1)\n\t/tmp/p/main.go:7 +0x20\nmain.deep(0x2)\n\t/tmp/p/main.go:7 +0x20\nmain.main()\n\t/tmp/p/main.go:10 +0x1c\nFailure: plugin protoc-gen-panic: exit status 2\n";
        assert_eq!(
            filter_buf_generate(raw, 1),
            "panic: protoc-gen-panic: boom\ngoroutine 1 [running]:\nmain.deep(0x0)\n\t/tmp/p/main.go:5 +0x2c\n… (6 stack lines omitted)\nFailure: plugin protoc-gen-panic: exit status 2"
        );
    }

    #[test]
    fn generate_collapses_repeated_lines() {
        assert_eq!(
            filter_buf_generate("Failure: x\nFailure: x\nFailure: x\n", 1),
            "Failure: x (x3)"
        );
    }

    #[test]
    fn generate_keeps_the_failure_line_past_the_cap() {
        let mut raw: String = (1..=25)
            .map(|i| format!("WARNING: Missing 'go_package' option in \"a/v1/x{i}.proto\"\n"))
            .collect();
        raw.push_str("Failure: plugin protoc-gen-go: exit status 1\n");
        let out = filter_buf_generate(&raw, 1);
        assert!(
            out.ends_with("Failure: plugin protoc-gen-go: exit status 1"),
            "{out}"
        );
        assert!(out.contains("… +"), "{out}");
    }

    #[test]
    fn generate_panic_fixture() {
        let input = include_str!("../../../tests/fixtures/buf_generate_panic_raw.txt");
        let out = filter_buf_generate(input, 1);
        assert!(out.contains("panic: protoc-gen-panic: boom"), "{out}");
        assert!(out.contains("stack lines omitted)"), "{out}");
        assert_savings("buf generate", input, &out);
    }

    #[test]
    fn exit_zero_hint_only_when_something_was_capped() {
        let small = diag("a.proto", 1, "R", "m");
        let filtered = filter_buf_diagnostics("lint", &small);
        assert_eq!(
            with_exit_zero_hint(DiagSub::Lint, &small, filtered.clone(), 0, never_store),
            filtered
        );
        // Non-zero exit: the runner's tee owns recovery, the filter adds nothing.
        let many: Vec<String> = (1..=7).map(|i| diag("a.proto", i, "R", "m")).collect();
        let many = many.join("\n");
        let capped = filter_buf_diagnostics("lint", &many);
        assert_eq!(
            with_exit_zero_hint(DiagSub::Lint, &many, capped.clone(), 100, never_store),
            capped
        );
    }

    #[test]
    fn exit_zero_hint_stores_the_uncapped_listing() {
        let many: Vec<String> = (1..=7)
            .map(|i| diag("a.proto", i, "R", &format!("m{i}")))
            .collect();
        let many = many.join("\n");
        let capped = filter_buf_diagnostics("lint", &many);
        let stored = std::cell::RefCell::new(None);
        let out = with_exit_zero_hint(DiagSub::Lint, &many, capped.clone(), 0, |content, slug| {
            stored.replace(Some((content.to_string(), slug.to_string())));
            Some("[full output: rtk recall abc]".to_string())
        });
        assert_eq!(out, format!("{capped}\n[full output: rtk recall abc]"));
        let (content, slug) = stored.into_inner().expect("listing stored");
        assert_eq!(slug, "buf-lint");
        assert!(content.contains("  a.proto:7 m7"), "{content}");
        assert!(!content.contains("more"), "{content}");
    }

    #[test]
    fn lint_fixture() {
        let input = include_str!("../../../tests/fixtures/buf_lint_raw.jsonl");
        let out = filter_buf_diagnostics("lint", input);
        assert_eq!(
            out,
            r#"buf lint: 400 issues in 15 files (6 groups)
ENUM_VALUE_PREFIX (330x)
  google/cloud/talent/v4/common.proto:47 Enum value name "MINI" should be prefixed with "COMPANY_SIZE_".
  google/cloud/talent/v4/common.proto:50 Enum value name "SMALL" should be prefixed with "COMPANY_SIZE_".
  google/cloud/talent/v4/common.proto:53 Enum value name "SMEDIUM" should be prefixed with "COMPANY_SIZE_".
  google/cloud/talent/v4/common.proto:56 Enum value name "MEDIUM" should be prefixed with "COMPANY_SIZE_".
  google/cloud/talent/v4/common.proto:59 Enum value name "BIG" should be prefixed with "COMPANY_SIZE_".
  … +325 more
RPC_REQUEST_RESPONSE_UNIQUE (34x)
  google/cloud/talent/v4/company_service.proto:42 "google.cloud.talent.v4.Company" is used as the request or response type for multiple RPCs.
  google/cloud/talent/v4/company_service.proto:51 "google.cloud.talent.v4.Company" is used as the request or response type for multiple RPCs.
  google/cloud/talent/v4/company_service.proto:59 "google.cloud.talent.v4.Company" is used as the request or response type for multiple RPCs.
  google/cloud/talent/v4/company_service.proto:69 "google.protobuf.Empty" is used as the request or response type for multiple RPCs.
  google/cloud/talent/v4/job_service.proto:50 "google.cloud.talent.v4.Job" is used as the request or response type for multiple RPCs.
  … +29 more
RPC_RESPONSE_STANDARD_NAME (30x)
  google/cloud/talent/v4/company_service.proto:42 RPC response type "Company" should be named "CreateCompanyResponse" or "CompanyServiceCreateCompanyResponse".
  google/cloud/talent/v4/company_service.proto:51 RPC response type "Company" should be named "GetCompanyResponse" or "CompanyServiceGetCompanyResponse".
  google/cloud/talent/v4/company_service.proto:59 RPC response type "Company" should be named "UpdateCompanyResponse" or "CompanyServiceUpdateCompanyResponse".
  google/cloud/talent/v4/company_service.proto:69 RPC response type "Empty" should be named "DeleteCompanyResponse" or "CompanyServiceDeleteCompanyResponse".
  google/cloud/talent/v4/event_service.proto:45 RPC response type "ClientEvent" should be named "CreateClientEventResponse" or "EventServiceCreateClientEventResponse".
  … +25 more
COMMENT_ENUM (2x)
  google/cloud/talent/v4/common.proto:876 Enum "State" should have a non-empty comment for documentation.
  google/cloud/talent/v4beta1/common.proto:872 Enum "State" should have a non-empty comment for documentation.
RPC_REQUEST_STANDARD_NAME (2x)
  google/cloud/talent/v4/job_service.proto:166 RPC request type "SearchJobsRequest" should be named "SearchJobsForAlertRequest" or "JobServiceSearchJobsForAlertRequest".
  google/cloud/talent/v4beta1/job_service.proto:187 RPC request type "SearchJobsRequest" should be named "SearchJobsForAlertRequest" or "JobServiceSearchJobsForAlertRequest".
SERVICE_SUFFIX (2x)
  google/cloud/talent/v4/completion_service.proto:32 Service name "Completion" should be suffixed with "Service".
  google/cloud/talent/v4beta1/completion_service.proto:32 Service name "Completion" should be suffixed with "Service"."#
        );
        assert_savings("buf lint", input, &out);
    }

    #[test]
    fn breaking_fixture() {
        let input = include_str!("../../../tests/fixtures/buf_breaking_raw.jsonl");
        let out = filter_buf_diagnostics("breaking", input);
        assert!(
            out.contains(
                "FILE_NO_DELETE (1x)\n  Previously present file \"acme/v1/thing6.proto\" was deleted."
            ),
            "{out}"
        );
        assert!(out.contains("FIELD_SAME_TYPE ("), "{out}");
        assert_savings("buf breaking", input, &out);
    }

    #[test]
    fn build_compile_cascade_fixture() {
        let input = include_str!("../../../tests/fixtures/buf_build_compile_raw.jsonl");
        let out = filter_buf_diagnostics("build", input);
        assert_eq!(
            out,
            "buf build: 140 issues in 20 files (2 groups)
COMPILE imported file does not exist (20x, root cause?)
  acme/v1/broken1.proto:3
  acme/v1/broken10.proto:3
  acme/v1/broken11.proto:3
  acme/v1/broken12.proto:3
  acme/v1/broken13.proto:3
  … +15 more
COMPILE cannot find `…` in this scope (120x)
  acme/v1/broken1.proto:5 cannot find `Missing1` in this scope
  acme/v1/broken1.proto:6 cannot find `Missing2` in this scope
  acme/v1/broken1.proto:7 cannot find `Missing3` in this scope
  acme/v1/broken1.proto:8 cannot find `Missing4` in this scope
  acme/v1/broken1.proto:9 cannot find `Missing5` in this scope
  … +115 more"
        );
        assert_savings("buf build", input, &out);
    }
}
