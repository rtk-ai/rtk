//! PHP command filter: version banner, syntax-check summaries and generic cleanup.

use super::artisan_cmd::{filter_artisan_output, filter_artisan_test_output};
use super::utils::{PhpTestRunner, detect_php_test_runner, strip_ansi_and_controls};
use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::runner;
use crate::core::shell::display_args;
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

/// First line of `php -v`: `PHP <PHP_VERSION> (<sapi>) (built: <date>) (<NTS|ZTS ...>)`.
/// Requiring the `(built:` group keeps a `PHP Warning:` line, or a script's own text,
/// from passing for it.
static VERSION_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^PHP (\d+\.\d+\.\d+\S*) \([^)]*\) \(built: [^)]*\)").unwrap());

/// The lines `php -v` prints after its header, as `php_get_version` and
/// `zend_append_version_info` build them: the PHP Group copyright (dated before PHP 7.4),
/// PHP 8.5's `Built by <provider>`, the Zend Engine line, and one
/// `    with <name> v<version>, <copyright>, by <author>` per loaded Zend extension.
static BANNER_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:Copyright \(c\) .*The PHP Group|Built by \S.*|Zend Engine v\S+, Copyright \(c\) .*|\s+with \S.*, by .*)$",
    )
    .unwrap()
});

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("php");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: php {}", args.join(" "));
    }

    let is_version = requests_version(args);
    let is_artisan = args.first().map(String::as_str) == Some("artisan");
    let is_artisan_test = is_artisan && args.get(1).map(String::as_str) == Some("test");
    let is_lint = args.iter().any(|a| a == "-l" || a == "--syntax-check");
    let detected_runner = if is_artisan_test {
        detect_php_test_runner()
    } else {
        PhpTestRunner::Unknown
    };

    if verbose > 0 && is_artisan_test {
        eprintln!("Detected artisan test runner: {:?}", detected_runner);
    }

    // The runner passes `--version` (and `-v -h`, whose `-h` PHP never reaches) through
    // unfiltered as a usage request; PHP answers both with its version banner, which is
    // exactly what this filter is for.
    let opts = if is_version {
        runner::RunOptions::default().unguarded_help()
    } else {
        runner::RunOptions::default()
    };

    runner::run_filtered(
        cmd,
        "php",
        &display_args(args),
        move |raw| {
            if is_version {
                return filter_php_version_output(raw);
            }
            if is_lint {
                return filter_php_lint_output(raw);
            }
            if is_artisan_test {
                return filter_artisan_test_output(raw, detected_runner);
            }
            if is_artisan {
                return filter_artisan_output(raw);
            }
            filter_php_output(raw)
        },
        opts,
    )
}

/// PHP CLI options that take a value, from `OPTIONS` in `sapi/cli/php_cli.c` (PHP 8.5).
/// `--ini` takes an optional value that must be attached (`--ini=diff`); a separate word
/// after it is the script name.
fn php_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    let takes = match kind {
        TokenKind::Short => matches!(
            name,
            "B" | "c" | "d" | "E" | "F" | "f" | "R" | "r" | "S" | "t"
        ),
        TokenKind::Long => {
            if name == "ini" {
                return Some(ValueSpec::attached_only());
            }
            matches!(
                name,
                "process-begin"
                    | "php-ini"
                    | "define"
                    | "process-end"
                    | "process-file"
                    | "file"
                    | "process-code"
                    | "run"
                    | "server"
                    | "docroot"
                    | "rf"
                    | "rfunction"
                    | "rc"
                    | "rclass"
                    | "re"
                    | "rextension"
                    | "rz"
                    | "rzendextension"
                    | "ri"
                    | "rextinfo"
                    | "repeat"
            )
        }
        TokenKind::Positional | TokenKind::DashDash => false,
    };
    takes.then(ValueSpec::value)
}

/// True when PHP will answer `args` with its version banner.
///
/// PHP reads its own options up to the first script name or `--`; everything after is the
/// script's. Among them, the first of `-h`/`-?` (usage), `-i` (phpinfo), `-v` (version) and
/// `-m` (modules) decides what it prints and exits, so `php -m -v` lists modules while
/// `php -v -m` and `php -n -d x=y -v` print the version.
fn requests_version(args: &[String]) -> bool {
    let tokens = arg_tokenizer::tokenize_grammar(args, &php_takes_value, Dialect::Posix);
    for token in &tokens {
        let mode = match token.kind {
            TokenKind::DashDash => return false,
            TokenKind::Positional if token.is_free_positional() => return false,
            TokenKind::Positional => continue,
            TokenKind::Short => match token.text {
                "v" => Some(true),
                "h" | "?" | "i" | "m" => Some(false),
                _ => None,
            },
            TokenKind::Long => match token.text {
                "version" => Some(true),
                "help" | "usage" | "info" | "modules" => Some(false),
                _ => None,
            },
        };
        if let Some(is_version) = mode {
            return is_version;
        }
    }
    false
}

