//! Translates a raw shell command into its RTK-optimized equivalent.

use super::decision::{self, HookDecision};
use super::permissions::check_command;
use std::io::Write;

const TEE_READERS: &[&str] = &[
    "cat", "tail", "head", "less", "more", "bat", "grep", "rg", "sed", "awk",
];

fn expand_home(token: &str) -> String {
    if let Some(rest) = token.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    if let Some(rest) = token.strip_prefix("$HOME/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    token.to_string()
}

fn tee_read_slug(cmd: &str, tee_dir: &std::path::Path) -> Option<(String, String)> {
    let first = cmd.split_whitespace().next()?;
    let reader = first.rsplit('/').next().unwrap_or(first);
    if !TEE_READERS.contains(&reader) {
        return None;
    }
    for token in cmd.split_whitespace() {
        let t = token.trim_matches(|c| c == '"' || c == '\'');
        if !t.ends_with(".log") {
            continue;
        }
        let expanded = expand_home(t);
        let path = std::path::Path::new(&expanded);
        if !path.starts_with(tee_dir) {
            continue;
        }
        let stem = path.file_stem()?.to_str()?;
        if let Some((epoch, slug)) = stem.split_once('_')
            && !epoch.is_empty()
            && epoch.chars().all(|c| c.is_ascii_digit())
            && !slug.is_empty()
        {
            return Some((slug.to_string(), expanded));
        }
    }
    None
}

pub(crate) fn track_tee_read(cmd: &str) {
    if !cmd.contains(".log") {
        return;
    }
    let Some(tee_dir) = crate::core::tee_file::resolved_tee_dir() else {
        return;
    };
    if let Some((slug, path)) = tee_read_slug(cmd, &tee_dir) {
        crate::core::retriever::record_tee_recall(&slug, &path);
    }
}

/// Run the `rtk rewrite` command.
///
/// Prints the RTK-rewritten command to stdout and exits with a code that tells
/// the caller how to handle permissions:
///
/// | Exit | Stdout   | Meaning                                                      |
/// |------|----------|--------------------------------------------------------------|
/// | 0    | rewritten| Rewrite allowed — hook may auto-allow the rewritten command. |
/// | 1    | (none)   | No RTK equivalent — hook passes through unchanged.           |
/// | 2    | (none)   | Deny rule matched — hook defers to Claude Code native deny.  |
/// | 3    | rewritten| Ask rule matched — hook rewrites but lets Claude Code prompt.|
///
/// The decision itself is [`decision::decide`], shared with the in-process
/// `rtk hook <agent>` path; this function is only its exit-code rendering.
pub fn run(cmd: &str) -> anyhow::Result<()> {
    // `rtk rewrite` is a subprocess entry point with no way to be told which
    // host is asking, so every delegate that shells out to it -- hermes, omp,
    // opencode, openclaw, pi -- is judged against `~/.claude`'s rules. The
    // in-process `rtk hook <agent>` path is host-parameterized instead
    // (`permissions::Host`).
    let decided = decision::decide(cmd, check_command(cmd));
    if !matches!(decided, HookDecision::Deny) {
        track_tee_read(cmd);
    }
    match decided {
        HookDecision::AllowRewrite(rewritten) => {
            print!("{}", rewritten);
            let _ = std::io::stdout().flush();
            Ok(())
        }
        HookDecision::AskRewrite(rewritten) => {
            print!("{}", rewritten);
            let _ = std::io::stdout().flush();
            std::process::exit(3);
        }
        HookDecision::Deny => std::process::exit(2),
        HookDecision::Defer => std::process::exit(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::registry;

    #[test]
    fn test_tee_read_slug_detects_tail_hint_command() {
        let dir = std::path::Path::new("/home/u/.local/share/rtk/tee");
        let cmd = "tail -n +52 /home/u/.local/share/rtk/tee/1755590000_docker-images.log";
        assert_eq!(
            tee_read_slug(cmd, dir),
            Some((
                "docker-images".to_string(),
                "/home/u/.local/share/rtk/tee/1755590000_docker-images.log".to_string()
            ))
        );
    }

    #[test]
    fn test_tee_read_slug_detects_quoted_and_grep() {
        let dir = std::path::Path::new("/home/u/.local/share/rtk/tee");
        let cmd = r#"grep error "/home/u/.local/share/rtk/tee/1755590000_cargo_test.log""#;
        assert_eq!(
            tee_read_slug(cmd, dir).map(|(s, _)| s),
            Some("cargo_test".to_string())
        );
    }

    #[test]
    fn test_tee_read_slug_ignores_non_readers() {
        let dir = std::path::Path::new("/home/u/.local/share/rtk/tee");
        for cmd in [
            "rm /home/u/.local/share/rtk/tee/1755590000_x.log",
            "ls /home/u/.local/share/rtk/tee",
            "mv /home/u/.local/share/rtk/tee/1755590000_x.log /tmp/",
        ] {
            assert_eq!(tee_read_slug(cmd, dir), None, "{cmd}");
        }
    }

    #[test]
    fn test_tee_read_slug_ignores_logs_outside_tee_dir() {
        let dir = std::path::Path::new("/home/u/.local/share/rtk/tee");
        assert_eq!(tee_read_slug("cat /var/log/app_server.log", dir), None);
        assert_eq!(tee_read_slug("tail -f ./build_1234_out.log", dir), None);
    }

    #[test]
    fn test_tee_read_slug_requires_epoch_prefix() {
        let dir = std::path::Path::new("/home/u/.local/share/rtk/tee");
        assert_eq!(
            tee_read_slug("cat /home/u/.local/share/rtk/tee/notes_perso.log", dir),
            None
        );
    }

    #[test]
    fn test_tee_read_slug_reader_with_absolute_path() {
        let dir = std::path::Path::new("/home/u/.local/share/rtk/tee");
        let cmd = "/usr/bin/tail -n +5 /home/u/.local/share/rtk/tee/17_gh-prs.log";
        assert_eq!(
            tee_read_slug(cmd, dir).map(|(s, _)| s),
            Some("gh-prs".to_string())
        );
    }

    fn rewrite_command_no_prefixes(cmd: &str) -> Option<String> {
        registry::rewrite_command(cmd, &[], &[])
    }

    #[test]
    fn test_run_supported_command_succeeds() {
        assert!(rewrite_command_no_prefixes("git status").is_some());
    }

    #[test]
    fn test_run_unsupported_returns_none() {
        assert!(rewrite_command_no_prefixes("htop").is_none());
    }

    #[test]
    fn test_run_already_rtk_returns_some() {
        assert_eq!(
            rewrite_command_no_prefixes("rtk git status"),
            Some("rtk git status".into())
        );
    }

    /// SECURITY: Verify the exit code protocol for permission verdicts.
    ///
    /// The bash hook (.claude/hooks/rtk-rewrite.sh) interprets exit codes as:
    ///   0 → auto-allow (sets permissionDecision: "allow")
    ///   1 → passthrough (no RTK equivalent)
    ///   2 → deny (let Claude Code handle natively)
    ///   3 → ask (rewrite but omit permissionDecision, forcing user prompt)
    ///
    /// CRITICAL: PermissionVerdict::Default MUST map to exit 3 (ask), NOT exit 0.
    /// If Default were mapped to exit 0, any command without an explicit permission
    /// rule would be auto-allowed — bypassing Claude Code's least-privilege default.
    /// See: https://github.com/rtk-ai/rtk/issues/1155
    mod exit_code_protocol {
        use super::registry;
        use crate::hooks::permissions::{PermissionVerdict, check_command_with_rules};

        /// Exit code that `run()` returns for each verdict:
        ///   Allow  → 0 (exit Ok(()))
        ///   Ask    → 3 (process::exit(3))
        ///   Default→ 3 (process::exit(3)) — grouped with Ask
        ///   Deny   → 2 (process::exit(2)) — handled before rewrite match
        fn expected_exit_code(verdict: &PermissionVerdict) -> i32 {
            match verdict {
                PermissionVerdict::Allow => 0,
                PermissionVerdict::Deny => 2,
                PermissionVerdict::Ask => 3,
                PermissionVerdict::Default => 3, // MUST be 3, not 0!
            }
        }

        #[test]
        fn test_default_verdict_maps_to_ask_exit_code() {
            // When no rules match, verdict is Default → exit code must be 3 (ask).
            let verdict = check_command_with_rules("git status", &[], &[], &[]);
            assert_eq!(verdict, PermissionVerdict::Default);
            assert_eq!(
                expected_exit_code(&verdict),
                3,
                "Default verdict MUST exit with code 3 (ask), not 0 (allow)"
            );
        }

        #[test]
        fn test_allow_verdict_maps_to_allow_exit_code() {
            let allow = vec!["git *".to_string()];
            let verdict = check_command_with_rules("git status", &[], &[], &allow);
            assert_eq!(verdict, PermissionVerdict::Allow);
            assert_eq!(expected_exit_code(&verdict), 0);
        }

        #[test]
        fn test_ask_verdict_maps_to_ask_exit_code() {
            let ask = vec!["git push".to_string()];
            let verdict = check_command_with_rules("git push origin main", &[], &ask, &[]);
            assert_eq!(verdict, PermissionVerdict::Ask);
            assert_eq!(expected_exit_code(&verdict), 3);
        }

        #[test]
        fn test_deny_verdict_maps_to_deny_exit_code() {
            let deny = vec!["rm -rf".to_string()];
            let verdict = check_command_with_rules("rm -rf /tmp/test", &deny, &[], &[]);
            assert_eq!(verdict, PermissionVerdict::Deny);
            assert_eq!(expected_exit_code(&verdict), 2);
        }

        #[test]
        fn test_no_auto_allow_bypass_for_unrecognized_commands() {
            // SECURITY: A command with no permission rules and no matching allow rule
            // must NOT be auto-allowed. This is the core of issue #1155.
            // Even though `git status` can be rewritten to `rtk git status`,
            // the absence of an allow rule means Default → exit 3 → ask.
            let verdict = check_command_with_rules("git status", &[], &[], &[]);
            assert_eq!(verdict, PermissionVerdict::Default);

            // Verify the rewrite exists (so the hook would output it),
            // but the exit code forces user confirmation.
            assert!(registry::rewrite_command("git status", &[], &[]).is_some());
            assert_eq!(expected_exit_code(&verdict), 3);
        }

        #[test]
        fn test_default_never_equals_allow() {
            // Sentinel: ensure Default and Allow are distinct enum variants.
            // If this ever fails, the entire permission model is broken.
            assert_ne!(PermissionVerdict::Default, PermissionVerdict::Allow);
        }
    }
}
