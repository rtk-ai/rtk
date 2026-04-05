//! Returns the RTK-rewritten form of a command for use in suggestion hooks.
//!
//! Unlike `rtk rewrite` (which also enforces permission rules), `rtk suggest`
//! only checks whether a command has an RTK equivalent and prints it.
//!
//! Exit codes:
//!   0 + stdout  — suggestion found (caller should emit systemMessage)
//!   1           — no RTK equivalent (caller should exit silently)

use crate::discover::registry;
use std::io::Write;

/// Run the `rtk suggest` command.
pub fn run(cmd: &str) -> anyhow::Result<()> {
    let excluded = crate::core::config::Config::load()
        .map(|c| c.hooks.exclude_commands)
        .unwrap_or_default();

    match registry::rewrite_command(cmd, &excluded) {
        Some(rewritten) if rewritten != cmd => {
            print!("{}", rewritten);
            let _ = std::io::stdout().flush();
            Ok(())
        }
        _ => {
            // No RTK equivalent — exit 1 so the hook knows to skip.
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::discover::registry;

    #[test]
    fn test_suggest_returns_rewrite_for_known_command() {
        let result = registry::rewrite_command("git status", &[]);
        assert!(result.is_some());
        assert_ne!(result.unwrap(), "git status");
    }

    #[test]
    fn test_suggest_returns_none_for_unknown_command() {
        assert!(registry::rewrite_command("htop", &[]).is_none());
    }

    #[test]
    fn test_suggest_skips_excluded_command() {
        let excluded = vec!["curl".to_string()];
        assert!(registry::rewrite_command("curl https://example.com", &excluded).is_none());
    }
}
