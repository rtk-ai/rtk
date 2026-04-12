//! Runs ansible-playbook and filters verbose output to show only changes and failures.
//!
//! Strips ANSI codes, blank lines, `ok:` (implicit success) and `skipping:` lines.
//! Preserves PLAY/TASK headers, `changed:`, `failed:`, `unreachable:`, and PLAY RECAP.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

const MAX_LINES: usize = 60;

lazy_static! {
    static ref OK_LINE: Regex = Regex::new(r"^ok: \[").unwrap();
    static ref SKIPPING_LINE: Regex = Regex::new(r"^skipping: \[").unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("ansible-playbook");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: ansible-playbook {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "ansible-playbook",
        &args.join(" "),
        filter_ansible_playbook_output,
        RunOptions::default(),
    )
}

fn filter_ansible_playbook_output(output: &str) -> String {
    let stripped = strip_ansi(output);
    let lines: Vec<&str> = stripped
        .lines()
        .filter(|line| {
            let t = line.trim();
            !t.is_empty() && !OK_LINE.is_match(t) && !SKIPPING_LINE.is_match(t)
        })
        .collect();

    if lines.len() > MAX_LINES {
        format!(
            "{}\n\n... ({} more lines)",
            lines[..MAX_LINES].join("\n"),
            lines.len() - MAX_LINES
        )
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_strips_ok_and_skipping() {
        let input = "\
PLAY [all] ****
TASK [Gathering Facts] ****
ok: [web01]
ok: [web02]
TASK [Install nginx] ****
changed: [web01]
skipping: [web02]
PLAY RECAP ****
web01 : ok=2 changed=1 unreachable=0 failed=0";
        let result = filter_ansible_playbook_output(input);
        assert!(!result.contains("ok: ["), "ok lines should be stripped");
        assert!(!result.contains("skipping:"), "skipping lines should be stripped");
        assert!(result.contains("PLAY [all]"));
        assert!(result.contains("TASK [Gathering Facts]"));
        assert!(result.contains("TASK [Install nginx]"));
        assert!(result.contains("changed: [web01]"));
        assert!(result.contains("PLAY RECAP"));
        assert!(result.contains("web01 : ok=2 changed=1"));
    }

    #[test]
    fn test_filter_preserves_failed_lines() {
        let input = "\
TASK [Start service] ***
failed: [web01] => {\"msg\": \"Service not found\"}
PLAY RECAP ***
web01 : ok=1 failed=1";
        let result = filter_ansible_playbook_output(input);
        assert!(result.contains("failed: [web01]"));
        assert!(result.contains("PLAY RECAP"));
    }

    #[test]
    fn test_filter_preserves_unreachable_lines() {
        let input = "\
TASK [ping] ***
unreachable: [web03] => {\"msg\": \"Failed to connect\"}
PLAY RECAP ***
web03 : unreachable=1 failed=0";
        let result = filter_ansible_playbook_output(input);
        assert!(result.contains("unreachable: [web03]"));
    }

    #[test]
    fn test_filter_strips_ansi() {
        let input = "\x1b[32mok: [web01]\x1b[0m\nchanged: [web01]";
        let result = filter_ansible_playbook_output(input);
        assert!(!result.contains("ok: [web01]"), "ANSI-colored ok should be stripped");
        assert!(result.contains("changed: [web01]"));
    }

    #[test]
    fn test_filter_truncates_long_output() {
        let lines: Vec<String> = (0..100)
            .map(|i| format!("changed: [host{}]", i))
            .collect();
        let input = lines.join("\n");
        let result = filter_ansible_playbook_output(&input);
        assert!(result.contains("more lines"), "Long output should be truncated");
    }

    #[test]
    fn test_token_savings() {
        let input = "\
PLAY [all] ****
TASK [Gathering Facts] ****
ok: [web01]
ok: [web02]
ok: [web03]
ok: [web04]
ok: [web05]
TASK [Install nginx] ****
ok: [web01]
ok: [web02]
ok: [web03]
ok: [web04]
ok: [web05]
TASK [Start nginx] ****
changed: [web01]
PLAY RECAP ****
web01 : ok=3 changed=1 unreachable=0 failed=0
web02 : ok=2 changed=0 unreachable=0 failed=0";
        let result = filter_ansible_playbook_output(input);
        let savings_pct = 100.0 * (1.0 - result.len() as f64 / input.len() as f64);
        assert!(
            savings_pct >= 30.0,
            "Expected >=30% token savings, got {:.1}%",
            savings_pct
        );
    }
}
