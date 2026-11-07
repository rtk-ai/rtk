use crate::core::arg_tokenizer::{self, Dialect, Token, TokenKind, ValueSpec};
use crate::core::guard::never_worse;
use crate::core::shell::quote_word;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};

/// wget's value-taking short flags, from `wget --help` (GNU Wget 1.25).
const SHORT_VALUE_FLAGS: &[&str] = &[
    "e", "o", "a", "i", "B", "t", "O", "T", "w", "Q", "P", "U", "l", "A", "R", "D", "I", "X",
];

/// wget's value-taking long flags, from `wget --help` (GNU Wget 1.25).
const LONG_VALUE_FLAGS: &[&str] = &[
    "execute",
    "output-file",
    "append-output",
    "input-file",
    "base",
    "tries",
    "output-document",
    "timeout",
    "wait",
    "quota",
    "directory-prefix",
    "user-agent",
    "level",
    "accept",
    "reject",
    "domains",
    "include-directories",
    "exclude-directories",
    "report-speed",
    "config",
    "rejected-log",
    "retry-on-http-error",
    "start-pos",
    "progress",
    "dns-timeout",
    "connect-timeout",
    "read-timeout",
    "waitretry",
    "bind-address",
    "limit-rate",
    "restrict-file-names",
    "prefer-family",
    "user",
    "password",
    "use-askpass",
    "local-encoding",
    "remote-encoding",
    "cut-dirs",
    "http-user",
    "http-password",
    "default-page",
    "header",
    "compression",
    "proxy-user",
    "proxy-password",
    "referer",
    "load-cookies",
    "save-cookies",
    "post-data",
    "post-file",
    "method",
    "body-data",
    "body-file",
    "secure-protocol",
    "certificate",
    "certificate-type",
    "private-key",
    "private-key-type",
    "ca-certificate",
    "ca-directory",
    "crl-file",
    "pinnedpubkey",
    "random-file",
    "ciphers",
    "ftp-user",
    "ftp-password",
    "warc-file",
    "warc-header",
    "warc-max-size",
    "warc-dedup",
    "warc-tempdir",
    "backups",
    "accept-regex",
    "reject-regex",
    "regex-type",
    "exclude-domains",
    "follow-tags",
    "ignore-tags",
];

fn wget_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    let takes = match kind {
        TokenKind::Short => SHORT_VALUE_FLAGS.contains(&name),
        TokenKind::Long => LONG_VALUE_FLAGS.contains(&name),
        _ => false,
    };
    takes.then(ValueSpec::value)
}

fn tokenize(args: &[String]) -> Vec<Token<'_>> {
    arg_tokenizer::tokenize_grammar(args, &wget_takes_value, Dialect::Posix)
}

/// `-O -` in any spelling: `-O -`, `-O-`, `-qO-`, `--output-document -`, `--output-document=-`.
fn is_stdout_output(token: &Token<'_>, tokens: &[Token<'_>]) -> bool {
    let is_output = match token.kind {
        TokenKind::Short => token.text == "O",
        TokenKind::Long => token.text == "output-document",
        _ => false,
    };
    is_output && token.value(tokens) == Some("-")
}

/// Check whether trailing args ask wget to write the document to stdout
/// (Clap's trailing_var_arg absorbs `-O -` when another flag precedes it).
pub fn has_stdout_output_flag(args: &[String]) -> bool {
    let tokens = tokenize(args);
    tokens.iter().any(|t| is_stdout_output(t, &tokens))
}

/// Remove the stdout `-O -` from args, returning the remaining arguments to pass
/// through to wget. Other flags clustered with it survive: `-qO-` becomes `-q`.
pub fn strip_output_flag(args: &[String]) -> Vec<String> {
    let tokens = tokenize(args);
    let mut kept: Vec<Option<String>> = args.iter().cloned().map(Some).collect();
    for token in tokens.iter().filter(|t| is_stdout_output(t, &tokens)) {
        if let Some(value) = token.linked.and_then(|i| tokens.get(i)) {
            kept[value.source_index] = None;
        }
        // A Short `O` can only follow boolean flags in its cluster: a value-taking
        // flag before it would have consumed the rest of the arg as its value.
        let flags_before = match token.kind {
            TokenKind::Short => args[token.source_index][1..]
                .split_once('O')
                .map_or("", |(before, _)| before),
            _ => "",
        };
        kept[token.source_index] = (!flags_before.is_empty()).then(|| format!("-{flags_before}"));
    }
    kept.into_iter().flatten().collect()
}

