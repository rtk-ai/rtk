//! The single place RTK decides what a hook should do with a command.
//!
//! Three entry points ask the same question — may this command be rewritten,
//! and may the rewrite be auto-allowed?
//!
//! | Entry point | Verdict source | Identity rewrite |
//! |---|---|---|
//! | `rtk hook <agent>` (`hook_cmd`) | `check_command_for(cmd, host)` | suppressed |
//! | `rtk rewrite` (`rewrite_cmd`, run as a subprocess by the shell/TS/Python delegates) | `check_command` (always `Host::Claude`) | reported |
//! | `rtk hook check` (`main.rs`) | whatever the named `--agent` consults, via [`AgentPath`] | suppressed |
//!
//! [`decide`] is the shared answer. What legitimately differs between callers
//! stays outside it: the verdict is passed in rather than looked up, so each
//! host consults its own permission rules and tests stay independent of the
//! machine's settings (#3146); and the no-op-rewrite policy lives in
//! [`decide_for_agent`], which every hook shares and the CLI does not.

use super::permissions::{check_command_for, Host, PermissionVerdict};
use crate::discover::registry::rewrite_command;

/// What a hook should do with a command.
///
/// `rewrite_cmd` renders these as the exit codes its delegates branch on
/// (`AllowRewrite` → 0, `AskRewrite` → 3, `Deny` → 2, `Defer` → 1); the
/// in-process hosts render them as their own JSON response shapes.
#[derive(Debug, PartialEq)]
pub(crate) enum HookDecision {
    /// Rewrite, and the host may auto-allow it — an explicit user allow rule matched.
    AllowRewrite(String),
    /// Rewrite, but leave approval to the host's own prompt.
    AskRewrite(String),
    /// Say nothing; let the host handle the command untouched.
    Defer,
    /// A deny rule matched — stay out of the way of the host's native deny.
    Deny,
}

/// Decide what to do with `cmd`, given a permission verdict for it.
///
/// The gate order is load-bearing:
///
/// 1. **Deny wins outright.** Checked before anything else so a denied command
///    is never even considered for rewriting.
/// 2. **Unattestable constructs are refused.** Command substitution and
///    file-target redirects can't be decomposed into segments the permission
///    gate can check individually, so a rewrite could smuggle an unchecked
///    command past an allow rule. `check_command_with_rules` already forces
///    such a command to `Ask`; refusing to rewrite it at all is the stronger
///    guarantee. Heredocs reach the same outcome, though most of them stop at
///    this gate only because a `<<` operand reads as a file target; what
///    actually refuses them is `rewrite_command`'s own `has_heredoc`, which
///    catches the forms this gate lets past (see #3980).
/// 3. **Otherwise rewrite if a rule matches**, and auto-allow only on an
///    explicit `Allow`. Every other verdict — including `Default`, where no
///    rule matched at all — yields `AskRewrite`. `Default` must never reach
///    `AllowRewrite`: that would auto-approve every rewritable command on a
///    machine with no permission rules configured (#1155).
///
/// An identity rewrite (`cmd` was already RTK-prefixed) is reported here as a
/// normal rewrite. Callers that want it suppressed apply [`suppress_identity`].
pub(crate) fn decide(cmd: &str, verdict: PermissionVerdict) -> HookDecision {
    let (excluded, transparent_prefixes) = crate::core::config::hook_rewrite_params();
    decide_with_params(cmd, verdict, &excluded, &transparent_prefixes)
}

