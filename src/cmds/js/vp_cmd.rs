//! Compact the native success summary for one-shot Vite+ tests.
use regex::Regex;
use std::{
    io::{self, IsTerminal, Write},
    sync::LazyLock,
};

use crate::core::{
    guard::never_worse,
    shell::display_args,
    stream::{die_by_relayed_signal, exec_capture_stdin_with_relay},
    tracking::TimedExecution,
    utils::{ChildArgExt, resolved_command, strip_ansi},
};

pub fn one_shot(args: &[String]) -> bool {
    let [command, tail @ ..] = args else {
        return false;
    };
    // Leave terminal-driven progress and interactive stdin to the native tool.
    command == "test"
        && !io::stdin().is_terminal()
        && !io::stdout().is_terminal()
        && !io::stderr().is_terminal()
        && !tail.first().is_some_and(|arg| {
            matches!(arg.as_str(), "watch" | "dev" | "related" | "bench" | "list")
        })
        // Never guess the intent of flags (reporters, output files, watch, help,
        // coverage, etc.) or change which options/configuration Vitest sees.
        && tail.iter().all(|arg| {
            !arg.starts_with('-') || matches!(arg.as_str(), "--run" | "--watch=false")
        })
}

pub fn run(args: &[String]) -> i32 {
    let result = run_test(args);
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    die_by_relayed_signal();
    result
}

fn run_test(args: &[String]) -> i32 {
    let timer = TimedExecution::start();
    let mut cmd = resolved_command("vp");
    cmd.child_args(args);
    let result = match exec_capture_stdin_with_relay(&mut cmd) {
        Ok(result) => result,
        Err(error) => {
            // The native fallback uses 127 on an execution error. Never retry
            // here: the child may already have started and run some tests.
            eprintln!("[rtk: {error:#}]");
            return 127;
        }
    };
    let filtered = if result.success() {
        compact_success(&result.stdout)
    } else {
        result.stdout.clone()
    };
    let stdout = never_worse(&result.stdout, &filtered);
    print!("{stdout}");
    eprint!("{}", result.stderr);

    let display = display_args(args);
    timer.track(
        &format!("vp {display}"),
        &format!("rtk vp {display}"),
        &result.combined(),
        &format!("{stdout}{}", result.stderr),
    );
    result.exit_code
}

fn compact_success(raw: &str) -> String {
    static BANNER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^ RUN  v\d+\.\d+\.\d+\S* .+$").unwrap());
    static FILES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*Test Files  \d+ passed(?: \| \d+ skipped)? \(\d+\)$").unwrap()
    });
    static TESTS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*Tests  \d+ passed(?: \| \d+ skipped)? \(\d+\)$").unwrap()
    });
    static START: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*Start at  \d{2}:\d{2}:\d{2}$").unwrap());
    static DURATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*Duration  \d+(?:\.\d+)?m?s \([^\r\n]+\)$").unwrap());

    let lines: Vec<_> = raw.split_inclusive('\n').collect();
    let clean: Vec<_> = lines
        .iter()
        .map(|line| strip_ansi(line.trim_end_matches(['\r', '\n'])))
        .collect();
    let Some(start) = clean.iter().position(|line| !line.is_empty()) else {
        return raw.to_owned();
    };
    let nonblank_end = clean.iter().rposition(|line| !line.is_empty()).unwrap() + 1;
    // Large suites may append an isolate optimization hint. Preserve it and
    // its padding; don't guess whether other trailer text is a reporter.
    let end = match clean[..nonblank_end] {
        [.., ref headline, ref detail]
            if headline.trim_start().starts_with("Isolate  ")
                && detail.trim_start().starts_with("at least ~")
                && detail.contains("faster with isolate: false") =>
        {
            clean[..nonblank_end - 2]
                .iter()
                .rposition(|line| !line.is_empty())
                .map_or(0, |index| index + 1)
        }
        _ => nonblank_end,
    };
    if end < start + 5
        || !BANNER.is_match(&clean[start])
        || !FILES.is_match(&clean[end - 4])
        || !TESTS.is_match(&clean[end - 3])
        || !START.is_match(&clean[end - 2])
        || !DURATION.is_match(&clean[end - 1])
    {
        return raw.to_owned();
    }

    // Keep all output between the banner and the summary, including custom
    // test logs. Only skip the blank padding just after the banner.
    let body_start = (start + 1..end - 4)
        .find(|&index| !clean[index].is_empty())
        .unwrap_or(end - 4);
    let mut compact = lines[body_start..end - 2].concat();
    if end != nonblank_end {
        compact.push_str(&lines[end..].concat());
    }
    compact
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_color_counts_and_custom_logs() {
        let raw = "\r\n \x1b[7mRUN\x1b[0m  v5.1.1 /test\r\n\r\nstdout | example.test.ts\r\nhello world\r\n\r\n Test Files  1 passed (1)\r\n      Tests  \x1b[32m2 passed\x1b[0m | 1 skipped (3)\r\n   Start at  08:00:05\r\n   Duration  1.41s (transform 1%)\r\n";
        let compact = compact_success(raw);
        assert_eq!(
            compact,
            "stdout | example.test.ts\r\nhello world\r\n\r\n Test Files  1 passed (1)\r\n      Tests  \x1b[32m2 passed\x1b[0m | 1 skipped (3)\r\n"
        );
    }

    #[test]
    fn unrecognized_output_is_not_trimmed() {
        for raw in [
            "\n  \n",
            " Test Files  1 passed (1)\n      Tests  2 passed (2)\n",
            " RUN  v5.1.1 /test\n Test Files  1 passed (1)\n      Tests  2 passed (2)\n   Start at  08:00:05\n   Duration  1ms (transform 1%)\nextra reporter output\n",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites></testsuites>\n",
        ] {
            assert_eq!(compact_success(raw), raw);
        }
    }
}
