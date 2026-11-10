//! Recovery coordinates for native ya `[fail]` records.
//!
//! Derive coordinates from the persisted raw prefix, never from compacted blocks
//! or the interleaved feed_line order. Streaming capture stores stdout followed
//! by stderr; pipe capture stores stdin verbatim. Both use this same index.

use crate::cmds::yandex::adapters::generic_fail::MAX_FAIL_BLOCKS;
use crate::core::tee::SavedOutput;
use crate::core::utils::strip_ansi;

fn failure_lines(raw: &str) -> impl Iterator<Item = (usize, String)> + '_ {
    raw.lines().enumerate().filter_map(|(index, line)| {
        let clean = strip_ansi(line);
        let label = clean.trim();
        label
            .starts_with("[fail]")
            .then(|| (index + 1, label.chars().take(160).collect()))
    })
}

pub fn has_failures(raw: &str) -> bool {
    failure_lines(raw).next().is_some()
}

/// Short index and a bounded, shell-quoted first read; never a `cat`/unbounded tail.
pub fn hint(raw: &str, saved: &SavedOutput) -> String {
    let mut out = saved.hint();
    let mut failures = failure_lines(saved.complete_prefix(raw));
    let shown: Vec<_> = failures.by_ref().take(MAX_FAIL_BLOCKS).collect();
    if shown.is_empty() {
        return out;
    }
    out.push_str("\n[fail locations in saved output]\n");
    for (line, label) in &shown {
        out.push_str(&format!("  {line}: {label}\n"));
    }
    if let Some((next, _)) = failures.next() {
        let remaining = 1 + failures.count();
        out.push_str(&format!(
            "  ... {remaining} more saved failures; next at line {next}\n"
        ));
    }
    if cfg!(unix) {
        // Use the absolute path. Tilde expansion inside shell quotes is not reliable.
        let path = saved.path.to_string_lossy().replace('\'', "'\\''");
        let first = shown[0].0;
        out.push_str(&format!(
            "[read first failure: sed -n '{first},{}p' '{path}']",
            first + 19
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_lines_include_ansi_blank_crlf_and_duplicate_headlines() {
        let raw = "warning\r\n\r\n\x1b[31m[fail] same\x1b[0m\r\nbody\r\n[fail] same";
        assert_eq!(
            failure_lines(raw).collect::<Vec<_>>(),
            vec![(3, "[fail] same".into()), (5, "[fail] same".into())]
        );
    }

    #[test]
    fn labels_are_bounded_at_utf8_characters() {
        let raw = format!("[fail] {}\n", "漢".repeat(1000));
        let (_, label) = failure_lines(&raw).next().expect("failure");
        assert_eq!(label.chars().count(), 160);
    }
}
