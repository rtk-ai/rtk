//! Filters git output — log, status, diff, and more — keeping just the essential info.

use crate::core::args_utils;
use crate::core::guard::never_worse;
use crate::core::runner::{self, RunOptions};
use crate::core::stream::{
    self, exec_capture, exec_capture_stdin, CaptureResult, FilterMode, LineHandler,
    LineStreamFilter, StdinMode,
};
use crate::core::tracking;
use crate::core::truncate::{CAP_LIST, CAP_WARNINGS};
use crate::core::utils::{
    exit_code_from_status, join_with_overflow, resolved_command, strip_ansi,
};
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::process::Command;

#[derive(Debug, Clone)]
pub enum GitCommand {
    Diff,
    Log,
    Status,
    Show,
    Add,
    Commit,
    Checkout,
    Push,
    Pull,
    Branch,
    Fetch,
    Stash { subcommand: Option<String> },
    Worktree,
}

/// Create a git Command with global options (e.g. -C, -c, --git-dir, --work-tree)
/// prepended before any subcommand arguments.
fn git_cmd(global_args: &[String]) -> Command {
    let mut cmd = resolved_command("git");
    for arg in global_args {
        cmd.arg(arg);
    }
    cmd
}

/// Create a git Command for internal parsing that must be locale-stable.
///
/// We only use this for non-user-facing parses where RTK depends on git's
/// English status phrases. User-visible passthrough output keeps the user's
/// locale.
fn git_cmd_c_locale(global_args: &[String]) -> Command {
    let mut cmd = git_cmd(global_args);
    cmd.env("LC_ALL", "C");
    cmd
}

fn uses_compact_status_path(args: &[String]) -> bool {
    if args.is_empty() {
        return true;
    }

    let mut saw_branch = false;
    for arg in args {
        match arg.as_str() {
            "-b" | "--branch" => saw_branch = true,
            "-sb" | "-bs" => return true,
            "-s" | "--short" => {}
            _ => return false,
        }
    }

    saw_branch
}

fn build_status_command(args: &[String], global_args: &[String]) -> Command {
    let mut cmd = git_cmd(global_args);
    cmd.arg("status");
    if uses_compact_status_path(args) {
        cmd.args(["--porcelain", "-b"]);
    } else {
        cmd.args(args);
    }
    cmd
}

pub fn run(
    cmd: GitCommand,
    args: &[String],
    max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<i32> {
    match cmd {
        GitCommand::Diff => run_diff(args, max_lines, verbose, global_args),
        GitCommand::Log => run_log(args, max_lines, verbose, global_args),
        GitCommand::Status => run_status(args, verbose, global_args),
        GitCommand::Show => run_show(args, max_lines, verbose, global_args),
        GitCommand::Add => run_add(args, verbose, global_args),
        GitCommand::Commit => run_commit(args, verbose, global_args),
        GitCommand::Checkout => run_checkout(args, verbose, global_args),
        GitCommand::Push => run_push(args, verbose, global_args),
        GitCommand::Pull => run_pull(args, verbose, global_args),
        GitCommand::Branch => run_branch(args, verbose, global_args),
        GitCommand::Fetch => run_fetch(args, verbose, global_args),
        GitCommand::Stash { subcommand } => {
            run_stash(subcommand.as_deref(), args, verbose, global_args)
        }
        GitCommand::Worktree => run_worktree(args, verbose, global_args),
    }
}

fn run_diff(
    args: &[String],
    max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // Re-insert `--` when clap's trailing_var_arg consumed it (issue #1215)
    let args = &args_utils::restore_double_dash(args);

    // Check if user wants stat output
    let wants_stat = args
        .iter()
        .any(|arg| arg == "--stat" || arg == "--numstat" || arg == "--shortstat");

    // Check if user wants compact diff (default RTK behavior)
    let wants_compact = !args.iter().any(|arg| arg == "--no-compact") && !emits_word_diff(args);

    if wants_stat || !wants_compact {
        // User wants stat or explicitly no compacting - pass through directly
        let mut cmd = git_cmd(global_args);
        cmd.arg("diff");
        for arg in args {
            if arg == "--no-compact" {
                continue; // RTK flag, not a git flag
            }
            cmd.arg(arg);
        }

        let result = exec_capture(&mut cmd).context("Failed to run git diff")?;

        if !result.success() {
            eprintln!("{}", result.stderr);
            return Ok(result.exit_code);
        }

        println!("{}", result.stdout.trim());

        timer.track(
            &format!("git diff {}", args.join(" ")),
            &format!("rtk git diff {} (passthrough)", args.join(" ")),
            &result.stdout,
            &result.stdout,
        );

        return Ok(0);
    }

    // Default RTK behavior: stat first, then compacted diff
    let mut cmd = git_cmd(global_args);
    cmd.arg("diff").arg("--stat");

    for arg in args {
        cmd.arg(arg);
    }

    let result = exec_capture(&mut cmd).context("Failed to run git diff")?;

    if !result.success() {
        if !result.stderr.trim().is_empty() {
            eprint!("{}", result.stderr);
        }
        timer.track(
            &format!("git diff {}", args.join(" ")),
            &format!("rtk git diff {}", args.join(" ")),
            &result.stdout,
            &result.stdout,
        );
        return Ok(result.exit_code);
    }

    if verbose > 0 {
        eprintln!("Git diff summary:");
    }

    // Now get actual diff but compact it
    let mut diff_cmd = git_cmd(global_args);
    diff_cmd.arg("diff");
    for arg in args {
        diff_cmd.arg(arg);
    }

    let diff_result = exec_capture(&mut diff_cmd).context("Failed to run git diff")?;

    let printed = if !diff_result.stdout.is_empty() {
        let compacted = compact_diff(&diff_result.stdout, max_lines.unwrap_or(500));
        format!("{}\n\nChanges:\n{}", result.stdout.trim(), compacted)
    } else {
        result.stdout.trim().to_string()
    };

    let raw = format!("{}\n{}", result.stdout, diff_result.stdout);
    let shown = never_worse(&raw, &printed);
    println!("{}", shown);

    timer.track(
        &format!("git diff {}", args.join(" ")),
        &format!("rtk git diff {}", args.join(" ")),
        &raw,
        shown,
    );

    Ok(0)
}

fn run_show(
    args: &[String],
    max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // If user wants --stat or --format only, pass through
    let wants_stat_only = args
        .iter()
        .any(|arg| arg == "--stat" || arg == "--numstat" || arg == "--shortstat");

    let wants_format = args
        .iter()
        .any(|arg| arg.starts_with("--pretty") || arg.starts_with("--format"));

    // `git show rev:path` prints a blob, not a commit diff. In this mode we should
    // pass through directly to avoid duplicated output from compact-show steps.
    let wants_blob_show = args.iter().any(|arg| is_blob_show_arg(arg));

    if wants_stat_only || wants_format || wants_blob_show || emits_word_diff(args) {
        let mut cmd = git_cmd(global_args);
        cmd.arg("show");
        for arg in args {
            cmd.arg(arg);
        }
        let result = exec_capture(&mut cmd).context("Failed to run git show")?;
        if !result.success() {
            eprintln!("{}", result.stderr);
            return Ok(result.exit_code);
        }
        if wants_blob_show {
            print!("{}", result.stdout);
        } else {
            println!("{}", result.stdout.trim());
        }

        timer.track(
            &format!("git show {}", args.join(" ")),
            &format!("rtk git show {} (passthrough)", args.join(" ")),
            &result.stdout,
            &result.stdout,
        );

        return Ok(0);
    }

    // Get raw output for tracking
    let mut raw_cmd = git_cmd(global_args);
    raw_cmd.arg("show");
    for arg in args {
        raw_cmd.arg(arg);
    }
    let raw_output = exec_capture(&mut raw_cmd)
        .map(|r| r.stdout)
        .unwrap_or_default();

    // Step 1: one-line commit summary
    let mut summary_cmd = git_cmd(global_args);
    summary_cmd.args(["show", "--no-patch", "--pretty=format:%h %s (%ar) <%an>"]);
    for arg in args {
        summary_cmd.arg(arg);
    }
    let summary_result = exec_capture(&mut summary_cmd).context("Failed to run git show")?;
    if !summary_result.success() {
        eprintln!("{}", summary_result.stderr);
        return Ok(summary_result.exit_code);
    }
    let mut printed = summary_result.stdout.trim().to_string();

    // Step 2: --stat summary
    let mut stat_cmd = git_cmd(global_args);
    stat_cmd.args(["show", "--stat", "--pretty=format:"]);
    for arg in args {
        stat_cmd.arg(arg);
    }
    let stat_result = exec_capture(&mut stat_cmd).context("Failed to run git show --stat")?;
    let stat_text = stat_result.stdout.trim();
    if !stat_text.is_empty() {
        printed.push('\n');
        printed.push_str(stat_text);
    }

    // Step 3: compacted diff
    let mut diff_cmd = git_cmd(global_args);
    diff_cmd.args(["show", "--pretty=format:"]);
    for arg in args {
        diff_cmd.arg(arg);
    }
    let diff_result = exec_capture(&mut diff_cmd).context("Failed to run git show (diff)")?;
    let diff_text = diff_result.stdout.trim();

    if !diff_text.is_empty() {
        if verbose > 0 {
            printed.push_str("\n\nChanges:");
        }
        let compacted = compact_diff(diff_text, max_lines.unwrap_or(500));
        printed.push('\n');
        printed.push_str(&compacted);
    }

    let shown = never_worse(&raw_output, &printed);
    println!("{}", shown);

    timer.track(
        &format!("git show {}", args.join(" ")),
        &format!("rtk git show {}", args.join(" ")),
        &raw_output,
        shown,
    );

    Ok(0)
}

/// Whether these args make git emit a word diff rather than a line diff.
///
/// `compact_diff` reads a unified or combined diff: a body line's first column
/// (or columns) is a marker and the rest is content. A word diff drops the
/// marker entirely and puts `[-removed-]` / `{+added+}` inline, so its body
/// lines are arbitrary content in the marker position. A line starting with `+`
/// then counts as an addition, one starting with `\` is dropped as a
/// no-newline annotation, and one whose content happens to start `diff --`
/// opens a new file section. There is nothing to compact faithfully, so these
/// modes pass through.
///
/// `--word-diff=none` is the mode that turns a word diff back off, leaving an
/// ordinary unified diff to compact. Modes are last-one-wins, which is what
/// that mode is for: overriding an alias or an earlier flag on the same line.
fn emits_word_diff(args: &[String]) -> bool {
    let mut word_diff = false;
    for arg in args {
        if let Some(mode) = arg.strip_prefix("--word-diff=") {
            word_diff = mode != "none";
        } else if arg == "--word-diff"
            || arg.starts_with("--word-diff-regex")
            || arg == "--color-words"
            || arg.starts_with("--color-words=")
        {
            // `--color-words[=<regex>]` takes a regex rather than a mode, so
            // there is no `none` to honour on that spelling.
            word_diff = true;
        }
    }
    word_diff
}

fn is_blob_show_arg(arg: &str) -> bool {
    // Detect `rev:path` style arguments while ignoring flags like `--pretty=format:...`.
    !arg.starts_with('-') && arg.contains(':')
}

/// Path named by a diff section header.
///
/// `diff --git a/p b/p` carries the path twice; `diff --cc p` and
/// `diff --combined p` carry it once, as the whole remainder of the line. Only
/// the two-path form can be split at its midpoint, so the header kind decides
/// which shape to read: `diff --cc dup dup` names one file called `dup dup`,
/// not the file `dup` twice.
///
/// Under the default `core.quotepath`, git wraps a path in `"` and escapes any
/// non-ASCII byte, control character, quote or backslash inside it — but not a
/// space. The quoting is undone here, so the header carries the path as it is
/// on disk and a `grep` over the output finds it by name.
fn diff_header_path(line: &str) -> String {
    let Some(rest) = line.splitn(3, ' ').nth(2) else {
        return "unknown".to_string();
    };
    if !line.starts_with("diff --git ") {
        return unquote_path(rest);
    }
    if let Some(path) = same_path_twice(rest) {
        return path;
    }
    // A rename names two different paths, and the destination is the second.
    if let Some(quoted) = rest
        .split(" \"b/")
        .nth(1)
        .and_then(|dst| dst.strip_suffix('"'))
    {
        return unescape_path(quoted);
    }
    match rest.split(" b/").nth(1) {
        Some(path) => path.to_string(),
        None => unquote_path(rest),
    }
}

/// The path a `diff --git` header names twice, split at the midpoint.
///
/// Anything but a rename names the same path on both sides, so the two halves
/// are the same length and the separating space sits dead centre. Splitting
/// there instead of on the first ` b/` keeps a path that contains that
/// substring — a file under a directory named `x b`. Prefixes are then dropped
/// by matching the halves against each other rather than by name, so
/// `--no-prefix` and any custom `--src-prefix` / `--dst-prefix` read alike.
///
/// `None` for a rename, whose halves differ past their first component, and for
/// anything else the two halves disagree on; both fall through to the ` b/`
/// split. A `--no-prefix` rename between two directories is the one shape this
/// cannot tell from a prefix pair — space-separated paths with no prefix are
/// ambiguous by construction — and it reads as the shared trailing path.
fn same_path_twice(rest: &str) -> Option<String> {
    if rest.len().is_multiple_of(2) {
        return None;
    }
    let mid = rest.len() / 2;
    // A space at the midpoint is a char boundary, so both halves are valid.
    if rest.as_bytes().get(mid) != Some(&b' ') {
        return None;
    }
    let (left, right) = (unquote_path(&rest[..mid]), unquote_path(&rest[mid + 1..]));
    if left == right {
        return Some(left);
    }
    let (_, left_path) = left.split_once('/')?;
    let (_, right_path) = right.split_once('/')?;
    (left_path == right_path).then(|| right_path.to_string())
}

/// Undo git's `core.quotepath` quoting: `"a/\303\251.txt"` becomes `a/é.txt`.
///
/// A path git did not quote is returned as-is, so either form can be passed.
fn unquote_path(raw: &str) -> String {
    match raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(quoted) => unescape_path(quoted),
        None => raw.to_string(),
    }
}

