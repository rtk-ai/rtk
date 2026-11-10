//! `arc` CLI proxy — compact status / log / diff / show; other subcommands passthrough.
//!
//! Arc status/log resemble git; diffs are unified without `diff --git` headers, so we
//! normalize then reuse [`crate::cmds::git::git::compact_diff`].

use crate::cmds::git::git::compact_diff_indented;
use crate::core::guard::never_worse;
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::ffi::OsString;
use std::process::Command;

/// Default commit cap when the user did not pass `-n` / `--max-count`.
const DEFAULT_LOG_LIMIT: usize = 10;

/// Diff line budget (same ballpark as `rtk git diff`).
const ARC_DIFF_MAX_LINES: usize = 500;

/// First-token subdispatch for filter routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArcKind {
    Status,
    Log,
    Diff,
    Show,
    Other,
}

/// Execution plan for `run` — testable without spawning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArcPipeline {
    StatusFiltered,
    LogFiltered,
    DiffFiltered,
    ShowFiltered,
    Passthrough,
}

/// Classify `arc` argv by the first positional token.
pub fn classify(args: &[String]) -> ArcKind {
    match args.first().map(String::as_str) {
        Some("status") => ArcKind::Status,
        Some("log") => ArcKind::Log,
        Some("diff") => ArcKind::Diff,
        Some("show") => ArcKind::Show,
        _ => ArcKind::Other,
    }
}

pub fn pipeline(args: &[String]) -> ArcPipeline {
    match classify(args) {
        ArcKind::Status => ArcPipeline::StatusFiltered,
        ArcKind::Log => ArcPipeline::LogFiltered,
        ArcKind::Diff => ArcPipeline::DiffFiltered,
        ArcKind::Show => ArcPipeline::ShowFiltered,
        ArcKind::Other => ArcPipeline::Passthrough,
    }
}

fn build_arc_command(args: &[String]) -> Command {
    let mut cmd = resolved_command("arc");
    for arg in args {
        cmd.arg(arg);
    }
    cmd
}

/// Tee + skip filter on non-zero exit (stderr-only errors stay intact).
fn arc_filter_opts() -> runner::RunOptions<'static> {
    runner::RunOptions::with_tee("arc").early_exit_on_failure()
}

/// Inject `-n DEFAULT` for `arc log` when the user did not set a limit.
fn log_args_with_default_limit(args: &[String]) -> Vec<String> {
    if args.is_empty() || args[0] != "log" {
        return args.to_vec();
    }
    if has_log_limit_flag(&args[1..]) {
        return args.to_vec();
    }
    let mut out = Vec::with_capacity(args.len() + 2);
    out.push("log".to_string());
    out.push("-n".to_string());
    out.push(DEFAULT_LOG_LIMIT.to_string());
    out.extend(args.iter().skip(1).cloned());
    out
}

fn has_log_limit_flag(args: &[String]) -> bool {
    parse_user_log_limit(args).is_some()
}

/// Parse an explicit user log limit, if any.
///
/// Recognizes: `-n N`, `-nN`, `--max-count N`, `--max-count=N`, and git-style `-N`
/// (e.g. `-20`) so agents coming from `git log` habits are not double-capped.
fn parse_user_log_limit(args: &[String]) -> Option<usize> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-n" || arg == "--max-count" {
            if let Some(next) = args.get(i + 1) {
                if let Ok(n) = next.parse::<usize>() {
                    return Some(n);
                }
            }
            i += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--max-count=") {
            if let Ok(n) = rest.parse::<usize>() {
                return Some(n);
            }
        }
        // Combined `-n20`
        if let Some(rest) = arg.strip_prefix("-n") {
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(n) = rest.parse::<usize>() {
                    return Some(n);
                }
            }
        }
        // Git-style `-20` (dash + digits only)
        if arg.starts_with('-')
            && arg.len() > 1
            && arg[1..].chars().all(|c| c.is_ascii_digit())
        {
            if let Ok(n) = arg[1..].parse::<usize>() {
                return Some(n);
            }
        }
        i += 1;
    }
    None
}