/// [`decide`] with the rewrite parameters supplied by the caller, mirroring
/// [`check_command_with_rules`](super::permissions::check_command_with_rules).
///
/// `hook_rewrite_params` reads the user's `config.toml`, so a test calling
/// [`decide`] changes answer with the developer's own `exclude_commands` and
/// `transparent_prefixes` — an `exclude_commands = ["git"]` on the machine
/// turns an expected rewrite into a defer, and the test fails there and only
/// there. Taking the parameters keeps the decision under test independent of
/// the host configuration, the same way the verdict is passed in rather than
/// looked up (#3146).
pub(crate) fn decide_with_params(
    cmd: &str,
    verdict: PermissionVerdict,
    excluded: &[String],
    transparent_prefixes: &[String],
) -> HookDecision {
    if verdict == PermissionVerdict::Deny {
        return HookDecision::Deny;
    }

    if crate::discover::lexer::contains_unattestable_construct(cmd) {
        return HookDecision::Defer;
    }

    match rewrite_command(cmd, excluded, transparent_prefixes) {
        Some(rewritten) if verdict == PermissionVerdict::Allow => {
            HookDecision::AllowRewrite(rewritten)
        }
        Some(rewritten) => HookDecision::AskRewrite(rewritten),
        None => HookDecision::Defer,
    }
}

/// [`decide`], plus the no-op suppression every agent applies.
///
/// This is the composition every *hook* wants, as opposed to [`decide`] alone,
/// which is what the `rtk rewrite` CLI renders. Every hook entry point goes
/// through here so they cannot drift apart.
pub(crate) fn decide_for_agent(cmd: &str, verdict: PermissionVerdict) -> HookDecision {
    decide_for_agent_at(cmd, verdict, None)
}

/// [`decide_for_agent`] for a hook whose payload reports the command's working
/// directory: inside a Claude Code managed worktree, `git` is left unrewritten.
///
/// Claude Code's worktree-isolation guard runs on the command *after* the hook
/// has rewritten it, and it only accepts git spelled plainly (or under the few
/// launchers it models). A rewritten `rtk git …` is refused outright as
/// "cannot be shown not to be git", so no git command could run from an
/// isolated session (#3864). The payload carries no isolation flag, but the
/// managed worktrees always live at `<repo>/.claude/worktrees/<name>`, so that
/// path shape is the signal (see [`in_claude_worktree`]). The rest of a chain
/// is still rewritten: `git status && cargo build` → `git status && rtk cargo build`.
pub(crate) fn decide_for_agent_at(
    cmd: &str,
    verdict: PermissionVerdict,
    cwd: Option<&str>,
) -> HookDecision {
    let (mut excluded, transparent_prefixes) = crate::core::config::hook_rewrite_params();
    if cwd.is_some_and(in_claude_worktree) {
        excluded.push("git".to_string());
    }
    suppress_identity(
        cmd,
        decide_with_params(cmd, verdict, &excluded, &transparent_prefixes),
    )
}

/// Whether `cwd` is inside a worktree Claude Code manages
/// (`<repo>/.claude/worktrees/<name>`, or deeper).
pub(crate) fn in_claude_worktree(cwd: &str) -> bool {
    let parts: Vec<&str> = std::path::Path::new(cwd)
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    parts
        .windows(3)
        .any(|w| w[0] == ".claude" && w[1] == "worktrees")
}

/// Turn a rewrite that changed nothing into a [`HookDecision::Defer`].
///
/// A command that is already RTK-prefixed rewrites to itself, and a hook that
/// reports that is asking its host to apply an edit with no effect. What that
/// costs is per host: Copilot IDE renders a rewrite as a deny-with-suggestion,
/// so it refuses the command and tells the user to re-run the very thing they
/// ran; Cursor raises a permission prompt for it. The plugins that shell out to
/// `rtk rewrite` reach the same outcome in their own code --
/// `hooks/opencode/rtk.ts`, `hooks/pi/rtk.ts` (shared with omp),
/// `hooks/hermes/rtk-rewrite/__init__.py` and `openclaw/index.ts` all gate on
/// `rewritten != command`.
///
/// `Defer` is not uniformly neutral, though: Gemini renders it as `ask_user`,
/// so there suppression trades a no-op rewrite for a confirmation prompt even
/// when that host's own allow rule covers the command.
///
/// The comparison is byte-exact while `rewrite_command` returns a trimmed,
/// continuation-collapsed string, so a command differing only in leading or
/// trailing whitespace is not suppressed.
///
/// `rtk rewrite` itself reports the no-op as a normal rewrite, exit 0 or 3 with
/// the command unchanged on stdout. It answers "what is the RTK form of this
/// command", and for an already-prefixed one that form is itself; whether that
/// counts as a change is the caller's question, which is why those plugins
/// compare. `decision_consistency` in `tests/hook_decision_protocol_test.rs`
/// pins the difference.
pub(crate) fn suppress_identity(cmd: &str, decision: HookDecision) -> HookDecision {
    match decision {
        HookDecision::AllowRewrite(ref rewritten) | HookDecision::AskRewrite(ref rewritten)
            if rewritten == cmd =>
        {
            HookDecision::Defer
        }
        other => other,
    }
}

