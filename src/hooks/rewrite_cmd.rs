//! Translates a raw shell command into its RTK-optimized equivalent.

use super::permissions::{check_command_for, Host, PermissionVerdict};
use crate::discover::registry;
use std::io::Write;

/// Run the `rtk rewrite` command for `host`.
///
/// Prints the RTK-rewritten command to stdout and exits with a code that tells
/// the caller how to handle permissions:
///
/// | Exit | Stdout   | Meaning                                                      |
/// |------|----------|--------------------------------------------------------------|
/// | 0    | rewritten| Rewrite allowed — hook may auto-allow the rewritten command. |
/// | 1    | (none)   | No RTK equivalent — hook passes through unchanged.           |
/// | 2    | (none)   | Deny rule matched — hook defers to the host's native deny.   |
/// | 3    | rewritten| Ask rule matched — hook rewrites but lets the host prompt.   |
///
/// `host` selects which agent's permission rules are consulted; it defaults to
/// [`Host::Claude`] at the CLI layer, so every pre-existing caller keeps today's
/// behavior byte for byte. A host that is its own permission authority
/// (see [`Host::is_permission_authority`]) never receives exit 3.
pub fn run(cmd: &str, host: Host) -> anyhow::Result<()> {
    let (excluded, transparent_prefixes) = crate::core::config::hook_rewrite_params();

    match evaluate_for(cmd, host, &excluded, &transparent_prefixes) {
        RewriteOutcome::Allow(rewritten) => {
            print!("{}", rewritten);
            let _ = std::io::stdout().flush();
            Ok(())
        }
        RewriteOutcome::Ask(rewritten) => {
            print!("{}", rewritten);
            let _ = std::io::stdout().flush();
            std::process::exit(3);
        }
        RewriteOutcome::Deny => std::process::exit(2),
        RewriteOutcome::Passthrough => std::process::exit(1),
    }
}

#[derive(Debug, PartialEq)]
enum RewriteOutcome {
    Allow(String),
    Passthrough,
    Deny,
    Ask(String),
}

fn evaluate_for(
    cmd: &str,
    host: Host,
    excluded: &[String],
    transparent_prefixes: &[String],
) -> RewriteOutcome {
    evaluate_with_verdict_for(
        cmd,
        check_command_for(cmd, host),
        host,
        excluded,
        transparent_prefixes,
    )
}

/// Decision logic for [`evaluate_for`] with the permission verdict supplied by
/// the caller, mirroring
/// [`check_command_with_rules`](super::permissions::check_command_with_rules).
///
/// `check_command_for` reads the machine's settings files, so tests that call
/// [`evaluate_for`] directly would change verdict with the developer's local
/// `settings.local.json`. Taking the verdict as a parameter keeps the rewrite
/// logic under test independent of the host configuration (#3146).
///
/// Verdict handling, in order:
///
/// 1. `Deny` short-circuits to [`RewriteOutcome::Deny`] for **every** host.
/// 2. An unattestable construct short-circuits to [`RewriteOutcome::Passthrough`]
///    for **every** host, so substitution and file-target redirects are never
///    handed back as an allowed rewrite.
/// 3. `Allow` yields [`RewriteOutcome::Allow`].
/// 4. `Ask` and `Default` yield [`RewriteOutcome::Ask`] — except for a host that
///    is its own permission authority, where they yield `Allow` (#3908). See
///    [`Host::is_permission_authority`] for why that is not an escalation.
fn evaluate_with_verdict_for(
    cmd: &str,
    verdict: PermissionVerdict,
    host: Host,
    excluded: &[String],
    transparent_prefixes: &[String],
) -> RewriteOutcome {
    if verdict == PermissionVerdict::Deny {
        return RewriteOutcome::Deny;
    }

    if crate::discover::lexer::contains_unattestable_construct(cmd) {
        return RewriteOutcome::Passthrough;
    }

    match registry::rewrite_command(cmd, excluded, transparent_prefixes) {
        Some(rewritten) => match verdict {
            PermissionVerdict::Allow => RewriteOutcome::Allow(rewritten),
            _ if host.is_permission_authority() => RewriteOutcome::Allow(rewritten),
            _ => RewriteOutcome::Ask(rewritten),
        },
        None => RewriteOutcome::Passthrough,
    }
}