/// `(limit, user_set_limit)` — mirrors git log: explicit user `-n` is honored fully.
fn resolve_log_limit(args: &[String]) -> (usize, bool) {
    match parse_user_log_limit(args) {
        Some(n) => (n.max(1), true),
        None => (DEFAULT_LOG_LIMIT, false),
    }
}

fn user_log_format(args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "--oneline"
            || a.starts_with("--pretty")
            || a.starts_with("--format")
            || a == "--json"
            || a == "--json-lines"
    })
}

/// Run `arc` — filtered for status/log/diff/show, passthrough otherwise.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let kind = classify(args);

    if verbose > 0 {
        eprintln!(
            "arc {:?} → {}",
            kind,
            match pipeline(args) {
                ArcPipeline::StatusFiltered => "status filter",
                ArcPipeline::LogFiltered => "log filter",
                ArcPipeline::DiffFiltered => "diff filter",
                ArcPipeline::ShowFiltered => "show filter",
                ArcPipeline::Passthrough => "passthrough",
            }
        );
    }

    match pipeline(args) {
        ArcPipeline::StatusFiltered => runner::run_filtered(
            build_arc_command(args),
            "arc",
            &args.join(" "),
            filter_arc_status_safe,
            arc_filter_opts(),
        ),
        ArcPipeline::LogFiltered => {
            let spawn_args = log_args_with_default_limit(args);
            let (limit, user_set_limit) = resolve_log_limit(&spawn_args[1..]);
            let format = user_log_format(&spawn_args[1..]);
            runner::run_filtered(
                build_arc_command(&spawn_args),
                "arc",
                &args.join(" "),
                move |raw| filter_arc_log_safe(raw, limit, user_set_limit, format),
                arc_filter_opts(),
            )
        }
        ArcPipeline::DiffFiltered => runner::run_filtered(
            build_arc_command(args),
            "arc",
            &args.join(" "),
            filter_arc_diff_safe,
            arc_filter_opts(),
        ),
        ArcPipeline::ShowFiltered => runner::run_filtered(
            build_arc_command(args),
            "arc",
            &args.join(" "),
            filter_arc_show_safe,
            arc_filter_opts(),
        ),
        ArcPipeline::Passthrough => {
            let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
            runner::run_passthrough("arc", &os_args, verbose)
        }
    }
}

fn filter_arc_status_safe(raw: &str) -> String {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| filter_arc_status(raw))) {
        Ok(s) => never_worse(raw, &s).to_string(),
        Err(_) => {
            eprintln!("rtk: arc filter warning: panic in filter_arc_status; showing raw output");
            raw.to_string()
        }
    }
}

fn filter_arc_log_safe(
    raw: &str,
    limit: usize,
    user_set_limit: bool,
    user_format: bool,
) -> String {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        filter_arc_log(raw, limit, user_set_limit, user_format)
    })) {
        Ok(s) => never_worse(raw, &s).to_string(),
        Err(_) => {
            eprintln!("rtk: arc filter warning: panic in filter_arc_log; showing raw output");
            raw.to_string()
        }
    }
}

fn filter_arc_diff_safe(raw: &str) -> String {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| filter_arc_diff(raw))) {
        Ok(s) => never_worse(raw, &s).to_string(),
        Err(_) => {
            eprintln!("rtk: arc filter warning: panic in filter_arc_diff; showing raw output");
            raw.to_string()
        }
    }
}

fn filter_arc_show_safe(raw: &str) -> String {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| filter_arc_show(raw))) {
        Ok(s) => never_worse(raw, &s).to_string(),
        Err(_) => {
            eprintln!("rtk: arc filter warning: panic in filter_arc_show; showing raw output");
            raw.to_string()
        }
    }
}