/// Compact wget - strips progress bars, shows only result
pub fn run(url: &str, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("wget: {}", url);
    }

    // Run wget normally but capture output to parse it
    let mut cmd_args: Vec<&str> = vec![];

    // Add user args
    for arg in args {
        cmd_args.push(arg);
    }
    cmd_args.push(url);

    let mut cmd = resolved_command("wget");
    cmd.args(&cmd_args);
    let result = exec_capture(&mut cmd).context("Failed to run wget")?;

    let raw_output = format!("{}\n{}", result.stderr, result.stdout);

    if result.success() {
        let filename = extract_filename_from_output(&result.stderr, url, args);
        let size = get_file_size(&filename);
        let msg = format!(
            "{} ok | {} | {}",
            compact_url(url),
            filename,
            format_size(size)
        );
        let shown = never_worse(&raw_output, &msg);
        println!("{}", shown);
        timer.track(
            &format!("wget {}", quote_word(url)),
            "rtk wget",
            &raw_output,
            shown,
        );
    } else {
        let error = parse_error(&result.stderr, &result.stdout);
        let msg = format!("{} FAILED: {}", compact_url(url), error);
        let shown = never_worse(&raw_output, &msg);
        println!("{}", shown);
        timer.track(
            &format!("wget {}", quote_word(url)),
            "rtk wget",
            &raw_output,
            shown,
        );
        return Ok(result.exit_code);
    }

    Ok(0)
}

/// Run wget and output to stdout (for piping)
pub fn run_stdout(url: &str, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("wget: {} -> stdout", url);
    }

    let mut cmd_args = vec!["-q", "-O", "-"];
    for arg in args {
        cmd_args.push(arg);
    }
    cmd_args.push(url);

    let mut cmd = resolved_command("wget");
    cmd.args(&cmd_args);
    let result = exec_capture(&mut cmd).context("Failed to run wget")?;

    if result.success() {
        let lines: Vec<&str> = result.stdout.lines().collect();
        let total = lines.len();

        let mut rtk_output = String::new();
        if total > 20 {
            rtk_output.push_str(&format!(
                "{} ok | {} lines | {}\n",
                compact_url(url),
                total,
                format_size(result.stdout.len() as u64)
            ));
            rtk_output.push_str("first 10 lines:\n");
            for line in lines.iter().take(10) {
                rtk_output.push_str(&format!("{}\n", truncate_line(line, 100)));
            }
            rtk_output.push_str(&format!("... +{} more lines", total - 10));
        } else {
            rtk_output.push_str(&format!("{} ok | {} lines\n", compact_url(url), total));
            for line in &lines {
                rtk_output.push_str(&format!("{}\n", line));
            }
        }
        let shown = never_worse(&result.stdout, &rtk_output);
        print!("{}", shown);
        timer.track(
            &format!("wget -O - {}", quote_word(url)),
            "rtk wget -o",
            &result.stdout,
            shown,
        );
    } else {
        let error = parse_error(&result.stderr, "");
        let msg = format!("{} FAILED: {}", compact_url(url), error);
        let shown = never_worse(&result.stderr, &msg);
        println!("{}", shown);
        timer.track(
            &format!("wget -O - {}", quote_word(url)),
            "rtk wget -o",
            &result.stderr,
            shown,
        );
        return Ok(result.exit_code);
    }

    Ok(0)
}

fn extract_filename_from_output(stderr: &str, url: &str, args: &[String]) -> String {
    // Check for -O argument first
    for (i, arg) in args.iter().enumerate() {
        if (arg == "-O" || arg == "--output-document")
            && let Some(name) = args.get(i + 1)
        {
            return name.clone();
        }
        if let Some(name) = arg.strip_prefix("-O") {
            return name.to_string();
        }
    }

    // Parse wget output for "Sauvegarde en" or "Saving to"
    for line in stderr.lines() {
        // French: Sauvegarde en : « filename »
        if line.contains("Sauvegarde en") || line.contains("Saving to") {
            // Use char-based parsing to handle Unicode properly
            let chars: Vec<char> = line.chars().collect();
            let mut start_idx = None;
            let mut end_idx = None;

            for (i, c) in chars.iter().enumerate() {
                if *c == '«' || (*c == '\'' && start_idx.is_none()) {
                    start_idx = Some(i);
                }
                if *c == '»' || (*c == '\'' && start_idx.is_some()) {
                    end_idx = Some(i);
                }
            }

            if let (Some(s), Some(e)) = (start_idx, end_idx)
                && e > s + 1
            {
                let filename: String = chars[s + 1..e].iter().collect();
                return filename.trim().to_string();
            }
        }
    }

    // Fallback: extract from URL
    let path = url.rsplit("://").next().unwrap_or(url);
    let filename = path
        .rsplit('/')
        .next()
        .unwrap_or("index.html")
        .split('?')
        .next()
        .unwrap_or("index.html");

    if filename.is_empty() || !filename.contains('.') {
        "index.html".to_string()
    } else {
        filename.to_string()
    }
}