/// Decode the C escapes inside a quoted path.
///
/// The octal escapes spell out the path's bytes one at a time, so a multi-byte
/// character arrives as several of them; they are collected as bytes and
/// decoded once at the end rather than per escape. A path whose bytes are not
/// UTF-8 keeps replacement characters, which is as close as a `String` gets.
fn unescape_path(quoted: &str) -> String {
    let bytes = quoted.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 == bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let escape = bytes[i + 1];
        if escape.is_ascii_digit() {
            let end = (i + 4).min(bytes.len());
            let octal = std::str::from_utf8(&bytes[i + 1..end])
                .ok()
                .and_then(|digits| u8::from_str_radix(digits, 8).ok());
            match octal {
                Some(byte) => {
                    out.push(byte);
                    i = end;
                }
                // Not an octal escape after all: keep the backslash verbatim.
                None => {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            continue;
        }
        out.push(match escape {
            b'a' => 0x07,
            b'b' => 0x08,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'r' => b'\r',
            // `\"` and `\\` stand for themselves.
            other => other,
        });
        i += 2;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Line budget a hunk header declares, and how wide its body prefix is.
struct HunkHeader {
    /// Lines the hunk spans in each parent, in marker-column order. One entry
    /// for a unified `@@`, one per parent for a combined `@@@`.
    parents: Vec<usize>,
    /// Lines the hunk spans in the result file.
    new: usize,
    /// Marker columns: 1 for `@@`, and one per parent for a combined `@@@`.
    prefix_width: usize,
}

impl HunkHeader {
    /// Whether every declared line has been accounted for, which is where the
    /// hunk body ends. A combined hunk is not done until *every* parent's
    /// budget is spent: a line removed from only the second parent spends that
    /// parent's budget and neither the first's nor the result's.
    fn exhausted(&self) -> bool {
        self.new == 0 && self.parents.iter().all(|&remaining| remaining == 0)
    }

    /// Charge a body line against the budgets it occupies.
    ///
    /// Column `i` is the line's marker against parent `i + 1`. `-` there means
    /// the line is in that parent and is being removed; a space on a line that
    /// is not a removal means the line is in that parent unchanged. Both spend
    /// one of that parent's lines. `+` there, or a space on a removal line,
    /// means the line is not in that parent at all.
    ///
    /// Collapsing the columns into one add/delete pair, as an aggregate over
    /// the whole prefix does, loses that distinction and leaves a combined
    /// hunk's budget unable to converge.
    fn consume(&mut self, markers: &[u8]) {
        let is_add = markers.contains(&b'+');
        let is_del = markers.contains(&b'-');
        for (i, remaining) in self.parents.iter_mut().enumerate() {
            let column = markers.get(i).copied();
            let present = if is_del {
                column == Some(b'-')
            } else {
                // A line shorter than the prefix reads as context, which is
                // what a bare blank line in a unified diff body is.
                column != Some(b'+')
            };
            if present {
                *remaining = remaining.saturating_sub(1);
            }
        }
        // In the result file unless the line is a pure deletion.
        if is_add || !is_del {
            self.new = self.new.saturating_sub(1);
        }
    }
}

/// Parse `@@ -a,b +c,d @@` and the combined `@@@ -a,b -c,d +e,f @@@`.
///
/// The counts bound the hunk body, which is what lets the body end where the
/// hunk ends rather than running on until the next header. Anything after it —
/// an mbox envelope, a `--` signature, trailing prose — is then outside every
/// hunk and cannot be read as diff content. A count is 1 when the header omits
/// it (`@@ -1 +1 @@`).
fn parse_hunk_header(line: &str) -> Option<HunkHeader> {
    let at_run = line.len() - line.trim_start_matches('@').len();
    if at_run < 2 {
        return None;
    }
    let body = line[at_run..].split('@').next()?;

    let mut parents: Vec<usize> = Vec::new();
    let mut new = None;
    for group in body.split_whitespace() {
        let Some(rest) = group.strip_prefix(['-', '+']) else {
            continue;
        };
        let count = match rest.split_once(',') {
            Some((_, c)) => c.parse::<usize>().ok()?,
            None => 1,
        };
        if group.starts_with('-') {
            // A combined header lists one range per parent, in the same order
            // as the marker columns.
            parents.push(count);
        } else {
            new = Some(count);
        }
    }

    // `@@` has one marker column, `@@@` two, and so on for more parents.
    let prefix_width = at_run - 1;
    // A well-formed header lists exactly one range per marker column. When it
    // does not, only the columns can be charged, so trust them: an untracked
    // parent would otherwise sit at its declared count forever and the hunk
    // would never close, while a parent with no column of its own would be
    // charged against nothing. A missing range gets `usize::MAX`, which keeps
    // the hunk open to the next header rather than dropping its body.
    if parents.len() != prefix_width {
        parents.resize(prefix_width, usize::MAX);
    }

    Some(HunkHeader {
        parents,
        new: new.unwrap_or(0),
        prefix_width,
    })
}

/// Render the note for change lines dropped past `max_hunk_lines`, split by
/// sign so an anchored `^-` / `^+` audit can tell what it did not see.
fn hunk_truncation_note(deletions: usize, additions: usize) -> Option<String> {
    fn count(n: usize, noun: &str) -> String {
        if n == 1 {
            format!("{} {}", n, noun)
        } else {
            format!("{} {}s", n, noun)
        }
    }
    match (deletions, additions) {
        (0, 0) => None,
        (0, a) => Some(format!("  ... ({} truncated)", count(a, "addition"))),
        (d, 0) => Some(format!("  ... ({} truncated)", count(d, "deletion"))),
        (d, a) => Some(format!(
            "  ... ({}, {} truncated)",
            count(d, "deletion"),
            count(a, "addition")
        )),
    }
}

/// Emit the buffered leading context, charged against the diff-wide budget.
///
/// Keeps the lines closest to the change when the budget cannot take all of
/// them. Called wherever a hunk closes as well as at its first change line:
/// context buffered by a hunk that ends without one would otherwise be dropped,
/// leaving a bare hunk header with nothing under it.
fn flush_leading_context(
    buffer: &mut Vec<String>,
    result: &mut Vec<String>,
    total: &mut usize,
    cap: usize,
) {
    let room = cap.saturating_sub(*total);
    let keep = buffer.len().min(room);
    let skip = buffer.len() - keep;
    for ctx in buffer.drain(..).skip(skip) {
        result.push(ctx);
    }
    *total += keep;
}

pub(crate) fn compact_diff(diff: &str, max_lines: usize) -> String {
    let mut result = Vec::new();
    let mut current_file = String::new();
    let mut added = 0;
    let mut removed = 0;
    let mut hunk: Option<HunkHeader> = None;
    let mut hunk_shown = 0;
    let mut skipped_add = 0usize;
    let mut skipped_del = 0usize;
    let mut leading_context: Vec<String> = Vec::new();
    let mut leading_context_total = 0usize;
    let max_hunk_lines = 100;
    // Context before a hunk's first change, up to three lines per hunk and
    // `max_lines / 10` across the diff. It does not count against `max_lines`,
    // so it cannot displace change lines, and the diff-wide cap is what bounds
    // the overrun that exemption would otherwise allow: a diff of many small
    // hunks would otherwise spend three exempt lines on every one of them.
    let max_leading_context = 3;
    let leading_context_cap = max_lines / 10;
    let mut was_truncated = false;

    for line in diff.lines() {
        // Every diff section header (`--git`, `--cc`, `--combined`) opens a new
        // file and closes any open hunk, so the `---` / `+++` headers that
        // follow it are never read as hunk content.
        if line.starts_with("diff --") {
            flush_leading_context(
                &mut leading_context,
                &mut result,
                &mut leading_context_total,
                leading_context_cap,
            );
            if let Some(note) = hunk_truncation_note(skipped_del, skipped_add) {
                result.push(note);
                was_truncated = true;
                skipped_del = 0;
                skipped_add = 0;
            }
            if !current_file.is_empty() && (added > 0 || removed > 0) {
                result.push(format!("  +{} -{}", added, removed));
            }
            current_file = diff_header_path(line);
            result.push(format!("\n{}", current_file));
            added = 0;
            removed = 0;
            hunk = None;
            hunk_shown = 0;
        } else if let Some(header) = parse_hunk_header(line) {
            flush_leading_context(
                &mut leading_context,
                &mut result,
                &mut leading_context_total,
                leading_context_cap,
            );
            if let Some(note) = hunk_truncation_note(skipped_del, skipped_add) {
                result.push(note);
                was_truncated = true;
                skipped_del = 0;
                skipped_add = 0;
            }
            hunk = Some(header);
            hunk_shown = 0;
            // Preserve the full unified diff hunk header, including trailing
            // function / symbol context after the second @@ marker.
            result.push(line.to_string());
        } else if let Some(header) = hunk.as_mut() {
            if header.exhausted() {
                hunk = None;
                continue;
            }
            if line.starts_with('\\') {
                // "\ No newline at end of file" annotates the line above and
                // occupies no line in either file.
                continue;
            }

            // Slice the marker columns as bytes. `prefix_width` counts columns,
            // and the markers are ASCII by construction, but the body content
            // right after them is not: `--word-diff` emits body lines with no
            // marker column at all, so a `char`-unaware `&line[..width]` splits
            // a leading multi-byte character and panics.
            let width = header.prefix_width.min(line.len());
            let markers = &line.as_bytes()[..width];
            let is_add = markers.contains(&b'+');
            let is_del = markers.contains(&b'-');
            header.consume(markers);

            // Hunk bodies emit at column 0 in git's own unified shape, so
            // `^+` / `^-` anchor. rtk's own annotations stay indented so those
            // same anchors never match them. Inside a hunk every `+`/`-` line
            // is content: the `---` / `+++` file headers only ever appear
            // before the first hunk header.
            if is_add || is_del {
                if is_add {
                    added += 1;
                }
                if is_del {
                    removed += 1;
                }
                if hunk_shown < max_hunk_lines {
                    // The context immediately preceding the change, so the body
                    // reads as contiguous with it. The diff-wide budget is
                    // charged on emit rather than on buffering, so a line the
                    // ring evicted never costs anything.
                    flush_leading_context(
                        &mut leading_context,
                        &mut result,
                        &mut leading_context_total,
                        leading_context_cap,
                    );
                    result.push(line.to_string());
                    hunk_shown += 1;
                } else if is_del {
                    skipped_del += 1;
                } else {
                    skipped_add += 1;
                }
                leading_context.clear();
            } else if hunk_shown > 0 {
                if hunk_shown < max_hunk_lines {
                    result.push(line.to_string());
                    hunk_shown += 1;
                }
            } else if leading_context_total < leading_context_cap {
                // Keep the last `max_leading_context` lines rather than the
                // first: with `-U10` or `--function-context` the first ones sit
                // ten lines above the change and would imply an adjacency the
                // file does not have.
                if leading_context.len() == max_leading_context {
                    leading_context.remove(0);
                }
                leading_context.push(line.to_string());
            }

            if header.exhausted() {
                hunk = None;
                flush_leading_context(
                    &mut leading_context,
                    &mut result,
                    &mut leading_context_total,
                    leading_context_cap,
                );
            }
        }

        if result.len().saturating_sub(leading_context_total) >= max_lines {
            result.push("\n... (more changes truncated)".to_string());
            was_truncated = true;
            break;
        }
    }

    // Flush last hunk
    flush_leading_context(
        &mut leading_context,
        &mut result,
        &mut leading_context_total,
        leading_context_cap,
    );
    if let Some(note) = hunk_truncation_note(skipped_del, skipped_add) {
        result.push(note);
        was_truncated = true;
    }

    if !current_file.is_empty() && (added > 0 || removed > 0) {
        result.push(format!("  +{} -{}", added, removed));
    }

    if was_truncated {
        result.push("[full diff: rtk git diff --no-compact]".to_string());
    }

    result.join("\n")
}

fn run_log(
    args: &[String],
    _max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<i32> {
    // Re-insert `--` when clap's trailing_var_arg consumed it (issue #1215):
    // without this, `rtk git log -- -p` loses its literal "--" and
    // `requests_raw_log_output`/`log_arg_tokens` can no longer tell that
    // `-p` is a pathspec, not the real patch flag.
    let args = &args_utils::restore_double_dash(args);

    if requests_raw_log_output(args) {
        let passthrough_args: Vec<OsString> = std::iter::once(OsString::from("log"))
            .chain(args.iter().map(OsString::from))
            .collect();
        return run_passthrough(&passthrough_args, global_args, verbose);
    }

    let timer = tracking::TimedExecution::start();

    let mut cmd = git_cmd(global_args);
    cmd.arg("log");

    // Tokenize once and share it: flag-vs-value classification is reused
    // below by both the flag-presence checks and the limit parsing, and a
    // value belonging to --grep/--author/etc. (e.g. `--grep --pretty`) must
    // not be misread as one of the flags below.
    let tokens = log_arg_tokens(args);
    let flag_args = flag_args_from_tokens(&tokens);

    // Check if user provided format flags
    let has_format_flag = flag_args.iter().any(|arg| {
        arg.starts_with("--oneline") || arg.starts_with("--pretty") || arg.starts_with("--format")
    });

    // Check if user provided limit flag (-N, -n N, --max-count=N, --max-count N)
    let has_limit_flag = flag_args.iter().any(|arg| {
        (arg.starts_with('-') && arg.chars().nth(1).is_some_and(|c| c.is_ascii_digit()))
            || *arg == "-n"
            || arg.starts_with("--max-count")
    });

    // Apply RTK defaults only if user didn't specify them
    // Use %b (body) to preserve first line of commit body for agent context
    // (BREAKING CHANGE, Closes #xxx, design notes)
    if !has_format_flag {
        cmd.args(["--pretty=format:%h %s (%ar) <%an>%n%b%n---END---"]);
    }

    // Determine limit: respect user's explicit -N flag, use sensible defaults otherwise
    let (limit, user_set_limit) = if has_limit_flag {
        // User explicitly passed -N / -n N / --max-count=N → respect their choice
        let n = parse_limit_from_tokens(&tokens).unwrap_or(10);
        (n, true)
    } else if has_format_flag {
        // --oneline / --pretty without -N: user wants compact output, allow more
        cmd.arg("-50");
        (50, false)
    } else {
        // No flags at all: default to 10
        cmd.arg("-10");
        (10, false)
    };

    // Only add --no-merges if user didn't explicitly request merge commits
    let wants_merges = flag_args
        .iter()
        .any(|arg| *arg == "--merges" || *arg == "--min-parents=2" || *arg == "--no-merges");
    // Don't add --no-merges if user explicitly requested merges or an exact count (-n N / --max-count)
    if !wants_merges && !has_limit_flag {
        cmd.arg("--no-merges");
    }

    // Pass all user arguments
    for arg in args {
        cmd.arg(arg);
    }

    let result = exec_capture(&mut cmd).context("Failed to run git log")?;

    if !result.success() {
        eprintln!("{}", result.stderr);
        return Ok(result.exit_code);
    }

    if verbose > 0 {
        eprintln!("Git log output:");
    }

    // Post-process: truncate long messages, cap lines only if RTK set the default
    let filtered = filter_log_output(&result.stdout, limit, user_set_limit, has_format_flag);
    let filtered = never_worse(&result.stdout, &filtered).to_string();
    println!("{}", filtered);

    timer.track(
        &format!("git log {}", args.join(" ")),
        &format!("rtk git log {}", args.join(" ")),
        &result.stdout,
        &filtered,
    );

    Ok(0)
}

/// True for git log/diff options that take their value as a separate,
/// space-delimited token (e.g. `--grep -p` searches messages for the
/// literal string "-p"; it does not request patch output). Consuming
/// that value token keeps flag-lookalike values from being misread as
/// the corresponding boolean flag.
fn consumes_next_token_as_value(arg: &str) -> bool {
    matches!(
        arg,
        "--after"
            | "--anchored"
            | "--author"
            | "--before"
            | "--color-moved-ws"
            | "--committer"
            | "--date"
            | "--decorate-refs"
            | "--decorate-refs-exclude"
            | "--diff-algorithm"
            | "--diff-filter"
            | "--diff-merges"
            | "--dst-prefix"
            | "--encoding"
            | "--exclude"
            | "--find-object"
            | "--glob"
            | "--grep"
            | "--grep-reflog"
            | "--inter-hunk-context"
            | "--line-prefix"
            | "--max-depth"
            | "--output"
            | "--output-indicator-context"
            | "--output-indicator-new"
            | "--output-indicator-old"
            | "--rotate-to"
            | "--since"
            | "--since-as-filter"
            | "--skip"
            | "--skip-to"
            | "--src-prefix"
            | "--stat-count"
            | "--stat-name-width"
            | "--stat-width"
            | "--until"
            | "--word-diff-regex"
            | "--ws-error-highlight"
            | "-G"
            | "-I"
            | "-L"
            | "-O"
            | "-S"
            | "-l"
            | "-n"
    )
}

/// A git log argument, classified as either a flag or the value consumed
/// by the preceding flag.
enum LogArg<'a> {
    Flag(&'a str),
    Value { flag: &'a str, value: &'a str },
}

/// Tokenizes git log `args` into [`LogArg`]s, stopping at the `--` pathspec
/// separator (tokens after it are paths, never flags or their values —
/// e.g. `git log -- -5` means "history for the path literally named -5").
/// `-n`/`--max-count`'s own count and every option in
/// [`consumes_next_token_as_value`] are paired with the flag that consumes
/// them. Shared by every git-log flag/value/limit check in [`run_log`] so
/// `--`-handling and option-value handling live in one place instead of
/// being reimplemented per check.
fn log_arg_tokens(args: &[String]) -> Vec<LogArg<'_>> {
    let mut tokens = Vec::with_capacity(args.len());
    let mut iter = args.iter().take_while(|arg| *arg != "--");
    while let Some(arg) = iter.next() {
        let arg_str = arg.as_str();
        if arg_str == "--max-count" || consumes_next_token_as_value(arg_str) {
            if let Some(value) = iter.next() {
                tokens.push(LogArg::Value {
                    flag: arg_str,
                    value: value.as_str(),
                });
                continue;
            }
        }
        tokens.push(LogArg::Flag(arg_str));
    }
    tokens
}

/// Filters `tokens` down to the flags themselves, dropping every value
/// consumed by the preceding option.
fn flag_args_from_tokens<'a>(tokens: &[LogArg<'a>]) -> Vec<&'a str> {
    tokens
        .iter()
        .map(|token| match token {
            LogArg::Flag(flag) | LogArg::Value { flag, .. } => *flag,
        })
        .collect()
}

/// Filters `args` down to the tokens that are actual flags, dropping every
/// token consumed as a value by the preceding option. `run_log` shares a
/// single tokenization via [`flag_args_from_tokens`] instead; this
/// convenience wrapper exists for tests that only care about the flags.
#[cfg(test)]
fn real_flag_args(args: &[String]) -> Vec<&str> {
    flag_args_from_tokens(&log_arg_tokens(args))
}

/// True for git log/diff flags that change the *shape* of git's raw output
/// (patch text, diffstat, name lists) in a way RTK's injected
/// `--pretty=format` + `---END---` markers can't coexist with — matching
/// this must request the untouched passthrough path instead of RTK's
/// filtered one (see [`requests_raw_log_output`]).
fn requests_raw_diff_shape(flag: &str) -> bool {
    matches!(
        flag,
        "-p" | "-u"
            | "--dirstat"
            | "--name-only"
            | "--name-status"
            | "--numstat"
            | "--patch"
            | "--patch-with-raw"
            | "--patch-with-stat"
            | "--raw"
            | "--shortstat"
            | "--stat"
            | "--summary"
    ) || flag.starts_with("--stat=")
        || flag.starts_with("--dirstat=")
}

fn requests_raw_log_output(args: &[String]) -> bool {
    log_arg_tokens(args)
        .iter()
        .any(|token| matches!(token, LogArg::Flag(flag) if requests_raw_diff_shape(flag)))
}

/// Parse the user-specified limit from git log args.
/// Handles: -20, -n 20, --max-count=20, --max-count 20
/// `run_log` shares a single tokenization via [`parse_limit_from_tokens`]
/// instead; this convenience wrapper exists for tests.
#[cfg(test)]
fn parse_user_limit(args: &[String]) -> Option<usize> {
    parse_limit_from_tokens(&log_arg_tokens(args))
}

fn parse_limit_from_tokens(tokens: &[LogArg<'_>]) -> Option<usize> {
    for token in tokens {
        match token {
            // -20 (combined digit form)
            LogArg::Flag(flag)
                if flag.starts_with('-')
                    && flag.len() > 1
                    && flag.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) =>
            {
                if let Ok(n) = flag[1..].parse::<usize>() {
                    return Some(n);
                }
            }
            // -n 20 / --max-count 20 (two-token form)
            LogArg::Value {
                flag: "-n" | "--max-count",
                value,
            } => {
                if let Ok(n) = value.parse::<usize>() {
                    return Some(n);
                }
            }
            // --max-count=20
            LogArg::Flag(flag) => {
                if let Some(rest) = flag.strip_prefix("--max-count=") {
                    if let Ok(n) = rest.parse::<usize>() {
                        return Some(n);
                    }
                }
            }
            LogArg::Value { .. } => {}
        }
    }
    None
}

/// When `user_set_limit` is true, the user explicitly passed `-N` to git log,
/// so we skip line capping (git already returns exactly N commits) and use a
/// wider truncation threshold (120 chars) to preserve commit context that LLMs
/// need for rebase/squash operations.
pub(crate) fn filter_log_output(
    output: &str,
    limit: usize,
    user_set_limit: bool,
    user_format: bool,
) -> String {
    let truncate_width = if user_set_limit { 120 } else { 80 };

    // When user specified their own format (--oneline, --pretty, --format),
    // RTK did not inject ---END--- markers. Use simple line-based truncation.
    if user_format {
        let lines: Vec<&str> = output.lines().collect();
        let max_lines = if user_set_limit { lines.len() } else { limit };
        return lines
            .iter()
            .take(max_lines)
            .map(|l| truncate_line(l, truncate_width))
            .collect::<Vec<_>>()
            .join("\n");
    }

    // RTK injected format: split output into commit blocks separated by ---END---
    let commits: Vec<&str> = output.split("---END---").collect();
    let max_commits = if user_set_limit { commits.len() } else { limit };

    let mut result = Vec::new();
    for block in commits.iter().take(max_commits) {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        let mut lines = block.lines();
        // First line is the header: hash subject (date) <author>
        let header = match lines.next() {
            Some(h) => truncate_line(h.trim(), truncate_width),
            None => continue,
        };
        // Remaining lines are the body — keep up to 3 non-empty, non-trailer lines
        let all_body_lines: Vec<&str> = lines
            .map(|l| l.trim())
            .filter(|l| {
                !l.is_empty()
                    && !l.starts_with("Signed-off-by:")
                    && !l.starts_with("Co-authored-by:")
            })
            .collect();
        let body_omitted = all_body_lines.len().saturating_sub(3);
        let body_lines = &all_body_lines[..all_body_lines.len().min(3)];

        if body_lines.is_empty() {
            result.push(header);
        } else {
            let mut entry = header;
            for body in body_lines {
                entry.push_str(&format!("\n  {}", truncate_line(body, truncate_width)));
            }
            if body_omitted > 0 {
                entry.push_str(&format!("\n  [+{} lines omitted]", body_omitted));
            }
            result.push(entry);
        }
    }

    result.join("\n").trim().to_string()
}

