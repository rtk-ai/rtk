//! The single place RTK decides what a hook should do with a command.
//!
//! Three entry points ask the same question — may this command be rewritten,
//! and may the rewrite be auto-allowed?
//!
//! | Entry point | Rule source (deny/ask/allow, read once per call) | Identity rewrite |
//! |---|---|---|
//! | `rtk hook <agent>` (`hook_cmd`) | `load_rules_for(host)` | suppressed |
//! | `rtk rewrite` (`rewrite_cmd`, run as a subprocess by the shell/TS/Python delegates) | `load_rules_for(Host::Claude)` | reported |
//! | `rtk hook check` (`main.rs`) | whatever the named `--agent` consults, via [`AgentPath`] | suppressed |
//!
//! Each reads its rules once and uses them twice: `check_command_with_rules`
//! judges the command, and the deny side is handed to [`decide`] as well,
//! because the fish wrap has to clear the rules the verdict was judged against
//! (see [`deny_rule_could_match`]).
//!
//! [`decide`] is the shared answer. What legitimately differs between callers
//! stays outside it: the verdict is passed in rather than looked up, so each
//! host consults its own permission rules and tests stay independent of the
//! machine's settings (#3146); the no-op-rewrite policy lives in
//! [`decide_for_agent`], which every hook shares and the CLI does not; and
//! whether the caller runs its own approval gate on the result lives in
//! [`ApprovalOwner`], which only ever relaxes the *default* ask, and never on
//! a fish wrap.