fn get_file_size(filename: &str) -> u64 {
    std::fs::metadata(filename).map(|m| m.len()).unwrap_or(0)
}

fn format_size(bytes: u64) -> String {
    if bytes == 0 {
        return "?".to_string();
    }
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn compact_url(url: &str) -> String {
    // Remove protocol
    let without_proto = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);

    // Truncate if too long
    let chars: Vec<char> = without_proto.chars().collect();
    if chars.len() <= 50 {
        without_proto.to_string()
    } else {
        let prefix: String = chars[..25].iter().collect();
        let suffix: String = chars[chars.len() - 20..].iter().collect();
        format!("{}...{}", prefix, suffix)
    }
}

#[allow(dead_code)]
fn parse_error(stderr: &str, stdout: &str) -> String {
    // Common wget error patterns
    let combined = format!("{}\n{}", stderr, stdout);

    if combined.contains("404") {
        return "404 Not Found".to_string();
    }
    if combined.contains("403") {
        return "403 Forbidden".to_string();
    }
    if combined.contains("401") {
        return "401 Unauthorized".to_string();
    }
    if combined.contains("500") {
        return "500 Server Error".to_string();
    }
    if combined.contains("Connection refused") {
        return "Connection refused".to_string();
    }
    if combined.contains("unable to resolve") || combined.contains("Name or service not known") {
        return "DNS lookup failed".to_string();
    }
    if combined.contains("timed out") {
        return "Connection timed out".to_string();
    }
    if combined.contains("SSL") || combined.contains("certificate") {
        return "SSL/TLS error".to_string();
    }

    // Return first meaningful line
    for line in stderr.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("--") {
            if trimmed.len() > 60 {
                let t: String = trimmed.chars().take(60).collect();
                return format!("{}...", t);
            }
            return trimmed.to_string();
        }
    }

    "Unknown error".to_string()
}