/// Truncate a single line to `width` characters, appending "..." if needed
fn truncate_line(line: &str, width: usize) -> String {
    if line.chars().count() > width {
        let truncated: String = line.chars().take(width - 3).collect();
        format!("{}...", truncated)
    } else {
        line.to_string()
    }
}

pub(crate) fn format_status_output(porcelain: &str) -> String {
    format_status_inner(porcelain, None)
}

pub(crate) fn format_status_output_detached(porcelain: &str, detached_ref: &str) -> String {
    format_status_inner(porcelain, Some(detached_ref))
}

fn format_status_inner(porcelain: &str, detached: Option<&str>) -> String {
    let lines: Vec<&str> = porcelain
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();

    if lines.is_empty() {
        return "Clean working tree".to_string();
    }

    let mut output = Vec::new();

    if let Some(branch_line) = lines.first() {
        if branch_line.starts_with("##") {
            let branch = branch_line.trim_start_matches("## ");
            let display = detached.unwrap_or(branch);
            output.push(format!("* {}", display));
        } else {
            output.push((*branch_line).to_string());
        }
    }

    for line in lines.iter().skip(1) {
        output.push((*line).to_string());
    }

    if lines.len() == 1 && lines[0].starts_with("##") {
        output.push("clean — nothing to commit".to_string());
    }

    output.join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitStatusState {
    Rebase,
    MergeConflicts,
    MergeReadyToCommit,
    CherryPick,
    Revert,
    Bisect,
    Am,
    SparseCheckout,
}

impl GitStatusState {
    fn summary(self) -> &'static str {
        match self {
            Self::Rebase => "rebase in progress",
            Self::MergeConflicts => "merge in progress. unresolved conflicts",
            Self::MergeReadyToCommit => "merge in progress. no conflicts",
            Self::CherryPick => "cherry-pick in progress",
            Self::Revert => "revert in progress",
            Self::Bisect => "bisect in progress",
            Self::Am => "am session in progress",
            Self::SparseCheckout => "sparse checkout enabled",
        }
    }
}

const REBASE_INDICATORS: &[&str] = &[
    "rebase in progress",
    "You are currently rebasing",
    "You are currently editing",
    "You are currently splitting",
    "Last command done",
    "Next command to do",
    "No commands remaining",
];

fn detect_status_state(line: &str) -> Option<GitStatusState> {
    if line.contains("All conflicts fixed but you are still merging") {
        Some(GitStatusState::MergeReadyToCommit)
    } else if line.contains("You have unmerged paths") {
        Some(GitStatusState::MergeConflicts)
    } else if line.contains("You are currently cherry-picking") {
        Some(GitStatusState::CherryPick)
    } else if line.contains("You are currently reverting") {
        Some(GitStatusState::Revert)
    } else if line.contains("You are currently bisecting") {
        Some(GitStatusState::Bisect)
    } else if line.contains("You are in the middle of an am session") {
        Some(GitStatusState::Am)
    } else if line.contains("You are in a sparse checkout") {
        Some(GitStatusState::SparseCheckout)
    } else if REBASE_INDICATORS.iter().any(|i| line.contains(i)) {
        Some(GitStatusState::Rebase)
    } else {
        None
    }
}

/// Extract a compact in-progress state summary from plain `git status` output.
///
/// Compact mode runs `git status --porcelain -b`, which omits the state header
/// git prints for rebase / merge / cherry-pick / revert / bisect / am / sparse
/// checkout. Hiding that block is a correctness bug — e.g. during an interactive
/// rebase edit, the user sees a "clean" status and misses "You are currently
/// editing a commit while rebasing ...".
///
/// This helper walks the plain-status output we already capture for tracking
/// and emits a compact, RTK-style summary rather than dumping git's full prose.
/// Returns `None` when no state is in progress.
fn extract_state_header(raw: &str) -> Option<String> {
    // Headers of the file-change blocks — everything relevant to state appears
    // above these in git's output, so they double as a terminator.
    const STOPPERS: &[&str] = &[
        "Changes to be committed:",
        "Changes not staged for commit:",
        "Untracked files:",
        "Unmerged paths:",
        "no changes added to commit",
        "nothing to commit",
        "nothing added to commit",
    ];

    for line in raw.lines() {
        let stripped = line.trim();

        if STOPPERS.iter().any(|s| stripped.starts_with(s)) {
            break;
        }

        if let Some(state) = detect_status_state(stripped) {
            return Some(state.summary().to_string());
        }
    }

    None
}

/// Extract the explicit "HEAD detached at/from <ref>" line from plain
/// `git status` output.
///
/// Porcelain `-b` collapses a detached HEAD to the opaque `## HEAD (no branch)`,
/// which an agent (or a distracted human) can misread as a branch literally
/// named `HEAD`. The plain-status output keeps the explicit SHA/ref, so we
/// surface that instead. Returns `None` when HEAD is on a branch.
fn extract_detached_head(raw: &str) -> Option<String> {
    raw.lines()
        .map(str::trim)
        .find(|l| l.starts_with("HEAD detached "))
        .map(str::to_string)
}

/// Minimal filtering for git status with user-provided args
fn filter_status_with_args(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        // Skip git hints - can appear at start or within line
        if trimmed.starts_with("(use \"git")
            || trimmed.starts_with("(create/copy files")
            || trimmed.contains("(use \"git add")
            || trimmed.contains("(use \"git restore")
        {
            continue;
        }

        // Special case: clean working tree
        if trimmed.contains("nothing to commit") && trimmed.contains("working tree clean") {
            result.push(trimmed.to_string());
            break;
        }

        result.push(line.to_string());
    }

    if result.is_empty() {
        "ok".to_string()
    } else {
        result.join("\n")
    }
}

fn run_status(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // Keep a narrow compact path for no-arg status and branch/short-only flags.
    // More complex explicit args still use the existing minimal-filter path.
    if !uses_compact_status_path(args) {
        let mut cmd = build_status_command(args, global_args);
        let result = exec_capture(&mut cmd).context("Failed to run git status")?;

        if !result.success() {
            if !result.stderr.trim().is_empty() {
                eprint!("{}", result.stderr);
            }
            timer.track(
                &format!("git status {}", args.join(" ")),
                &format!("rtk git status {}", args.join(" ")),
                &result.stdout,
                &result.stdout,
            );
            return Ok(result.exit_code);
        }

        if verbose > 0 || !result.stderr.is_empty() {
            eprint!("{}", result.stderr);
        }

        // Apply minimal filtering: strip ANSI, remove hints, empty lines
        let filtered = filter_status_with_args(&result.stdout);
        let filtered = never_worse(&result.stdout, &filtered).to_string();
        print!("{}", filtered);

        timer.track(
            &format!("git status {}", args.join(" ")),
            &format!("rtk git status {}", args.join(" ")),
            &result.stdout,
            &filtered,
        );

        return Ok(0);
    }

    let mut raw_cmd = git_cmd_c_locale(global_args);
    raw_cmd.arg("status");
    raw_cmd.args(args);
    let raw_output = exec_capture(&mut raw_cmd)
        .map(|r| r.stdout)
        .unwrap_or_default();

    let mut cmd = build_status_command(args, global_args);
    let result = exec_capture(&mut cmd).context("Failed to run git status")?;

    if !result.success() {
        let message = if result.stderr.contains("not a git repository") {
            "Not a git repository".to_string()
        } else {
            result.stderr.trim().to_string()
        };
        if !message.is_empty() {
            eprintln!("{}", message);
        }
        let original_cmd = if args.is_empty() {
            "git status".to_string()
        } else {
            format!("git status {}", args.join(" "))
        };
        let rtk_cmd = if args.is_empty() {
            "rtk git status".to_string()
        } else {
            format!("rtk git status {}", args.join(" "))
        };
        let shown = never_worse(&raw_output, &message);
        timer.track(&original_cmd, &rtk_cmd, &raw_output, shown);
        return Ok(result.exit_code);
    }

    let formatted = match extract_detached_head(&raw_output) {
        Some(detached_ref) => format_status_output_detached(&result.stdout, &detached_ref),
        None => format_status_output(&result.stdout),
    };

    // Surface in-progress state (rebase/merge/cherry-pick/bisect/am) from the
    // plain-status output we already captured for tracking. Porcelain omits it
    // and hiding it misleads the user about the true repo state.
    let final_output = match extract_state_header(&raw_output) {
        Some(state) => format!("{}\n{}", state, formatted),
        None => formatted,
    };

    let shown = never_worse(&raw_output, &final_output);
    println!("{}", shown);

    let original_cmd = if args.is_empty() {
        "git status".to_string()
    } else {
        format!("git status {}", args.join(" "))
    };
    let rtk_cmd = if args.is_empty() {
        "rtk git status".to_string()
    } else {
        format!("rtk git status {}", args.join(" "))
    };

    timer.track(&original_cmd, &rtk_cmd, &raw_output, shown);

    Ok(0)
}

fn run_add(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = git_cmd(global_args);
    cmd.arg("add");

    // Pass all arguments directly to git (flags like -A, -p, --all, etc.)
    if args.is_empty() {
        cmd.arg(".");
    } else {
        for arg in args {
            cmd.arg(arg);
        }
    }

    let result = exec_capture(&mut cmd).context("Failed to run git add")?;

    if verbose > 0 {
        eprintln!("git add executed");
    }

    let raw_output = format!("{}\n{}", result.stdout, result.stderr);

    if result.success() {
        // Count what was added
        let mut stat_cmd = git_cmd(global_args);
        stat_cmd.args(["diff", "--cached", "--stat", "--shortstat"]);
        let stat_result = exec_capture(&mut stat_cmd).context("Failed to check staged files")?;

        // Mirror git's own behaviour: a no-op `git add` is silent. Emitting a
        // generic "ok" here is misleading — an agent can't tell "staged N files"
        // from "staged nothing" when both print "ok".
        let compact = if stat_result.stdout.trim().is_empty() {
            String::new()
        } else {
            // Parse "1 file changed, 5 insertions(+)" format
            let short = stat_result.stdout.lines().last().unwrap_or("").trim();
            if short.is_empty() {
                "ok".to_string()
            } else {
                format!("ok {}", short)
            }
        };

        if !compact.is_empty() {
            println!("{}", compact);
        }

        timer.track(
            &format!("git add {}", args.join(" ")),
            &format!("rtk git add {}", args.join(" ")),
            &raw_output,
            &compact,
        );
    } else {
        eprintln!("FAILED: git add");
        if !result.stderr.trim().is_empty() {
            eprintln!("{}", result.stderr);
        }
        if !result.stdout.trim().is_empty() {
            eprintln!("{}", result.stdout);
        }
        return Ok(result.exit_code);
    }

    Ok(0)
}

fn build_commit_command(args: &[String], global_args: &[String]) -> Command {
    let mut cmd = git_cmd(global_args);
    cmd.arg("commit");
    for arg in args {
        cmd.arg(arg);
    }
    cmd
}

/// Parse the first line of `git commit` success output and return a compact token.
/// Handles: `[main abc1234def] message`, `[main (root-commit) abc1234def] msg`,
/// localized variants, and multibyte branch names.
fn parse_commit_output(line: &str) -> String {
    // Locate the summary's own brackets rather than assuming the line starts
    // with '['. git prints hook output before its summary, so the first line
    // is often something else entirely; slicing from byte 1 panics outright
    // when that line opens with a multi-byte character ("✅ lint passed]"),
    // and a line decoded from non-UTF-8 bytes starts with a multi-byte U+FFFD.
    // Both indices come from `find`, so both land on character boundaries.
    let (Some(open), Some(bracket_end)) = (line.find('['), line.find(']')) else {
        return "ok".to_string();
    };
    if open >= bracket_end {
        return "ok".to_string();
    }

    let bracket_content = &line[open + 1..bracket_end];
    let hash = bracket_content.split_whitespace().next_back().unwrap_or("");
    if hash.chars().count() >= 7 {
        let short_hash: String = hash.chars().take(7).collect();
        format!("ok {}", short_hash)
    } else {
        "ok".to_string()
    }
}

fn run_commit(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let original_cmd = format!("git commit {}", args.join(" "));

    if verbose > 0 {
        eprintln!("{}", original_cmd);
    }

    // stdin is inherited so an interactive editor, GPG passphrase prompt or
    // credential helper still reaches the terminal.
    let CaptureResult {
        stdout,
        stderr,
        exit_code,
    } = exec_capture_stdin(&mut build_commit_command(args, global_args))
        .context("Failed to run git commit")?;
    let raw_output = format!("{}\n{}", stdout, stderr);

    match classify_commit_outcome(exit_code == 0, &stdout, exit_code) {
        CommitOutcome::Ok(compact) => {
            println!("{}", compact);
            timer.track(&original_cmd, "rtk git commit", &raw_output, &compact);
            Ok(0)
        }
        CommitOutcome::Failed(code) => {
            if !stderr.trim().is_empty() {
                eprint!("{}", stderr);
            }
            if !stdout.trim().is_empty() {
                eprint!("{}", stdout);
            }
            timer.track(&original_cmd, "rtk git commit", &raw_output, &raw_output);
            Ok(code)
        }
    }
}

/// Outcome of a `git commit`: a non-success status propagates the exit code
/// rather than being reported as "ok" (#2494).
enum CommitOutcome {
    Ok(String),
    Failed(i32),
}

/// Classify a `git commit` result.
fn classify_commit_outcome(success: bool, stdout: &str, exit_code: i32) -> CommitOutcome {
    if success {
        // Extract commit hash from output
        let compact = stdout
            .lines()
            .next()
            .map(parse_commit_output)
            .unwrap_or_else(|| "ok".to_string());
        CommitOutcome::Ok(compact)
    } else {
        CommitOutcome::Failed(exit_code)
    }
}

fn run_checkout(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let args = args_utils::restore_double_dash(args);

    if verbose > 0 {
        eprintln!("git checkout");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("checkout");
    for arg in &args {
        cmd.arg(arg);
    }

    let args_display = args.join(" ");
    let args_for_filter = args.clone();
    runner::run_filtered_with_exit(
        cmd,
        "git checkout",
        &args_display,
        move |raw, exit_code| format_checkout_output(&args_for_filter, raw, exit_code),
        RunOptions::with_tee("git_checkout"),
    )
}

fn format_checkout_output(args: &[String], raw: &str, exit_code: i32) -> String {
    if exit_code == 0 {
        format_checkout_success(args, raw)
    } else {
        filter_checkout_failure(raw)
    }
}

fn format_checkout_success(args: &[String], raw: &str) -> String {
    if let Some(restored) = checkout_restored_count(args) {
        return format!(
            "ok {} {}",
            restored,
            pluralize(restored, "file restored", "files restored")
        );
    }
    if let Some(branch) = checkout_reset_branch_arg(args) {
        return format!("ok {}", branch);
    }

    for line in raw.lines().map(str::trim) {
        if let Some(branch) = quoted_suffix(line, "Switched to a new branch ") {
            return format!("ok {} (new)", branch);
        }
        if let Some(branch) = quoted_suffix(line, "Switched to branch ") {
            return format!("ok {}", branch);
        }
        if let Some(branch) = quoted_suffix(line, "Already on ") {
            return format!("ok {}", branch);
        }
        if let Some(rest) = line.strip_prefix("HEAD is now at ") {
            let hash = rest.split_whitespace().next().unwrap_or("HEAD");
            return format!("ok HEAD {}", hash);
        }
        if line.starts_with("Updated ") && line.contains(" path") {
            return format!("ok {}", line.to_ascii_lowercase());
        }
    }

    if let Some(branch) = checkout_new_branch_arg(args) {
        return format!("ok {} (new)", branch);
    }
    if let Some(branch) = checkout_branch_arg(args) {
        return format!("ok {}", branch);
    }

    "ok".to_string()
}

fn checkout_restored_count(args: &[String]) -> Option<usize> {
    let separator = args.iter().position(|arg| arg == "--")?;
    let count = args[separator + 1..]
        .iter()
        .filter(|arg| !arg.is_empty())
        .count();
    (count > 0).then_some(count)
}

fn checkout_new_branch_arg(args: &[String]) -> Option<&str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-b" | "--orphan" => return iter.next().map(String::as_str),
            "-B" => {
                iter.next();
            }
            _ => {
                if let Some(branch) = arg.strip_prefix("--orphan=") {
                    return Some(branch);
                }
            }
        }
    }
    None
}

fn checkout_reset_branch_arg(args: &[String]) -> Option<&str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-B" {
            return iter.next().map(String::as_str);
        }
    }
    None
}

fn checkout_branch_arg(args: &[String]) -> Option<&str> {
    if args.iter().any(|arg| arg == "--") {
        return None;
    }

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-b" | "-B" | "--orphan" => {
                iter.next();
            }
            "-t" | "--track" | "--detach" => {}
            _ if arg.starts_with('-') => {}
            _ => return Some(arg),
        }
    }
    None
}

fn quoted_suffix<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('\''))
        .and_then(|rest| rest.strip_suffix('\''))
}

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn filter_checkout_failure(raw: &str) -> String {
    let mut important = Vec::new();
    let mut in_file_list = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let is_header = trimmed.starts_with("error:")
            || trimmed.starts_with("fatal:")
            || trimmed.starts_with("CONFLICT");

        if is_header {
            in_file_list = trimmed.contains("following")
                && trimmed.contains("files")
                && trimmed.ends_with(':');
            important.push(trimmed.to_string());
            continue;
        }

        if in_file_list {
            if trimmed.starts_with("Please ") || trimmed.starts_with("Aborting") {
                in_file_list = false;
            } else if line.starts_with(char::is_whitespace) {
                important.push(line.to_string());
                continue;
            }
        }

        if trimmed.starts_with("Aborting") {
            important.push(trimmed.to_string());
        }
    }

    if important.is_empty() {
        raw.trim().to_string()
    } else {
        important.join("\n")
    }
}

// Git push progress prefixes (stderr) — dropped from the stream.
const GIT_PUSH_NOISE_PREFIXES: &[&str] = &[
    "Enumerating objects:",
    "Counting objects:",
    "Compressing objects:",
    "Writing objects:",
    "Delta compression using",
    "Total ",
];

#[derive(Default)]
struct GitPushLineHandler {
    up_to_date: bool,
    pushed_ref: Option<String>,
}

impl LineHandler for GitPushLineHandler {
    fn should_skip(&mut self, line: &str) -> bool {
        if line.is_empty() {
            return true;
        }
        let trimmed = line.trim_start();
        GIT_PUSH_NOISE_PREFIXES
            .iter()
            .any(|p| trimmed.starts_with(p))
    }

