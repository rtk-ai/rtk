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
//! machine's settings (#3146); the no-op-rewrite policy lives in
//! [`decide_for_agent`], which every hook shares and the CLI does not; and
//! whether the caller runs its own approval gate on the result lives in
//! [`ApprovalOwner`], which only ever relaxes the *default* ask.

use super::permissions::{Host, PermissionVerdict, check_command_for};
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

/// The environment variable a delegate sets to say which agent it speaks for.
///
/// Deliberately not a CLI flag. `Commands::Rewrite`'s positional is
/// `trailing_var_arg = true, allow_hyphen_values = true`, so an rtk that does
/// not know a flag folds it into the command text it is asked to judge — and
/// the permission gate then sees `--host openclaw git push` where the user
/// wrote `git push`, which no `Bash(git push *)` deny rule matches. A delegate
/// ships independently of the binary, so that skew is the normal case during
/// an upgrade, not an edge one. An rtk that does not know this variable
/// ignores it and keeps its current behaviour, which is the only arrangement
/// where old and new cannot disagree about a deny.
/// `the_host_is_not_an_argv_token_so_versions_cannot_disagree` in
/// `tests/hook_decision_protocol_test.rs` pins both halves.
pub(crate) const REWRITE_HOST_ENV: &str = "RTK_REWRITE_HOST";

/// Who decides whether the rewritten command may actually run.
///
/// `rtk rewrite` reports a decision through an exit code, and delegates read
/// that code in one of two ways. Most treat it as the permission decision
/// itself. OpenClaw does not: it applies `tools.exec.mode`, `security` and
/// `ask` to whatever the `before_tool_call` hook hands back, so an `Ask` from
/// RTK becomes a *second* prompt, sourced from Claude Code's settings files,
/// on a runtime that never opted into them (#3908).
///
/// This only ever relaxes the *default* ask. It is applied by
/// [`ApprovalOwner::apply`], which matches on a [`HookDecision::AskRewrite`]
/// carrying [`PermissionVerdict::Default`] alone. A `Default` verdict means no
/// rule matched, so RTK is imposing another agent's settings on a runtime that
/// never opted into them — that is the prompt #3908 is about. An explicit
/// [`PermissionVerdict::Ask`] is the user's own instruction and is left for the
/// host to honour, and a [`HookDecision::Deny`] and a [`HookDecision::Defer`]
/// are structurally out of reach — a host name cannot turn a denied command
/// into an allowed rewrite, nor discard an explicit ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalOwner {
    /// RTK's exit code is the permission decision. The default, and what every
    /// delegate but OpenClaw wants.
    Rtk,
    /// The delegate gates the rewritten command itself, so RTK asking too
    /// would be the second gate. Its deny gate still applies.
    Delegate,
}

impl ApprovalOwner {
    /// Read [`REWRITE_HOST_ENV`], resolving the name through [`AgentPath`] so
    /// there is one list of agent names rather than a second one here.
    ///
    /// Fails closed. An unknown name, a differently-cased one, an agent that
    /// does not reach RTK through `rtk rewrite` at all, or no variable set
    /// gives [`ApprovalOwner::Rtk`] — the stricter behaviour, and today's. A
    /// typo must never borrow another host's rules or drop a gate, which is
    /// what an unknown-value fallback to a *named* host would do.
    pub(crate) fn from_env() -> Self {
        match std::env::var(REWRITE_HOST_ENV) {
            Ok(name) => match AgentPath::lookup(&name) {
                Some(AgentPath::ViaRewrite(owner)) => owner,
                _ => Self::Rtk,
            },
            Err(_) => Self::Rtk,
        }
    }

    /// Relax the default ask into an allow when the delegate owns approval.
    ///
    /// Only a [`PermissionVerdict::Default`] verdict relaxes: it means no rule
    /// matched, so RTK is imposing another agent's settings on a runtime that
    /// never opted into them (#3908). An explicit [`PermissionVerdict::Ask`] is
    /// the user's own instruction and is left for the host to honour; every
    /// other decision passes through by construction.
    pub(crate) fn apply(self, decision: HookDecision, verdict: PermissionVerdict) -> HookDecision {
        match (self, verdict, decision) {
            (Self::Delegate, PermissionVerdict::Default, HookDecision::AskRewrite(rewritten)) => {
                HookDecision::AllowRewrite(rewritten)
            }
            (_, _, other) => other,
        }
    }
}

