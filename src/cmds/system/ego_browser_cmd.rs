//! Compact Ego Lite snapshots without dropping text, selectors, or named nodes.
use crate::core::arg_tokenizer::tokenize;
use crate::core::runner::{self, RunOptions};
use crate::core::shell::display_args;
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::ffi::OsString;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if !tokenize(args)
        .first()
        .is_some_and(|token| token.is_free_positional() && token.text == "nodejs")
    {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough("ego-browser", &args, verbose);
    }
    let mut command = resolved_command("ego-browser");
    command.args(args);
    runner::run_filtered(
        command,
        "ego-browser",
        &display_args(args),
        filter_snapshot,
        RunOptions::with_tee("ego-browser").inherit_stdin(),
    )
}

pub fn filter_snapshot(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let mut output = Vec::new();
    let mut removed = Vec::new();
    let mut snapshot = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let body = line.trim_start_matches(' ');
        let indent = line.len() - body.len();
        if line.starts_with("[p") && line.contains(" | space ") && line.ends_with(']') {
            snapshot = true;
            removed.clear();
            output.push(line.to_string());
        } else if snapshot && (indent > 0 || body == "root") {
            removed.retain(|level| *level < indent);
            if body == "container" {
                removed.push(indent);
                i += 1;
                continue;
            }
            let mut body = body.to_string();
            if body.starts_with("anchor [")
                && let Some((prefix, fields)) = body.split_once(", loc=href:")
                && let Some((locator, url)) = fields.rsplit_once(", url=")
                && url.strip_suffix(']') == Some(locator)
            {
                body = format!("{prefix}, loc=href:{locator}]");
            }
            if let Some(next) = lines.get(i + 1) {
                let child = next.trim_start_matches(' ');
                let child_indent = next.len() - child.len();
                let only_child = lines.get(i + 2).is_none_or(|after| {
                    after.len() - after.trim_start_matches(' ').len() <= indent
                });
                if child_indent == indent + 2
                    && only_child
                    && let Some(text) = child.strip_prefix("text ")
                {
                    body.push(' ');
                    body.push_str(text);
                    i += 1;
                }
            }
            output.push(format!(
                "{}{}",
                " ".repeat(indent.saturating_sub(removed.len() * 2) / 2),
                body
            ));
        } else {
            snapshot = false;
            output.push(line.to_string());
        }
        i += 1;
    }
    let mut result = output.join("\n");
    if raw.ends_with('\n') {
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_snapshot_preserves_content_and_cuts_at_least_twenty_percent() {
        let raw = include_str!("../../../tests/fixtures/ego_browser_snapshot.txt");
        let filtered = filter_snapshot(raw);
        assert!(filtered.len() * 100 <= raw.len() * 80);
        for line in raw
            .lines()
            .filter(|line| line.trim_start().starts_with("text "))
        {
            assert!(filtered.contains(line.trim_start().trim_start_matches("text ")));
        }
        assert!(filtered.contains("anchor [ref=11, loc=href:/tools/install, url=https://rust-lang.org/tools/install] \"Install\""));
        assert!(filtered.contains("combobox [ref=19] \"English (en-US)\""));
    }

    #[test]
    fn arbitrary_logs_json_errors_and_screenshot_paths_pass_through() {
        for raw in [
            "",
            "container\n  text output\n",
            "{\"result\": [1, 2]}\n",
            "Error: connection failed\n/tmp/screenshot.png\n",
        ] {
            assert_eq!(filter_snapshot(raw), raw);
        }
    }

    #[test]
    fn named_containers_multiple_children_and_post_snapshot_logs_survive() {
        let raw = "[p1 \"Example\" | space \"Test\"(1)]\nroot\n  container\n    container [ref=1]\n      text \"First\"\n      text \"Second\"\nError: example\n  container\n";
        let filtered = filter_snapshot(raw);
        assert!(filtered.contains(" container [ref=1]\n  text \"First\"\n  text \"Second\""));
        assert!(filtered.ends_with("Error: example\n  container\n"));
    }
}
