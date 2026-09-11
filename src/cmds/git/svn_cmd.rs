//! Filters Apache Subversion output while preserving native SVN semantics.

use crate::core::guard::never_worse;
use crate::core::runner::{self, RunMode, RunOptions};
use crate::core::tracking::TimedExecution;
use crate::core::truncate::{reduced, CAP_LIST};
use crate::core::utils::{decode_process_output, exit_code_from_status, resolved_command};
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::io::{self, Read, Write};
use std::process::Stdio;
use std::thread;

const LOG_SEPARATOR: &str =
    "------------------------------------------------------------------------";
// SVN messages are multi-line, so keep the default below the generic flat-list cap.
const DEFAULT_LOG_LIMIT: usize = reduced(CAP_LIST, 10);
const LOG_CAPTURE_LIMIT: usize = 10 * 1024 * 1024;

/// Run SVN with compact output for supported read-only commands.
///
/// Only `svn log` is filtered initially. Every other invocation uses exact
/// passthrough so adding first-class routing cannot change SVN behavior.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if args.first().is_some_and(|arg| arg == "log") && !requests_raw_log_output(&args[1..]) {
        return run_log(args, verbose);
    }

    run_passthrough(args, verbose)
}

fn run_log(args: &[String], verbose: u8) -> Result<i32> {
    let display = redacted_args_display(args);
    let command_args = log_args_with_default_limit(args);
    let effective_display = redacted_args_display(&command_args);
    if verbose > 0 {
        eprintln!("svn filter: {}", effective_display);
    }

    let mut cmd = resolved_command("svn");
    cmd.args(&command_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let timer = TimedExecution::start();
    let mut child = cmd.spawn().context("Failed to run svn")?;
    let mut child_stdout = child.stdout.take().context("No svn stdout handle")?;
    let child_stderr = child.stderr.take().context("No svn stderr handle")?;

    // SVN may prompt for credentials or certificate trust on stderr while it
    // reads the answer from inherited stdin. Relay raw chunks (not lines) so a
    // prompt without a trailing newline is visible immediately.
    let stderr_thread = thread::spawn(move || relay_stderr(child_stderr));

    let mut raw_stdout = Vec::new();
    let mut stdout_passthrough = false;
    let mut stdout_sink_open = true;
    let mut stdout_write_error = None;
    let mut stdout_read_error = None;
    let stdout_handle = io::stdout();
    let mut visible_stdout = stdout_handle.lock();
    let mut buffer = [0u8; 8192];

    loop {
        match child_stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                let chunk = &buffer[..count];
                if !stdout_passthrough && raw_stdout.len() + chunk.len() > LOG_CAPTURE_LIMIT {
                    stdout_passthrough = true;
                    eprintln!(
                        "[rtk] svn log output exceeds 10 MiB; streaming the complete native output"
                    );
                    write_visible(
                        &mut visible_stdout,
                        &raw_stdout,
                        &mut stdout_sink_open,
                        &mut stdout_write_error,
                    );
                    raw_stdout.clear();
                }

                if stdout_passthrough {
                    write_visible(
                        &mut visible_stdout,
                        chunk,
                        &mut stdout_sink_open,
                        &mut stdout_write_error,
                    );
                } else {
                    raw_stdout.extend_from_slice(chunk);
                }
            }
            Err(error) => {
                stdout_read_error = Some(error);
                break;
            }
        }
    }

    if stdout_passthrough && stdout_sink_open {
        if let Err(error) = visible_stdout.flush() {
            if error.kind() != io::ErrorKind::BrokenPipe {
                stdout_write_error = Some(error);
            }
        }
    }

    // Always reap the child and join the stderr relay before returning an I/O
    // error, otherwise an SVN process blocked on a full pipe could be orphaned.
    drop(child_stdout);
    let status_result = child.wait();
    let stderr_result = stderr_thread
        .join()
        .map_err(|_| anyhow::anyhow!("svn stderr relay thread panicked"))?;
    let status = status_result.context("Failed to wait for svn")?;

    if let Some(error) = stdout_read_error {
        return Err(error).context("Failed to read svn stdout");
    }
    if let Some(error) = stdout_write_error {
        return Err(error).context("Failed to write svn stdout");
    }
    if let Some(error) = stderr_result.read_error {
        return Err(error).context("Failed to read svn stderr");
    }
    if let Some(error) = stderr_result.write_error {
        return Err(error).context("Failed to write svn stderr");
    }

    let original_label = format!("svn {}", display);
    let rtk_label = format!("rtk svn {}", display);
    let exit_code = exit_code_from_status(&status, "svn");

    if stdout_passthrough {
        if !has_explicit_log_window(&args[1..]) {
            eprintln!(
                "[default log limit: {}; add -l/--limit to show more]",
                DEFAULT_LOG_LIMIT
            );
        }
        timer.track_passthrough(&original_label, &format!("{} (passthrough)", rtk_label));
        return Ok(exit_code);
    }

    if exit_code != 0 {
        write_stdout_bytes(&raw_stdout)?;
        let raw_stdout_text = decode_process_output(&raw_stdout);
        let raw_stderr_text = decode_process_output(&stderr_result.captured);
        let raw = format!("{}{}", raw_stdout_text, raw_stderr_text);
        timer.track(&original_label, &rtk_label, &raw, &raw);
        return Ok(exit_code);
    }

    let raw_stdout_text = decode_process_output(&raw_stdout);
    let filtered = filter_log_output(&raw_stdout_text);
    let recovery_hint = (!has_explicit_log_window(&args[1..])
        && log_reaches_default_limit(&raw_stdout_text))
    .then(|| {
        format!(
            "[default log limit: {}; add -l/--limit to show more]",
            DEFAULT_LOG_LIMIT
        )
    });
    let filtered = if let Some(hint) = &recovery_hint {
        let newline = if filtered.ends_with('\n') { "" } else { "\n" };
        format!("{}{}{}\n", filtered, newline, hint)
    } else {
        filtered
    };
    let shown = never_worse(&raw_stdout_text, &filtered);
    write_stdout_bytes(shown.as_bytes())?;

    // An unfamiliar locale/format may prevent separator compaction. If the
    // never-worse guard therefore restores raw stdout, keep the recovery path
    // visible on stderr rather than silently hiding history beyond the limit.
    let hint_on_stderr = recovery_hint
        .as_ref()
        .filter(|hint| !shown.contains(hint.as_str()));
    if let Some(hint) = hint_on_stderr {
        eprintln!("{}", hint);
    }

    let raw_stderr_text = decode_process_output(&stderr_result.captured);
    let raw_for_tracking = format!("{}{}", raw_stdout_text, raw_stderr_text);
    let shown_for_tracking = format!(
        "{}{}{}",
        shown,
        raw_stderr_text,
        hint_on_stderr.map_or("", |hint| hint.as_str())
    );
    timer.track(
        &original_label,
        &rtk_label,
        &raw_for_tracking,
        &shown_for_tracking,
    );

    Ok(exit_code)
}