/// `php -v` reduced to the version PHP reports as `PHP_VERSION` (`8.5.12`,
/// `8.1.2-1ubuntu2.18`, `8.5.0-dev`). Only the banner block goes; anything else in the
/// output -- a startup warning about an extension that failed to load -- is kept, in
/// place. Output without a recognisable header falls back to the generic cleanup.
fn filter_php_version_output(output: &str) -> String {
    let cleaned = strip_ansi_and_controls(output);
    let mut lines: Vec<&str> = Vec::new();
    let mut found = false;
    let mut in_banner = false;

    for line in cleaned.lines() {
        if !found && let Some(version) = VERSION_HEADER_RE.captures(line).and_then(|c| c.get(1)) {
            lines.push(version.as_str());
            found = true;
            in_banner = true;
            continue;
        }
        if in_banner && BANNER_LINE_RE.is_match(line) {
            continue;
        }
        in_banner = false;
        if !line.trim().is_empty() {
            lines.push(line.trim_end());
        }
    }

    if !found {
        return filter_php_output(output);
    }
    lines.join("\n")
}

fn filter_php_lint_output(output: &str) -> String {
    let mut lines = Vec::new();

    for line in strip_ansi_and_controls(output).lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(file) = trimmed.strip_prefix("No syntax errors detected in ") {
            lines.push(format!("ok {}", file.trim()));
            continue;
        }

        if trimmed.starts_with("Errors parsing ")
            || trimmed.contains("Parse error")
            || trimmed.contains("Fatal error")
            || trimmed.contains("syntax error")
        {
            lines.push(trimmed.to_string());
        }
    }

    if lines.is_empty() {
        let fallback = output.trim();
        if fallback.is_empty() {
            "ok".to_string()
        } else {
            fallback.to_string()
        }
    } else {
        lines.join("\n")
    }
}