/// Compact verbose `arc status` (strip hints; branch + short file lines).
pub fn filter_arc_status(raw: &str) -> String {
    let stripped = strip_ansi_lite(raw);
    if stripped.trim().is_empty() {
        return String::new();
    }

    // Already short / porcelain-like: keep as-is minus empty lines.
    if looks_like_short_status(&stripped) {
        return stripped
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }

    let mut out: Vec<String> = Vec::new();
    let mut branch: Option<String> = None;
    let mut ahead_behind = String::new();
    let mut section = StatusSection::None;

    for line in stripped.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if is_arc_hint(trimmed) {
            continue;
        }

        if let Some(name) = trimmed.strip_prefix("On branch ") {
            branch = Some(name.to_string());
            continue;
        }
        if trimmed.starts_with("HEAD detached ") {
            branch = Some(trimmed.to_string());
            continue;
        }
        if trimmed.starts_with("Your branch is up-to-date") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Your branch is ahead of ") {
            if let Some(n) = extract_by_n_commits(rest) {
                ahead_behind = format!(" [ahead {n}]");
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Your branch is behind ") {
            if let Some(n) = extract_by_n_commits(rest) {
                ahead_behind.push_str(&format!(" [behind {n}]"));
            }
            continue;
        }

        if trimmed.starts_with("Changes to be committed:") {
            section = StatusSection::Staged;
            continue;
        }
        if trimmed.starts_with("Changes not staged for commit:") {
            section = StatusSection::Unstaged;
            continue;
        }
        if trimmed.starts_with("Untracked files:") {
            section = StatusSection::Untracked;
            continue;
        }
        if trimmed.starts_with("Unmerged paths:") {
            section = StatusSection::Unmerged;
            continue;
        }
        if trimmed.contains("nothing to commit") && trimmed.contains("working tree clean") {
            section = StatusSection::None;
            continue;
        }
        if trimmed.starts_with("nothing added to commit") {
            continue;
        }

        if let Some(entry) = parse_status_entry(trimmed, section) {
            out.push(entry);
        }
    }

    let mut result = Vec::new();
    if let Some(b) = branch {
        result.push(format!("* {b}{ahead_behind}"));
    }
    result.extend(out);

    if result.is_empty() {
        "clean — nothing to commit".to_string()
    } else if result.len() == 1 && result[0].starts_with('*') {
        result.push("clean — nothing to commit".to_string());
        result.join("\n")
    } else {
        result.join("\n")
    }
}

#[derive(Clone, Copy)]
enum StatusSection {
    None,
    Staged,
    Unstaged,
    Untracked,
    Unmerged,
}

fn looks_like_short_status(raw: &str) -> bool {
    let mut saw_line = false;
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        saw_line = true;
        if t.starts_with("On branch ")
            || t.starts_with("Changes ")
            || t.starts_with("Untracked ")
            || t.starts_with("Your branch ")
        {
            return false;
        }
        // `?? path`, ` M path`, `M  path`, etc.
        if t.len() >= 2 {
            continue;
        }
        return false;
    }
    saw_line
}

fn is_arc_hint(line: &str) -> bool {
    line.starts_with("(use \"arc")
        || line.starts_with("(use \"git")
        || line.contains("(use \"arc ")
        || line.contains("(use \"arc\"")
}

fn extract_by_n_commits(rest: &str) -> Option<usize> {
    // "'ref' by N commits." / "'ref' by N commit."
    let by = rest.find(" by ")?;
    let after = &rest[by + 4..];
    let n: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    n.parse().ok()
}

fn parse_status_entry(trimmed: &str, section: StatusSection) -> Option<String> {
    let path = match section {
        StatusSection::Untracked => {
            // Indented path only (already trimmed → path)
            if trimmed.contains(':') && !trimmed.contains('/') {
                return None;
            }
            Some(trimmed.to_string())
        }
        StatusSection::Staged | StatusSection::Unstaged | StatusSection::Unmerged => {
            // "modified:   path", "new file:   path", "both modified:   path"
            if let Some(idx) = trimmed.find(':') {
                let path = trimmed[idx + 1..].trim();
                if path.is_empty() {
                    None
                } else {
                    Some(path.to_string())
                }
            } else {
                None
            }
        }
        StatusSection::None => None,
    }?;

    let code = match section {
        StatusSection::Staged => {
            if trimmed.starts_with("new file") || trimmed.starts_with("added") {
                "A "
            } else if trimmed.starts_with("deleted") {
                "D "
            } else if trimmed.starts_with("renamed") {
                "R "
            } else {
                "M "
            }
        }
        StatusSection::Unstaged => {
            if trimmed.starts_with("deleted") {
                " D"
            } else {
                " M"
            }
        }
        StatusSection::Untracked => "??",
        StatusSection::Unmerged => "UU",
        StatusSection::None => return None,
    };

    Some(format!("{code} {path}"))
}