/// [`Host::Claude`] case of [`evaluate_with_verdict_for`].
///
/// Kept so the pre-#3908 assertions below read unchanged: every one of them is a
/// statement about Claude Code, whose gate must not move.
#[cfg(test)]
fn evaluate_with_verdict(
    cmd: &str,
    verdict: PermissionVerdict,
    excluded: &[String],
    transparent_prefixes: &[String],
) -> RewriteOutcome {
    evaluate_with_verdict_for(cmd, verdict, Host::Claude, excluded, transparent_prefixes)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The verdict still drives the outcome: an allow rule yields `Allow`.
    /// Pinning both directions keeps the mapping covered without depending
    /// on which rules the developer happens to have configured.
    #[test]
    fn test_allow_verdict_yields_allow() {
        assert!(matches!(
            evaluate_with_verdict("git status", PermissionVerdict::Allow, &[], &[]),
            RewriteOutcome::Allow(_)
        ));
    }

    #[test]
    fn test_deny_verdict_yields_deny() {
        assert_eq!(
            evaluate_with_verdict("git status", PermissionVerdict::Deny, &[], &[]),
            RewriteOutcome::Deny
        );
    }

    /// Commands with an unattestable construct are always a passthrough,
    /// regardless of permission verdict.
    ///
    /// The verdict is pinned to `Default` rather than going through `evaluate`,
    /// which reads the developer's own `.claude/settings.local.json`: a
    /// `Bash(git *)` allow rule turns the expected `Ask` into `Allow` and these
    /// tests fail on that machine only (#3146). Pinning the verdict keeps the
    /// assertions about the rewrite logic and nothing about the host.
    mod unattestable_passthrough {
        use super::super::{evaluate_with_verdict, RewriteOutcome};
        use crate::hooks::permissions::PermissionVerdict;

        #[test]
        fn test_backtick_substitution_passthrough() {
            assert_eq!(
                evaluate_with_verdict(
                    "git status `rm -rf /tmp/x`",
                    PermissionVerdict::Default,
                    &[],
                    &[]
                ),
                RewriteOutcome::Passthrough
            );
        }

        #[test]
        fn test_dollar_substitution_passthrough() {
            assert_eq!(
                evaluate_with_verdict(
                    "git status $(rm -rf /tmp/x)",
                    PermissionVerdict::Default,
                    &[],
                    &[]
                ),
                RewriteOutcome::Passthrough
            );
        }

        #[test]
        fn test_double_quoted_substitution_passthrough() {
            assert_eq!(
                evaluate_with_verdict(
                    "git log --pretty=\"$(rm -rf /tmp/x)\"",
                    PermissionVerdict::Default,
                    &[],
                    &[]
                ),
                RewriteOutcome::Passthrough
            );
        }

        #[test]
        fn test_file_redirect_passthrough() {
            assert_eq!(
                evaluate_with_verdict(
                    "git log > /tmp/out.txt",
                    PermissionVerdict::Default,
                    &[],
                    &[]
                ),
                RewriteOutcome::Passthrough
            );
        }

        #[test]
        fn test_fd_dup_redirect_still_rewrites() {
            assert!(matches!(
                evaluate_with_verdict("git status 2>&1", PermissionVerdict::Default, &[], &[]),
                RewriteOutcome::Ask(_)
            ));
        }

        #[test]
        fn test_plain_command_still_rewrites() {
            assert!(matches!(
                evaluate_with_verdict("git status", PermissionVerdict::Default, &[], &[]),
                RewriteOutcome::Ask(_)
            ));
        }
    }

    /// SECURITY: Verify the exit code protocol for permission verdicts.
    ///
    /// The bash hook (.claude/hooks/rtk-rewrite.sh) interprets exit codes as:
    ///   0 → auto-allow (sets permissionDecision: "allow")
    ///   1 → passthrough (no RTK equivalent)
    ///   2 → deny (let Claude Code handle natively)
    ///   3 → ask (rewrite but omit permissionDecision, forcing user prompt)
    ///
    /// CRITICAL: under `Host::Claude`, PermissionVerdict::Default MUST map to
    /// exit 3 (ask), NOT exit 0. If Default were mapped to exit 0, any command
    /// without an explicit permission rule would be auto-allowed — bypassing
    /// Claude Code's least-privilege default.
    /// See: https://github.com/rtk-ai/rtk/issues/1155
    ///
    /// The invariant is host-scoped, not global (#3908). It binds a host that
    /// consumes the exit code as a *permission* decision, which is every host
    /// RTK ships a rule source for. A host that is its own permission authority
    /// consumes it as a *rewrite* decision and re-applies its own exec policy
    /// afterwards; its mapping is pinned separately in the
    /// `host_scoped_permission_authority` module below.
    mod exit_code_protocol {
        use super::registry;
        use crate::hooks::permissions::{check_command_with_rules, PermissionVerdict};

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

    /// SECURITY (#3908): the host-scoped half of the #1155 invariant.
    ///
    /// `Host::OpenClaw` is its own permission authority: it re-evaluates the
    /// rewritten command against `tools.exec.mode` / `security` before running
    /// it, so an RTK-side ask adds a second gate derived from a config file
    /// OpenClaw never opted into. Collapsing Ask/Default to Allow there removes
    /// the duplicate gate without removing any gate — the host still has one.
    ///
    /// Two properties must survive the collapse for every host:
    ///   * `Deny` still short-circuits to `Deny`.
    ///   * An unattestable construct still short-circuits to `Passthrough`,
    ///     never to `Allow`.
    ///
    /// The verdict is pinned rather than read from disk for the same reason as
    /// `unattestable_passthrough` above: these are assertions about the rewrite
    /// logic, not about the developer's machine (#3146).
    mod host_scoped_permission_authority {
        use super::super::{evaluate_with_verdict_for, RewriteOutcome};
        use crate::hooks::permissions::{Host, PermissionVerdict};

        fn outcome_for(host: Host, verdict: PermissionVerdict, cmd: &str) -> RewriteOutcome {
            evaluate_with_verdict_for(cmd, verdict, host, &[], &[])
        }

        /// The change itself: no rule matched, and OpenClaw still gets a plain
        /// allowed rewrite rather than exit 3.
        #[test]
        fn test_openclaw_default_yields_allow() {
            assert!(matches!(
                outcome_for(Host::OpenClaw, PermissionVerdict::Default, "git status"),
                RewriteOutcome::Allow(_)
            ));
        }

        /// An explicit ask rule is a Claude-Code-shaped signal. OpenClaw is not
        /// governed by it either, so it collapses the same way.
        #[test]
        fn test_openclaw_ask_yields_allow() {
            assert!(matches!(
                outcome_for(Host::OpenClaw, PermissionVerdict::Ask, "git status"),
                RewriteOutcome::Allow(_)
            ));
        }

        /// #1155 guard: the identical input under `Host::Claude` must still ask.
        /// If this ever returns `Allow`, the Claude Code gate is gone.
        #[test]
        fn test_claude_default_still_yields_ask() {
            assert!(matches!(
                outcome_for(Host::Claude, PermissionVerdict::Default, "git status"),
                RewriteOutcome::Ask(_)
            ));
        }

        /// Only OpenClaw collapses. Every other host keeps exit 3 on `Default`.
        #[test]
        fn test_every_other_host_still_yields_ask_on_default() {
            for host in [
                Host::Claude,
                Host::Cursor,
                Host::Gemini,
                Host::Droid,
                Host::Vibe,
            ] {
                assert!(
                    matches!(
                        outcome_for(host, PermissionVerdict::Default, "git status"),
                        RewriteOutcome::Ask(_)
                    ),
                    "{host:?} must keep the Default -> Ask mapping"
                );
            }
        }

        /// Deny is evaluated before anything host-scoped and pre-empts the
        /// collapse, so a deny verdict still blocks on OpenClaw.
        #[test]
        fn test_openclaw_deny_still_denies() {
            assert_eq!(
                outcome_for(Host::OpenClaw, PermissionVerdict::Deny, "rm -rf /tmp/x"),
                RewriteOutcome::Deny
            );
        }

        /// The substitution / redirect surface stays closed under the
        /// authoritative host: passthrough, never `Allow`.
        #[test]
        fn test_openclaw_unattestable_construct_is_passthrough_never_allow() {
            for cmd in [
                "git status `rm -rf /tmp/x`",
                "git status $(rm -rf /tmp/x)",
                "git log --pretty=\"$(rm -rf /tmp/x)\"",
                "git log > /tmp/out.txt",
            ] {
                assert_eq!(
                    outcome_for(Host::OpenClaw, PermissionVerdict::Default, cmd),
                    RewriteOutcome::Passthrough,
                    "{cmd:?} must not be rewritten for a host-authoritative host"
                );
            }
        }

        /// A command with no RTK equivalent stays a passthrough. The collapse
        /// only reclassifies rewrites, it never invents one.
        #[test]
        fn test_openclaw_unsupported_command_is_passthrough() {
            assert_eq!(
                outcome_for(Host::OpenClaw, PermissionVerdict::Default, "htop"),
                RewriteOutcome::Passthrough
            );
        }
    }
}