struct StderrRelayResult {
    captured: Vec<u8>,
    read_error: Option<io::Error>,
    write_error: Option<io::Error>,
}

fn relay_stderr(mut child_stderr: impl Read) -> StderrRelayResult {
    let mut captured = Vec::new();
    let mut sink_open = true;
    let mut write_error = None;
    let mut read_error = None;
    let mut buffer = [0u8; 1024];

    loop {
        match child_stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                let chunk = &buffer[..count];
                if captured.len() < LOG_CAPTURE_LIMIT {
                    let remaining = LOG_CAPTURE_LIMIT - captured.len();
                    captured.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                }
                if sink_open {
                    // Keep the global stderr lock scoped to one chunk. Holding
                    // it while blocking on the next child read would deadlock
                    // if the stdout path needed to emit a cap warning.
                    let stderr_handle = io::stderr();
                    let mut visible_stderr = stderr_handle.lock();
                    write_visible(
                        &mut visible_stderr,
                        chunk,
                        &mut sink_open,
                        &mut write_error,
                    );
                    if let Err(error) = visible_stderr.flush() {
                        if error.kind() == io::ErrorKind::BrokenPipe {
                            sink_open = false;
                        } else {
                            write_error = Some(error);
                            sink_open = false;
                        }
                    }
                }
            }
            Err(error) => {
                read_error = Some(error);
                break;
            }
        }
    }

    StderrRelayResult {
        captured,
        read_error,
        write_error,
    }
}

