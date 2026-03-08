use crate::discover::registry;

/// Run the `rtk rewrite` command.
///
/// Prints the RTK-rewritten command to stdout and exits 0.
/// Exits 1 (without output) if the command has no RTK equivalent.
///
/// Used by shell hooks to rewrite commands transparently:
/// ```bash
/// REWRITTEN=$(rtk rewrite "$CMD") || exit 0
/// [ "$CMD" = "$REWRITTEN" ] && exit 0  # already RTK, skip
/// ```
pub fn run(cmd: &str) -> anyhow::Result<()> {
    let excluded = crate::config::Config::load()
        .map(|c| c.hooks.exclude_commands)
        .unwrap_or_default();

    match registry::rewrite_command(cmd, &excluded) {
        Some(rewritten) => {
            print!("{}", rewritten);
            Ok(())
        }
        None => {
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_supported_command_succeeds() {
        assert!(registry::rewrite_command("git status", &[]).is_some());
    }

    #[test]
    fn test_run_unsupported_returns_none() {
        assert!(registry::rewrite_command("terraform plan", &[]).is_none());
    }

    #[test]
    fn test_run_already_rtk_returns_some() {
        assert_eq!(
            registry::rewrite_command("rtk git status", &[]),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_bazel() {
        use std::collections::HashMap;

        // Maps input command -> expected rewritten command
        let commands = HashMap::from([
            // bazel build
            (
                "bazel build //src:bazel-dev",
                Some("rtk bazel build //src:bazel-dev"),
            ),
            // bazel query
            ("bazel query //src/...", Some("rtk bazel query //src/...")),
            (
                "USE_BAZEL_VERSION=8.2.0 bazel query //src/...",
                Some("USE_BAZEL_VERSION=8.2.0 rtk bazel query //src/..."),
            ),
            // bazel run
            (
                "bazel run //src:bazel-dev",
                Some("rtk bazel run //src:bazel-dev"),
            ),
            (
                "bazel run //src:bazel-dev -- --arg0 --arg1=test -c foo/bar",
                Some("rtk bazel run //src:bazel-dev -- --arg0 --arg1=test -c foo/bar"),
            ),
            // bazel test
            (
                "bazel test //src:bazel-dev",
                Some("rtk bazel test //src:bazel-dev"),
            ),
            // bazel passthrough commands
            ("bazel clean", None),
            ("bazel clean --expunge", None),
            ("bazel dump", None),
            ("bazel help", None),
            ("bazel -h", None),
            ("bazel --help", None),
            ("bazel mod tidy", None),
            ("bazel print_action //src:bazel-dev", None),
            ("bazel shutdown", None),
            ("bazel version", None),
        ]);

        for (cmd, expected) in commands {
            let actual = registry::rewrite_command(cmd, &[]);
            assert_eq!(
                expected.map(|s| s.to_string()),
                actual.map(|s| s.to_string())
            );
        }
    }
}