    fn observe_line(&mut self, line: &str) {
        if line.contains("Everything up-to-date") {
            self.up_to_date = true;
        }
        if self.pushed_ref.is_none() {
            if let Some(idx) = line.find(" -> ") {
                let after = &line[idx + 4..];
                if let Some(dest) = after.split_whitespace().next() {
                    self.pushed_ref = Some(dest.to_string());
                }
            }
        }
    }

    fn format_summary(&self, exit_code: i32, _raw: &str) -> Option<String> {
        if exit_code != 0 {
            return None;
        }
        let summary = if self.up_to_date {
            "ok (up-to-date)".to_string()
        } else if let Some(dest) = &self.pushed_ref {
            format!("ok {}", dest)
        } else {
            "ok".to_string()
        };
        Some(format!("{}\n", summary))
    }
}

fn run_push(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git push");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("push");
    for arg in args {
        cmd.arg(arg);
    }

    let cmd_label = format!("git push {}", args.join(" "));
    let filter = LineStreamFilter::new(GitPushLineHandler::default());
    let result = stream::run_streaming(
        &mut cmd,
        StdinMode::Inherit,
        FilterMode::Streaming(Box::new(filter)),
    )
    .context("Failed to run git push")?;

    timer.track(
        &cmd_label,
        &format!("rtk {}", cmd_label),
        &result.raw,
        &result.filtered,
    );

    Ok(result.exit_code)
}

fn run_pull(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git pull");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("pull");
    for arg in args {
        cmd.arg(arg);
    }

    let result = exec_capture(&mut cmd).context("Failed to run git pull")?;

    let raw_output = format!("{}\n{}", result.stdout, result.stderr);

    if result.success() {
        let compact = if result.stdout.contains("Already up to date")
            || result.stdout.contains("Already up-to-date")
        {
            "ok (up-to-date)".to_string()
        } else {
            // Count files changed
            let mut files = 0;
            let mut insertions = 0;
            let mut deletions = 0;

            for line in result.stdout.lines() {
                if line.contains("file") && line.contains("changed") {
                    // Parse "3 files changed, 10 insertions(+), 2 deletions(-)"
                    for part in line.split(',') {
                        let part = part.trim();
                        if part.contains("file") {
                            files = part
                                .split_whitespace()
                                .next()
                                .and_then(|n| n.parse().ok())
                                .unwrap_or(0);
                        } else if part.contains("insertion") {
                            insertions = part
                                .split_whitespace()
                                .next()
                                .and_then(|n| n.parse().ok())
                                .unwrap_or(0);
                        } else if part.contains("deletion") {
                            deletions = part
                                .split_whitespace()
                                .next()
                                .and_then(|n| n.parse().ok())
                                .unwrap_or(0);
                        }
                    }
                }
            }

            if files > 0 {
                format!("ok {} files +{} -{}", files, insertions, deletions)
            } else {
                "ok".to_string()
            }
        };

        println!("{}", compact);

        timer.track(
            &format!("git pull {}", args.join(" ")),
            &format!("rtk git pull {}", args.join(" ")),
            &raw_output,
            &compact,
        );
    } else {
        eprintln!("FAILED: git pull");
        if !result.stderr.trim().is_empty() {
            eprintln!("{}", result.stderr);
        }
        if !result.stdout.trim().is_empty() {
            eprintln!("{}", result.stdout);
        }
        return Ok(result.exit_code);
    }

    Ok(0)
}

fn run_branch(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git branch");
    }

    // Detect write operations: delete, rename, copy, upstream tracking
    let has_action_flag = args.iter().any(|a| {
        a == "-d"
            || a == "-D"
            || a == "-m"
            || a == "-M"
            || a == "-c"
            || a == "-C"
            || a == "--set-upstream-to"
            || a.starts_with("--set-upstream-to=")
            || a == "-u"
            || a == "--unset-upstream"
            || a == "--edit-description"
    });

    // Detect flags that produce specific output (not a branch list)
    let has_show_flag = args.iter().any(|a| a == "--show-current");

    // Detect list-mode flags
    let has_list_flag = args.iter().any(|a| {
        a == "-a"
            || a == "--all"
            || a == "-r"
            || a == "--remotes"
            || a == "--list"
            || a == "--merged"
            || a == "--no-merged"
            || a == "--contains"
            || a == "--no-contains"
            || a == "--format"
            || a.starts_with("--format=")
            || a == "--sort"
            || a.starts_with("--sort=")
            || a == "--points-at"
            || a.starts_with("--points-at=")
    });

    // Detect positional arguments (not flags) — indicates branch creation
    let has_positional_arg = args.iter().any(|a| !a.starts_with('-'));

    // --show-current: passthrough with raw stdout (not "ok")
    if has_show_flag {
        let mut cmd = git_cmd(global_args);
        cmd.arg("branch");
        for arg in args {
            cmd.arg(arg);
        }
        let result = exec_capture(&mut cmd).context("Failed to run git branch")?;
        let combined = result.combined();

        let trimmed = result.stdout.trim();
        timer.track(
            &format!("git branch {}", args.join(" ")),
            &format!("rtk git branch {}", args.join(" ")),
            &combined,
            trimmed,
        );

        if result.success() {
            println!("{}", trimmed);
        } else {
            eprintln!("FAILED: git branch {}", args.join(" "));
            if !result.stderr.trim().is_empty() {
                eprintln!("{}", result.stderr);
            }
            return Ok(result.exit_code);
        }
        return Ok(0);
    }

    // Write operation: action flags, or positional args without list flags (= branch creation)
    if has_action_flag || (has_positional_arg && !has_list_flag) {
        let mut cmd = git_cmd(global_args);
        cmd.arg("branch");
        for arg in args {
            cmd.arg(arg);
        }
        let result = exec_capture(&mut cmd).context("Failed to run git branch")?;
        let combined = result.combined();

        let msg = if result.success() { "ok" } else { &combined };

        timer.track(
            &format!("git branch {}", args.join(" ")),
            &format!("rtk git branch {}", args.join(" ")),
            &combined,
            msg,
        );

        if result.success() {
            println!("ok");
        } else {
            eprintln!("FAILED: git branch {}", args.join(" "));
            if !result.stderr.trim().is_empty() {
                eprintln!("{}", result.stderr);
            }
            if !result.stdout.trim().is_empty() {
                eprintln!("{}", result.stdout);
            }
            return Ok(result.exit_code);
        }
        return Ok(0);
    }

    // List mode: show compact branch list
    let mut cmd = git_cmd(global_args);
    cmd.arg("branch");
    if !has_list_flag {
        cmd.arg("-a");
    }
    cmd.arg("--no-color");
    for arg in args {
        cmd.arg(arg);
    }

    let result = exec_capture(&mut cmd).context("Failed to run git branch")?;

    if !result.success() {
        if !result.stderr.trim().is_empty() {
            eprint!("{}", result.stderr);
        }
        timer.track(
            &format!("git branch {}", args.join(" ")),
            &format!("rtk git branch {}", args.join(" ")),
            &result.stdout,
            &result.stdout,
        );
        return Ok(result.exit_code);
    }

    let filtered = filter_branch_output(&result.stdout);
    let filtered = never_worse(&result.stdout, &filtered).to_string();
    println!("{}", filtered);

    timer.track(
        &format!("git branch {}", args.join(" ")),
        &format!("rtk git branch {}", args.join(" ")),
        &result.stdout,
        &filtered,
    );

    Ok(0)
}

fn filter_branch_output(output: &str) -> String {
    let mut current = String::new();
    let mut local: Vec<String> = Vec::new();
    let mut remote: Vec<String> = Vec::new();
    let mut seen_remote: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(branch) = line.strip_prefix("* ") {
            current = branch.to_string();
        } else if let Some(rest) = line.strip_prefix("remotes/") {
            if let Some(slash_pos) = rest.find('/') {
                let branch = &rest[slash_pos + 1..];
                if branch.starts_with("HEAD ") {
                    continue;
                }
                if seen_remote.insert(branch.to_string()) {
                    remote.push(branch.to_string());
                }
            }
        } else {
            local.push(line.to_string());
        }
    }

    let mut result = Vec::new();
    result.push(format!("* {}", current));

    if !local.is_empty() {
        for b in &local {
            result.push(format!("  {}", b));
        }
    }

    if !remote.is_empty() {
        let remote_only: Vec<&String> = remote
            .iter()
            .filter(|r| *r != &current && !local.contains(r))
            .collect();
        if !remote_only.is_empty() {
            const MAX_REMOTE_BRANCHES: usize = CAP_WARNINGS;
            result.push(format!("  remote-only ({}):", remote_only.len()));
            for b in remote_only.iter().take(MAX_REMOTE_BRANCHES) {
                result.push(format!("    {}", b));
            }
            if remote_only.len() > MAX_REMOTE_BRANCHES {
                result.push(format!(
                    "    ... +{} more",
                    remote_only.len() - MAX_REMOTE_BRANCHES
                ));
            }
        }
    }

    result.join("\n")
}

fn run_fetch(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git fetch");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("fetch");
    for arg in args {
        cmd.arg(arg);
    }

    let result = exec_capture(&mut cmd).context("Failed to run git fetch")?;
    let raw = result.combined();

    if !result.success() {
        eprintln!("FAILED: git fetch");
        if !result.stderr.trim().is_empty() {
            eprintln!("{}", result.stderr);
        }
        return Ok(result.exit_code);
    }

    // Count new refs from stderr (git fetch outputs to stderr)
    let new_refs: usize = result
        .stderr
        .lines()
        .filter(|l| l.contains("->") || l.contains("[new"))
        .count();

    let msg = if new_refs > 0 {
        format!("ok fetched ({} new refs)", new_refs)
    } else {
        "ok fetched".to_string()
    };

    println!("{}", msg);
    timer.track("git fetch", "rtk git fetch", &raw, &msg);

    Ok(0)
}

/// Format status message for stash operations.
/// - For create operations (push/save): checks for "No local changes"
/// - For other operations: uses "ok stash <subcommand>" format
fn format_stash_message(subcommand: Option<&str>, result: &CaptureResult) -> String {
    match subcommand {
        None | Some("push") | Some("save") => {
            // A successful stash collapses to "ok stashed" (the WIP ref/sha git
            // prints isn't needed to `git stash pop`). But a no-op must NOT look
            // like success — pass git's "No local changes to save" through so the
            // agent can tell nothing was stashed.
            if result.combined().contains("No local changes") {
                "No local changes to save".to_string()
            } else {
                "ok stashed".to_string()
            }
        }
        Some(sub) => format!("ok stash {}", sub),
    }
}

fn run_stash(
    subcommand: Option<&str>,
    args: &[String],
    verbose: u8,
    global_args: &[String],
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git stash {:?}", subcommand);
    }

    match subcommand {
        Some("list") => {
            let mut cmd = git_cmd(global_args);
            cmd.args(["stash", "list"]);
            let result = exec_capture(&mut cmd).context("Failed to run git stash list")?;

            if result.stdout.trim().is_empty() {
                if !result.success() && !result.stderr.trim().is_empty() {
                    eprintln!("{}", result.stderr.trim());
                }
                timer.track("git stash list", "rtk git stash list", &result.stdout, "");
                return Ok(result.exit_code);
            }

            let filtered = filter_stash_list(&result.stdout);
            let filtered = never_worse(&result.stdout, &filtered).to_string();
            println!("{}", filtered);
            timer.track(
                "git stash list",
                "rtk git stash list",
                &result.stdout,
                &filtered,
            );
        }
        Some("show") => {
            let patch_mode = args.iter().any(|a| a == "-p" || a == "--patch");

            let mut cmd = git_cmd(global_args);
            cmd.args(["stash", "show"]);
            for arg in args {
                cmd.arg(arg);
            }
            let result = exec_capture(&mut cmd).context("Failed to run git stash show")?;

            if result.stdout.trim().is_empty() {
                if !result.success() && !result.stderr.trim().is_empty() {
                    eprintln!("{}", result.stderr.trim());
                }
                timer.track("git stash show", "rtk git stash show", &result.stdout, "");
                return Ok(result.exit_code);
            }

            let filtered = if patch_mode && !emits_word_diff(args) {
                compact_diff(&result.stdout, 100)
            } else if patch_mode {
                result.stdout.clone()
            } else {
                compact_stash_stat(&result.stdout)
            };
            let shown = crate::core::runner::emit_guarded(&filtered, None, &result.stdout);
            timer.track(
                "git stash show",
                "rtk git stash show",
                &result.stdout,
                &shown,
            );
        }
        Some("apply") | Some("branch") | Some("clear") | Some("create") | Some("drop")
        | Some("export") | Some("import") | Some("pop") | Some("store") => {
            let sub = subcommand.unwrap();
            let mut cmd = git_cmd(global_args);
            cmd.args(["stash", sub]);
            for arg in args {
                cmd.arg(arg);
            }
            let result = exec_capture(&mut cmd).context("Failed to run git stash")?;
            let combined = result.combined();

            let msg = if result.success() {
                let msg = format_stash_message(subcommand, &result);
                println!("{}", msg);
                msg
            } else {
                eprintln!("FAILED: git stash {}", sub);
                if !result.stderr.trim().is_empty() {
                    eprintln!("{}", result.stderr);
                }
                combined.clone()
            };

            timer.track(
                &format!("git stash {}", sub),
                &format!("rtk git stash {}", sub),
                &combined,
                &msg,
            );

            if !result.success() {
                return Ok(result.exit_code);
            }
        }
        // Default: "git stash [push] [--] [<pathspec>...]" or "git stash save [<message>]"
        Some(_) | None => {
            let (sub, arg) = match subcommand {
                Some("save") => ("save", None),
                Some("push") => ("push", None),
                Some(s) => ("push", Some(s)),
                None => ("push", None),
            };
            let mut cmd = git_cmd(global_args);
            cmd.args(["stash", sub]);
            if let Some(arg) = arg {
                cmd.arg(arg);
            }
            for arg in args {
                cmd.arg(arg);
            }
            let result = exec_capture(&mut cmd).context("Failed to run git stash")?;
            let combined = result.combined();

            let msg = if result.success() {
                let msg = format_stash_message(subcommand, &result);
                println!("{}", msg);
                msg
            } else {
                eprintln!("FAILED: git stash {}", sub);
                if !result.stderr.trim().is_empty() {
                    eprintln!("{}", result.stderr);
                }
                combined.clone()
            };

            timer.track(
                &format!("git stash {}", sub),
                &format!("rtk git stash {}", sub),
                &combined,
                &msg,
            );

            if !result.success() {
                return Ok(result.exit_code);
            }
        }
    }

    Ok(0)
}

fn filter_stash_list(output: &str) -> String {
    // Format: "stash@{0}: WIP on main: abc1234 commit message"
    let mut result = Vec::new();
    for line in output.lines() {
        if let Some(colon_pos) = line.find(": ") {
            let index = &line[..colon_pos];
            let rest = &line[colon_pos + 2..];
            // Compact: strip "WIP on branch:" prefix if present
            let message = if let Some(second_colon) = rest.find(": ") {
                rest[second_colon + 2..].trim()
            } else {
                rest.trim()
            };
            result.push(format!("{}: {}", index, message));
        } else {
            result.push(line.to_string());
        }
    }
    result.join("\n")
}

fn compact_stash_stat(raw: &str) -> String {
    let (files, summary) = parse_stash_stat(raw);
    if files.is_empty() {
        return raw.trim_end().to_string();
    }
    let total = files.len();
    let mut out = join_with_overflow(&files[..total.min(CAP_LIST)], total, CAP_LIST, "files");
    if total > CAP_LIST {
        if let Some(hint) =
            crate::core::tee::force_tee_tail_hint(&files.join("\n"), "git-stash-show", CAP_LIST + 1)
        {
            out.push(' ');
            out.push_str(&hint);
        }
    }
    if !summary.is_empty() {
        out.push('\n');
        out.push_str(&compress_stat_summary(&summary));
    }
    out
}

fn compress_stat_summary(summary: &str) -> String {
    summary
        .replace("insertions(+)", "+")
        .replace("insertion(+)", "+")
        .replace("deletions(-)", "-")
        .replace("deletion(-)", "-")
        .replace("files changed", "changed")
        .replace("file changed", "changed")
        .replace(",", "")
}

fn parse_stash_stat(stat: &str) -> (Vec<String>, String) {
    let stat = strip_ansi(stat);
    let mut files = Vec::new();
    let mut summary = String::new();

    for line in stat.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match diffstat_row(line) {
            Some(row) => files.push(row),
            None => summary = line.to_string(),
        }
    }

    (files, summary)
}

fn diffstat_row(line: &str) -> Option<String> {
    let bar = line.rfind('|')?;
    let path = line[..bar].trim();
    let rhs = line[bar + 1..].trim();
    let is_diffstat_row = rhs.starts_with("Bin") || rhs.starts_with(|c: char| c.is_ascii_digit());
    if path.is_empty() || !is_diffstat_row {
        return None;
    }
    if rhs.starts_with("Bin") {
        return Some(format!("{} (binary)", path));
    }
    let count = rhs.split_whitespace().next().unwrap_or("");
    let sign = match (rhs.contains('+'), rhs.contains('-')) {
        (true, true) => " +-",
        (true, false) => " +",
        (false, true) => " -",
        (false, false) => "",
    };
    Some(format!("{} {}{}", path, count, sign))
}

fn run_worktree(args: &[String], verbose: u8, global_args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git worktree list");
    }

    // If args contain "add", "remove", "prune" etc., pass through
    let has_action = args.iter().any(|a| {
        a == "add" || a == "remove" || a == "prune" || a == "lock" || a == "unlock" || a == "move"
    });

    if has_action {
        let mut cmd = git_cmd(global_args);
        cmd.arg("worktree");
        for arg in args {
            cmd.arg(arg);
        }
        let result = exec_capture(&mut cmd).context("Failed to run git worktree")?;
        let combined = result.combined();

        let msg = if result.success() { "ok" } else { &combined };

        timer.track(
            &format!("git worktree {}", args.join(" ")),
            &format!("rtk git worktree {}", args.join(" ")),
            &combined,
            msg,
        );

        if result.success() {
            println!("ok");
        } else {
            eprintln!("FAILED: git worktree {}", args.join(" "));
            if !result.stderr.trim().is_empty() {
                eprintln!("{}", result.stderr);
            }
            return Ok(result.exit_code);
        }
        return Ok(0);
    }

    // Default: list mode
    let mut cmd = git_cmd(global_args);
    cmd.args(["worktree", "list"]);
    let result = exec_capture(&mut cmd).context("Failed to run git worktree list")?;

    if !result.success() {
        if !result.stderr.trim().is_empty() {
            eprintln!("{}", result.stderr);
        }
        timer.track(
            "git worktree list",
            "rtk git worktree",
            &result.stdout,
            &result.stderr,
        );
        return Ok(result.exit_code);
    }

    let filtered = filter_worktree_list(&result.stdout);
    let filtered = never_worse(&result.stdout, &filtered).to_string();
    println!("{}", filtered);
    timer.track(
        "git worktree list",
        "rtk git worktree",
        &result.stdout,
        &filtered,
    );

    Ok(0)
}