fn write_visible(
    writer: &mut impl Write,
    bytes: &[u8],
    sink_open: &mut bool,
    write_error: &mut Option<io::Error>,
) {
    if !*sink_open {
        return;
    }

    if let Err(error) = writer.write_all(bytes) {
        if error.kind() != io::ErrorKind::BrokenPipe {
            *write_error = Some(error);
        }
        *sink_open = false;
    }
}

fn write_stdout_bytes(bytes: &[u8]) -> Result<()> {
    let stdout_handle = io::stdout();
    let mut stdout = stdout_handle.lock();
    match stdout.write_all(bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error).context("Failed to write svn stdout"),
    }
}

fn log_args_with_default_limit(args: &[String]) -> Vec<String> {
    if has_explicit_log_window(&args[1..]) {
        return args.to_vec();
    }

    let mut command_args = args.to_vec();
    command_args.insert(1, DEFAULT_LOG_LIMIT.to_string());
    command_args.insert(1, "--limit".to_string());
    command_args
}

fn has_explicit_log_window(args: &[String]) -> bool {
    args.iter().take_while(|arg| arg.as_str() != "--").any(|arg| {
        arg == "-l"
            || arg == "-r"
            || arg == "-c"
            || arg == "--limit"
            || arg == "--revision"
            || arg == "--change"
            || arg.starts_with("--limit=")
            || arg.starts_with("--revision=")
            || arg.starts_with("--change=")
            || arg.strip_prefix("-l").is_some_and(|value| !value.is_empty())
            || arg.strip_prefix("-r").is_some_and(|value| !value.is_empty())
            || arg.strip_prefix("-c").is_some_and(|value| !value.is_empty())
    })
}

fn log_reaches_default_limit(output: &str) -> bool {
    let lines: Vec<(&str, &str)> = output
        .split_inclusive('\n')
        .map(|full| {
            let content = full
                .strip_suffix("\r\n")
                .or_else(|| full.strip_suffix('\n'))
                .unwrap_or(full);
            (content, full)
        })
        .collect();
    if let Some(separators) = structural_separator_indexes(&lines) {
        return separators.len().saturating_sub(1) >= DEFAULT_LOG_LIMIT;
    }

    // Preserve a recovery path even for an unfamiliar localized format that
    // cannot be compacted structurally. This is deliberately only a fallback;
    // structurally recognized logs use their exact record count above.
    output
        .lines()
        .filter(|line| {
            let Some(rest) = line.strip_prefix('r') else {
                return false;
            };
            let digit_count = rest.chars().take_while(|c| c.is_ascii_digit()).count();
            digit_count > 0
                && rest
                    .get(digit_count..)
                    .is_some_and(|tail| tail.starts_with(" | "))
        })
        .count()
        >= DEFAULT_LOG_LIMIT
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let display = redacted_args_display(args);
    if verbose > 0 {
        eprintln!("svn passthrough: {}", display);
    }

    let mut cmd = resolved_command("svn");
    cmd.args(args);
    runner::run(
        cmd,
        "svn",
        &display,
        RunMode::Passthrough,
        RunOptions::default(),
    )
}

/// Explicitly detailed or machine-readable log requests remain byte-for-byte
/// passthrough. The default filter only removes decorative record separators,
/// but passthrough also avoids buffering output that can contain large diffs or
/// merge-history expansions.
pub(crate) fn requests_raw_log_output(args: &[String]) -> bool {
    let mut before_double_dash = true;

    args.iter().any(|arg| {
        if !before_double_dash {
            return false;
        }
        if arg == "--" {
            before_double_dash = false;
            return false;
        }

        matches!(
            arg.as_str(),
            "--xml"
                | "--incremental"
                | "--diff"
                | "--diff-cmd"
                | "--internal-diff"
                | "--quiet"
                | "--verbose"
                | "--use-merge-history"
                | "--with-all-revprops"
                | "--with-no-revprops"
                | "--with-revprop"
                | "--search"
                | "--search-and"
                | "--help"
                | "--version"
                | "-h"
        ) || arg.starts_with("--diff-cmd=")
            || arg.starts_with("--with-revprop=")
            || arg.starts_with("--search=")
            || arg.starts_with("--search-and=")
            || short_option_requests_raw(arg)
    })
}