/// Cap / oneline-bias for verbose `arc log` (or light truncate when already `--oneline`).
///
/// When `user_set_limit` is true the caller already asked arc for exactly `limit`
/// commits — do not silently re-cap (git parity).
pub fn filter_arc_log(
    raw: &str,
    limit: usize,
    user_set_limit: bool,
    user_format: bool,
) -> String {
    let stripped = strip_ansi_lite(raw);
    if stripped.trim().is_empty() {
        return String::new();
    }

    let limit = if user_set_limit {
        limit.max(1)
    } else {
        // RTK default path: soft-cap to the injected default (subprocess already limited).
        limit.clamp(1, DEFAULT_LOG_LIMIT)
    };

    if user_format {
        let max_lines = if user_set_limit {
            stripped.lines().filter(|l| !l.trim().is_empty()).count()
        } else {
            limit
        };
        return stripped
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(max_lines)
            .map(|l| truncate_line(l, if user_set_limit { 120 } else { 100 }))
            .collect::<Vec<_>>()
            .join("\n");
    }

    let mut commits: Vec<String> = Vec::new();
    let mut cur_hash: Option<String> = None;
    let mut cur_deco = String::new();
    let mut cur_subject: Option<String> = None;

    let flush =
        |hash: &str, deco: &str, subject: &Option<String>, out: &mut Vec<String>| {
            let subj = subject.as_deref().unwrap_or("(no subject)");
            let short = short_hash(hash);
            // Subject first so long decorations cannot wipe the commit message.
            let deco_part = if deco.is_empty() {
                String::new()
            } else {
                format!(" {}", truncate_line(deco.trim(), 48))
            };
            out.push(truncate_line(&format!("{short}{deco_part} {subj}"), 140));
        };

    for line in stripped.lines() {
        if let Some(rest) = line.strip_prefix("commit ") {
            if let Some(h) = &cur_hash {
                flush(h, &cur_deco, &cur_subject, &mut commits);
                if commits.len() >= limit {
                    cur_hash = None;
                    break;
                }
            }
            let (hash, deco) = split_commit_header(rest);
            cur_hash = Some(hash);
            cur_deco = deco;
            cur_subject = None;
            continue;
        }

        if cur_hash.is_none() {
            continue;
        }
        if line.starts_with("author:")
            || line.starts_with("date:")
            || line.starts_with("merge:")
            || line.starts_with("revision:")
        {
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Message lines are indented in arc log; skip non-message metadata.
        if cur_subject.is_none()
            && !line.starts_with(' ')
            && !line.starts_with('\t')
            && trimmed.starts_with("revision:")
        {
            continue;
        }

        if cur_subject.is_none() {
            cur_subject = Some(trimmed.to_string());
        }
        // Oneline bias: drop commit bodies (tee recovers full `arc log`).
    }

    if commits.len() < limit {
        if let Some(h) = &cur_hash {
            flush(h, &cur_deco, &cur_subject, &mut commits);
        }
    }

    if commits.len() > limit {
        commits.truncate(limit);
    }

    commits.join("\n")
}

fn short_hash(hash: &str) -> &str {
    let end = hash.len().min(12);
    &hash[..end]
}

fn split_commit_header(rest: &str) -> (String, String) {
    let rest = rest.trim();
    if let Some(idx) = rest.find(" (") {
        let hash = rest[..idx].to_string();
        let deco = rest[idx..].to_string();
        (hash, deco)
    } else {
        (rest.to_string(), String::new())
    }
}

/// Normalize arc unified diffs then compact (file paths + capped hunk changes).
pub fn filter_arc_diff(raw: &str) -> String {
    let stripped = strip_ansi_lite(raw);
    if stripped.trim().is_empty() {
        return String::new();
    }
    let normalized = normalize_arc_unified_diff(&stripped);
    // Reuse git compact_diff structure, then tighten hunk bodies for token ROI.
    let compacted = compact_diff_indented(&normalized, ARC_DIFF_MAX_LINES);
    tighten_arc_diff(&compacted)
}

/// Keep file paths, hunk headers, and a small number of +/- lines per hunk.
///
/// When many files are touched, collapse to path + `+N -M` only (stat mode) —
/// typical for large `arc show` commits where hunk bodies dominate tokens.
fn tighten_arc_diff(compacted: &str) -> String {
    const MAX_CHANGE_PER_HUNK: usize = 3;
    const STAT_MODE_FILE_THRESHOLD: usize = 6;

    let file_count = compacted
        .lines()
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with(' ')
                && !l.starts_with('\t')
                && !l.trim().starts_with("...")
                && !l.trim().starts_with("[full diff:")
        })
        .count();
    let stat_mode = file_count >= STAT_MODE_FILE_THRESHOLD;

    let mut out: Vec<String> = Vec::new();
    let mut shown = 0usize;
    let mut skipped = 0usize;
    let mut in_hunk = false;
    let mut pending_path: Option<String> = None;

    let flush_skip = |skipped: &mut usize, out: &mut Vec<String>| {
        if *skipped > 0 {
            out.push(format!("  ... ({skipped} more changed lines)"));
            *skipped = 0;
        }
    };

    for line in compacted.lines() {
        if line.is_empty() {
            continue;
        }

        // compact_diff: file paths have no leading indent; everything else is `  …`.
        if !line.starts_with(' ') && !line.starts_with('\t') {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            flush_skip(&mut skipped, &mut out);
            if stat_mode {
                if let Some(p) = pending_path.take() {
                    // Previous file had no summary — still emit path.
                    out.push(p);
                }
                pending_path = Some(trimmed.to_string());
            } else {
                out.push(trimmed.to_string());
            }
            in_hunk = false;
            shown = 0;
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("...") || trimmed.starts_with("[full diff:") {
            flush_skip(&mut skipped, &mut out);
            if let Some(p) = pending_path.take() {
                out.push(p);
            }
            out.push(format!("  {trimmed}"));
            in_hunk = false;
            continue;
        }

        // Per-file `+N -M` summary
        if trimmed.starts_with('+')
            && trimmed.contains(" -")
            && trimmed
                .chars()
                .nth(1)
                .is_some_and(|c| c.is_ascii_digit())
        {
            flush_skip(&mut skipped, &mut out);
            if let Some(p) = pending_path.take() {
                out.push(p);
            }
            out.push(format!("  {trimmed}"));
            in_hunk = false;
            continue;
        }

        if stat_mode {
            // Skip hunk bodies in stat mode.
            continue;
        }

        if trimmed.starts_with("@@") {
            flush_skip(&mut skipped, &mut out);
            out.push(format!("  {trimmed}"));
            in_hunk = true;
            shown = 0;
            continue;
        }

        if in_hunk
            && (trimmed.starts_with('+') || trimmed.starts_with('-'))
        {
            if shown < MAX_CHANGE_PER_HUNK {
                out.push(format!("  {trimmed}"));
                shown += 1;
            } else {
                skipped += 1;
            }
            continue;
        }
    }
    flush_skip(&mut skipped, &mut out);
    if let Some(p) = pending_path.take() {
        out.push(p);
    }
    out.join("\n")
}