fn filter_php_output(output: &str) -> String {
    let cleaned = strip_ansi_and_controls(output);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "ok".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_php_lint_summary() {
        let out = "No syntax errors detected in app/Http/Controller.php\n";
        assert_eq!(filter_php_lint_output(out), "ok app/Http/Controller.php");
    }

    #[test]
    fn test_php_lint_error_preserved() {
        let out = "Errors parsing app/Foo.php\nParse error: syntax error, unexpected ')' in app/Foo.php on line 10\n";
        let filtered = filter_php_lint_output(out);
        assert!(filtered.contains("Errors parsing app/Foo.php"));
        assert!(filtered.contains("Parse error"));
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn owned(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    // --- version banner -------------------------------------------------

    #[test]
    fn test_php_version_banner_is_the_bare_version() {
        let raw = include_str!("../../../tests/fixtures/php_v_raw.txt");
        assert_eq!(filter_php_version_output(raw), "8.5.12");
    }

    #[test]
    fn test_php_version_savings() {
        let raw = include_str!("../../../tests/fixtures/php_v_raw.txt");
        let filtered = filter_php_version_output(raw);
        let savings = 100.0 - (count_tokens(&filtered) as f64 / count_tokens(raw) as f64 * 100.0);
        assert!(
            savings >= 20.0,
            "php -v filter: expected >=20% savings, got {savings:.1}%"
        );
    }

    /// A startup warning is the one thing `php -v` can say besides its banner, and it is
    /// exactly what someone checking their PHP setup needs to see. With display_errors and
    /// log_errors both on, PHP prints it on stdout (after a blank line) and again on stderr;
    /// rtk captures stdout then stderr, so the banner sits between the two copies.
    #[test]
    fn test_php_version_keeps_startup_warnings() {
        let raw = include_str!("../../../tests/fixtures/php_v_startup_warning_raw.txt");
        let filtered = filter_php_version_output(raw);
        let lines: Vec<&str> = filtered.lines().collect();
        assert_eq!(lines.len(), 3, "unexpected output:\n{filtered}");
        assert!(lines[0].starts_with("Warning: PHP Startup: Unable to load dynamic library"));
        assert!(lines[0].ends_with("in Unknown on line 0"));
        assert_eq!(lines[1], "8.5.12");
        assert!(lines[2].starts_with("PHP Warning:  PHP Startup: Unable to load dynamic library"));
        assert!(!filtered.contains("Copyright"));
        assert!(!filtered.contains("Zend Engine"));
    }

    /// The version is printed as PHP_VERSION spells it: a distro patch level, a `-dev`
    /// or an `RC` suffix says which build this is, so none of it is cut.
    #[test]
    fn test_php_version_keeps_the_whole_version_token() {
        let ubuntu = "PHP 8.1.2-1ubuntu2.18 (cli) (built: Jun 14 2024 15:52:55) (NTS)\n\
                      Copyright (c) The PHP Group\n\
                      Zend Engine v4.1.2, Copyright (c) Zend Technologies\n    \
                      with Zend OPcache v8.1.2-1ubuntu2.18, Copyright (c), by Zend Technologies\n";
        assert_eq!(filter_php_version_output(ubuntu), "8.1.2-1ubuntu2.18");

        let dev = "PHP 8.5.0-dev (cli) (built: Jan  7 2025 09:12:44) (NTS DEBUG)\n\
                   Copyright (c) The PHP Group\n\
                   Zend Engine v4.5.0-dev, Copyright (c) Zend Technologies\n";
        assert_eq!(filter_php_version_output(dev), "8.5.0-dev");

        let rc = "PHP 8.3.0RC6 (cli) (built: Nov  9 2023 10:01:02) (NTS)\n\
                  Copyright (c) The PHP Group\n\
                  Zend Engine v4.3.0RC6, Copyright (c) Zend Technologies\n";
        assert_eq!(filter_php_version_output(rc), "8.3.0RC6");
    }

    /// Banner shapes this machine cannot produce: a Windows ZTS build (compiler and arch
    /// in the header), PHP 7's dated copyright lines, PHP 8.5's `Built by` line for a
    /// build configured with a provider, and Zend extensions whose copyright is a URL.
    #[test]
    fn test_php_version_other_banner_shapes() {
        let windows = "PHP 8.3.0 (cli) (built: Nov 21 2023 14:40:35) (ZTS Visual C++ 2019 x64)\r\n\
                       Copyright (c) The PHP Group\r\n\
                       Zend Engine v4.3.0, Copyright (c) Zend Technologies\r\n";
        assert_eq!(filter_php_version_output(windows), "8.3.0");

        let php7 = "PHP 7.2.24-0ubuntu0.18.04.17 (cli) (built: Feb 23 2023 13:29:25) ( NTS )\n\
                    Copyright (c) 1997-2018 The PHP Group\n\
                    Zend Engine v3.2.0, Copyright (c) 1998-2018 Zend Technologies\n    \
                    with Zend OPcache v7.2.24-0ubuntu0.18.04.17, Copyright (c) 1999-2018, by Zend Technologies\n";
        assert_eq!(filter_php_version_output(php7), "7.2.24-0ubuntu0.18.04.17");

        let provider = "PHP 8.5.0 (cli) (built: Nov 18 2025 08:02:20) (NTS)\n\
                        Copyright (c) The PHP Group\n\
                        Built by Debian\n\
                        Zend Engine v4.5.0, Copyright (c) Zend Technologies\n    \
                        with blackfire v1.92.0~linux-x64-non_zts85, https://blackfire.io, by Blackfire\n    \
                        with the ionCube PHP Loader v14.0.0, Copyright (c) 2002-2025, by ionCube Ltd.\n";
        assert_eq!(filter_php_version_output(provider), "8.5.0");
    }

    #[test]
    fn test_php_version_strips_ansi() {
        let raw = "\x1b[1mPHP 8.4.1 (cli) (built: Nov 20 2024 09:48:35) (NTS)\x1b[0m\n\
                   Copyright (c) The PHP Group\n\
                   Zend Engine v4.4.1, Copyright (c) Zend Technologies\n";
        assert_eq!(filter_php_version_output(raw), "8.4.1");
    }

    /// Banner lines are only banner lines right after the header: the same text anywhere
    /// else is something else's output and stays.
    #[test]
    fn test_php_version_only_collapses_the_banner_block() {
        let raw = "Copyright (c) The PHP Group\n\
                   PHP 8.4.1 (cli) (built: Nov 20 2024 09:48:35) (NTS)\n\
                   Copyright (c) The PHP Group\n\
                   Zend Engine v4.4.1, Copyright (c) Zend Technologies\n\
                   PHP Deprecated:  Something in Unknown on line 0\n    \
                   with Xdebug v3.4.0, Copyright (c) 2002-2024, by Derick Rethans\n";
        assert_eq!(
            filter_php_version_output(raw),
            "Copyright (c) The PHP Group\n8.4.1\nPHP Deprecated:  Something in Unknown on line 0\n    \
             with Xdebug v3.4.0, Copyright (c) 2002-2024, by Derick Rethans"
        );
    }

    /// No recognisable header means this is not the banner (an invalid option printed
    /// usage, PHP failed to start): output is left as `rtk php` always left it.
    #[test]
    fn test_php_version_falls_back_without_a_header() {
        for raw in [
            "",
            "Error in argument 1, char 2: option not found x\nUsage: php [options] [-f] <file> [--] [args...]\n",
            "PHP Warning:  PHP Startup: Unable to load dynamic library 'x' in Unknown on line 0\n",
            "PHP 8 is great\n",
        ] {
            assert_eq!(
                filter_php_version_output(raw),
                filter_php_output(raw),
                "{raw:?}"
            );
        }
    }

    // --- version request detection --------------------------------------
    //
    // Every case below was checked against the real `php` 8.5 binary: PHP stops reading
    // its own options at the first script name or `--`, and the first of -h/-?/-i/-v/-m
    // decides what it prints.

    #[test]
    fn test_requests_version_plain_and_long() {
        assert!(requests_version(&owned(&["-v"])));
        assert!(requests_version(&owned(&["--version"])));
        assert!(requests_version(&owned(&["--version=x"])));
    }

    #[test]
    fn test_requests_version_after_other_options() {
        assert!(requests_version(&owned(&["-n", "-v"])));
        assert!(requests_version(&owned(&["-nv"])));
        assert!(requests_version(&owned(&["-vn"])));
        assert!(requests_version(&owned(&["-d", "memory_limit=1G", "-v"])));
        assert!(requests_version(&owned(&["-dmemory_limit=1G", "-v"])));
        assert!(requests_version(&owned(&[
            "--define",
            "memory_limit=1G",
            "-v"
        ])));
        assert!(requests_version(&owned(&["-c", "/etc/php.ini", "-v"])));
        assert!(requests_version(&owned(&["-f", "script.php", "-v"])));
        assert!(requests_version(&owned(&["-r", "echo 1;", "-v"])));
        assert!(requests_version(&owned(&["--repeat", "2", "-v"])));
        assert!(requests_version(&owned(&["--ini=diff", "-v"])));
        assert!(requests_version(&owned(&["-l", "-v"])));
        assert!(requests_version(&owned(&["-v", "-h"])));
        assert!(requests_version(&owned(&["-v", "-m"])));
        assert!(requests_version(&owned(&["-v", "script.php"])));
        assert!(requests_version(&owned(&["-v", "--", "x"])));
    }

    #[test]
    fn test_requests_version_not_when_another_mode_comes_first() {
        assert!(!requests_version(&owned(&["-m", "-v"])));
        assert!(!requests_version(&owned(&["-i", "-v"])));
        assert!(!requests_version(&owned(&["-h", "-v"])));
        assert!(!requests_version(&owned(&["-?", "-v"])));
        assert!(!requests_version(&owned(&["--help", "-v"])));
        assert!(!requests_version(&owned(&["--usage", "-v"])));
        assert!(!requests_version(&owned(&["--info", "-v"])));
        assert!(!requests_version(&owned(&["--modules", "-v"])));
    }

    #[test]
    fn test_requests_version_not_when_v_is_a_value_or_a_script_argument() {
        // A script's own -v.
        assert!(!requests_version(&owned(&["script.php", "-v"])));
        assert!(!requests_version(&owned(&["artisan", "-v"])));
        assert!(!requests_version(&owned(&["--", "-v"])));
        // --ini's value is attached only, so `diff` is the script name.
        assert!(!requests_version(&owned(&["--ini", "diff", "-v"])));
        // -v consumed as a flag's value.
        assert!(!requests_version(&owned(&["-d", "-v"])));
        assert!(!requests_version(&owned(&["-dv"])));
        assert!(!requests_version(&owned(&["--rf", "-v"])));
        // Not spelled as PHP spells it.
        assert!(!requests_version(&owned(&["--vers"])));
        assert!(!requests_version(&owned(&[])));
        assert!(!requests_version(&owned(&["-l", "app/Foo.php"])));
    }
}
