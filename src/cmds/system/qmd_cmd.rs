//! Compact output for QMD virtual-file listings.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;

const MAX_ENTRIES: usize = 50;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("qmd");
    for arg in args {
        cmd.arg(arg);
    }

    let args_display = args.join(" ");
    if verbose > 0 {
        eprintln!("Running: qmd {}", args_display);
    }

    runner::run_filtered(
        cmd,
        "qmd",
        &args_display,
        compact_qmd_ls,
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
}

fn compact_qmd_ls(output: &str) -> String {
    let clean = strip_ansi(output);
    let mut entries = Vec::new();
    let mut is_collection_list = false;

    for line in clean.lines() {
        let trimmed = line.trim();
        if trimmed == "Collections:" {
            is_collection_list = true;
        }
        if let Some(start) = trimmed.find("qmd://") {
            entries.push(trimmed[start..].to_string());
        }
    }

    if entries.is_empty() {
        return clean.trim().to_string();
    }

    let total = entries.len();
    let mut result = Vec::with_capacity(MAX_ENTRIES + 2);
    if is_collection_list {
        result.push("Collections:".to_string());
    }
    result.extend(entries.into_iter().take(MAX_ENTRIES));

    if total > MAX_ENTRIES {
        result.push(format!("... ({} more entries)", total - MAX_ENTRIES));
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacts_file_listing_to_virtual_paths() {
        let output = concat!(
            "1.2 KB  Jan 10 12:30  qmd://notes/meeting.md\n",
            " 98 B  Dec 20  2025  qmd://notes/todo list.md\n",
        );

        assert_eq!(
            compact_qmd_ls(output),
            "qmd://notes/meeting.md\nqmd://notes/todo list.md"
        );
    }

    #[test]
    fn compacts_collection_listing_and_keeps_counts() {
        let output = concat!(
            "\u{1b}[1mCollections:\u{1b}[0m\n\n",
            "  \u{1b}[2mqmd://\u{1b}[0m\u{1b}[36mnotes/\u{1b}[0m  \u{1b}[2m(12 files)\u{1b}[0m\n",
            "  qmd://docs/  (4 files)\n",
        );

        assert_eq!(
            compact_qmd_ls(output),
            "Collections:\nqmd://notes/  (12 files)\nqmd://docs/  (4 files)"
        );
    }

    #[test]
    fn caps_large_listings() {
        let output = (0..55)
            .map(|index| format!("1 KB  Jan 10 12:30  qmd://docs/{index}.md"))
            .collect::<Vec<_>>()
            .join("\n");
        let compact = compact_qmd_ls(&output);

        assert_eq!(compact.lines().count(), 51);
        assert!(compact.contains("qmd://docs/49.md"));
        assert!(!compact.contains("qmd://docs/50.md"));
        assert!(compact.ends_with("... (5 more entries)"));
    }

    #[test]
    fn preserves_non_listing_messages() {
        assert_eq!(
            compact_qmd_ls("No files found in collection: notes\n"),
            "No files found in collection: notes"
        );
    }
}