use super::permissions::{Host, PermissionVerdict};
use crate::core::user_env;
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
        match user_env::var(REWRITE_HOST_ENV) {
            Some(name) => match AgentPath::lookup(&name) {
                Some(AgentPath::ViaRewrite(owner)) => owner,
                _ => Self::Rtk,
            },
            None => Self::Rtk,
        }
    }

    /// Relax the default ask into an allow when the delegate owns approval.
    ///
    /// Only a [`PermissionVerdict::Default`] verdict relaxes: it means no rule
    /// matched, so RTK is imposing another agent's settings on a runtime that
    /// never opted into them (#3908). An explicit [`PermissionVerdict::Ask`] is
    /// the user's own instruction and is left for the host to honour; every
    /// other decision passes through by construction.
    ///
    /// A fish wrap never relaxes, whatever the verdict says. The verdict was
    /// judged by segmenters that read the script as bash, and bash does not
    /// even start a command where fish does — so `Default` there means "no
    /// rule was *read*", not "no rule matched". The wrap's own promise is that
    /// its strongest outcome is `Ask`, and that has to hold for the delegate
    /// path too.
    pub(crate) fn apply(self, decision: HookDecision, verdict: PermissionVerdict) -> HookDecision {
        match (self, verdict, decision) {
            (Self::Delegate, PermissionVerdict::Default, HookDecision::AskRewrite(rewritten))
                if !crate::discover::fish_script::is_wrapped(&rewritten) =>
            {
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
///    refuses everything gate 3 refuses; it also refuses whenever a deny rule
///    matches a run of words anywhere in the script, because handing it to a
///    different shell puts commands where gate 1's segmenter does not look
///    ([`deny_rule_could_match`]). `hooks.wrap_fish_scripts` turns it off;
///    `src/hooks/README.md` records why it runs ahead of gate 3.
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
pub(crate) fn decide(cmd: &str, verdict: PermissionVerdict, deny_rules: &[String]) -> HookDecision {
    let (excluded, transparent_prefixes) = crate::core::config::hook_rewrite_params();
    decide_with_params(cmd, verdict, &excluded, &transparent_prefixes, deny_rules)
}

/// [`decide`] with the rewrite parameters supplied by the caller, mirroring
/// [`check_command_with_rules`](super::permissions::check_command_with_rules).
///
/// `hook_rewrite_params` reads `config.toml`, which in a test build is the
/// calling test's own. Taking the parameters lets a test state the
/// `exclude_commands` and `transparent_prefixes` it means instead of writing a
/// file for them, the same way the verdict is passed in rather than looked up
/// (#3146).
pub(crate) fn decide_with_params(
    cmd: &str,
    verdict: PermissionVerdict,
    excluded: &[String],
    transparent_prefixes: &[String],
    deny_rules: &[String],
) -> HookDecision {
    decide_with_wrap(
        cmd,
        verdict,
        excluded,
        transparent_prefixes,
        deny_rules,
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
    deny_rules: &[String],
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
    // attested, so its strongest verdict is `Ask` even under an allow rule
    // (`ApprovalOwner::apply` leaves a wrap alone for the same reason).
    if wrap_fish(cmd).is_some() {
        let rewritten = rewrite_command(cmd, excluded, transparent_prefixes);
        let script = rewritten.as_deref().unwrap_or(cmd);
        // Both texts are asked about: the one the host submitted, and the one
        // RTK would hand to the fish. The rewrite inserts commands of its own
        // (`ls` becomes `rtk ls`), and a rule may name those.
        if !deny_rule_could_match(cmd, deny_rules) && !deny_rule_could_match(script, deny_rules) {
            return HookDecision::AskRewrite(crate::discover::fish_script::wrap(script));
        }
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

/// True when a deny rule could match anything the wrapped script would run.
///
/// The wrap is the one decision that hands a script to a *different* shell, so
/// the rule that would have stopped a command has to hold there too. The
/// segmenters cannot help: they split where bash starts a command, and fish
/// starts one in places bash has no grammar for — the condition of an `if` or
/// a `while`, the right side of `and`/`or`/`not`. So every word of the script
/// is treated as a possible command start, read from the same comment-stripped
/// and dequoted words the classification reads, and the gate's own matcher
/// answers for each run.
///
/// Conservative by construction, twice over: a denied program named as a plain
/// argument keeps the wrap off, and so does a script whose words cannot be
/// read the way fish would read them. Both cost a rewrite, never a guarantee.
fn deny_rule_could_match(cmd: &str, deny_rules: &[String]) -> bool {
    if deny_rules.is_empty() {
        return false;
    }
    let Some(segments) = crate::discover::fish_script::code_word_runs(cmd) else {
        // The words are not known — a comment the two shells read differently,
        // or an escape whose resolution differs between this lexer and fish —
        // and a wrap cannot be cleared on words nobody could read.
        return true;
    };
    super::permissions::deny_matches_any_word_run(&segments, deny_rules)
}

/// [`decide`], plus the no-op suppression every agent applies.
///
/// This is the composition every *hook* wants, as opposed to [`decide`] alone,
/// which is what the `rtk rewrite` CLI renders. Every hook entry point goes
/// through here so they cannot drift apart.
pub(crate) fn decide_for_agent(
    cmd: &str,
    verdict: PermissionVerdict,
    deny_rules: &[String],
) -> HookDecision {
    suppress_identity(cmd, decide(cmd, verdict, deny_rules))
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
            "antigravity" => Some(Self::InProcess(Host::Antigravity)),
            "cline" | "kilocode" | "kimi" | "windsurf" => Some(Self::RulesOnly),
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

    /// The rules this agent's hook judges `cmd` against — read once, because
    /// the verdict and the fish wrap both need them.
    fn rules(&self) -> (Vec<String>, Vec<String>, Vec<String>) {
        match self {
            Self::InProcess(host) => super::permissions::load_rules_for(*host),
            // `rtk rewrite` always reads Claude Code's rules, for every
            // delegate. Naming a host changes what is done with the verdict,
            // never where the verdict comes from.
            Self::ViaRewrite(_) => super::permissions::load_rules_for(Host::Claude),
            // No hook, so no rules to consult.
            Self::RulesOnly => (Vec::new(), Vec::new(), Vec::new()),
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
        let (deny, ask, allow) = self.rules();
        let verdict = super::permissions::check_command_with_rules(cmd, &deny, &ask, &allow);
        self.approval_owner()
            .apply(decide_for_agent(cmd, verdict, &deny), verdict)
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
            decide_with_params("git status", PermissionVerdict::Default, &[], &[], &[]),
            HookDecision::AskRewrite(_)
        ));
    }

    #[test]
    fn explicit_allow_permits_the_rewrite() {
        assert!(matches!(
            decide_with_params("git status", PermissionVerdict::Allow, &[], &[], &[]),
            HookDecision::AllowRewrite(_)
        ));
    }

    #[test]
    fn deny_wins_before_anything_else() {
        assert_eq!(
            decide_with_params("git status", PermissionVerdict::Deny, &[], &[], &[]),
            HookDecision::Deny
        );
    }

    #[test]
    fn command_without_an_rtk_equivalent_defers() {
        assert_eq!(
            decide_with_params("htop", PermissionVerdict::Default, &[], &[], &[]),
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
                decide_with_params(cmd, PermissionVerdict::Default, &[], &[], &[]),
                HookDecision::Defer,
                "cmd: {cmd}"
            );
        }
    }

    /// A file-descriptor dup is not a file target — the rewrite still happens.
    #[test]
    fn fd_dup_redirect_still_rewrites() {
        assert!(matches!(
            decide_with_params("git status 2>&1", PermissionVerdict::Default, &[], &[], &[]),
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
                &[],
                &[],
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
        let decided = decide_with_params(cmd, PermissionVerdict::Default, &[], &[], &[]);
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
                    &[],
                    wrap_stub
                ),
                HookDecision::AskRewrite(
                    "rtk run --shell fish -c 'rtk git diff HEAD~3 HEAD; and true'".to_string()
                )
            );
        }

        /// The wrap hands the script to a *different* shell, and fish starts
        /// commands where the bash-shaped segmenter does not look — the
        /// condition of an `if`/`while`, the right side of `and`/`or`/`not`.
        /// So a deny rule that matches a run of words anywhere keeps the wrap
        /// off rather than being read through one segmentation.
        ///
        /// Each row is a rule and a script the rule has to reach; the property
        /// is *never wrapped*, whatever the decision turns out to be.
        #[cfg(not(windows))]
        #[test]
        fn a_deny_rule_keeps_the_wrap_off_wherever_it_matches() {
            for (rule, cmd) in [
                ("rm:*", "test -d src; and rm -rf victim"),
                ("rm:*", "not rm -rf victim"),
                ("rm:*", "if not rm -rf victim\n  echo x\nend"),
                ("rm:*", "while rm -rf victim\n  break\nend"),
                ("rm:*", "if test -d src\n  true; and rm -rf victim\nend"),
                // The words are compared dequoted, as the shell reads them.
                ("rm:*", "if not 'rm' -rf victim\n  echo x\nend"),
                ("rm:*", "if not r'm' -rf victim\n  echo x\nend"),
                // A word carries the operator glued to it (`push;`), so runs
                // are read per segment rather than across the whole script.
                ("git push:*", "not git push; echo blocked"),
                ("git push:*", "not git push|cat"),
                // A rule may anchor its tail, so a run has to be able to end
                // before the segment does — `echo ARG DENIED` is a command
                // here, with `extra` an argument of the same one.
                ("echo * DENIED", "if not echo ARG DENIED\n  echo BODY\nend"),
                ("echo * DENIED", "not echo ARG DENIED extra; and true"),
                ("rm * victim", "if not rm -rf victim\n  echo x\nend"),
                // A `\r` splits words in fish and stays inside one here, so
                // `git\rpush` runs `git push` and reads as a single word.
                ("git push:*", "not git\rpush; and true"),
                // The rewrite inserts commands of its own, and a rule may name
                // those: the emitted script is asked about as well as the
                // submitted one.
                ("rtk ls:*", "ls; and true"),
                ("rtk git:*", "git diff HEAD~3 HEAD; and true"),
                // Escapes this lexer and fish resolve differently: a `\r` bash
                // keeps inside the word, a line continuation fish elides, and
                // `\x70`, which is `p` to fish and `x70` here. The words cannot
                // be read, so the wrap is refused rather than cleared.
                ("pwd:*", "if not pwd\r\n  echo BODY\r\nend"),
                ("pwd:*", "if not p\\\nwd\n  echo BODY\nend"),
                ("pwd:*", "if not \\x70wd\n  echo BODY\nend"),
                // A redirect ends the word, not the command: fish still passes
                // what follows it to the same program.
                ("echo * DENIED", "not echo A 2>&1 DENIED"),
                ("git push:*", "not git 2>&1 push"),
                // The words arrive dequoted, so a rule the user wrote quoted —
                // the spelling the unwrapped gate matches raw — is tried
                // dequoted too.
                (
                    "echo \"DENIED\"",
                    "if not echo \"DENIED\"\n  echo BODY\nend",
                ),
                ("echo 'DENIED'", "if not echo DENIED\n  echo BODY\nend"),
                // An expansion's value is the command fish runs, and no reading
                // of `$runner` says which.
                (
                    "echo DENIED",
                    "set -l runner echo; $runner DENIED; and true",
                ),
                ("pwd:*", "set -l a p; set -l b wd; $a$b; and true"),
                ("echo:*", "{echo,ls} DENIED; and true"),
                // An empty argument is a word to the lexer and nothing at all
                // to the matcher, so the words either side are adjacent there.
                ("echo * DENIED", "not echo '' A DENIED"),
                // A redirect's operand may be spaced off the operator; what
                // comes after the operand is the command's own argument.
                ("echo DENIED", "not echo 2> /dev/null DENIED"),
                ("echo A DENIED", "not echo A 2> /dev/null DENIED"),
                // A glob, a bracket and a `~` resolve before the command runs,
                // so the word in the text is not the word fish executes.
                ("/bin/echo DENIED", "not /bin/ec*o DENIED"),
                ("/bin/echo DENIED", "not /bin/ec[h]o DENIED"),
                ("echo /var/root", "not echo ~root"),
            ] {
                let deny = vec![rule.to_string()];
                let decided =
                    decide_with_wrap(cmd, PermissionVerdict::Default, &[], &[], &deny, wrap_stub);
                assert!(
                    !matches!(
                        &decided,
                        HookDecision::AskRewrite(rewritten)
                            if rewritten.starts_with("rtk run --shell fish")
                    ),
                    "a denied command must never be wrapped: {rule:?} {cmd:?} -> {decided:?}"
                );
            }
        }

        /// Only what the rules name: an unrelated rule costs nothing, with no
        /// rules at all the gate is not consulted, and a rule naming nothing —
        /// an empty string — does not match the empty argument in `printf %s ''`.
        #[cfg(not(windows))]
        #[test]
        fn an_unrelated_deny_rule_leaves_the_wrap_alone() {
            for (deny, cmd, wrapped) in [
                (
                    vec![],
                    "test -d src; and git status",
                    "rtk run --shell fish -c 'test -d src; and git status'",
                ),
                (
                    vec!["git push:*".to_string()],
                    "test -d src; and git status",
                    "rtk run --shell fish -c 'test -d src; and git status'",
                ),
                (
                    vec![String::new()],
                    "not printf %s ''",
                    "rtk run --shell fish -c 'not printf %s '\\'''\\'''",
                ),
            ] {
                assert_eq!(
                    decide_with_wrap(cmd, PermissionVerdict::Default, &[], &[], &deny, wrap_stub),
                    HookDecision::AskRewrite(wrapped.to_string()),
                    "{deny:?} {cmd:?}"
                );
            }
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
                    &[],
                    wrap_stub
                ),
                HookDecision::AskRewrite(_)
            ));
        }

        /// The delegate relaxation cannot reach the wrap either. A `Default`
        /// verdict on a fish script means the segmenters found no rule to read,
        /// not that the user wrote none — `and cargo test` is a command called
        /// `and` to them, so an explicit `ask` on `cargo test` never matched.
        #[cfg(not(windows))]
        #[test]
        fn the_delegate_relaxation_cannot_auto_allow_a_wrap() {
            let wrapped = decide_with_wrap(
                "test -d src; and cargo test",
                PermissionVerdict::Default,
                &[],
                &[],
                &[],
                wrap_stub,
            );
            assert_eq!(
                super::super::ApprovalOwner::Delegate.apply(wrapped, PermissionVerdict::Default),
                HookDecision::AskRewrite(
                    "rtk run --shell fish -c 'test -d src; and cargo test'".to_string()
                )
            );

            // The relaxation itself is intact for an ordinary rewrite.
            let plain = decide_with_wrap(
                "git status",
                PermissionVerdict::Default,
                &[],
                &[],
                &[],
                wrap_none,
            );
            assert_eq!(
                super::super::ApprovalOwner::Delegate.apply(plain, PermissionVerdict::Default),
                HookDecision::AllowRewrite("rtk git status".to_string())
            );
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
        let decided = decide_with_params("git status", PermissionVerdict::Allow, &[], &[], &[]);
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

    /// The verdict an agent's path judges `cmd` against, read through the one
    /// rule load [`AgentPath::decide`] performs.
    fn verdict_for(path: &AgentPath, cmd: &str) -> PermissionVerdict {
        let (deny, ask, allow) = path.rules();
        super::super::permissions::check_command_with_rules(cmd, &deny, &ask, &allow)
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
            verdict_for(&AgentPath::InProcess(Host::Codex), "git status"),
            PermissionVerdict::Default
        );
        let (deny, ask, allow) = super::super::permissions::load_rules_for(Host::Codex);
        assert!(deny.is_empty() && ask.is_empty() && allow.is_empty());
    }

    #[test]
    fn antigravity_uses_shared_decision_without_claiming_permission() {
        assert!(matches!(
            AgentPath::lookup("antigravity"),
            Some(AgentPath::InProcess(Host::Antigravity))
        ));
        assert_eq!(
            verdict_for(&AgentPath::InProcess(Host::Antigravity), "git status"),
            PermissionVerdict::Default
        );
        let (deny, ask, allow) = super::super::permissions::load_rules_for(Host::Antigravity);
        assert!(deny.is_empty() && ask.is_empty() && allow.is_empty());
    }

    /// A rules-file agent has no hook and no permission rules, so its answer
    /// must not depend on any host's settings.
    #[test]
    fn rules_only_agent_uses_the_default_verdict() {
        assert_eq!(
            verdict_for(&AgentPath::RulesOnly, "git status"),
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
                .rules(),
            super::super::permissions::load_rules_for(Host::Claude),
            "naming a host must not change whose rules are read"
        );
    }

    /// The environment name resolves through [`AgentPath::lookup`], so there
    /// is one vocabulary; everything else is the stricter default.
    #[test]
    fn the_host_environment_variable_fails_closed() {
        user_env::with_vars(&[(REWRITE_HOST_ENV, Some("openclaw"))], || {
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
            user_env::with_vars(&[(REWRITE_HOST_ENV, Some(name))], || {
                assert_eq!(
                    ApprovalOwner::from_env(),
                    ApprovalOwner::Rtk,
                    "name: {name:?}"
                );
            });
        }
        user_env::with_vars(&[(REWRITE_HOST_ENV, None)], || {
            assert_eq!(ApprovalOwner::from_env(), ApprovalOwner::Rtk);
        });
    }
}