fn short_option_requests_raw(arg: &str) -> bool {
    let Some(cluster) = arg.strip_prefix('-') else {
        return false;
    };
    if cluster.is_empty() || cluster.starts_with('-') {
        return false;
    }

    cluster
        .chars()
        .any(|flag| matches!(flag, 'q' | 'v' | 'g' | 'x'))
}

/// Remove only SVN's decorative 72-dash record separators. Commit headers,
/// complete messages, blank lines, and separator-shaped message content remain
/// unchanged. If the output does not match native text-log structure, return it
/// verbatim.
pub(crate) fn filter_log_output(output: &str) -> String {
    if output.is_empty() {
        return String::new();
    }

    let lines: Vec<(&str, &str)> = output
        .split_inclusive('\n')
        .map(|full| {
            let content = full
                .strip_suffix("\r\n")
                .or_else(|| full.strip_suffix('\n'))
                .unwrap_or(full);
            (content, full)
        })
        .collect();
    let Some(separator_indexes) = structural_separator_indexes(&lines) else {
        return output.to_string();
    };
    let mut separator_indexes = separator_indexes.into_iter().peekable();

    lines
        .iter()
        .enumerate()
        .filter_map(|(index, (_, full))| {
            if separator_indexes.peek().copied() == Some(index) {
                separator_indexes.next();
                None
            } else {
                Some(*full)
            }
        })
        .collect()
}

fn structural_separator_indexes(lines: &[(&str, &str)]) -> Option<Vec<usize>> {
    if lines.first()?.0 != LOG_SEPARATOR {
        return None;
    }

    let mut separators = vec![0];
    let mut index = 1usize;

    loop {
        let message_lines = parse_log_header(lines.get(index)?.0)?;
        index += 1;

        // Native text logs place one empty line between the header and the
        // message, then exactly the number of message lines declared in the
        // header. Following that count is what keeps separator-shaped message
        // content distinguishable from record boundaries.
        if !lines.get(index)?.0.is_empty() {
            return None;
        }
        index += 1;
        index = index.checked_add(message_lines)?;

        if lines.get(index)?.0 != LOG_SEPARATOR {
            return None;
        }
        separators.push(index);
        index += 1;

        if index == lines.len() {
            return Some(separators);
        }
    }
}

fn parse_log_header(line: &str) -> Option<usize> {
    let rest = line.strip_prefix('r')?;
    let digit_count = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_count == 0 {
        return None;
    }

    let metadata = rest.get(digit_count..)?.strip_prefix(" | ")?;
    let (author_and_date, line_count) = metadata.rsplit_once(" | ")?;
    if !author_and_date.contains(" | ") {
        return None;
    }

    let mut line_count_parts = line_count.split_whitespace();
    let count = line_count_parts.next()?;
    line_count_parts.next()?;
    count.parse().ok()
}

fn redacted_args_display(args: &[String]) -> String {
    let mut redact_next_password = false;
    let mut config_option_next = false;
    let mut displayed = Vec::with_capacity(args.len());

    for arg in args {
        if redact_next_password {
            displayed.push("[REDACTED]".to_string());
            redact_next_password = false;
            continue;
        }

        if config_option_next {
            displayed.push(redact_config_option(arg).into_owned());
            config_option_next = false;
            continue;
        }

        if arg == "--password" {
            displayed.push(arg.clone());
            redact_next_password = true;
        } else if arg == "--config-option" {
            displayed.push(arg.clone());
            config_option_next = true;
        } else if arg.starts_with("--password=") {
            displayed.push("--password=[REDACTED]".to_string());
        } else if let Some(value) = arg.strip_prefix("--config-option=") {
            displayed.push(format!(
                "--config-option={}",
                redact_config_option(value)
            ));
        } else {
            displayed.push(redact_url_userinfo(arg).into_owned());
        }
    }

    displayed.join(" ")
}

fn redact_config_option(value: &str) -> Cow<'_, str> {
    if !value.to_ascii_lowercase().contains("password") {
        return Cow::Borrowed(value);
    }

    match value.split_once('=') {
        Some((key, _)) => Cow::Owned(format!("{}=[REDACTED]", key)),
        None => Cow::Borrowed("[REDACTED]"),
    }
}