fn filter_worktree_list(output: &str) -> String {
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut result = Vec::new();
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Format: "/path/to/worktree  abc1234 [branch]"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            let mut path = parts[0].to_string();
            if !home.is_empty() && path.starts_with(&home) {
                path = format!("~{}", &path[home.len()..]);
            }
            let hash = parts[1];
            let branch = parts[2..].join(" ");
            result.push(format!("{} {} {}", path, hash, branch));
        } else {
            result.push(line.to_string());
        }
    }
    result.join("\n")
}

/// Runs an unsupported git subcommand by passing it through directly
pub fn run_passthrough(args: &[OsString], global_args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git passthrough: {:?}", args);
    }
    let status = git_cmd(global_args)
        .args(args)
        .status()
        .context("Failed to run git")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("git {}", args_str),
        &format!("rtk git {} (passthrough)", args_str),
    );

    if !status.success() {
        return Ok(exit_code_from_status(&status, "git"));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_cmd_no_global_args() {
        let cmd = git_cmd(&[]);
        let program = cmd.get_program().to_string_lossy().to_string();
        // On Windows, resolved_command returns full path (e.g. "C:\Program Files\Git\bin\git.exe")
        let basename = std::path::Path::new(&program)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(basename, "git");
        let args: Vec<_> = cmd.get_args().collect();
        assert!(args.is_empty());
    }

    #[test]
    fn test_git_cmd_with_directory() {
        let global_args = vec!["-C".to_string(), "/tmp".to_string()];
        let cmd = git_cmd(&global_args);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, vec!["-C", "/tmp"]);
    }

    #[test]
    fn test_git_cmd_with_multiple_global_args() {
        let global_args = vec![
            "-C".to_string(),
            "/tmp".to_string(),
            "-c".to_string(),
            "user.name=test".to_string(),
            "--git-dir".to_string(),
            "/foo/.git".to_string(),
        ];
        let cmd = git_cmd(&global_args);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args,
            vec![
                "-C",
                "/tmp",
                "-c",
                "user.name=test",
                "--git-dir",
                "/foo/.git"
            ]
        );
    }

    #[test]
    fn test_git_cmd_with_boolean_flags() {
        let global_args = vec!["--no-pager".to_string(), "--bare".to_string()];
        let cmd = git_cmd(&global_args);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, vec!["--no-pager", "--bare"]);
    }

    #[test]
    fn test_git_cmd_c_locale_sets_stable_env() {
        let cmd = git_cmd_c_locale(&[]);
        let envs: Vec<_> = cmd
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.expect("env value").to_string_lossy().to_string(),
                )
            })
            .collect();
        assert!(envs.contains(&("LC_ALL".to_string(), "C".to_string())));
    }

    #[test]
    fn test_build_status_command_default_compact() {
        let cmd = build_status_command(&[], &[]);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, vec!["status", "--porcelain", "-b"]);
    }

    #[test]
    fn test_uses_compact_status_path_for_branch_and_short_flags() {
        assert!(uses_compact_status_path(&["-b".to_string()]));
        assert!(uses_compact_status_path(&["--branch".to_string()]));
        assert!(uses_compact_status_path(&["-sb".to_string()]));
        assert!(uses_compact_status_path(&[
            "-s".to_string(),
            "-b".to_string()
        ]));
        assert!(uses_compact_status_path(&[
            "--short".to_string(),
            "--branch".to_string()
        ]));
        assert!(!uses_compact_status_path(&["-s".to_string()]));
        assert!(!uses_compact_status_path(&["--short".to_string()]));
        assert!(!uses_compact_status_path(&["--porcelain".to_string()]));
        assert!(!uses_compact_status_path(&["-uno".to_string()]));
    }

    #[test]
    fn test_build_status_command_with_user_args_passthrough() {
        let args = vec!["--short".to_string(), "--branch".to_string()];
        let cmd = build_status_command(&args, &[]);
        let cmd_args: Vec<_> = cmd.get_args().collect();
        assert_eq!(cmd_args, vec!["status", "--porcelain", "-b"]);
    }

    #[test]
    fn test_build_status_command_with_incompatible_user_args_passthrough() {
        let args = vec!["--porcelain".to_string(), "-uno".to_string()];
        let cmd = build_status_command(&args, &[]);
        let cmd_args: Vec<_> = cmd.get_args().collect();
        assert_eq!(cmd_args, vec!["status", "--porcelain", "-uno"]);
    }

    #[test]
    fn test_run_status_compact_propagates_non_repo_failure() {
        // #2497: a `git status` failure other than "not a git repository"
        // (here: a corrupt index) must propagate a non-zero exit, not be
        // flattened into "Clean working tree" + exit 0.
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().to_string_lossy().into_owned();
        assert!(
            Command::new("git")
                .args(["-C", &p, "init", "-q"])
                .status()
                .expect("git init")
                .success(),
            "git init should succeed"
        );
        std::fs::write(dir.path().join(".git/index"), "corrupt-index").expect("corrupt index");
        let global = vec!["-C".to_string(), p];
        let code = run_status(&[], 0, &global).expect("run_status");
        assert_ne!(
            code, 0,
            "corrupt-index git status must not be reported as success"
        );
    }

    #[test]
    fn test_compact_diff() {
        let diff = r#"diff --git a/foo.rs b/foo.rs
--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("hello");
 }