/// Decide what to do with `cmd`, given a permission verdict for it.
///
/// The gate order is load-bearing:
///
/// 1. **Deny wins outright.** Checked before anything else so a denied command
///    is never even considered for rewriting.
/// 2. **A provably-fish script is wrapped.** A host that evaluates the command
///    string with a POSIX layer fails to parse it before RTK is consulted at
///    all, so it is handed back as `rtk run --shell fish -c '<script>'` — with
///    its own commands rewritten first, so wrapping costs no savings. The wrap
///    refuses everything gate 3 refuses, and `hooks.wrap_fish_scripts` turns it
///    off; `src/hooks/README.md` records why it runs ahead of that gate.
/// 3. **Unattestable constructs are refused.** Command substitution and
///    file-target redirects can't be decomposed into segments the permission
///    gate can check individually, so a rewrite could smuggle an unchecked
///    command past an allow rule. `check_command_with_rules` already forces
///    such a command to `Ask`; refusing to rewrite it at all is the stronger
///    guarantee. Heredocs reach the same outcome, though most of them stop at
///    this gate only because a `<<` operand reads as a file target; what
///    actually refuses them is `rewrite_command`'s own `has_heredoc`, which
///    catches the forms this gate lets past (see #3980).
/// 4. **Otherwise rewrite if a rule matches**, and auto-allow only on an
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
    decide_with_wrap(
        cmd,
        verdict,
        excluded,
        transparent_prefixes,
        crate::discover::fish_script::try_wrap,
    )
}

