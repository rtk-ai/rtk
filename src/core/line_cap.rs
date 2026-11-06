//! Caps very long lines in output an agent reads, keeping the full output recallable.
//!
//! One line of minified JavaScript, a JSONL record or a one-line JSON document can
//! run to tens of thousands of characters, and `grep`, `cat` or `head -1` hands it to
//! the agent whole. The agents' own file tools already cut a line at 2,000 characters
//! (Claude Code's Read, OpenCode's read and grep); this gives the shell path the same
//! cap. A cut line keeps its first `max` characters and says how many are missing and
//! where to get them. When the full output can't be kept for `rtk recall` (recovery
//! disabled), nothing is cut: a cap that drops content for good is the silent loss
//! #1313 and #2988 are about.

use std::borrow::Cow;

/// The line length the agents' own read tools cut at.
pub const DEFAULT_MAX_LINE_CHARS: usize = 2000;

/// Cuts every line of `output` longer than `[limits] max_line_chars` (0 turns it off),
/// storing the full output for `rtk recall` under `command`.
pub fn cap_long_lines<'a>(output: &'a str, command: &str) -> Cow<'a, str> {
    let max = crate::core::config::limits().max_line_chars;
    cap_with(output, max, || {
        crate::core::tee::force_tee_hint(output, command)
    })
}

/// [`cap_long_lines`] for output kept as bytes. Output that isn't UTF-8 is left alone.
pub fn cap_long_line_bytes<'a>(output: &'a [u8], command: &str) -> Cow<'a, [u8]> {
    // A line's byte length bounds its char count, so this skips the decode for
    // nearly every output.
    let max = crate::core::config::limits().max_line_chars;
    if max == 0 || !output.split(|&b| b == b'\n').any(|line| line.len() > max) {
        return Cow::Borrowed(output);
    }
    match std::str::from_utf8(output) {
        Ok(text) => match cap_long_lines(text, command) {
            Cow::Borrowed(_) => Cow::Borrowed(output),
            Cow::Owned(capped) => Cow::Owned(capped.into_bytes()),
        },
        Err(_) => Cow::Borrowed(output),
    }
}

/// The cap itself, with the recall store passed in so tests never touch it.
/// `hint` is called once, and only when a line is actually cut.
fn cap_with<'a>(
    output: &'a str,
    max: usize,
    hint: impl FnOnce() -> Option<String>,
) -> Cow<'a, str> {
    if max == 0 || !output.split('\n').any(|line| exceeds(line, max)) {
        return Cow::Borrowed(output);
    }
    let Some(hint) = hint() else {
        return Cow::Borrowed(output);
    };
    let mut capped = String::with_capacity(output.len().min(1 << 20));
    for (i, line) in output.split('\n').enumerate() {
        if i > 0 {
            capped.push('\n');
        }
        let (body, cr) = match line.strip_suffix('\r') {
            Some(body) => (body, "\r"),
            None => (line, ""),
        };
        match body.char_indices().nth(max) {
            Some((cut, _)) => {
                let hidden = body[cut..].chars().count();
                capped.push_str(&body[..cut]);
                capped.push_str(&format!("… [+{hidden} chars] {hint}"));
            }
            None => capped.push_str(body),
        }
        capped.push_str(cr);
    }
    Cow::Owned(capped)
}

fn exceeds(line: &str, max: usize) -> bool {
    let line = line.strip_suffix('\r').unwrap_or(line);
    line.len() > max && line.chars().nth(max).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HINT: &str = "[full output: rtk recall 0123456789ab]";

    fn hint() -> Option<String> {
        Some(HINT.to_string())
    }

    #[test]
    fn short_lines_are_returned_untouched_without_storing_anything() {
        let out = "a.ex:1:defmodule A do\na.ex:2:  def f, do: 1\n";
        let capped = cap_with(out, 40, || panic!("nothing to cut, nothing to store"));
        assert!(matches!(capped, Cow::Borrowed(_)));
        assert_eq!(capped, out);
    }

    #[test]
    fn a_line_of_exactly_max_chars_is_kept_whole() {
        let out = format!("{}\n", "x".repeat(10));
        assert!(matches!(cap_with(&out, 10, hint), Cow::Borrowed(_)));
    }

    #[test]
    fn only_long_lines_are_cut_and_each_says_where_the_rest_is() {
        let long = format!(
            "{{\"name\":\"job.execute\",\"attrs\":\"{}\"}}",
            "v".repeat(100)
        );
        let out = format!("first\n{long}\nlast\n");
        let capped = cap_with(&out, 20, hint);
        let lines: Vec<&str> = capped.split('\n').collect();
        assert_eq!(lines[0], "first");
        assert_eq!(
            lines[1],
            format!("{}… [+{} chars] {HINT}", &long[..20], long.len() - 20)
        );
        assert_eq!(lines[2], "last");
        assert_eq!(lines[3], "", "the trailing newline is kept");
    }

    #[test]
    fn the_marker_survives_a_head_that_keeps_only_the_cut_line() {
        // `rtk grep x f.jsonl | head -1`: a hint printed after the output would be cut off.
        let out = format!("{}\n{}\n", "a".repeat(50), "b".repeat(50));
        let capped = cap_with(&out, 10, hint);
        assert!(capped.lines().next().unwrap().ends_with(HINT));
    }

    #[test]
    fn counts_chars_not_bytes() {
        let out = "é".repeat(15);
        let capped = cap_with(&out, 10, hint);
        assert_eq!(capped, format!("{}… [+5 chars] {HINT}", "é".repeat(10)));
    }

    #[test]
    fn keeps_crlf_line_endings() {
        let out = format!("{}\r\nshort\r\n", "a".repeat(12));
        let capped = cap_with(&out, 10, hint);
        assert_eq!(
            capped,
            format!("{}… [+2 chars] {HINT}\r\nshort\r\n", "a".repeat(10))
        );
    }

    #[test]
    fn nothing_is_cut_when_the_full_output_cannot_be_recalled() {
        let out = "z".repeat(50);
        let capped = cap_with(&out, 10, || None);
        assert!(matches!(capped, Cow::Borrowed(_)));
        assert_eq!(capped, out);
    }

    #[test]
    fn zero_turns_the_cap_off() {
        let out = "z".repeat(50);
        assert!(matches!(
            cap_with(&out, 0, || panic!("cap is off")),
            Cow::Borrowed(_)
        ));
    }
}