/// How the hook RTK installs for a given `--agent` actually reaches a decision.
///
/// `rtk init` supports more agents than [`Host`] has variants, because they
/// differ in *whose* permission rules their hook consults — not all of them
/// consult any. A diagnostic that ignores that reports the wrong hook's answer.
///
/// They do not differ on a rewrite that changed nothing: every agent discards
/// it. The in-process hosts do so in `hook_cmd`; the ones that shell out to
/// `rtk rewrite` do so in their own plugin, because `rtk rewrite` reports the
/// no-op rather than suppressing it (see [`suppress_identity`]).
pub(crate) enum AgentPath {
    /// `rtk hook <agent>` — decides in this process, against the host's own
    /// permission rules.
    InProcess(Host),
    /// A plugin or shell script that shells out to `rtk rewrite`. That entry
    /// point has no way to be told which host is asking, so it always reads
    /// Claude Code's rules.
    ViaRewrite,
    /// A rules-file install — RTK ships instructions telling the agent to
    /// prefix commands itself. There is no hook and no permission surface, so
    /// only the rewrite rules apply.
    RulesOnly,
}

impl AgentPath {
    /// The path for an `--agent` value, reporting the accepted values on stderr
    /// when there is no such install target.
    ///
    /// Every [`crate::AgentTarget`] must resolve, plus the targets installed by
    /// a flag rather than an enum variant — `rtk init --copilot`, `--gemini`,
    /// `--codex`, `--opencode`, and OpenClaw's own installer — pinned by
    /// `agent_path_covers_every_install_target`.
    // Reached only from `rtk hook check`, never from a hook's own stream.
    #[allow(clippy::print_stderr)]
    pub(crate) fn from_agent(agent: &str) -> Option<Self> {
        let path = Self::lookup(agent);
        if path.is_none() {
            eprintln!(
                "Unknown agent: {} (expected one of: {})",
                agent,
                Self::AGENTS.join(", ")
            );
        }
        path
    }

    /// The mapping itself, so tests can exercise it without writing to stderr.
    fn lookup(agent: &str) -> Option<Self> {
        match agent {
            // `copilot` reads Claude Code's settings rather than a Copilot file
            // (see `hook_cmd`'s `vscode_response` and `copilot_cli_response`).
            "antigravity" | "cline" | "codex" | "kilocode" | "kimi" | "windsurf" => {
                Some(Self::RulesOnly)
            }
            "claude" | "copilot" => Some(Self::InProcess(Host::Claude)),
            "cursor" => Some(Self::InProcess(Host::Cursor)),
            "droid" => Some(Self::InProcess(Host::Droid)),
            "gemini" => Some(Self::InProcess(Host::Gemini)),
            "hermes" | "omp" | "openclaw" | "opencode" | "pi" => Some(Self::ViaRewrite),
            "vibe" => Some(Self::InProcess(Host::Vibe)),
            _ => None,
        }
    }