/// [`decide_with_params`] with the fish-script wrapper injected, so a test pins
/// the wrap outcome instead of depending on the machine's `fish` binary and
/// `hooks.wrap_fish_scripts` — the same reason the verdict and the rewrite
/// parameters are passed in rather than looked up.
pub(crate) fn decide_with_wrap(
    cmd: &str,
    verdict: PermissionVerdict,
    excluded: &[String],
    transparent_prefixes: &[String],
    wrap_fish: fn(&str) -> Option<String>,
) -> HookDecision {
    if verdict == PermissionVerdict::Deny {
        return HookDecision::Deny;
    }

    // A provably-fish script fails to parse in a POSIX host layer before RTK is
    // ever consulted, so it is handed back as one quoted argument every layer
    // can parse. Its own commands are rewritten first — the wrap would
    // otherwise cost every saving the rewrite rules deliver for the script's
    // leading command. Never auto-allowed: the script's content is not
    // attested, so its strongest verdict is `Ask` even under an allow rule.
    if wrap_fish(cmd).is_some() {
        let rewritten = rewrite_command(cmd, excluded, transparent_prefixes);
        let script = rewritten.as_deref().unwrap_or(cmd);
        return HookDecision::AskRewrite(crate::discover::fish_script::wrap(script));
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
    suppress_identity(cmd, decide(cmd, verdict))
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
    /// A plugin or shell script that shells out to `rtk rewrite`. Every one of
    /// them is judged against Claude Code's rules, because that entry point has
    /// no rule source of its own — including the deny rules, which is what
    /// keeps an explicit deny enforced for all of them.
    ///
    /// They differ only in what they do with the answer, which is what the
    /// [`ApprovalOwner`] records: a delegate that gates the rewritten command
    /// itself does not want RTK to ask as well for a command no rule matched.
    ViaRewrite(ApprovalOwner),
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
            "antigravity" | "cline" | "kilocode" | "kimi" | "windsurf" => Some(Self::RulesOnly),
            "claude" | "copilot" => Some(Self::InProcess(Host::Claude)),
            "codex" => Some(Self::InProcess(Host::Codex)),
            "trae" => Some(Self::InProcess(Host::Trae)),
            "cursor" => Some(Self::InProcess(Host::Cursor)),
            "droid" => Some(Self::InProcess(Host::Droid)),
            "gemini" => Some(Self::InProcess(Host::Gemini)),
            // OpenClaw applies its own exec policy to whatever the
            // `before_tool_call` hook returns, so RTK asking as well is a
            // second gate on a runtime that never opted into Claude Code's
            // settings (#3908). Its deny gate is unaffected -- see
            // `ApprovalOwner`.
            "openclaw" => Some(Self::ViaRewrite(ApprovalOwner::Delegate)),
            "hermes" | "omp" | "opencode" | "pi" => Some(Self::ViaRewrite(ApprovalOwner::Rtk)),
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
        "trae",
        "vibe",
        "windsurf",
    ];

    /// The verdict this agent's hook would judge `cmd` against.
    fn verdict(&self, cmd: &str) -> PermissionVerdict {
        match self {
            Self::InProcess(host) => check_command_for(cmd, *host),
            // `rtk rewrite` always reads Claude Code's rules, for every
            // delegate. Naming a host changes what is done with the verdict,
            // never where the verdict comes from.
            Self::ViaRewrite(_) => check_command_for(cmd, Host::Claude),
            // No hook, so no rules to consult.
            Self::RulesOnly => PermissionVerdict::Default,
        }
    }

    /// Who owns approval for this agent — [`ApprovalOwner::Rtk`] for every
    /// path but a delegate that gates the rewritten command itself.
    fn approval_owner(&self) -> ApprovalOwner {
        match self {
            Self::ViaRewrite(owner) => *owner,
            Self::InProcess(_) | Self::RulesOnly => ApprovalOwner::Rtk,
        }
    }

    /// What this agent's hook would do with `cmd` — the same answer it gives at
    /// runtime, including discarding a rewrite that changed nothing and
    /// relaxing the default ask the agent would only ask about twice.
    pub(crate) fn decide(&self, cmd: &str) -> HookDecision {
        let verdict = self.verdict(cmd);
        self.approval_owner()
            .apply(decide_for_agent(cmd, verdict), verdict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    mod fish_wrap {
        use super::super::{HookDecision, decide_with_wrap};
        use crate::hooks::permissions::PermissionVerdict;

        /// Real classification and assembly, with the environment gates pinned
        /// open so the answer does not depend on a local `fish`.
        fn wrap_stub(cmd: &str) -> Option<String> {
            crate::discover::fish_script::try_wrap_gated(cmd, true)
        }

        /// Wrap unavailable: no fish binary, flag off, or Windows.
        fn wrap_none(_cmd: &str) -> Option<String> {
            None
        }

        #[cfg(not(windows))]
        #[test]
        fn fish_script_is_wrapped_as_ask() {
            assert_eq!(
                decide_with_wrap(
                    "test -d src; and git status",
                    PermissionVerdict::Default,
                    &[],
                    &[],
                    wrap_stub
                ),
                HookDecision::AskRewrite(
                    "rtk run --shell fish -c 'test -d src; and git status'".to_string()
                )
            );
        }

        /// The wrap used to cost every saving the rewrite rules deliver for the
        /// script's own commands. They are rewritten first, inside the wrap.
        #[cfg(not(windows))]
        #[test]
        fn fish_script_keeps_the_rewrite_it_would_have_had() {
            assert_eq!(
                decide_with_wrap(
                    "git diff HEAD~3 HEAD; and true",
                    PermissionVerdict::Default,
                    &[],
                    &[],
                    wrap_stub
                ),
                HookDecision::AskRewrite(
                    "rtk run --shell fish -c 'rtk git diff HEAD~3 HEAD; and true'".to_string()
                )
            );
        }

        /// The rewrite inside the wrap honours the same configuration the
        /// unwrapped path does.
        #[cfg(not(windows))]
        #[test]
        fn the_nested_rewrite_honours_exclusions() {
            assert_eq!(
                decide_with_wrap(
                    "git diff HEAD~3 HEAD; and true",
                    PermissionVerdict::Default,
                    &["git".to_string()],
                    &[],
                    wrap_stub
                ),
                HookDecision::AskRewrite(
                    "rtk run --shell fish -c 'git diff HEAD~3 HEAD; and true'".to_string()
                )
            );
        }

        /// An explicit allow rule cannot promote the wrap: the script's content
        /// was never parsed, so `Ask` is its strongest verdict.
        #[cfg(not(windows))]
        #[test]
        fn fish_wrap_is_never_auto_allowed() {
            assert!(matches!(
                decide_with_wrap(
                    "test -d src; and git status",
                    PermissionVerdict::Allow,
                    &[],
                    &[],
                    wrap_stub
                ),
                HookDecision::AskRewrite(_)
            ));
        }

        /// A deny rule still wins: the wrap is never consulted.
        #[test]
        fn deny_outranks_the_wrap() {
            assert_eq!(
                decide_with_wrap(
                    "test -d src; and git status",
                    PermissionVerdict::Deny,
                    &[],
                    &[],
                    wrap_stub
                ),
                HookDecision::Deny
            );
        }

        #[test]
        fn without_a_wrap_the_command_takes_its_usual_path() {
            // No fish available: `and git status` is just a command bash would
            // run, and nothing in it is rewritable, so the decision defers.
            assert_eq!(
                decide_with_wrap(
                    "test -d src; and git status",
                    PermissionVerdict::Default,
                    &[],
                    &[],
                    wrap_none
                ),
                HookDecision::Defer
            );
        }

        #[test]
        fn posix_script_is_not_wrapped() {
            assert!(matches!(
                decide_with_wrap(
                    "git status; if true; then echo x; fi",
                    PermissionVerdict::Default,
                    &[],
                    &[],
                    wrap_stub
                ),
                HookDecision::AskRewrite(rewritten) if rewritten.starts_with("rtk git status")
            ));
        }
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

    #[test]
    fn codex_uses_shared_decision_without_claiming_permission() {
        assert!(matches!(
            AgentPath::lookup("codex"),
            Some(AgentPath::InProcess(Host::Codex))
        ));
        assert_eq!(
            check_command_for("git status", Host::Codex),
            PermissionVerdict::Default
        );
        let (deny, ask, allow) = super::super::permissions::load_rules_for(Host::Codex);
        assert!(deny.is_empty() && ask.is_empty() && allow.is_empty());
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

    /// The load-bearing property of [`ApprovalOwner`]: it relaxes the
    /// *default* ask and touches nothing else. A deny reaching `AllowRewrite`
    /// would auto-apply a command the user forbade, so it is asserted directly
    /// rather than left to the exit-code layer. An explicit ask is a rule the
    /// user wrote and must survive.
    #[test]
    fn a_delegate_owning_approval_relaxes_the_default_ask_only() {
        let rewritten = || "rtk git status".to_string();
        assert_eq!(
            ApprovalOwner::Delegate.apply(
                HookDecision::AskRewrite(rewritten()),
                PermissionVerdict::Default
            ),
            HookDecision::AllowRewrite(rewritten())
        );
        assert_eq!(
            ApprovalOwner::Delegate.apply(
                HookDecision::AskRewrite(rewritten()),
                PermissionVerdict::Ask
            ),
            HookDecision::AskRewrite(rewritten())
        );
        assert_eq!(
            ApprovalOwner::Delegate.apply(HookDecision::Deny, PermissionVerdict::Default),
            HookDecision::Deny
        );
        assert_eq!(
            ApprovalOwner::Delegate.apply(HookDecision::Defer, PermissionVerdict::Default),
            HookDecision::Defer
        );
        assert_eq!(
            ApprovalOwner::Delegate.apply(
                HookDecision::AllowRewrite(rewritten()),
                PermissionVerdict::Allow
            ),
            HookDecision::AllowRewrite(rewritten())
        );
    }

    /// The default owner is the identity, so nothing moves for the delegates
    /// that read RTK's exit code as the permission decision.
    #[test]
    fn rtk_owning_approval_changes_no_decision() {
        for (verdict, decision) in [
            (
                PermissionVerdict::Ask,
                HookDecision::AskRewrite("rtk git status".to_string()),
            ),
            (
                PermissionVerdict::Allow,
                HookDecision::AllowRewrite("rtk git status".to_string()),
            ),
            (PermissionVerdict::Deny, HookDecision::Deny),
            (PermissionVerdict::Default, HookDecision::Defer),
        ] {
            let expected = match &decision {
                HookDecision::AskRewrite(r) => HookDecision::AskRewrite(r.clone()),
                HookDecision::AllowRewrite(r) => HookDecision::AllowRewrite(r.clone()),
                HookDecision::Deny => HookDecision::Deny,
                HookDecision::Defer => HookDecision::Defer,
            };
            assert_eq!(ApprovalOwner::Rtk.apply(decision, verdict), expected);
        }
    }

    /// OpenClaw is the only agent that owns approval, and it is still judged
    /// against Claude Code's rules -- including their deny list, which is what
    /// keeps an explicit deny enforced there.
    #[test]
    fn openclaw_is_the_only_agent_that_owns_approval() {
        for name in AgentPath::AGENTS {
            let owner = AgentPath::lookup(name)
                .expect("listed agent resolves")
                .approval_owner();
            let expected = if *name == "openclaw" {
                ApprovalOwner::Delegate
            } else {
                ApprovalOwner::Rtk
            };
            assert_eq!(owner, expected, "agent: {name}");
        }

        assert!(matches!(
            AgentPath::lookup("openclaw"),
            Some(AgentPath::ViaRewrite(ApprovalOwner::Delegate))
        ));
        assert_eq!(
            AgentPath::lookup("openclaw")
                .expect("openclaw resolves")
                .verdict("git status"),
            check_command_for("git status", Host::Claude),
            "naming a host must not change whose rules are read"
        );
    }

    /// The environment name resolves through [`AgentPath::lookup`], so there
    /// is one vocabulary; everything else is the stricter default.
    #[test]
    fn the_host_environment_variable_fails_closed() {
        temp_env::with_var(REWRITE_HOST_ENV, Some("openclaw"), || {
            assert_eq!(ApprovalOwner::from_env(), ApprovalOwner::Delegate);
        });
        for name in [
            // Not a delegate at all: an in-process host cannot claim the
            // relaxation by naming itself on the `rtk rewrite` path.
            "claude",
            "cursor",
            "codex",
            "vibe", // Delegates that keep RTK's gate.
            "pi",
            "hermes",
            "opencode",
            "omp", // Nothing that resolves at all.
            "open-claw",
            "OpenClaw",
            "openclaw ",
            "",
            "nope",
        ] {
            temp_env::with_var(REWRITE_HOST_ENV, Some(name), || {
                assert_eq!(
                    ApprovalOwner::from_env(),
                    ApprovalOwner::Rtk,
                    "name: {name:?}"
                );
            });
        }
        temp_env::with_var_unset(REWRITE_HOST_ENV, || {
            assert_eq!(ApprovalOwner::from_env(), ApprovalOwner::Rtk);
        });
    }
}