fn truncate_line(line: &str, max: usize) -> String {
    if line.len() <= max {
        line.to_string()
    } else {
        let t: String = line.chars().take(max.saturating_sub(3)).collect();
        format!("{}...", t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // --- has_stdout_output_flag ---

    #[test]
    fn detect_dash_o_dash_separated() {
        assert!(has_stdout_output_flag(&s(&["-O", "-"])));
    }

    #[test]
    fn detect_dash_o_dash_combined() {
        assert!(has_stdout_output_flag(&s(&["-O-"])));
    }

    #[test]
    fn detect_long_form_separated() {
        assert!(has_stdout_output_flag(&s(&["--output-document", "-"])));
    }

    #[test]
    fn detect_long_form_equals() {
        assert!(has_stdout_output_flag(&s(&["--output-document=-"])));
    }

    #[test]
    fn detect_stdout_after_other_flags() {
        assert!(has_stdout_output_flag(&s(&[
            "--no-check-certificate",
            "-q",
            "-O",
            "-"
        ])));
    }

    #[test]
    fn no_detect_output_to_file() {
        assert!(!has_stdout_output_flag(&s(&["-O", "file.html"])));
    }

    #[test]
    fn no_detect_no_output_flag() {
        assert!(!has_stdout_output_flag(&s(&[
            "-q",
            "--no-check-certificate"
        ])));
    }

    #[test]
    fn no_detect_empty_args() {
        assert!(!has_stdout_output_flag(&s(&[])));
    }

    #[test]
    fn no_detect_dash_o_at_end_without_value() {
        assert!(!has_stdout_output_flag(&s(&["-O"])));
    }

    // --- strip_output_flag ---

    #[test]
    fn strip_dash_o_dash_separated() {
        assert_eq!(strip_output_flag(&s(&["-q", "-O", "-"])), s(&["-q"]));
    }

    #[test]
    fn strip_dash_o_dash_combined() {
        assert_eq!(
            strip_output_flag(&s(&["--no-check-certificate", "-O-"])),
            s(&["--no-check-certificate"])
        );
    }

    #[test]
    fn strip_long_form_equals() {
        assert_eq!(
            strip_output_flag(&s(&["-q", "--output-document=-"])),
            s(&["-q"])
        );
    }

    #[test]
    fn strip_preserves_output_to_file() {
        let args = s(&["-O", "file.html", "-q"]);
        assert_eq!(strip_output_flag(&args), args);
    }

    #[test]
    fn strip_preserves_no_output_flag() {
        let args = s(&["-q", "--no-check-certificate"]);
        assert_eq!(strip_output_flag(&args), args);
    }

    #[test]
    fn detect_and_strip_clustered_short_flags() {
        // `wget URL -qO-`: the common combined spelling, after the URL
        assert!(has_stdout_output_flag(&s(&["-qO-"])));
        assert_eq!(strip_output_flag(&s(&["-qO-"])), s(&["-q"]));
        assert!(has_stdout_output_flag(&s(&["-qO", "-"])));
        assert_eq!(strip_output_flag(&s(&["-qO", "-", "-v"])), s(&["-q", "-v"]));
    }

    #[test]
    fn strip_long_form_separated() {
        assert_eq!(
            strip_output_flag(&s(&["--output-document", "-", "-q"])),
            s(&["-q"])
        );
    }

    #[test]
    fn no_detect_dash_as_value_of_another_flag() {
        // `-` is the value of --header / -U here, not of -O
        assert!(!has_stdout_output_flag(&s(&["--header", "-", "-O", "out"])));
        assert!(!has_stdout_output_flag(&s(&["-U", "-O-"])));
        assert!(!has_stdout_output_flag(&s(&["-tO-"])));
    }

    #[test]
    fn no_detect_after_dashdash() {
        assert!(!has_stdout_output_flag(&s(&["--", "-O", "-"])));
    }

    // --- compact_url ---

    #[test]
    fn test_compact_url_strips_protocol() {
        assert_eq!(
            compact_url("https://example.com/file.zip"),
            "example.com/file.zip"
        );
        assert_eq!(
            compact_url("http://example.com/file.zip"),
            "example.com/file.zip"
        );
    }

    #[test]
    fn test_compact_url_truncates_long_url() {
        let long = "https://example.com/very/long/path/that/exceeds/fifty/characters/file.zip";
        let result = compact_url(long);
        assert!(
            result.contains("..."),
            "Long URL should be truncated with ..."
        );
        assert!(result.len() < long.len());
    }

    #[test]
    fn test_compact_url_short_unchanged() {
        let short = "https://x.com/f";
        assert_eq!(compact_url(short), "x.com/f");
    }

    // --- format_size ---

    #[test]
    fn test_format_size_zero() {
        assert_eq!(format_size(0), "?");
    }

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(512), "512B");
    }

    #[test]
    fn test_format_size_kilobytes() {
        let result = format_size(2048);
        assert!(result.ends_with("KB"), "Expected KB, got {}", result);
    }

    #[test]
    fn test_format_size_megabytes() {
        let result = format_size(2 * 1024 * 1024);
        assert!(result.ends_with("MB"), "Expected MB, got {}", result);
    }

    // --- parse_error ---

    #[test]
    fn test_parse_error_404() {
        assert_eq!(parse_error("HTTP request failed: 404", ""), "404 Not Found");
    }

    #[test]
    fn test_parse_error_dns() {
        assert_eq!(
            parse_error("unable to resolve host example.com", ""),
            "DNS lookup failed"
        );
    }

    #[test]
    fn test_parse_error_ssl() {
        assert_eq!(
            parse_error("SSL certificate verification failed", ""),
            "SSL/TLS error"
        );
    }

    #[test]
    fn test_parse_error_unknown() {
        assert_eq!(parse_error("", ""), "Unknown error");
    }

    // --- truncate_line ---

    #[test]
    fn test_truncate_line_short() {
        assert_eq!(truncate_line("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_line_exact() {
        assert_eq!(truncate_line("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_line_long() {
        let result = truncate_line("hello world this is long", 10);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 10);
    }

    // --- extract_filename_from_output ---

    #[test]
    fn test_extract_filename_from_output_flag() {
        let args = vec!["-O".to_string(), "myfile.zip".to_string()];
        assert_eq!(
            extract_filename_from_output("", "https://example.com/x", &args),
            "myfile.zip"
        );
    }

    #[test]
    fn test_extract_filename_from_url_fallback() {
        let result = extract_filename_from_output("", "https://example.com/file.tar.gz", &[]);
        assert_eq!(result, "file.tar.gz");
    }

    #[test]
    fn test_extract_filename_empty_url_fallback() {
        let result = extract_filename_from_output("", "https://example.com/", &[]);
        assert_eq!(result, "index.html");
    }
}