    /// The `--agent` values [`AgentPath::lookup`] accepts.
    const AGENTS: &'static [&'static str] = &[
        "antigravity",
        "claude",
        "cline",
        "codex",
        "copilot",
        "cursor",
        "droid",
        "gemini",
        "hermes",
        "kilocode",
        "kimi",
        "omp",
        "openclaw",
        "opencode",
        "pi",
        "vibe",
        "windsurf",
    ];

    /// The verdict this agent's hook would judge `cmd` against.
    fn verdict(&self, cmd: &str) -> PermissionVerdict {
        match self {
            Self::InProcess(host) => check_command_for(cmd, *host),
            // `rtk rewrite` always reads Claude Code's rules.
            Self::ViaRewrite => check_command_for(cmd, Host::Claude),
            // No hook, so no rules to consult.
            Self::RulesOnly => PermissionVerdict::Default,
        }
    }

    /// What this agent's hook would do with `cmd` — the same answer it gives at
    /// runtime, including discarding a rewrite that changed nothing.
    pub(crate) fn decide(&self, cmd: &str) -> HookDecision {
        decide_for_agent(cmd, self.verdict(cmd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- #3864: Claude Code worktree isolation ---

    #[test]
    fn in_claude_worktree_matches_only_managed_worktree_paths() {
        assert!(in_claude_worktree("/repo/.claude/worktrees/feat-x"));
        assert!(in_claude_worktree("/repo/.claude/worktrees/feat-x/src"));
        assert!(!in_claude_worktree("/repo"));
        assert!(!in_claude_worktree("/repo/.claude/worktrees"));
        assert!(!in_claude_worktree("/repo/worktrees/feat-x"));
    }

    #[test]
    fn claude_worktree_leaves_git_plain_but_rewrites_the_rest() {
        let wt = Some("/repo/.claude/worktrees/feat-x");
        assert_eq!(
            decide_for_agent_at("git status", PermissionVerdict::Default, wt),
            HookDecision::Defer
        );
        assert_eq!(
            decide_for_agent_at("git status && cargo build", PermissionVerdict::Allow, wt),
            HookDecision::AllowRewrite("git status && rtk cargo build".into())
        );
        assert_eq!(
            decide_for_agent_at("git status", PermissionVerdict::Allow, Some("/repo")),
            HookDecision::AllowRewrite("rtk git status".into())
        );
    }

    /// A rewritable command with no rule matching it is an ask-rewrite, never
    /// an allow-rewrite (#1155).
    #[test]
    fn default_verdict_asks_rather_than_allows() {
        assert!(matches!(
            decide_with_params("git status", PermissionVerdict::Default, &[], &[]),
            HookDecision::AskRewrite(_)
        ));
    }

    #[test]
    fn explicit_allow_permits_the_rewrite() {
        assert!(matches!(
            decide_with_params("git status", PermissionVerdict::Allow, &[], &[]),
            HookDecision::AllowRewrite(_)
        ));
    }

    #[test]
    fn deny_wins_before_anything_else() {
        assert_eq!(
            decide_with_params("git status", PermissionVerdict::Deny, &[], &[]),
            HookDecision::Deny
        );
    }

    #[test]
    fn command_without_an_rtk_equivalent_defers() {
        assert_eq!(
            decide_with_params("htop", PermissionVerdict::Default, &[], &[]),
            HookDecision::Defer
        );
    }

    /// Refused whatever the verdict: the permission gate can't decompose these
    /// into segments it can check, so a rewrite could carry an unchecked
    /// command past an allow rule.
    #[test]
    fn unattestable_constructs_defer() {
        for cmd in [
            "git status `rm -rf /tmp/x`",
            "git status $(rm -rf /tmp/x)",
            "git log --pretty=\"$(rm -rf /tmp/x)\"",
            "git log > /tmp/out.txt",
        ] {
            assert_eq!(
                decide_with_params(cmd, PermissionVerdict::Default, &[], &[]),
                HookDecision::Defer,
                "cmd: {cmd}"
            );
        }
    }

    /// A file-descriptor dup is not a file target — the rewrite still happens.
    #[test]
    fn fd_dup_redirect_still_rewrites() {
        assert!(matches!(
            decide_with_params("git status 2>&1", PermissionVerdict::Default, &[], &[]),
            HookDecision::AskRewrite(_)
        ));
    }

    #[test]
    fn heredoc_defers() {
        assert_eq!(
            decide_with_params(
                "cat <<'EOF'\nhello\nEOF",
                PermissionVerdict::Default,
                &[],
                &[]
            ),
            HookDecision::Defer
        );
    }

    /// `decide` itself reports a no-op rewrite as a normal one; only
    /// `suppress_identity` turns it into a defer. `rtk rewrite` depends on the
    /// former, the in-process hosts on the latter.
    #[test]
    fn identity_rewrite_is_reported_and_only_suppressed_on_request() {
        let cmd = "rtk git status";
        let decided = decide_with_params(cmd, PermissionVerdict::Default, &[], &[]);
        assert_eq!(decided, HookDecision::AskRewrite(cmd.to_string()));
        assert_eq!(suppress_identity(cmd, decided), HookDecision::Defer);
    }

    /// Suppression only fires on an actual no-op — a real rewrite is untouched.
    #[test]
    fn suppress_identity_leaves_a_real_rewrite_alone() {
        let decided = decide_with_params("git status", PermissionVerdict::Allow, &[], &[]);
        assert_eq!(
            suppress_identity("git status", decided),
            HookDecision::AllowRewrite("rtk git status".to_string())
        );
    }

    /// Suppression only ever collapses a rewrite: a decision that carries no
    /// rewritten command passes through untouched, so it can never mask a deny.
    #[test]
    fn suppress_identity_passes_deny_and_defer_through() {
        assert_eq!(
            suppress_identity("x", HookDecision::Deny),
            HookDecision::Deny
        );
        assert_eq!(
            suppress_identity("x", HookDecision::Defer),
            HookDecision::Defer
        );
    }

    /// Every install target `rtk` supports must resolve, or `rtk hook check`
    /// rejects an agent the user really installed. Derived from `AgentTarget`
    /// so a new variant fails here instead of silently going unanswerable.
    #[test]
    fn agent_path_covers_every_install_target() {
        use clap::ValueEnum;

        for variant in crate::AgentTarget::value_variants() {
            let name = variant
                .to_possible_value()
                .expect("AgentTarget variant is not skipped")
                .get_name()
                .to_string();
            assert!(
                AgentPath::lookup(&name).is_some(),
                "unmapped AgentTarget: {name}"
            );
            assert!(
                AgentPath::AGENTS.contains(&name.as_str()),
                "AgentTarget missing from the error message: {name}"
            );
        }

        // Install targets reached by a flag rather than an `AgentTarget`
        // variant, so the loop above cannot see them.
        for name in ["codex", "copilot", "gemini", "openclaw", "opencode"] {
            assert!(AgentPath::lookup(name).is_some(), "unmapped: {name}");
            assert!(AgentPath::AGENTS.contains(&name), "not listed: {name}");
        }
    }

    /// Everything advertised in the error message must actually resolve.
    #[test]
    fn every_listed_agent_resolves() {
        for name in AgentPath::AGENTS {
            assert!(
                AgentPath::lookup(name).is_some(),
                "listed but unmapped: {name}"
            );
        }
    }

    #[test]
    fn agent_path_rejects_the_unknown() {
        assert!(AgentPath::lookup("nope").is_none());
        assert!(AgentPath::lookup("").is_none());
        assert!(AgentPath::lookup("Claude").is_none());
    }

    /// A rules-file agent has no hook and no permission rules, so its answer
    /// must not depend on any host's settings.
    #[test]
    fn rules_only_agent_uses_the_default_verdict() {
        assert_eq!(
            AgentPath::RulesOnly.verdict("git status"),
            PermissionVerdict::Default
        );
    }
}