fn redact_url_userinfo(value: &str) -> Cow<'_, str> {
    let Some(scheme_end) = value.find("://") else {
        return Cow::Borrowed(value);
    };
    let authority_start = scheme_end + 3;
    let authority_end = value[authority_start..]
        .find('/')
        .map(|offset| authority_start + offset)
        .unwrap_or(value.len());
    let authority = &value[authority_start..authority_end];
    let Some(at_offset) = authority.rfind('@') else {
        return Cow::Borrowed(value);
    };
    let host_start = authority_start + at_offset + 1;

    Cow::Owned(format!(
        "{}[REDACTED]@{}",
        &value[..authority_start],
        &value[host_start..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_SVN_LOG: &str = include_str!("../../../tests/fixtures/svn_log_1_14_raw.txt");

    #[test]
    fn filters_real_svn_log_without_dropping_content() {
        let output = filter_log_output(REAL_SVN_LOG);

        assert!(!output.lines().any(|line| line == LOG_SEPARATOR));
        for expected in [
            "r4 | root |",
            "Clarify configuration documentation",
            "Link the README to the checked-in defaults.",
            "Refs: OPS-142",
            "r1 | root |",
            "Initialize project structure",
        ] {
            assert!(output.contains(expected), "missing: {expected}");
        }
    }

    #[test]
    fn real_svn_log_clears_admission_threshold() {
        let output = filter_log_output(REAL_SVN_LOG);
        let savings =
            100.0 - (output.len() as f64 / REAL_SVN_LOG.len() as f64 * 100.0);

        assert!(
            savings >= 20.0,
            "svn log: expected at least 20% savings, got {savings:.1}%"
        );
    }

    #[test]
    fn preserves_separator_shaped_commit_message() {
        let input = format!(
            "{0}\nr2 | dev | 2026-01-01 | 1 line\n\n{0}\n{0}\nr1 | dev | 2026-01-01 | 1 line\n\nmessage\n{0}\n",
            LOG_SEPARATOR
        );
        let output = filter_log_output(&input);

        assert!(output.contains(LOG_SEPARATOR));
        assert_eq!(
            output.lines().filter(|line| *line == LOG_SEPARATOR).count(),
            1
        );
    }

    #[test]
    fn preserves_message_that_mimics_a_separator_and_header() {
        let fake_header = "r123 | fake-author | 2026-01-01 | 1 line";
        let input = format!(
            "{0}\nr2 | dev | 2026-01-01 | 2 lines\n\n{0}\n{1}\n{0}\nr1 | dev | 2026-01-01 | 1 line\n\nmessage\n{0}\n",
            LOG_SEPARATOR, fake_header
        );
        let output = filter_log_output(&input);

        assert!(output.contains(&format!("{}\n{}", LOG_SEPARATOR, fake_header)));
        assert_eq!(
            output.lines().filter(|line| *line == LOG_SEPARATOR).count(),
            1
        );
    }

    #[test]
    fn unfamiliar_and_xml_output_fall_back_verbatim() {
        for input in [
            "svn: E155007: not a working copy\n",
            "<?xml version=\"1.0\"?><log><logentry revision=\"1\"/></log>\n",
            "r1 | partial record without boundaries\n",
        ] {
            assert_eq!(filter_log_output(input), input);
        }
    }

    #[test]
    fn detailed_log_shapes_request_passthrough() {
        for args in [
            vec!["--xml"],
            vec!["--quiet"],
            vec!["-q"],
            vec!["-v"],
            vec!["-vg"],
            vec!["--diff"],
            vec!["--diff-cmd=meld"],
            vec!["--with-revprop", "custom:property"],
            vec!["--search", "OPS-*"],
            vec!["--search-and=critical"],
        ] {
            let args: Vec<String> = args.into_iter().map(String::from).collect();
            assert!(requests_raw_log_output(&args), "args: {args:?}");
        }

        let path_args = vec!["--".to_string(), "-verbose-path".to_string()];
        assert!(!requests_raw_log_output(&path_args));
    }

    #[test]
    fn preserves_crlf_line_endings() {
        let input = format!(
            "{0}\r\nr1 | dev | 2026-01-01 | 1 line\r\n\r\nmessage\r\n{0}\r\n",
            LOG_SEPARATOR
        );
        let output = filter_log_output(&input);

        assert_eq!(
            output,
            "r1 | dev | 2026-01-01 | 1 line\r\n\r\nmessage\r\n"
        );
        assert!(!output.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn filters_localized_line_count_nouns() {
        for noun in ["ligne", "Zeile", "línea", "行", "줄"] {
            let input = format!(
                "{0}\nr1 | dev | 2026-01-01 | 1 {1}\n\nmessage\n{0}\n",
                LOG_SEPARATOR, noun
            );
            assert_eq!(
                filter_log_output(&input),
                format!("r1 | dev | 2026-01-01 | 1 {noun}\n\nmessage\n")
            );
        }
    }

    #[test]
    fn tracking_display_redacts_credentials() {
        let args = vec![
            "log".to_string(),
            "--username".to_string(),
            "buildbot".to_string(),
            "--password".to_string(),
            "super-secret".to_string(),
            "--config-option=servers:global:http-proxy-password=proxy-secret".to_string(),
            "https://alice:url-secret@example.com/repos/app".to_string(),
        ];
        let display = redacted_args_display(&args);

        assert!(display.contains("--username buildbot"));
        assert!(display.contains("--password [REDACTED]"));
        assert!(display.contains("http-proxy-password=[REDACTED]"));
        assert!(display.contains("https://[REDACTED]@example.com/repos/app"));
        for secret in ["super-secret", "proxy-secret", "url-secret"] {
            assert!(!display.contains(secret));
        }
    }

    #[test]
    fn tracking_display_redacts_split_config_password() {
        let args = vec![
            "status".to_string(),
            "--config-option".to_string(),
            "servers:global:http-proxy-password=hunter2".to_string(),
            "--password=another-secret".to_string(),
        ];

        let display = redacted_args_display(&args);
        assert_eq!(
            display,
            "status --config-option servers:global:http-proxy-password=[REDACTED] --password=[REDACTED]"
        );
    }

    #[test]
    fn tracking_display_redacts_equals_inside_config_password() {
        let args = vec![
            "log".to_string(),
            "--config-option=servers:global:http-proxy-password=abc=def".to_string(),
        ];

        let display = redacted_args_display(&args);
        assert_eq!(
            display,
            "log --config-option=servers:global:http-proxy-password=[REDACTED]"
        );
        assert!(!display.contains("abc"));
        assert!(!display.contains("def"));
    }

    #[test]
    fn default_log_limit_respects_explicit_windows_and_double_dash() {
        let args = vec!["log".to_string(), "trunk".to_string()];
        assert_eq!(
            log_args_with_default_limit(&args),
            ["log", "--limit", "10", "trunk"]
        );

        for explicit in [
            vec!["log", "-l", "25"],
            vec!["log", "-l25"],
            vec!["log", "--limit", "25"],
            vec!["log", "--limit=25"],
            vec!["log", "-r", "1:25"],
            vec!["log", "-r1:25"],
            vec!["log", "--revision=1:25"],
            vec!["log", "-c", "25"],
            vec!["log", "-c25"],
            vec!["log", "--change=25"],
        ] {
            let explicit: Vec<String> = explicit.into_iter().map(String::from).collect();
            assert_eq!(log_args_with_default_limit(&explicit), explicit);
        }

        let dashed_path = vec!["log".to_string(), "--".to_string(), "-l25".to_string()];
        assert_eq!(
            log_args_with_default_limit(&dashed_path),
            ["log", "--limit", "10", "--", "-l25"]
        );
    }

    #[test]
    fn default_limit_count_ignores_header_shaped_message_lines() {
        let mut output = String::new();
        for revision in (1..=9).rev() {
            output.push_str(LOG_SEPARATOR);
            output.push('\n');
            let message_lines = if revision == 9 { 2 } else { 1 };
            output.push_str(&format!(
                "r{revision} | dev | 2026-01-01 | {message_lines} lines\n\nmessage {revision}\n"
            ));
            if revision == 9 {
                output.push_str("r123 | pasted | header-shaped message | 1 line\n");
            }
        }
        output.push_str(LOG_SEPARATOR);
        output.push('\n');

        assert!(!log_reaches_default_limit(&output));

        output.insert_str(
            0,
            &format!(
                "{0}\nr10 | dev | 2026-01-01 | 1 line\n\nmessage 10\n",
                LOG_SEPARATOR
            ),
        );
        assert!(log_reaches_default_limit(&output));
    }
}