/// Compact commit header + patch body for `arc show`.
pub fn filter_arc_show(raw: &str) -> String {
    let stripped = strip_ansi_lite(raw);
    if stripped.trim().is_empty() {
        return String::new();
    }

    let mut header_lines = Vec::new();
    let mut patch_start = None;
    for (i, line) in stripped.lines().enumerate() {
        if line.starts_with("--- ") {
            patch_start = Some(i);
            break;
        }
        header_lines.push(line);
    }

    let header = filter_arc_log(&header_lines.join("\n"), 1, true, false);
    let Some(idx) = patch_start else {
        return header;
    };
    let patch: String = stripped.lines().skip(idx).collect::<Vec<_>>().join("\n");
    let compact = filter_arc_diff(&patch);
    if header.is_empty() {
        compact
    } else if compact.is_empty() {
        header
    } else {
        format!("{header}\n{compact}")
    }
}

/// Inject `diff --git a/path b/path` before each arc `---`/`+++` file pair.
pub fn normalize_arc_unified_diff(diff: &str) -> String {
    let lines: Vec<&str> = diff.lines().collect();
    let mut out = String::with_capacity(diff.len() + 64);
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("--- ")
            && i + 1 < lines.len()
            && lines[i + 1].starts_with("+++ ")
            && (i == 0 || !lines[i.saturating_sub(1)].starts_with("diff --git"))
        {
            let path = extract_arc_diff_path(lines[i + 1]);
            out.push_str(&format!("diff --git a/{path} b/{path}\n"));
            out.push_str(line);
            out.push('\n');
            out.push_str(lines[i + 1]);
            out.push('\n');
            i += 2;
            continue;
        }
        out.push_str(line);
        out.push('\n');
        i += 1;
    }
    out
}