"#;
        let result = compact_diff(diff, 100);
        assert!(result.contains("foo.rs"));
        assert!(result.contains("+"));
    }

    #[test]
    fn test_compact_diff_hunk_lines_are_grep_anchorable() {
        let diff = "diff --git a/f.txt b/f.txt\n\
                    --- a/f.txt\n\
                    +++ b/f.txt\n\
                    @@ -1,5 +1,4 @@\n\
                    \x20keep1\n\
                    -DELETED_A\n\
                    \x20keep2\n\
                    -DELETED_B\n\
                    +ADDED\n";
        let result = compact_diff(diff, 100);

        let removed: Vec<&str> = result.lines().filter(|l| l.starts_with('-')).collect();
        let added: Vec<&str> = result.lines().filter(|l| l.starts_with('+')).collect();

        assert_eq!(removed, vec!["-DELETED_A", "-DELETED_B"], "`^-` must anchor");
        assert_eq!(added, vec!["+ADDED"], "`^+` must anchor");

        // rtk's own tally stays indented so these same greps never count it as
        // a diff line. Without this, `^+` would pick up the "+1 -2" summary.
        assert!(result.contains("  +1 -2"), "tally must stay indented");

        // Context lines keep git's leading space, so they are not `^-`/`^+`.
        // Both are emitted: the one before the first change as well as the one
        // between changes.
        assert!(
            result.lines().any(|l| l == " keep1"),
            "leading context must survive, got:\n{}",
            result
        );
        assert!(result.lines().any(|l| l == " keep2"));
    }

    #[test]
    fn test_compact_diff_keeps_content_starting_with_plus_or_minus() {
        // `---` / `+++` are file headers only before the first `@@`. Inside a
        // hunk, `++i;` and `-- sql comment` are content and must be neither
        // dropped from the body nor missing from the tally.
        let diff = "diff --git a/f.sql b/f.sql\n\
                    --- a/f.sql\n\
                    +++ b/f.sql\n\
                    @@ -1,2 +1,2 @@\n\
                    --- sql comment\n\
                    +++i;\n";
        let result = compact_diff(diff, 100);

        assert!(
            result.lines().any(|l| l == "--- sql comment"),
            "deleted SQL comment must survive, got:\n{}",
            result
        );
        assert!(
            result.lines().any(|l| l == "+++i;"),
            "added `++i;` must survive, got:\n{}",
            result
        );
        assert!(result.contains("  +1 -1"), "tally must count both, got:\n{}", result);
    }

    #[test]
    fn test_compact_diff_leading_context_has_its_own_budget() {
        // Leading context must not consume the 100-line change budget: a hunk
        // opening with more context than the budget still shows every change.
        let mut diff = String::from("diff --git a/f.rs b/f.rs\n@@ -1,120 +1,120 @@\n");
        for i in 0..20 {
            diff.push_str(&format!(" ctx{}\n", i));
        }
        for i in 0..100 {
            diff.push_str(&format!("-del{}\n", i));
        }
        let result = compact_diff(&diff, 1000);

        let ctx = result.lines().filter(|l| l.starts_with(" ctx")).count();
        let dels = result.lines().filter(|l| l.starts_with("-del")).count();
        assert_eq!(ctx, 3, "leading context is capped, got:\n{}", result);
        assert_eq!(dels, 100, "every change must still be shown, got:\n{}", result);
        assert!(
            !result.contains("truncated"),
            "no change was dropped, got:\n{}",
            result
        );
    }

    #[test]
    fn test_compact_diff_combined_diff_headers_are_not_hunk_content() {
        // `diff --cc` sections do not match `diff --git`, so the reset fires on
        // the `diff --` prefix and the `---` / `+++` headers of every section
        // stay outside the hunk body.
        let diff = "diff --cc a.txt\n\
                    index ba2906d,e45c9c2..0000000\n\
                    --- a/a.txt\n\
                    +++ b/a.txt\n\
                    @@@ -1,1 -1,1 +1,5 @@@\n\
                    ++<<<<<<< HEAD\n\
                    \x20+main\n\
                    ++=======\n\
                    + side\n\
                    ++>>>>>>> side\n\
                    diff --cc z.txt\n\
                    index ba2906d,e45c9c2..0000000\n\
                    --- a/z.txt\n\
                    +++ b/z.txt\n\
                    @@@ -1,1 -1,1 +1,5 @@@\n\
                    ++<<<<<<< HEAD\n\
                    \x20+main\n\
                    ++=======\n\
                    + side\n\
                    ++>>>>>>> side\n";
        let result = compact_diff(diff, 500);

        assert!(
            !result.lines().any(|l| l.starts_with("+++ b/")),
            "file headers must not reach the hunk body, got:\n{}",
            result
        );
        assert!(result.contains("z.txt"), "got:\n{}", result);
        // A combined diff carries one marker column per parent, so ` +main` is
        // an addition against the second parent. The tally counts all five
        // added lines per file; an anchored `^+` sees only the four whose
        // marker sits in column 1. That gap is documented in FEATURES.md.
        assert_eq!(
            result.matches("  +5 -0").count(),
            2,
            "column-2 markers must be counted, got:\n{}",
            result
        );
        let anchored = result.lines().filter(|l| l.starts_with('+')).count();
        assert_eq!(anchored, 8, "four per file anchor, got:\n{}", result);
    }

    #[test]
    fn test_compact_diff_mbox_signature_is_not_a_deletion() {
        // `gh pr diff --patch` yields an mbox: a bare `---` before the diffstat
        // and a `-- ` signature after each patch, both at column 0. The hunk
        // ends where its declared line counts run out, so neither is read as
        // hunk content.
        let diff = "From abc Mon Sep 17 00:00:00 2001\n\
                    Subject: [PATCH 1/2] one\n\
                    \n\
                    ---\n\
                    \x20f.txt | 2 +-\n\
                    \n\
                    diff --git a/f.txt b/f.txt\n\
                    --- a/f.txt\n\
                    +++ b/f.txt\n\
                    @@ -1,2 +1,2 @@\n\
                    -old1\n\
                    +new1\n\
                    \x20tail1\n\
                    -- \n\
                    2.40.0\n\
                    \n\
                    From def Mon Sep 17 00:00:00 2001\n\
                    Subject: [PATCH 2/2] two\n\
                    \n\
                    ---\n\
                    diff --git a/g.txt b/g.txt\n\
                    --- a/g.txt\n\
                    +++ b/g.txt\n\
                    @@ -1,2 +1,2 @@\n\
                    -old2\n\
                    +new2\n\
                    \x20tail2\n\
                    -- \n\
                    2.40.0\n";
        let result = compact_diff(diff, 500);

        let removed: Vec<&str> = result.lines().filter(|l| l.starts_with('-')).collect();
        assert_eq!(
            removed,
            vec!["-old1", "-old2"],
            "only real deletions anchor, got:\n{}",
            result
        );
        assert!(
            !result.contains("Subject:"),
            "mbox envelope must stay out of the body, got:\n{}",
            result
        );
        assert!(result.contains("  +1 -1"), "tally counts real changes only, got:\n{}", result);
    }

    #[test]
    fn test_compact_diff_leading_context_is_adjacent_to_the_change() {
        // With `-U10` the first context lines sit ten lines above the change.
        // Emitting those would tell the reader that ctx3 precedes the deletion
        // when ctx10 does.
        let mut diff = String::from("diff --git a/f.rs b/f.rs\n@@ -1,11 +1,11 @@\n");
        for i in 1..=10 {
            diff.push_str(&format!(" ctx{}\n", i));
        }
        diff.push_str("-old\n+new\n");
        let result = compact_diff(&diff, 500);

        let ctx: Vec<&str> = result.lines().filter(|l| l.starts_with(" ctx")).collect();
        assert_eq!(
            ctx,
            vec![" ctx8", " ctx9", " ctx10"],
            "the last context lines, not the first, got:\n{}",
            result
        );
    }

    #[test]
    fn test_diff_header_path_keeps_spaces() {
        assert_eq!(
            diff_header_path("diff --git a/my file.txt b/my file.txt"),
            "my file.txt"
        );
        assert_eq!(diff_header_path("diff --cc my file.txt"), "my file.txt");
        assert_eq!(
            diff_header_path("diff --combined my file.txt"),
            "my file.txt"
        );
    }

    #[test]
    fn test_diff_header_path_handles_gits_quoted_paths() {
        // Under the default `core.quotepath`, git escapes a non-ASCII path and
        // wraps it in quotes, which removes the ` b/` separator the plain form
        // is split on. Without the quoted form handled, the fallback returned
        // the whole remainder — both paths — as the section header.
        assert_eq!(
            diff_header_path(r#"diff --git "a/Ã©tÃ©.txt" "b/Ã©tÃ©.txt""#),
            r"Ã©tÃ©.txt"
        );
        assert_eq!(
            diff_header_path(r#"diff --cc "Ã©tÃ©.txt""#),
            r"Ã©tÃ©.txt"
        );
        // A rename quotes each side on its own.
        assert_eq!(
            diff_header_path(r#"diff --git a/plain.txt "b/Ã©t.txt""#),
            r"Ã©t.txt"
        );
        assert_eq!(
            diff_header_path(r#"diff --git "a/Ã©t.txt" b/plain.txt"#),
            "plain.txt"
        );
    }

    #[test]
    fn test_diff_header_path_unescapes_gits_default_quoting() {
        // What git actually emits under the default `core.quotepath`: one octal
        // escape per byte, so the header has to be decoded rather than merely
        // unwrapped, or `rtk git diff | grep été` finds nothing.
        assert_eq!(
            diff_header_path(r#"diff --git "a/\303\251t\303\251.txt" "b/\303\251t\303\251.txt""#),
            "été.txt"
        );
        assert_eq!(
            diff_header_path(r#"diff --cc "\303\251t\303\251.txt""#),
            "été.txt"
        );
        assert_eq!(
            diff_header_path(r#"diff --git a/plain.txt "b/\303\251t.txt""#),
            "ét.txt"
        );
        // The single-character escapes, and a backslash standing for itself.
        assert_eq!(
            diff_header_path(r#"diff --cc "tab\there.txt""#),
            "tab\there.txt"
        );
        assert_eq!(
            diff_header_path(r#"diff --cc "quote\"here.txt""#),
            "quote\"here.txt"
        );
        assert_eq!(
            diff_header_path(r#"diff --cc "back\\slash.txt""#),
            r"back\slash.txt"
        );
    }

    #[test]
    fn test_emits_word_diff_detects_every_form() {
        for flag in [
            "--word-diff",
            "--word-diff=plain",
            "--word-diff=porcelain",
            "--word-diff-regex=.",
            "--color-words",
            "--color-words=.",
        ] {
            assert!(
                emits_word_diff(&[flag.to_string()]),
                "{} must pass through",
                flag
            );
        }
        assert!(!emits_word_diff(&["--stat".to_string()]));
        assert!(!emits_word_diff(&["-U10".to_string()]));
        assert!(!emits_word_diff(&[]));
    }

    #[test]
    fn test_emits_word_diff_honours_the_none_mode() {
        // `--word-diff=none` leaves an ordinary unified diff, which compacts
        // like any other. Treating it as a word diff passed the whole raw diff
        // through, so a defensive `--word-diff=none` lost every saving.
        assert!(!emits_word_diff(&["--word-diff=none".to_string()]));
        // Modes are last-one-wins, which is what `none` exists to do.
        assert!(!emits_word_diff(&[
            "--word-diff".to_string(),
            "--word-diff=none".to_string()
        ]));
        assert!(emits_word_diff(&[
            "--word-diff=none".to_string(),
            "--word-diff".to_string()
        ]));
        // `--color-words` takes a regex, so `none` there is a pattern.
        assert!(emits_word_diff(&["--color-words=none".to_string()]));
    }

    #[test]
    fn test_parse_hunk_header_reconciles_ranges_with_marker_columns() {
        // A well-formed header lists one range per marker column. When it does
        // not, only the columns can be charged. A missing range must not leave
        // an untracked parent holding the hunk open forever, and an extra one
        // must not sit at its declared count with no column to spend it.
        let h = parse_hunk_header("@@@ -1 +1 @@@").expect("two columns, one range");
        assert_eq!(h.prefix_width, 2);
        assert_eq!(h.parents, vec![1, usize::MAX]);

        let h = parse_hunk_header("@@@ -1 -1 -1 +0,0 @@@").expect("two columns, three ranges");
        assert_eq!(h.prefix_width, 2);
        assert_eq!(h.parents, vec![1, 1]);
    }

    #[test]
    fn test_compact_diff_extra_range_does_not_strand_a_hunk() {
        // With the third range untracked, `--x` left it at 1 forever, so the
        // hunk never closed and the mbox signature became its content.
        let out = compact_diff("diff --cc f\n@@@ -1 -1 -1 +0,0 @@@\n--x\n-- \n2.40.0\n", 100);
        assert!(out.contains("--x"), "got:\n{}", out);
        assert!(!out.contains("2.40.0"), "got:\n{}", out);
        assert!(!out.contains("-- "), "got:\n{}", out);
        // One line, removed from both parents, is one deletion.
        assert!(out.contains("+0 -1"), "got:\n{}", out);
    }

    #[test]
    fn test_compact_diff_missing_range_keeps_the_body() {
        // One range for two columns: the untracked parent gets `usize::MAX`, so
        // the hunk stays open to the next header rather than closing early and
        // dropping ` -lost`.
        let out = compact_diff("diff --cc f\n@@@ -1 +1 @@@\n +kept\n -lost\n", 100);
        assert!(out.contains(" +kept"), "got:\n{}", out);
        assert!(out.contains(" -lost"), "got:\n{}", out);
        assert!(out.contains("+1 -1"), "got:\n{}", out);
    }

    #[test]
    fn test_diff_header_path_splits_the_pair_at_its_midpoint() {
        // A file under a directory named `x b` puts the ` b/` separator inside
        // the path, so the first match is the wrong one.
        assert_eq!(diff_header_path("diff --git a/x b/y b/x b/y"), "x b/y");
        // `--no-prefix` and custom prefixes leave no ` b/` at all.
        assert_eq!(diff_header_path("diff --git x x"), "x");
        assert_eq!(
            diff_header_path("diff --git src/main.rs src/main.rs"),
            "src/main.rs"
        );
        // Prefixes are matched against each other, not by name, so a custom
        // `--dst-prefix` reads like any other pair.
        assert_eq!(diff_header_path("diff --git a/f.txt w/f.txt"), "f.txt");
        assert_eq!(diff_header_path("diff --git i/f.txt w/f.txt"), "f.txt");
        // A rename's halves disagree past their first component, so the ` b/`
        // split still names the destination.
        assert_eq!(diff_header_path("diff --git a/old.txt b/new.txt"), "new.txt");
    }

    #[test]
    fn test_diff_header_path_does_not_split_single_path_headers() {
        // `diff --cc` names one path. Splitting its remainder at the midpoint
        // would read a file called `dup dup` as the file `dup` named twice.
        assert_eq!(diff_header_path("diff --cc dup dup"), "dup dup");
        assert_eq!(diff_header_path("diff --combined dup dup"), "dup dup");
        assert_eq!(diff_header_path("diff --cc a/x b/x"), "a/x b/x");
    }

    #[test]
    fn test_parse_hunk_header_counts() {
        let h = parse_hunk_header("@@ -10,3 +10,4 @@ fn ctx() {").expect("unified header");
        assert_eq!((h.parents.as_slice(), h.new, h.prefix_width), (&[3][..], 4, 1));

        // Omitted counts mean one line.
        let h = parse_hunk_header("@@ -1 +1 @@").expect("single-line header");
        assert_eq!((h.parents.as_slice(), h.new), (&[1][..], 1));

        // A combined header lists one range per parent, in marker-column order.
        // Every one of them bounds the hunk body.
        let h = parse_hunk_header("@@@ -1,1 -1,4 +1,5 @@@").expect("combined header");
        assert_eq!(
            (h.parents.as_slice(), h.new, h.prefix_width),
            (&[1, 4][..], 5, 2)
        );

        assert!(parse_hunk_header("@ -1,1 +1,1 @").is_none());
        assert!(parse_hunk_header("-- ").is_none());
        assert!(parse_hunk_header("---").is_none());
    }

    #[test]
    fn test_compact_diff_non_ascii_body_line_without_a_marker_does_not_panic() {
        // `--word-diff` / `--color-words` emit body lines with no marker column,
        // so content lands where the markers are sliced. Slicing by byte index
        // split a leading multi-byte character and aborted the process.
        let out = compact_diff(
            "diff --git a/f.txt b/f.txt\n@@ -1,3 +1,3 @@\n-old\n+new\nécole ancienne ligne\n",
            100,
        );
        assert!(out.contains("-old"), "got:\n{}", out);
        assert!(out.contains("+new"), "got:\n{}", out);
        assert!(out.contains("école ancienne ligne"), "got:\n{}", out);
    }

    #[test]
    fn test_compact_diff_combined_hunk_ends_at_its_declared_length() {
        // Every parent's declared range bounds the body. Charging only the
        // first parent left `old` unable to converge on real conflict output,
        // so the hunk never closed by count and the mbox / signature / prose
        // guard did not apply to combined sections at all.
        let conflict = "diff --cc f.txt\n@@@ -1,1 -1,1 +1,5 @@@\n++<<<<<<<\n +main\n++=======\n+ side\n++>>>>>>>\n-- \ntrailing signature\n";
        let out = compact_diff(conflict, 100);
        assert!(!out.contains("trailing signature"), "got:\n{}", out);
        assert!(!out.contains("-- "), "got:\n{}", out);
        assert!(out.contains("+5 -0"), "got:\n{}", out);
    }

    #[test]
    fn test_compact_diff_combined_hunk_keeps_second_parent_removals() {
        // `-1,2 -1,4 +1,2`: two removals spend only the second parent's budget.
        // Closing on the first parent and the result alone dropped them with no
        // tally and no truncation note — a silent loss.
        let out = compact_diff(
            "diff --cc f.txt\n@@@ -1,2 -1,4 +1,2 @@@\n  a\n  b\n -x\n -y\n",
            100,
        );
        assert!(out.contains(" -x"), "got:\n{}", out);
        assert!(out.contains(" -y"), "got:\n{}", out);
        assert!(out.contains("+0 -2"), "got:\n{}", out);
    }

    #[test]
    fn test_compact_diff_flushes_context_from_a_hunk_with_no_change_line() {
        // The buffer drained only on the first change line, so a hunk that ends
        // without one rendered as a bare header with nothing under it.
        let out = compact_diff(
            "diff --git a/g.txt b/g.txt\n@@ -1,3 +1,3 @@\n ctx1\n ctx2\n ctx3\n",
            100,
        );
        assert!(out.contains(" ctx1"), "got:\n{}", out);
        assert!(out.contains(" ctx3"), "got:\n{}", out);
    }

    #[test]
    fn test_compact_diff_leading_context_does_not_displace_change_lines() {
        // Leading context is exempt from `max_lines`, so the same number of
        // change lines survives whether or not the hunks open with context.
        let build = |with_context: bool| {
            let mut diff = String::new();
            for f in 0..30 {
                diff.push_str(&format!("diff --git a/f{}.rs b/f{}.rs\n", f, f));
                diff.push_str("@@ -1,20 +1,20 @@\n");
                if with_context {
                    for c in 0..3 {
                        diff.push_str(&format!(" ctx{}_{}\n", f, c));
                    }
                }
                for i in 0..12 {
                    diff.push_str(&format!("-del{}_{}\n", f, i));
                }
            }
            diff
        };
        let count_changes =
            |out: &str| out.lines().filter(|l| l.starts_with("-del")).count();

        let without = compact_diff(&build(false), 500);
        let with = compact_diff(&build(true), 500);
        assert_eq!(
            count_changes(&with),
            count_changes(&without),
            "leading context displaced change lines:\n{}",
            with
        );
    }

    #[test]
    fn test_compact_diff_leading_context_is_capped_across_the_diff() {
        // The diff-wide cap is what bounds the exemption: without it, a diff of
        // many small hunks would spend three exempt lines on each one.
        let mut diff = String::new();
        for f in 0..200 {
            diff.push_str(&format!("diff --git a/f{}.rs b/f{}.rs\n", f, f));
            diff.push_str("@@ -1,4 +1,4 @@\n");
            for c in 0..3 {
                diff.push_str(&format!(" ctx{}_{}\n", f, c));
            }
            diff.push_str(&format!("-del{}\n", f));
        }
        let result = compact_diff(&diff, 500);

        let ctx = result.lines().filter(|l| l.starts_with(" ctx")).count();
        assert!(
            ctx <= 50,
            "leading context must stay within max_lines / 10, got {} in:\n{}",
            ctx,
            result
        );
    }

    #[test]
    fn test_hunk_truncation_note_counts_one_as_singular() {
        assert_eq!(
            hunk_truncation_note(1, 0).as_deref(),
            Some("  ... (1 deletion truncated)")
        );
        assert_eq!(
            hunk_truncation_note(0, 1).as_deref(),
            Some("  ... (1 addition truncated)")
        );
        assert_eq!(
            hunk_truncation_note(1, 2).as_deref(),
            Some("  ... (1 deletion, 2 additions truncated)")
        );
        assert_eq!(hunk_truncation_note(0, 0), None);
    }

    #[test]
    fn test_compact_diff_truncation_note_splits_by_sign() {
        // An anchored `^-` audit needs to know how many deletions it did not
        // see, which a merged "N lines truncated" cannot tell it.
        let mut diff = String::from("diff --git a/f.rs b/f.rs\n@@ -1,160 +1,160 @@\n");
        for i in 0..80 {
            diff.push_str(&format!("-del{}\n", i));
            diff.push_str(&format!("+add{}\n", i));
        }
        let result = compact_diff(&diff, 1000);

        assert!(
            result.contains("  ... (30 deletions, 30 additions truncated)"),
            "expected per-sign truncation note, got:\n{}",
            result
        );
    }

    #[test]
    fn test_compact_diff_preserves_full_hunk_header_context() {
        let diff = r#"diff --git a/foo.rs b/foo.rs
--- a/foo.rs
+++ b/foo.rs
@@ -10,3 +10,4 @@ fn important_context() {
 fn main() {
+    println!("hello");
 }
"#;
        let result = compact_diff(diff, 100);
        assert!(
            result.contains("@@ -10,3 +10,4 @@ fn important_context() {"),
            "Expected full hunk header with trailing context, got:\n{}",
            result
        );
    }

    #[test]
    fn test_compact_diff_increased_hunk_limit() {
        // Build a hunk with 25 changed lines — should NOT be truncated with limit 30
        let mut diff =
            "diff --git a/big.rs b/big.rs\n--- a/big.rs\n+++ b/big.rs\n@@ -1,25 +1,25 @@\n"
                .to_string();
        for i in 1..=25 {
            diff.push_str(&format!("+line{}\n", i));
        }
        let result = compact_diff(&diff, 500);
        assert!(
            !result.contains("... (truncated)"),
            "25 lines should not be truncated with max_hunk_lines=30"
        );
        assert!(result.contains("+line25"));
    }

    #[test]
    fn test_compact_diff_increased_total_limit() {
        // Build a diff with 150 output result lines across multiple files — should NOT be cut at 100
        let mut diff = String::new();
        for f in 1..=5 {
            diff.push_str(&format!("diff --git a/file{f}.rs b/file{f}.rs\n--- a/file{f}.rs\n+++ b/file{f}.rs\n@@ -1,20 +1,20 @@\n"));
            for i in 1..=20 {
                diff.push_str(&format!("+line{f}_{i}\n"));
            }
        }
        let result = compact_diff(&diff, 500);
        assert!(
            !result.contains("more changes truncated"),
            "5 files × 20 lines should not exceed max_lines=500"
        );
    }

    #[test]
    fn test_is_blob_show_arg() {
        assert!(is_blob_show_arg("develop:modules/pairs_backtest.py"));
        assert!(is_blob_show_arg("HEAD:src/main.rs"));
        assert!(!is_blob_show_arg("--pretty=format:%h"));
        assert!(!is_blob_show_arg("--format=short"));
        assert!(!is_blob_show_arg("HEAD"));
    }

    #[test]
    fn test_filter_branch_output() {
        let output = "* main\n  feature/auth\n  fix/bug-123\n  remotes/origin/HEAD -> origin/main\n  remotes/origin/main\n  remotes/origin/feature/auth\n  remotes/origin/release/v2\n";
        let result = filter_branch_output(output);
        assert!(result.contains("* main"));
        assert!(result.contains("feature/auth"));
        assert!(result.contains("fix/bug-123"));
        // remote-only should show release/v2 but not main or feature/auth (already local)
        assert!(result.contains("remote-only"));
        assert!(result.contains("release/v2"));
    }

    #[test]
    fn test_filter_branch_no_remotes() {
        let output = "* main\n  develop\n";
        let result = filter_branch_output(output);
        assert!(result.contains("* main"));
        assert!(result.contains("develop"));
        assert!(!result.contains("remote-only"));
    }

    #[test]
    fn test_filter_branch_multi_remote() {
        let output = "* main\n  develop\n  remotes/origin/HEAD -> origin/main\n  remotes/origin/main\n  remotes/origin/feature-x\n  remotes/upstream/main\n  remotes/upstream/release-v3\n  remotes/fork/main\n  remotes/fork/experiment\n";
        let result = filter_branch_output(output);
        assert!(result.contains("* main"));
        assert!(result.contains("develop"));
        assert!(
            result.contains("feature-x"),
            "origin branch shown: {}",
            result
        );
        assert!(
            result.contains("release-v3"),
            "upstream branch shown: {}",
            result
        );
        assert!(
            result.contains("experiment"),
            "fork branch shown: {}",
            result
        );
        assert!(
            !result.contains("remotes/"),
            "remote prefix stripped: {}",
            result
        );
        let main_count = result.matches("main").count();
        assert!(
            main_count <= 2,
            "main deduplicated across remotes (found {} occurrences): {}",
            main_count,
            result
        );
    }

    #[test]
    fn test_filter_stash_list() {
        let output =
            "stash@{0}: WIP on main: abc1234 fix login\nstash@{1}: On feature: def5678 wip\n";
        let result = filter_stash_list(output);
        assert!(result.contains("stash@{0}: abc1234 fix login"));
        assert!(result.contains("stash@{1}: def5678 wip"));
    }

    #[test]
    fn test_parse_stash_stat_strips_decorations() {
        let raw = " del.md   |   2 --\n keep.md  |   5 ++++-\n logo.bin | Bin 0 -> 1024 bytes\n \
                   new.rs   |  40 ++++++++\n 4 files changed, 44 insertions(+), 3 deletions(-)\n";
        let (files, summary) = parse_stash_stat(raw);
        assert_eq!(
            files,
            vec![
                "del.md 2 -",
                "keep.md 5 +-",
                "logo.bin (binary)",
                "new.rs 40 +"
            ]
        );
        assert_eq!(summary, "4 files changed, 44 insertions(+), 3 deletions(-)");
    }

    #[test]
    fn test_parse_stash_stat_collapsed_bar() {
        let (files, _) = parse_stash_stat(" .claude/CLAUDE.md | 234 +-\n");
        assert_eq!(files, vec![".claude/CLAUDE.md 234 +-"]);
    }

    #[test]
    fn test_compact_stash_stat_passthrough_numstat() {
        let raw = "0\t1\tdel.md\n3\t2\tkeep.md\n1\t0\tn1.rs\n";
        assert_eq!(
            compact_stash_stat(raw),
            "0\t1\tdel.md\n3\t2\tkeep.md\n1\t0\tn1.rs"
        );
    }

    #[test]
    fn test_compact_stash_stat_passthrough_name_only() {
        let raw = "del.md\nkeep.md\nn1.rs\n";
        assert_eq!(compact_stash_stat(raw), "del.md\nkeep.md\nn1.rs");
    }

    #[test]
    fn test_compress_stat_summary_variants() {
        assert_eq!(
            compress_stat_summary("4 files changed, 60 insertions(+), 313 deletions(-)"),
            "4 changed 60 + 313 -"
        );
        assert_eq!(
            compress_stat_summary("1 file changed, 1 insertion(+)"),
            "1 changed 1 +"
        );
        assert_eq!(
            compress_stat_summary("1 file changed, 1 deletion(-)"),
            "1 changed 1 -"
        );
        assert_eq!(
            compress_stat_summary("2 files changed, 4 insertions(+), 1 deletion(-)"),
            "2 changed 4 + 1 -"
        );
    }

    #[test]
    fn test_compact_stash_stat_compresses_summary() {
        let raw = " a.txt | 2 ++\n 1 file changed, 2 insertions(+)\n";
        assert_eq!(compact_stash_stat(raw), "a.txt 2 +\n1 changed 2 +");
    }

    #[test]
    fn test_parse_stash_stat_last_pipe_is_separator() {
        let (files, _) = parse_stash_stat(" weird|name.txt | 3 +++\n");
        assert_eq!(files, vec!["weird|name.txt 3 +"]);
    }

    #[test]
    fn test_parse_stash_stat_strips_ansi() {
        let (files, _) = parse_stash_stat(" a.txt | 2 \x1b[32m++\x1b[m\n");
        assert_eq!(files, vec!["a.txt 2 +"]);
    }

    #[test]
    fn test_parse_stash_stat_empty() {
        let (files, summary) = parse_stash_stat("");
        assert!(files.is_empty());
        assert!(summary.is_empty());
    }

    #[test]
    fn test_parse_stash_stat_unicode_and_malformed_never_panic() {
        let _ = parse_stash_stat("not a diffstat at all");
        let _ = parse_stash_stat("| | |");
        let (files, _) = parse_stash_stat(" 日本語.md | 5 +++--\n");
        assert_eq!(files, vec!["日本語.md 5 +-"]);
    }

    #[test]
    fn test_parse_stash_stat_savings() {
        use crate::core::tracking::estimate_tokens;
        let raw = " CONTRIBUTING.md | 305 \
                   ----------------------------------------------------------\n \
                   README.md       |  28 ++++--\n logo.bin        | Bin 0 -> 2048 bytes\n \
                   newfeature.rs   |  40 ++++++++\n \
                   4 files changed, 60 insertions(+), 313 deletions(-)\n";
        let (files, summary) = parse_stash_stat(raw);
        let compact = format!("{}\n{}", files.join("\n"), summary);
        let savings =
            100.0 - (estimate_tokens(&compact) as f64 / estimate_tokens(raw) as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "expected >=40% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_run_stash_list_propagates_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let global = vec!["-C".to_string(), dir.path().to_string_lossy().into_owned()];
        let code = run_stash(Some("list"), &[], 0, &global).expect("run_stash list");
        assert_ne!(code, 0, "git stash list failure must propagate");
    }

    #[test]
    fn test_run_stash_show_propagates_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let global = vec!["-C".to_string(), dir.path().to_string_lossy().into_owned()];
        let code = run_stash(Some("show"), &[], 0, &global).expect("run_stash show");
        assert_ne!(code, 0, "git stash show failure must propagate");
    }

    #[test]
    fn test_filter_worktree_list() {
        let output =
            "/home/user/project  abc1234 [main]\n/home/user/worktrees/feat  def5678 [feature]\n";
        let result = filter_worktree_list(output);
        assert!(result.contains("abc1234"));
        assert!(result.contains("[main]"));
        assert!(result.contains("[feature]"));
    }

    #[test]
    fn test_run_worktree_list_propagates_failure() {
        // #2497: `git worktree list` outside a repo exits non-zero; rtk must not
        // report success (empty output + exit 0).
        let dir = tempfile::tempdir().expect("tempdir");
        let global = vec!["-C".to_string(), dir.path().to_string_lossy().into_owned()];
        let code = run_worktree(&[], 0, &global).expect("run_worktree");
        assert_ne!(code, 0, "git worktree list failure must propagate");
    }

    #[test]
    fn test_format_status_output_clean() {
        let porcelain = "## main...origin/main\n";
        let result = format_status_output(porcelain);
        assert_eq!(result, "* main...origin/main\nclean — nothing to commit");
    }

    #[test]
    fn test_extract_state_header_clean_returns_none() {
        let raw = "On branch main\nYour branch is up to date with 'origin/main'.\n\nnothing to commit, working tree clean\n";
        assert_eq!(extract_state_header(raw), None);
    }

    #[test]
    fn test_extract_state_header_no_state_with_changes_returns_none() {
        let raw = "On branch main\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n\tmodified:   src/main.rs\n\nno changes added to commit\n";
        assert_eq!(extract_state_header(raw), None);
    }

    #[test]
    fn test_extract_state_header_editing_while_rebasing() {
        let raw = "On branch feature\n\ninteractive rebase in progress; onto abc1234\nLast command done (1 command done):\n   edit abc123 some message\nNo commands remaining.\nYou are currently editing a commit while rebasing branch 'feature' on 'abc1234'.\n  (use \"git commit --amend\" to amend the current commit)\n  (use \"git rebase --continue\" once you are satisfied with your changes)\n\nnothing to commit, working tree clean\n";
        let out = extract_state_header(raw).expect("state expected");
        assert_eq!(out, "rebase in progress");
    }

    #[test]
    fn test_extract_state_header_merge_unresolved() {
        let raw = "On branch main\nYou have unmerged paths.\n  (fix conflicts and run \"git commit\")\n  (use \"git merge --abort\" to abort the merge)\n\nUnmerged paths:\n\tboth modified:   src/main.rs\n";
        let out = extract_state_header(raw).expect("state expected");
        assert_eq!(out, "merge in progress. unresolved conflicts");
    }

    #[test]
    fn test_extract_state_header_cherry_pick() {
        let raw = "On branch main\n\nYou are currently cherry-picking commit abc1234.\n  (fix conflicts and run \"git cherry-pick --continue\")\n  (use \"git cherry-pick --abort\" to cancel the cherry-pick operation)\n\nnothing to commit, working tree clean\n";
        let out = extract_state_header(raw).expect("state expected");
        assert_eq!(out, "cherry-pick in progress");
    }

    #[test]
    fn test_extract_state_header_bisect() {
        let raw = "On branch main\n\nYou are currently bisecting, started from branch 'main'.\n  (use \"git bisect reset\" to get back to the original branch)\n\nnothing to commit, working tree clean\n";
        let out = extract_state_header(raw).expect("state expected");
        assert_eq!(out, "bisect in progress");
    }

    #[test]
    fn test_extract_state_header_revert() {
        let raw = "On branch main\n\nYou are currently reverting commit abc1234.\n  (fix conflicts and run \"git revert --continue\")\n  (use \"git revert --abort\" to cancel the revert operation)\n\nnothing to commit, working tree clean\n";
        let out = extract_state_header(raw).expect("state expected");
        assert_eq!(out, "revert in progress");
    }

    #[test]
    fn test_extract_state_header_merge_in_middle() {
        let raw = "On branch main\n\nAll conflicts fixed but you are still merging.\n  (use \"git commit\" to conclude merge)\n\nChanges to be committed:\n\tmodified:   src/main.rs\n";
        let out = extract_state_header(raw).expect("state expected");
        assert_eq!(out, "merge in progress. no conflicts");
    }

    #[test]
    fn test_extract_state_header_am_session() {
        let raw = "On branch main\n\nYou are in the middle of an am session.\n  (use \"git am --continue\" to continue)\n  (use \"git am --abort\" to restore the original branch)\n\nnothing to commit, working tree clean\n";
        let out = extract_state_header(raw).expect("state expected");
        assert_eq!(out, "am session in progress");
    }

    #[test]
    fn test_extract_state_header_sparse_checkout() {
        let raw = "On branch main\n\nYou are in a sparse checkout with 17% of tracked files present.\n\nnothing to commit, working tree clean\n";
        let out = extract_state_header(raw).expect("state expected");
        assert_eq!(out, "sparse checkout enabled");
    }

    #[test]
    fn test_format_status_output_preserves_nested_untracked_paths() {
        let porcelain = "## main\n?? tmp/c.txt\n?? tmp/nested/d.txt\n";
        let result = format_status_output(porcelain);
        assert!(result.contains("* main"));
        assert!(result.contains("?? tmp/c.txt"));
        assert!(result.contains("?? tmp/nested/d.txt"));
        assert!(
            result.lines().all(|line| line != "?? tmp/"),
            "Nested untracked files must not collapse back to a directory marker:\n{}",
            result
        );
    }

    #[test]
    fn test_format_status_output_mixed_changes() {
        let porcelain = r#"## main
M  staged.rs
 M modified.rs
A  added.rs
?? untracked.txt
"#;
        let result = format_status_output(porcelain);
        assert!(result.contains("* main"));
        assert!(result.contains("M  staged.rs"));
        assert!(result.contains(" M modified.rs"));
        assert!(result.contains("A  added.rs"));
        assert!(result.contains("?? untracked.txt"));
        assert!(!result.contains("Staged"));
        assert!(!result.contains("Modified"));
        assert!(!result.contains("Untracked"));
    }

    #[test]
    fn test_format_status_output_preserves_rename_and_conflict_lines() {
        let porcelain = "## main\nR  old.rs -> new.rs\nUU conflict.rs\nMM mixed.rs\n";
        let result = format_status_output(porcelain);
        assert!(result.contains("* main"));
        assert!(result.contains("R  old.rs -> new.rs"));
        assert!(result.contains("UU conflict.rs"));
        assert!(result.contains("MM mixed.rs"));
        assert!(!result.contains("conflicts:"));
    }

    #[test]
    fn test_run_passthrough_accepts_args() {
        // Test that run_passthrough compiles and has correct signature
        let _args: Vec<OsString> = vec![OsString::from("tag"), OsString::from("--list")];
        // Compile-time verification that the function exists with correct signature
    }

    #[test]
    fn test_filter_log_output() {
        let output = "abc1234 This is a commit message (2 days ago) <author>\n\n---END---\ndef5678 Another commit (1 week ago) <other>\n\n---END---\n";
        let result = filter_log_output(output, 10, false, false);
        assert!(result.contains("abc1234"));
        assert!(result.contains("def5678"));
        assert_eq!(result.lines().count(), 2);
    }

    #[test]
    fn test_filter_log_output_with_body() {
        // Commit with body: first non-trailer body line should appear indented
        let output = "abc1234 feat: add feature (2 days ago) <author>\nBREAKING CHANGE: removed old API\nSigned-off-by: Author <a@b.com>\n---END---\ndef5678 fix: typo (1 day ago) <other>\n\n---END---\n";
        let result = filter_log_output(output, 10, false, false);
        assert!(result.contains("abc1234"));
        assert!(result.contains("BREAKING CHANGE: removed old API"));
        assert!(!result.contains("Signed-off-by:"));
        // def5678 has no body — just header
        assert!(result.contains("def5678"));
        // 3 lines: header1, body1 indented, header2
        assert_eq!(result.lines().count(), 3);
    }

    #[test]
    fn test_filter_log_output_skips_trailers() {
        // Body with only trailers should not produce a body line
        let output = "abc1234 chore: bump (1 day ago) <bot>\nSigned-off-by: Bot <bot@ci>\nCo-authored-by: Human <h@b>\n---END---\n";
        let result = filter_log_output(output, 10, false, false);
        assert!(result.contains("abc1234"));
        assert!(!result.contains("Signed-off-by:"));
        assert!(!result.contains("Co-authored-by:"));
        assert_eq!(result.lines().count(), 1);
    }

    #[test]
    fn test_filter_log_output_truncate_long() {
        let long_line = "abc1234 ".to_string() + &"x".repeat(100) + " (2 days ago) <author>";
        let result = filter_log_output(&long_line, 10, false, false);
        assert!(result.chars().count() < long_line.chars().count());
        assert!(result.contains("..."));
        assert!(result.chars().count() <= 80);
    }

    #[test]
    fn test_filter_log_output_cap_lines() {
        let output = (0..20)
            .map(|i| format!("hash{} message {} (1 day ago) <author>\n\n---END---", i, i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = filter_log_output(&output, 5, false, false);
        assert_eq!(result.lines().count(), 5);
    }

    #[test]
    fn test_filter_log_output_user_limit_no_cap() {
        // When user explicitly passes -N, all N lines should be returned (no re-truncation)
        let output = (0..20)
            .map(|i| format!("hash{} message {} (1 day ago) <author>\n\n---END---", i, i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = filter_log_output(&output, 20, true, false);
        assert_eq!(
            result.lines().count(),
            20,
            "User's -20 should return all 20 lines"
        );
    }

    #[test]
    fn test_filter_log_output_user_limit_wider_truncation() {
        // When user explicitly passes -N, lines up to 120 chars should NOT be truncated
        let line_90_chars = format!("abc1234 {} (2 days ago) <author>", "x".repeat(60));
        assert!(line_90_chars.chars().count() > 80);
        assert!(line_90_chars.chars().count() < 120);

        let result_default = filter_log_output(&line_90_chars, 10, false, false);
        let result_user = filter_log_output(&line_90_chars, 10, true, false);

        // Default truncates at 80 chars
        assert!(
            result_default.contains("..."),
            "Default should truncate at 80 chars"
        );
        // User-set limit uses wider threshold (120 chars)
        assert!(
            !result_user.contains("..."),
            "User limit should not truncate 90-char line"
        );
    }

    #[test]
    fn test_parse_user_limit_combined() {
        let args: Vec<String> = vec!["-20".into()];
        assert_eq!(parse_user_limit(&args), Some(20));
    }

    #[test]
    fn test_parse_user_limit_n_space() {
        let args: Vec<String> = vec!["-n".into(), "15".into()];
        assert_eq!(parse_user_limit(&args), Some(15));
    }

    #[test]
    fn test_parse_user_limit_max_count_eq() {
        let args: Vec<String> = vec!["--max-count=30".into()];
        assert_eq!(parse_user_limit(&args), Some(30));
    }

    #[test]
    fn test_parse_user_limit_max_count_space() {
        let args: Vec<String> = vec!["--max-count".into(), "25".into()];
        assert_eq!(parse_user_limit(&args), Some(25));
    }

    #[test]
    fn test_parse_user_limit_none() {
        let args: Vec<String> = vec!["--oneline".into()];
        assert_eq!(parse_user_limit(&args), None);
    }

    #[test]
    fn test_patch_log_flags_request_raw_output() {
        for flag in [
            "-p",
            "-u",
            "--patch",
            "--patch-with-raw",
            "--patch-with-stat",
        ] {
            let args = vec![flag.to_string()];
            assert!(requests_raw_log_output(&args), "{flag} should pass through");
        }
    }

    #[test]
    fn test_patch_flag_after_pathspec_separator_is_ignored() {
        // `git log -- -p` means "show history for a path literally named -p",
        // not "show patches" — the flag lookalike appears after `--`.
        let args = vec!["--".to_string(), "-p".to_string()];
        assert!(
            !requests_raw_log_output(&args),
            "-p after -- is a pathspec, not a patch flag, and should stay on the filtered path"
        );
    }

    #[test]
    fn test_non_patch_log_flags_remain_filtered() {
        for flag in ["--no-patch", "--oneline", "--format=%H"] {
            let args = vec![flag.to_string()];
            assert!(
                !requests_raw_log_output(&args),
                "{flag} should remain on the filtered log path"
            );
        }
    }

    #[test]
    fn test_diff_shape_flags_request_raw_output() {
        // These change the shape of git's raw output (diffstat, name lists)
        // the same way -p does — RTK's injected --pretty=format markers
        // can't coexist with them, so they must stay on the raw path too.
        for flag in [
            "--dirstat",
            "--dirstat=files",
            "--name-only",
            "--name-status",
            "--numstat",
            "--raw",
            "--shortstat",
            "--stat",
            "--stat=80",
            "--summary",
        ] {
            let args = vec![flag.to_string()];
            assert!(
                requests_raw_log_output(&args),
                "{flag} changes output shape and should request raw output"
            );
        }
    }

    #[test]
    fn test_diff_shape_flag_as_value_of_grep_is_not_misdetected() {
        // `git log --grep --stat` searches for the literal string
        // "--stat"; git consumes it as --grep's value, not the --stat flag.
        let args = vec!["--grep".to_string(), "--stat".to_string()];
        assert!(
            !requests_raw_log_output(&args),
            "--stat as the value of --grep should stay on the filtered path"
        );
    }

    #[test]
    fn test_patch_flag_as_value_of_grep_is_not_misdetected() {
        // `git log --grep -p` searches commit messages for the literal
        // string "-p"; git does not treat it as the patch flag.
        for opt in [
            "--author",
            "--committer",
            "--diff-algorithm",
            "--diff-filter",
            "--grep",
            "-G",
            "-S",
        ] {
            let args = vec![opt.to_string(), "-p".to_string()];
            assert!(
                !requests_raw_log_output(&args),
                "-p as the value of {opt} should stay on the filtered path"
            );
        }
    }

    #[test]
    fn test_patch_flag_still_detected_after_value_taking_option() {
        // The value-taking option consumes only its own value token;
        // a genuine -p later in the args still triggers the raw path.
        let args = vec!["--grep".to_string(), "fix".to_string(), "-p".to_string()];
        assert!(
            requests_raw_log_output(&args),
            "a real -p after --grep's value should still request raw output"
        );
    }

    #[test]
    fn test_optional_value_options_do_not_consume_next_token() {
        // These options only take an attached value (-U3, --unified=3,
        // --expand-tabs=4, --max-parents=2); a bare separate token after
        // them is not their value, so it must not be swallowed. Confirmed
        // against git 2.53.0: e.g. `git log --expand-tabs 4` fails with
        // "fatal: ambiguous argument '4'" rather than treating 4 as the
        // option's value.
        for opt in [
            "-U",
            "--unified",
            "--expand-tabs",
            "--max-parents",
            "--min-parents",
        ] {
            let args = vec![opt.to_string(), "-p".to_string()];
            assert!(
                requests_raw_log_output(&args),
                "a real -p after {opt} should still request raw output"
            );
        }
    }

    #[test]
    fn test_real_flag_args_drops_value_taking_option_values() {
        // `--grep`'s value is not itself a flag and must not appear in the
        // filtered set, even when it looks like -N, --pretty, or --merges.
        let args = vec!["--grep".to_string(), "-5".to_string()];
        assert_eq!(real_flag_args(&args), vec!["--grep"]);
    }

    #[test]
    fn test_real_flag_args_keeps_limit_flag_drops_its_value() {
        let args = vec!["-n".to_string(), "15".to_string()];
        assert_eq!(real_flag_args(&args), vec!["-n"]);

        let args = vec!["--max-count".to_string(), "25".to_string()];
        assert_eq!(real_flag_args(&args), vec!["--max-count"]);
    }

    #[test]
    fn test_real_flag_args_keeps_genuine_flags() {
        let args = vec!["--grep".to_string(), "fix".to_string(), "--oneline".to_string()];
        assert_eq!(real_flag_args(&args), vec!["--grep", "--oneline"]);
    }

    #[test]
    fn test_grep_value_looking_like_limit_flag_is_not_misdetected() {
        // `git log --grep -5` searches commit messages for the literal
        // string "-5"; it is not a request to limit output to 5 commits.
        let args = vec!["--grep".to_string(), "-5".to_string()];
        assert!(
            !real_flag_args(&args)
                .iter()
                .any(|arg| arg.starts_with('-') && arg.chars().nth(1).is_some_and(|c| c.is_ascii_digit())),
            "-5 as the value of --grep should not be seen as a limit flag"
        );
        assert_eq!(
            parse_user_limit(&args),
            None,
            "-5 as the value of --grep should not be parsed as a limit"
        );
    }

    #[test]
    fn test_grep_value_looking_like_format_flag_is_not_misdetected() {
        // `git log --grep --pretty` searches for the literal string
        // "--pretty"; git consumes it as --grep's value, not a format flag.
        let args = vec!["--grep".to_string(), "--pretty".to_string()];
        assert!(
            !real_flag_args(&args)
                .iter()
                .any(|arg| arg.starts_with("--pretty")),
            "--pretty as the value of --grep should not be seen as a format flag"
        );
    }

    #[test]
    fn test_grep_value_looking_like_merges_flag_is_not_misdetected() {
        // `git log --grep --merges` searches for the literal string
        // "--merges"; git consumes it as --grep's value, not --merges.
        let args = vec!["--grep".to_string(), "--merges".to_string()];
        assert!(
            !real_flag_args(&args).contains(&"--merges"),
            "--merges as the value of --grep should not be seen as the --merges flag"
        );
    }

    #[test]
    fn test_parse_user_limit_skips_foreign_option_values() {
        // A real limit later in the args is still found after a
        // value-taking option's value is skipped.
        let args = vec![
            "--grep".to_string(),
            "-5".to_string(),
            "-20".to_string(),
        ];
        assert_eq!(parse_user_limit(&args), Some(20));
    }

    #[test]
    fn test_log_arg_tokens_stop_at_pathspec_separator() {
        // `git log -- -5` means "history for the path literally named -5",
        // not a limit flag — tokens after `--` must be ignored entirely.
        let args = vec!["--".to_string(), "-5".to_string()];
        assert!(
            real_flag_args(&args).is_empty(),
            "-5 after -- is a pathspec, not a flag"
        );
        assert_eq!(
            parse_user_limit(&args),
            None,
            "-5 after -- should not be parsed as a limit"
        );
    }

    #[test]
    fn test_filter_log_output_token_savings() {
        fn count_tokens(text: &str) -> usize {
            text.split_whitespace().count()
        }
        // Simulate verbose git log output (default format with full metadata)
        let input = (0..20)
            .map(|i| {
                format!(
                    "commit abc123{:02x}\nAuthor: User Name <user@example.com>\nDate:   Mon Mar 10 10:00:00 2026 +0000\n\n    fix: commit message number {}\n\n    Extended body with details about the change.\n",
                    i, i
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let output = filter_log_output(&input, 10, false, false);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(&input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected ≥60% token savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_status_with_args() {
        let output = r#"On branch main
Your branch is up to date with 'origin/main'.

Changes not staged for commit:
  (use "git add <file>..." to update what will be committed)
  (use "git restore <file>..." to discard changes in working directory)
	modified:   src/main.rs

no changes added to commit (use "git add" and/or "git commit -a")
"#;
        let result = filter_status_with_args(output);
        eprintln!("Result:\n{}", result);
        assert!(result.contains("On branch main"));
        assert!(result.contains("modified:   src/main.rs"));
        assert!(
            !result.contains("(use \"git"),
            "Result should not contain git hints"
        );
    }

    #[test]
    fn test_filter_status_with_args_clean() {
        let output = "nothing to commit, working tree clean\n";
        let result = filter_status_with_args(output);
        assert!(result.contains("nothing to commit"));
    }

    #[test]
    fn test_filter_log_output_multibyte() {
        // Thai characters: each is 3 bytes. A line with >80 bytes but few chars
        let thai_msg = format!("abc1234 {} (2 days ago) <author>", "ก".repeat(30));
        let result = filter_log_output(&thai_msg, 10, false, false);
        // Should not panic
        assert!(result.contains("abc1234"));
        // The line has 30 Thai chars + other text, so > 80 chars total
        // truncate_line now counts chars, not bytes
        // 30 Thai + ~33 other = 63 chars < 80 threshold, so no truncation
        assert!(result.contains("abc1234"));
    }

    #[test]
    fn test_filter_log_output_emoji() {
        let emoji_msg = "abc1234 🎉🎊🎈🎁🎂🎄🎃🎆🎇✨🎉🎊🎈🎁🎂🎄🎃🎆🎇✨ (1 day ago) <user>";
        let result = filter_log_output(emoji_msg, 10, false, false);
        // Should not panic
        // 20 emoji + ~30 other chars = ~50 chars < 80, no truncation needed
        assert!(result.contains("abc1234"));
    }

    #[test]
    fn test_format_status_output_thai_filename() {
        let porcelain = "## main\n M สวัสดี.txt\n?? ทดสอบ.rs\n";
        let result = format_status_output(porcelain);
        // Should not panic
        assert!(result.contains("* main"));
        assert!(result.contains("สวัสดี.txt"));
        assert!(result.contains("ทดสอบ.rs"));
    }

    #[test]
    fn test_format_status_output_emoji_filename() {
        let porcelain = "## main\nA  🎉-party.txt\n M 日本語ファイル.rs\n";
        let result = format_status_output(porcelain);
        assert!(result.contains("* main"));
    }

    // --- commit output parsing ---

    #[test]
    fn test_parse_commit_output_normal() {
        let line = "[main abc1234def] add feature";
        assert_eq!(parse_commit_output(line), "ok abc1234");
    }

    #[test]
    fn test_parse_commit_output_root_commit() {
        let line = "[main (root-commit) abc1234def] initial commit";
        assert_eq!(parse_commit_output(line), "ok abc1234");
    }

    /// Regression test: multibyte branch name must not panic (was byte-slicing before fix)
    #[test]
    fn test_parse_commit_output_multibyte_branch() {
        let line = "[分支名 abc1234def] 提交消息";
        assert_eq!(parse_commit_output(line), "ok abc1234");
    }

    /// Regression test: Thai branch name (3 bytes per char)
    #[test]
    fn test_parse_commit_output_thai_branch() {
        let line = "[สาขา abc1234def] commit message";
        assert_eq!(parse_commit_output(line), "ok abc1234");
    }

    /// Regression: git prints hook output before its own summary. A first line
    /// that opens with a multi-byte character and contains ']' used to panic on
    /// `line[1..]` ("byte index 1 is not a char boundary").
    #[test]
    fn test_parse_commit_output_multibyte_prefix_does_not_panic() {
        assert_eq!(parse_commit_output("✅ lint passed]"), "ok");
        assert_eq!(parse_commit_output("→ hook] done"), "ok");
    }

    /// The same shape as above, but with a real summary after the hook text —
    /// the hash must still be found via the bracket pair.
    #[test]
    fn test_parse_commit_output_after_multibyte_hook_prefix() {
        assert_eq!(
            parse_commit_output("✅ [main abc1234def] add feature"),
            "ok abc1234"
        );
    }

    /// A U+FFFD from lossily decoded output is itself multi-byte.
    #[test]
    fn test_parse_commit_output_replacement_char_prefix() {
        assert_eq!(parse_commit_output("\u{FFFD}oops]"), "ok");
    }

    /// A closing bracket before any opening one must not slice backwards.
    #[test]
    fn test_parse_commit_output_close_before_open() {
        assert_eq!(parse_commit_output("] stray [main abc1234def]"), "ok");
    }

    #[test]
    fn test_parse_commit_output_no_bracket() {
        let line = "some other output";
        assert_eq!(parse_commit_output(line), "ok");
    }

    #[test]
    fn test_parse_commit_output_short_hash() {
        // Hash shorter than 7 chars — treat as "ok" (no hash shown)
        let line = "[main abc12] message";
        assert_eq!(parse_commit_output(line), "ok");
    }

    #[test]
    fn test_parse_commit_output_empty() {
        assert_eq!(parse_commit_output(""), "ok");
    }

    // --- commit outcome classification (issue #2494) ---

    #[test]
    fn test_classify_commit_success_extracts_hash() {
        match classify_commit_outcome(true, "[main abc1234def] add feature", 0) {
            CommitOutcome::Ok(s) => assert_eq!(s, "ok abc1234"),
            CommitOutcome::Failed(_) => panic!("successful commit must be Ok"),
        }
    }

    #[test]
    fn test_classify_commit_success_empty_stdout() {
        match classify_commit_outcome(true, "", 0) {
            CommitOutcome::Ok(s) => assert_eq!(s, "ok"),
            CommitOutcome::Failed(_) => panic!("successful commit must be Ok"),
        }
    }

    #[test]
    fn test_classify_commit_nothing_to_commit_is_failure() {
        match classify_commit_outcome(
            false,
            "On branch main\nnothing to commit, working tree clean",
            1,
        ) {
            CommitOutcome::Failed(code) => assert_eq!(code, 1),
            CommitOutcome::Ok(s) => panic!("nothing-to-commit must not be ok: {}", s),
        }
    }

    #[test]
    fn test_classify_commit_hook_abort_propagates_exit_code() {
        match classify_commit_outcome(false, "pre-commit hook failed", 2) {
            CommitOutcome::Failed(code) => assert_eq!(code, 2),
            CommitOutcome::Ok(_) => panic!("hook abort must be a failure"),
        }
    }

    /// Regression test: --oneline and other user format flags must preserve all commits.
    /// Before fix, filter_log_output split on ---END--- which doesn't exist when
    /// the user specifies their own format, resulting in only 2 commits surviving.
    #[test]
    fn test_filter_log_output_user_format_oneline() {
        let oneline_output = "abc1234 feat: add feature\n\
                              def5678 fix: typo\n\
                              ghi9012 chore: bump deps\n\
                              jkl3456 docs: update readme\n\
                              mno7890 test: add tests\n";

        let result = filter_log_output(oneline_output, 10, false, true);
        // All 5 lines must survive — no ---END--- splitting
        assert_eq!(result.lines().count(), 5);
        assert!(result.contains("abc1234"));
        assert!(result.contains("mno7890"));
    }

    #[test]
    fn test_filter_log_output_user_format_with_limit() {
        let oneline_output = "abc1234 feat: add feature\n\
                              def5678 fix: typo\n\
                              ghi9012 chore: bump deps\n\
                              jkl3456 docs: update readme\n\
                              mno7890 test: add tests\n";

        // user_set_limit=true means respect all lines (no cap)
        let result = filter_log_output(oneline_output, 3, true, true);
        assert_eq!(result.lines().count(), 5);

        // user_set_limit=false means cap at limit
        let result = filter_log_output(oneline_output, 3, false, true);
        assert_eq!(result.lines().count(), 3);
    }

    /// Regression test: `git branch <name>` must create, not list.
    /// Before fix, positional args fell into list mode which added `-a`,
    /// turning creation into a pattern-filtered listing (silent no-op).
    #[test]
    #[ignore] // Integration test: requires git repo
    fn test_branch_creation_not_swallowed() {
        let branch = "test-rtk-create-branch-regression";
        // Create branch via run_branch
        run_branch(&[branch.to_string()], 0, &[]).expect("run_branch should succeed");
        // Verify it exists
        let output = Command::new("git")
            .args(["branch", "--list", branch])
            .output()
            .expect("git branch --list should work");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(branch),
            "Branch '{}' was not created. run_branch silently swallowed the creation.",
            branch
        );
        // Cleanup
        let _ = Command::new("git").args(["branch", "-d", branch]).output();
    }

    /// Regression test: `git branch <name> <commit>` must create from commit.
    #[test]
    #[ignore] // Integration test: requires git repo
    fn test_branch_creation_from_commit() {
        let branch = "test-rtk-create-from-commit";
        run_branch(&[branch.to_string(), "HEAD".to_string()], 0, &[])
            .expect("run_branch with start-point should succeed");
        let output = Command::new("git")
            .args(["branch", "--list", branch])
            .output()
            .expect("git branch --list should work");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(branch),
            "Branch '{}' was not created from commit.",
            branch
        );
        let _ = Command::new("git").args(["branch", "-d", branch]).output();
    }

    #[test]
    fn test_commit_single_message() {
        let args = vec!["-m".to_string(), "fix: typo".to_string()];
        let cmd = build_commit_command(&args, &[]);
        let cmd_args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(cmd_args, vec!["commit", "-m", "fix: typo"]);
    }

    #[test]
    fn test_commit_multiple_messages() {
        let args = vec![
            "-m".to_string(),
            "feat: add multi-paragraph support".to_string(),
            "-m".to_string(),
            "This allows git commit -m \"title\" -m \"body\".".to_string(),
        ];
        let cmd = build_commit_command(&args, &[]);
        let cmd_args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            cmd_args,
            vec![
                "commit",
                "-m",
                "feat: add multi-paragraph support",
                "-m",
                "This allows git commit -m \"title\" -m \"body\"."
            ]
        );
    }

    // #327: git commit -am "msg" must pass -am through to git
    #[test]
    fn test_commit_am_flag() {
        let args = vec!["-am".to_string(), "quick fix".to_string()];
        let cmd = build_commit_command(&args, &[]);
        let cmd_args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(cmd_args, vec!["commit", "-am", "quick fix"]);
    }

    #[test]
    fn test_commit_amend() {
        let args = vec![
            "--amend".to_string(),
            "-m".to_string(),
            "new msg".to_string(),
        ];
        let cmd = build_commit_command(&args, &[]);
        let cmd_args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(cmd_args, vec!["commit", "--amend", "-m", "new msg"]);
    }

    #[test]
    #[ignore] // Requires `cargo build` first — run with `cargo test --ignored`
    fn test_git_status_not_a_repo_exits_nonzero() {
        // Run rtk git status in a directory that is not a git repo
        let tmp = std::env::temp_dir().join("rtk_test_not_a_repo");
        let _ = std::fs::create_dir_all(&tmp);

        let output = crate::core::test_isolation::rtk_command()
            .args(["git", "status"])
            .current_dir(&tmp)
            .output()
            .expect("Failed to run rtk");

        // Should exit with non-zero (128 from git)
        assert!(
            !output.status.success(),
            "Expected non-zero exit code for git status outside a repo, got {:?}",
            output.status.code()
        );

        // Message should be on stderr, not stdout
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stderr.to_lowercase().contains("not a git repository"),
            "Expected 'not a git repository' on stderr, got stderr={:?}, stdout={:?}",
            stderr,
            stdout
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- truncation accuracy ---

    #[test]
    fn test_format_status_output_shows_every_file_when_many_are_dirty() {
        let mut porcelain = String::from("## main...origin/main\n");
        for i in 0..25 {
            porcelain.push_str(&format!("M  staged_file_{}.rs\n", i));
        }
        let result = format_status_output(&porcelain);
        assert!(
            result.contains("staged_file_24.rs"),
            "Expected the last staged file to remain visible, got:\n{}",
            result
        );
        assert!(
            result.lines().count() == 26,
            "Expected branch + all 25 staged files, got:\n{}",
            result
        );
        assert!(
            !result.contains("... +"),
            "Status output must not hide dirty paths behind overflow markers:\n{}",
            result
        );
    }

    #[test]
    fn test_compact_diff_recovery_hint_present() {
        // A hunk with 110 lines exceeds max_hunk_lines (100), triggers truncation
        // The recovery hint must appear so LLMs can re-fetch the full diff
        let mut diff = String::new();
        diff.push_str("diff --git a/large.rs b/large.rs\n");
        diff.push_str("--- a/large.rs\n");
        diff.push_str("+++ b/large.rs\n");
        diff.push_str("@@ -1,150 +1,150 @@\n");
        for i in 0..110 {
            diff.push_str(&format!("+added line {}\n", i));
        }
        let result = compact_diff(&diff, 500);
        assert!(
            result.contains("[full diff: rtk git diff --no-compact]"),
            "Expected recovery hint when hunk is truncated, got:\n{}",
            result
        );
    }

    #[test]
    fn test_compact_diff_hunk_truncation_count_accurate() {
        // 150 change lines in one hunk: 100 shown, 50 silently dropped
        // Must report the exact count, not just "(truncated)"
        let mut diff = String::from(
            "diff --git a/large.rs b/large.rs\n--- a/large.rs\n+++ b/large.rs\n@@ -1,150 +1,150 @@\n",
        );
        for i in 0..150 {
            diff.push_str(&format!("+line {}\n", i));
        }
        let result = compact_diff(&diff, 500);
        assert!(
            result.contains("50 additions truncated"),
            "Expected '50 additions truncated' (150 - 100 = 50), got:\n{}",
            result
        );
    }

    #[test]
    fn test_extract_detached_head_returns_line() {
        let raw = "HEAD detached at abc1234\nnothing to commit, working tree clean\n";
        assert_eq!(
            extract_detached_head(raw),
            Some("HEAD detached at abc1234".to_string())
        );
    }

    #[test]
    fn test_extract_detached_head_on_branch_is_none() {
        let raw = "On branch main\nnothing to commit, working tree clean\n";
        assert!(extract_detached_head(raw).is_none());
    }

    #[test]
    fn test_format_status_output_detached_head() {
        let porcelain = "## HEAD (no branch)\n M src/main.rs\n";
        let result = format_status_output_detached(porcelain, "HEAD detached at abc1234");
        assert!(
            result.contains("HEAD detached at abc1234"),
            "should use explicit detached ref, got: {result}"
        );
        assert!(
            !result.contains("HEAD (no branch)"),
            "should not show opaque porcelain string, got: {result}"
        );
    }

    #[test]
    fn test_filter_log_output_body_omission_indicator() {
        // Commit with 6 meaningful body lines: only 3 shown, must signal "+3 lines omitted"
        let body_lines = (1..=6)
            .map(|i| format!("body line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let output = format!(
            "abc1234 feat: big change (1 day ago) <author>\n{}\n---END---\n",
            body_lines
        );
        let result = filter_log_output(&output, 10, false, false);
        assert!(
            result.contains("+3 lines omitted"),
            "Expected '+3 lines omitted' when 6 body lines truncated to 3, got:\n{}",
            result
        );
    }

    fn run_push_filter(input: &str, exit_code: i32) -> String {
        use crate::core::stream::StreamFilter;
        let mut f = LineStreamFilter::new(GitPushLineHandler::default());
        let mut out = String::new();
        for line in input.lines() {
            if let Some(s) = f.feed_line(line) {
                out.push_str(&s);
            }
        }
        out.push_str(&f.flush());
        if let Some(s) = f.on_exit(exit_code, input) {
            out.push_str(&s);
        }
        out
    }

    #[test]
    fn test_push_filter_drops_progress_phases() {
        let input = "\
Enumerating objects: 5, done.
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
Compressing objects: 100% (3/3), done.
Writing objects: 100% (3/3), 312 bytes | 312.00 KiB/s, done.
Total 3 (delta 2), reused 0 (delta 0)
To https://github.com/foo/bar.git
   abc1234..def5678  master -> master
";
        let result = run_push_filter(input, 0);
        for prefix in GIT_PUSH_NOISE_PREFIXES {
            assert!(
                !result.contains(prefix),
                "noise prefix '{}' leaked through, got: {}",
                prefix,
                result
            );
        }
        assert!(result.contains("To https://github.com/foo/bar.git"));
        assert!(result.contains("master -> master"));
        assert!(result.ends_with("ok master\n"), "got: {}", result);
    }

    #[test]
    fn test_push_filter_up_to_date_summary() {
        let input = "Everything up-to-date\n";
        let result = run_push_filter(input, 0);
        assert!(result.contains("Everything up-to-date"));
        assert!(result.ends_with("ok (up-to-date)\n"), "got: {}", result);
    }

    #[test]
    fn test_push_filter_passes_remote_messages_through() {
        let input = "\
remote: Resolving deltas: 100% (2/2), completed with 2 local objects.
remote: GitHub found 1 vulnerability on foo/bar's default branch (1 moderate).
To https://github.com/foo/bar.git
   abc1234..def5678  feature -> feature
";
        let result = run_push_filter(input, 0);
        assert!(result.contains("remote: Resolving deltas"));
        assert!(result.contains("remote: GitHub found 1 vulnerability"));
        assert!(result.ends_with("ok feature\n"), "got: {}", result);
    }

    #[test]
    fn test_push_filter_no_summary_on_failure() {
        let input = "\
To https://github.com/foo/bar.git
 ! [rejected]        master -> master (non-fast-forward)
error: failed to push some refs to 'https://github.com/foo/bar.git'
";
        let result = run_push_filter(input, 1);
        assert!(result.contains("[rejected]"));
        assert!(result.contains("error: failed to push"));
        assert!(
            !result.contains("ok "),
            "summary leaked on failure, got: {}",
            result
        );
    }

    #[test]
    fn test_push_filter_first_ref_wins_for_summary() {
        let input = "\
To https://github.com/foo/bar.git
   abc1234..def5678  feat/a -> feat/a
   1111111..2222222  feat/b -> feat/b
";
        let result = run_push_filter(input, 0);
        assert!(result.ends_with("ok feat/a\n"), "got: {}", result);
    }

    #[test]
    fn test_push_filter_token_savings_on_verbose_output() {
        let input = "\
Enumerating objects: 142, done.
Counting objects: 100% (142/142), done.
Delta compression using up to 8 threads
Compressing objects: 100% (88/88), done.
Writing objects: 100% (104/104), 28.50 KiB | 14.25 MiB/s, done.
Total 104 (delta 64), reused 0 (delta 0), pack-reused 0
remote: Resolving deltas: 100% (64/64), completed with 24 local objects.
To https://github.com/foo/bar.git
   abc1234..def5678  master -> master
";
        let result = run_push_filter(input, 0);
        let count_tokens = |s: &str| s.split_whitespace().count();
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "expected >=60% savings, got {:.1}% (in={}, out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }
}