fn extract_arc_diff_path(plus_line: &str) -> String {
    let rest = plus_line.strip_prefix("+++ ").unwrap_or(plus_line);
    let path = rest.split('\t').next().unwrap_or(rest).trim();
    if path == "/dev/null" {
        return "dev/null".to_string();
    }
    path.trim_start_matches("a/")
        .trim_start_matches("b/")
        .to_string()
}

fn truncate_line(line: &str, width: usize) -> String {
    if line.chars().count() > width {
        let truncated: String = line.chars().take(width.saturating_sub(3)).collect();
        format!("{truncated}...")
    } else {
        line.to_string()
    }
}

/// Minimal ANSI strip (arc rarely colors in fixtures; keep filter panic-safe).
fn strip_ansi_lite(s: &str) -> String {
    if !s.contains('\u{1b}') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    fn savings(raw: &str, out: &str) -> f64 {
        let i = count_tokens(raw);
        let o = count_tokens(out);
        if i == 0 {
            return 0.0;
        }
        100.0 * (1.0 - o as f64 / i as f64)
    }

    #[test]
    fn classify_known_and_other() {
        assert_eq!(classify(&["status".into()]), ArcKind::Status);
        assert_eq!(classify(&["log".into(), "-n".into(), "5".into()]), ArcKind::Log);
        assert_eq!(classify(&["diff".into()]), ArcKind::Diff);
        assert_eq!(classify(&["show".into(), "abc".into()]), ArcKind::Show);
        assert_eq!(classify(&["pr".into(), "list".into()]), ArcKind::Other);
        assert_eq!(classify(&[]), ArcKind::Other);
    }

    #[test]
    fn pipeline_routes_filtered_vs_passthrough() {
        assert_eq!(
            pipeline(&["status".into()]),
            ArcPipeline::StatusFiltered
        );
        assert_eq!(pipeline(&["log".into()]), ArcPipeline::LogFiltered);
        assert_eq!(pipeline(&["diff".into()]), ArcPipeline::DiffFiltered);
        assert_eq!(pipeline(&["show".into()]), ArcPipeline::ShowFiltered);
        assert_eq!(
            pipeline(&["info".into()]),
            ArcPipeline::Passthrough
        );
    }

    #[test]
    fn log_args_inject_default_n() {
        let args = ["log".into()];
        let out = log_args_with_default_limit(&args);
        assert_eq!(out, vec!["log", "-n", "10"]);
        let with_n = ["log".into(), "-n".into(), "3".into()];
        assert_eq!(log_args_with_default_limit(&with_n), with_n);
    }

    #[test]
    fn log_combined_n_forms_recognized_no_double_inject() {
        for args in [
            vec!["log".into(), "-n20".into()],
            vec!["log".into(), "-20".into()],
            vec!["log".into(), "--max-count=15".into()],
            vec!["log".into(), "--max-count".into(), "15".into()],
        ] {
            assert_eq!(
                log_args_with_default_limit(&args),
                args,
                "must not inject second -n for {args:?}"
            );
            assert!(has_log_limit_flag(&args[1..]), "{args:?}");
        }
        assert_eq!(parse_user_log_limit(&["-n20".into()]), Some(20));
        assert_eq!(parse_user_log_limit(&["-20".into()]), Some(20));
        assert_eq!(resolve_log_limit(&["-n".into(), "200".into()]), (200, true));
        assert_eq!(resolve_log_limit(&[]), (DEFAULT_LOG_LIMIT, false));
    }

    /// S7-T3 / S7-T7: status fixture → compact shape + ≥60%.
    #[test]
    fn status_fixture_compacts_and_saves() {
        let raw = include_str!("../../../tests/fixtures/arc/status_raw.txt");
        let out = filter_arc_status(raw);
        assert!(out.contains("feat/FININT-160_split_tfa_otp_channel"), "{out}");
        assert!(out.contains("progress.md"), "{out}");
        assert!(out.contains("??"), "{out}");
        assert!(!out.contains("(use \"arc"), "{out}");
        assert!(
            savings(raw, &out) >= 60.0,
            "savings {:.1}%\n{out}",
            savings(raw, &out)
        );
    }

    #[test]
    fn status_short_passthrough() {
        let raw = "?? billing/yandex_pay_plus/progress.md\n";
        let out = filter_arc_status(raw);
        assert_eq!(out, "?? billing/yandex_pay_plus/progress.md");
    }

    #[test]
    fn status_clean_shape() {
        let raw = "On branch trunk\nnothing to commit, working tree clean\n";
        let out = filter_arc_status(raw);
        assert!(out.contains("* trunk"), "{out}");
        assert!(out.contains("clean"), "{out}");
    }

    #[test]
    fn status_staged_and_unstaged_compact() {
        // Shape mirrors real `arc status` sections (same as fixture dialect).
        let raw = "\
On branch feat/example
Your branch is up-to-date with 'arcadia/users/me/feat/example'.
Changes to be committed:
  (use \"arc reset HEAD <file>...\" to unstage)

        modified:   path/a.py
        new file:   path/b.py

Changes not staged for commit:
  (use \"arc add <file>...\" to update what will be committed)

        modified:   path/c.py
        deleted:    path/d.py

Untracked files:
  (use \"arc add <file>...\" to include in what will be committed)

    path/e.md
";
        let out = filter_arc_status(raw);
        assert!(out.contains("* feat/example"), "{out}");
        assert!(out.contains("M  path/a.py"), "{out}");
        assert!(out.contains("A  path/b.py"), "{out}");
        assert!(out.contains(" M path/c.py"), "{out}");
        assert!(out.contains(" D path/d.py"), "{out}");
        assert!(out.contains("?? path/e.md"), "{out}");
        assert!(!out.contains("(use \"arc"), "{out}");
    }

    /// S7-T4 / S7-T7: log fixture → oneline bias, capped, ≥60%.
    #[test]
    fn log_fixture_oneline_cap_and_saves() {
        let raw = include_str!("../../../tests/fixtures/arc/log_raw.txt");
        let out = filter_arc_log(raw, DEFAULT_LOG_LIMIT, false, false);
        assert!(
            out.contains("7c37aa64a238") || out.contains("7c37aa64a238d993f85a67c12962cad5594e87b8"),
            "{out}"
        );
        assert!(
            out.contains("keep previous Split OTP channel"),
            "{out}"
        );
        assert!(!out.contains("author:"), "{out}");
        assert!(!out.contains("date:"), "{out}");
        let tops: Vec<_> = out.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(tops.len(), DEFAULT_LOG_LIMIT, "{out}");
        assert!(
            savings(raw, &out) >= 60.0,
            "savings {:.1}%\n{out}",
            savings(raw, &out)
        );
    }

    #[test]
    fn log_respects_limit() {
        let raw = include_str!("../../../tests/fixtures/arc/log_raw.txt");
        let out = filter_arc_log(raw, 3, true, false);
        assert_eq!(out.lines().filter(|l| !l.is_empty()).count(), 3, "{out}");
    }

    #[test]
    fn log_user_large_n_not_silently_capped() {
        let raw = include_str!("../../../tests/fixtures/arc/log_raw.txt");
        // Fixture has 20 commits; user asked for 200 — honor all available (no 100 soft-cap).
        let out = filter_arc_log(raw, 200, true, false);
        let n = out.lines().filter(|l| !l.is_empty()).count();
        assert!(n > 10, "expected >10 commits when user set large -n, got {n}\n{out}");
        assert_eq!(n, 20, "fixture has 20 commits; must not truncate below that\n{out}");
    }

    #[test]
    fn log_oneline_user_format_light_truncate() {
        let raw = "\
aaa111 subject one with a very long padding that should be truncated when over width for agents reading logs
bbb222 subject two
ccc333 subject three
";
        let out = filter_arc_log(raw, 2, false, true);
        assert_eq!(out.lines().count(), 2, "{out}");
        assert!(out.contains("aaa111"), "{out}");
        assert!(!out.contains("ccc333"), "{out}");

        // Explicit user limit: keep all lines arc already returned.
        let out_all = filter_arc_log(raw, 50, true, true);
        assert_eq!(out_all.lines().count(), 3, "{out_all}");
    }

    #[test]
    fn passthrough_pipeline_for_other_subcommands() {
        for sub in ["info", "pr", "branch", "root", "add", "commit"] {
            assert_eq!(
                pipeline(&[sub.into()]),
                ArcPipeline::Passthrough,
                "{sub} must passthrough + track"
            );
            assert_eq!(classify(&[sub.into()]), ArcKind::Other);
        }
    }

    #[test]
    fn arc_filter_opts_skip_filter_on_failure() {
        let opts = arc_filter_opts();
        assert!(opts.skip_filter_on_failure);
        assert_eq!(opts.tee_label, Some("arc"));
    }

    /// S7-T5 / S7-T7: diff fixture → file names kept, compact_diff reuse, ≥60%.
    #[test]
    fn diff_fixture_compacts_and_saves() {
        let raw = include_str!("../../../tests/fixtures/arc/diff_raw.txt");
        let out = filter_arc_diff(raw);
        assert!(
            out.contains("split_frameless.py") || out.contains("test_tfa.py"),
            "{out}"
        );
        assert!(
            out.contains("@@") || out.contains('+') || out.contains('-'),
            "{out}"
        );
        assert!(
            savings(raw, &out) >= 60.0,
            "savings {:.1}%\n{out}",
            savings(raw, &out)
        );
    }

    #[test]
    fn diff_preserves_repeated_sign_content_and_caps_changes() {
        let raw = "--- example.txt\n+++ example.txt\n@@ -1,2 +1,2 @@\n--- removed comment\n-old\n+++counter;\n+new\n";
        let out = filter_arc_diff(raw);
        assert!(out.contains("--- removed comment"), "{out}");
        assert!(out.contains("+++counter;"), "{out}");
        assert!(out.contains("1 more changed lines"), "{out}");
        assert!(out.contains("+2 -2"), "{out}");
    }

    #[test]
    fn normalize_injects_diff_git_headers() {
        let raw = "--- foo.py\t(abc)\n+++ foo.py\t(def)\n@@ -1 +1 @@\n-x\n+y\n";
        let n = normalize_arc_unified_diff(raw);
        assert!(n.contains("diff --git a/foo.py b/foo.py"), "{n}");
    }

    /// S7-T5 / S7-T7: show = header + compact patch, ≥60%.
    #[test]
    fn show_fixture_compacts_and_saves() {
        let raw = include_str!("../../../tests/fixtures/arc/show_raw.txt");
        let out = filter_arc_show(raw);
        assert!(
            out.contains("81f4fcbfd098") || out.contains("81f4fcbfd098e9e6f27bd881cdd5641e088a0d30"),
            "{out}"
        );
        assert!(
            out.contains("OTP channel") || out.contains("confirm_method"),
            "{out}"
        );
        assert!(!out.contains("author:"), "{out}");
        assert!(
            savings(raw, &out) >= 60.0,
            "savings {:.1}%\n{out}",
            savings(raw, &out)
        );
    }

    #[test]
    fn empty_and_malformed_do_not_panic() {
        assert!(filter_arc_status("").is_empty());
        assert!(filter_arc_log("", 10, false, false).is_empty());
        assert!(filter_arc_diff("").is_empty());
        assert!(filter_arc_show("").is_empty());
        let _ = filter_arc_status("garbage\nnot status\n");
        let _ = filter_arc_log("not a log\n", 5, false, false);
        let _ = filter_arc_diff("not a diff\n");
        let _ = filter_arc_show("orphan text\n");
    }

    #[test]
    fn build_arc_command_forwards_args() {
        let args = ["log".into(), "-n".into(), "5".into(), "--oneline".into()];
        let cmd = build_arc_command(&args);
        let forwarded: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(forwarded, args);
    }
}
