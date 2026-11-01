# Hook System

> See also [docs/contributing/TECHNICAL.md](../../docs/contributing/TECHNICAL.md) for the full architecture overview | [hooks/](../../hooks/README.md) for deployed hook artifacts

## Scope

The **lifecycle management** layer for LLM agent hooks: install, uninstall, verify integrity, audit usage, and manage trust. This component creates and maintains the hook artifacts that live in `hooks/` (root), but does **not** execute rewrite logic itself — that lives in `discover/registry`.

Owns: `rtk init` installation flows (Claude Code by default, `--agent <AgentTarget>` for the other enum agents, `--gemini`, `--codex` and `--copilot` for the agents outside the enum, `-g --opencode` for the OpenCode plugin), SHA-256 integrity verification, hook version checking, audit log analysis, `rtk rewrite` CLI entry point, and TOML filter trust management.

Does **not** own: the deployed hook scripts themselves (that's `hooks/`), the rewrite pattern registry (that's `discover/`), or command filtering (that's `cmds/`).

Boundary notes:
- `decision.rs` is the single place RTK decides what a hook should do with a command — deny, defer, rewrite-and-allow, or rewrite-and-ask. All three entry points route through it: the in-process `rtk hook <agent>` hosts (`hook_cmd.rs`), the `rtk rewrite` subprocess path (`rewrite_cmd.rs`), and the `rtk hook check` diagnostic (`main.rs`). Add a gate there, not in a caller.
- `rewrite_cmd.rs` is a thin CLI bridge — it exists to serve hooks (hooks call `rtk rewrite` as a subprocess) and renders `decision.rs`'s verdict as the exit codes those delegates branch on.
- `trust.rs` gates project-local TOML filter execution. It lives here because the trust workflow is tied to hook-installed filter discovery, not to the core filter engine.

## Purpose
LLM agent integration layer that installs, validates, and executes command-rewriting hooks for AI coding assistants. Hooks intercept raw CLI commands (e.g., `git status`) and rewrite them to RTK equivalents (e.g., `rtk git status`) so that LLM agents automatically benefit from token savings without explicit user configuration.

## Installation Modes

`rtk init` supports these installation flows:

| Mode | Command | Creates | Patches |
|------|---------|---------|----------|
| Default (global) | `rtk init -g` | Hook, SHA-256 hash, RTK.md | settings.json, CLAUDE.md |
| Hook only | `rtk init -g --hook-only` | Hook, SHA-256 hash | settings.json |
| Claude-MD (legacy) | `rtk init --claude-md` | 134-line RTK block | CLAUDE.md |
| Windsurf | `rtk init -g --agent windsurf` | `.windsurfrules` | -- |
| Cline | `rtk init --agent cline` | `.clinerules` | -- |
| Codex | `rtk init --codex` | RTK.md + `.codex/hooks.json` (local) or `$CODEX_HOME/hooks.json` (global) | AGENTS.md + `PreToolUse` hook |
| Cursor | `rtk init -g --agent cursor` | Cursor hook | hooks.json |
| Trae | `rtk init --agent trae` (project) or `rtk init -g --agent trae` (global) | Native `rtk hook trae` registration | `.trae/hooks.json`; global also patches existing `~/.trae-cn/hooks.json` |
| Google Antigravity | `rtk init --agent antigravity` / `rtk init -g --agent antigravity` | Antigravity Plugin in `plugins/rtk/` | plugin.json, hooks.json, rules/AGENTS.md (awareness) |
| Pi | `rtk init --agent pi` | `.pi/extensions/rtk.ts` | -- |
| Oh My Pi (OMP) | `rtk init --agent omp` | `.omp/extensions/rtk.ts` (shared Pi extension) | -- |
| Hermes | `rtk init --agent hermes` | Python plugin in `~/.hermes/plugins/rtk-rewrite/` | `config.yaml` `plugins.enabled` |


## Integrity Verification

The integrity system prevents unauthorized hook modifications:

1. At install: `integrity::store_hash()` computes SHA-256 of the hook file, writes to `~/.claude/hooks/.rtk-hook.sha256` (read-only 0o444)
2. At runtime: `integrity::runtime_check()` re-computes hash and compares; blocks execution if tampered
3. On demand: `rtk verify` prints detailed verification status (PASS/FAIL/WARN/SKIP)

Five integrity states:
- **Verified**: Hash matches stored value
- **Tampered**: Hash mismatch (blocks execution)
- **NoBaseline**: Hook exists but no hash stored (old install)
- **NotInstalled**: No hook, no hash
- **OrphanedHash**: Hash file exists, hook missing

## PatchMode Behavior

Controls how `rtk init` modifies agent settings files:

| Mode | Flag | Behavior |
|------|------|----------|
| Ask (default) | -- | Prompts before settings changes, protected Pi/OMP overwrites, and definitively shared uninstalls; defaults to No if stdin not terminal |
| Auto | `--auto-patch` | Patches without prompting and approves protected Pi/OMP extension updates; for CI/scripted installs |
| Skip | `--no-patch` | Protected Pi/OMP actions leave files unchanged and exit nonzero; settings changes print manual instructions and succeed |

## Atomicity and Safety

All file operations use atomic writes (tempfile + rename) to prevent corruption on crash. Settings files are backed up to `.bak` before modification. All operations are idempotent -- running `rtk init` multiple times is safe.

## Permission Model

RTK enforces a permission precedence that matches Claude Code's least-privilege default:

```
Deny > Defer > Ask > Allow (explicit) > Default (ask)
```

Rules are loaded from all Claude Code `settings.json` files (project + global, including `.local` variants). Only `Bash(...)` rules are extracted; other scopes (Read, Write) are ignored.

Quoted `sh -c`, `bash -c`, `zsh -c` and `fish -c` scripts add a
second command-parsing boundary. RTK checks deny rules against both the outer
wrapper and the parsed inner segments, including a wrapper behind a prefix the
rewrite strips (`env bash -c '…'`, `timeout 5 sh -c '…'`, a configured
`transparent_prefixes` entry — `registry::shell_wrapper_positions`). A shell
name that is merely an argument (`grep -rn bash -c src`) is not a wrapper. A
deny rule matching an inner segment yields `Defer`: RTK neither rewrites nor
decides, so the host sees the command exactly as typed.
A wrapper at the start of a segment is never auto-allowed: its strongest RTK
verdict is `Ask`. A wrapper behind a prefix is only checked for deny rules and
otherwise keeps the outer command's verdict, as on master. Ask-capable hosts
prompt, while other hosts retain their existing native permission flow.
Unsupported or ambiguous wrapper syntax is left unchanged for the host to
evaluate. Line continuations are joined before the check, as the rewrite joins
them.

Unambiguously-fish scripts (a fish-only keyword such as `end`, `begin`,
`switch`, `and`, `or`, or `not` at command position, with no POSIX
disambiguator anywhere, comments excluded before anything is classified) can
be rewritten to `rtk run --shell fish -c '<script>'`
(`discover/fish_script.rs`), with the script's own commands rewritten inside
the wrap so it costs no savings. **Off unless `hooks.wrap_fish_scripts =
true`**, and skipped anyway without a resolvable `fish` binary or on Windows;
those cases take the decision path they always did.

**Every wrap RTK builds carries `"permissionDecision": "ask"`.** Without it, Claude Code
runs its own permission check on what the hook returned — and what it sees is
one `rtk run` command, not the script's. An allow rule matching `rtk` would
run the whole script unprompted, and a rule naming one of the script's own
commands would match nothing. That is also why the feature is opt-in: some of
Claude Code's rule sources never reach the hook at all (`--settings`,
`--disallowedTools`, managed settings, and the project directory the agent has
`cd`'d away from), so a rule in one of those becomes a prompt rather than a
refusal. The user docs say so.

**It is offered only on `rtk hook claude`** (`DenyRules` and
`claude_hook_deny_rules` in `permissions.rs`), and only when every Claude Code
settings file RTK reads parsed and none of them has a deny rule. Once the
script reaches a fish, RTK's own view of it stops describing what runs: fish
starts a command where the bash-shaped segmenter does not look — the condition
of an `if` or a `while`, the right side of `and`/`or`/`not` — and resolves
words at run time, through variables, globs, aliases, functions and escapes
whose expansion differs between fish versions. The wrap also puts *every*
command of the script inside one quoted argument, which is what separates it
from an ordinary rewrite: a host's own rule on a command RTK never rewrites
still fires today, and would not fire on the wrapped form. So the wrap needs a
host whose rule files RTK reads and whose hook can ask for the wrap explicitly,
and Claude Code's hook is the only one:

- Gemini's loader reads `tools.allowed` and `tools.confirmationRequired` only
  and never yields a deny rule, so its exclude lists are invisible here;
- Droid's built-in defaults are deliberately not mirrored, so an empty
  `commandDenylist` says nothing about what Droid refuses;
- Cursor's loader reads the global `cli-config.json`, while the host also
  applies the project's;
- Copilot resolves to `Host::Claude` for its verdict but judges the returned
  command with its own allowlist flow, so it is `AgentPath::InProcess`, not
  `AgentPath::ClaudeHook`;
- OpenCode re-evaluates its own patterns against the string RTK returns, with
  the rule set resolved per agent at run time;
- Codex, Trae, Vibe and Antigravity have no RTK-side rule source at all;
- `rtk rewrite` reads Claude's rules, but the delegates that shell out to it
  apply exit 3 without prompting (`hooks/pi/rtk.ts`,
  `hooks/hermes/rtk-rewrite`) and OpenClaw matches its own exec checks against
  the returned string;
- a rules-file install (`AgentPath::RulesOnly`) has no hook at all, so
  `rtk hook check` must not show it a wrap nothing would ever emit.

The cost is deliberate and belongs in the user docs: a user with any deny rule
keeps the behaviour they had before the wrap existed.

**Why it runs before the unattestable gate.** Every other rewrite refuses a
command the gate could not decompose. The wrap cannot wait for that gate and
still do its job: a host that evaluates the command string with a POSIX layer
fails to parse a fish script *before* RTK is consulted at all, so deferring
means the command is lost rather than merely unrewritten. What it does instead
is refuse everything that gate refuses — command and process substitution,
file-target redirects, and fish's own `(cmd)` substitution, which the shared
bash lexer reads as a subshell — reading the same comment-stripped text the
classification uses, so a quote in a comment cannot blind it. It also refuses
fish's `&|` pipe, which that lexer reads as a background `&` plus a pipe and
the rewrite re-emits spaced, a form fish rejects outright. The only scripts
that reach the wrap are ones whose sole unattestable property is fish control
flow (`; and`, `if … end`), which the shared segmenter cannot split into
commands. For those:

- the script travels inside one quoted argument and RTK decides nothing about
  its contents beyond the rewrite rules it applies to them — those rules are
  the one edit it makes, so the argument carries the same commands rather than
  the same bytes;
- the decision is `Ask`, never `Allow`, and the hook says so explicitly rather
  than leaving it to the host's own check;
- the residual exposure is the rule sources RTK cannot read, listed above.
  `hooks.wrap_fish_scripts` (default `false`) is the switch for a user who
  does not want that trade.

| Verdict | Trigger | rewrite_cmd exit | Hook behavior |
|---------|---------|-----------------|---------------|
| Deny | `permissions.deny` rule matched | 2 | Passthrough — host tool handles denial |
| Ask | `permissions.ask` rule matched | 3 | Rewrite + let host tool prompt user |
| Allow | `permissions.allow` rule matched | 0 | Rewrite + auto-allow |
| Default | No rule matched | 3 | Rewrite + let host tool prompt user |
| Defer | `permissions.deny` rule matched inside a shell wrapper | 1 | Passthrough — command runs as without RTK |

A deny or ask rule is matched against each segment as written, without the grammar a split leaves (`{`, `!`, `(`), and past each leading assignment or wrapper in turn (`registry::matching_readings`): `env HUSKY=0 git push` is read as `HUSKY=0 git push` and as `git push`, as Claude Code reads it. A reading need not be what the shell runs (`nohup HUSKY=0 git push` makes nohup look for a program named `HUSKY=0`), so it can only make a verdict stricter. Past 32 prefixes the command behind them is unread, and where a deny or ask rule exists the verdict is at least Ask. An allow rule matches the segment as written only, so a wrapper never inherits the allow of the command it runs.

A delegate that shells out to `rtk rewrite` and applies its own exec policy to
the result can set `RTK_REWRITE_HOST=<agent>` on that subprocess. For an agent
whose `AgentPath` records it as owning approval — OpenClaw is the only one —
`Default` renders as exit 0 instead of 3, because a `Default` verdict means no
rule matched and RTK asking as well would be a second gate sourced from Claude
Code's settings the runtime never opted into (#3908). An explicit `Ask` rule is
the user's own instruction, so it still renders as exit 3 and the host still
prompts. The verdict source is unchanged, and `Deny` still renders as exit 2 for
every delegate, so naming a host can never relax an explicit deny or discard an
explicit ask. An unknown or unset value keeps the table above — and a caller
that does *not* own approval should scrub the variable before invoking `rtk
rewrite`, since it is inherited by every child process. See `decision.rs`'s
`ApprovalOwner`.

### Per-tool support

| Tool | ask support | Behavior on Default |
|------|------------|-------------------|
| Claude Code (rtk-rewrite.sh) | Yes | `permissionDecision: "ask"` — user prompted |
| Copilot VS Code (rtk hook copilot) | Yes | `permissionDecision: "ask"` — user prompted |
| Cursor (rtk hook cursor) | Ready | `permission: "ask",` — users will be prompted when Cursor enforces the permission; in the meantime, allow |
| Gemini CLI (rtk hook gemini) | No (allow/deny only) | allow (limitation — no ask mode in Gemini) |
| Copilot CLI (rtk hook copilot) | No updatedInput | deny-with-suggestion (unchanged) |
| Codex (`rtk hook codex`) | Native approval runs after rewrite | Emit required protocol `allow` with `updatedInput`; Codex then evaluates the rewritten command normally |
| Trae (`rtk hook trae`) | Host-owned approval | Return only `updatedInput`; omit `permissionDecision` |
| Mistral Vibe (rtk hook vibe) | No native ask surface | passthrough — Vibe's own approval prompt fires on the rewritten command |
| OpenClaw (`openclaw/index.ts` → `rtk rewrite`) | Host-owned approval (`RTK_REWRITE_HOST=openclaw`) | Rewrite with no RTK prompt when no rule matched; an explicit `Ask` still exits 3 and the plugin prompts. OpenClaw's `tools.exec.mode`/`security`/`ask` decide. A `Deny` still exits 2 and the plugin blocks the call |
| Google Antigravity (rtk hook antigravity) | Native approval runs after rewrite | allow with overwrite.CommandLine — Antigravity evaluates permissions after hook |

### Implementation

- `permissions.rs` — loads deny/ask/allow rules, evaluates precedence, returns `PermissionVerdict`
- `rewrite_cmd.rs` — maps verdict to exit code (consumed by shell hook)
- `hook_cmd.rs` — maps decisions to each agent's JSON protocol, including Codex `updatedInput`
- `discover/shell_wrapper.rs` — shares conservative script-span parsing between rewrite and permission checks

## Exit Code Contract

Hook processors in `hook_cmd.rs` must return `Ok(())` on every path — success, no-match, parse error, and unexpected input. Returning `Err` propagates to `main()` and exits non-zero, which blocks the agent's command from executing. This violates the non-blocking guarantee documented in `hooks/README.md`.

## Adding New Functionality
To add support for a new AI coding agent: (1) add the hook installation logic to `src/hooks/init/` following the existing agent patterns, (2) if the agent requires a custom hook protocol (like Gemini's `BeforeTool` or Vibe's `pre_tool`), add a processor function in `hook_cmd.rs` and a matching `HookCommands::<Agent>` variant + `AgentTarget::<Agent>` enum entry in `main.rs`, (3) if the agent has installable permission surfaces (denylist / allowlist), wire them into `permissions.rs::check_command_for` via a new `Host::<Agent>` variant, and (4) update `integrity.rs` with the expected hash for the new hook file. Add an `is_configured()` probe to the agent's init module that covers every scope its `rtk init` writes (global and project), make it reachable from `hook_check` (the module is `pub(crate)` in `init/mod.rs`), and list it in `hook_check.rs::AGENT_PROBES`, so that `rtk gain` and the daily reminder stop reporting a missing hook for users of that agent. Only the Claude Code hook has an outdated-hook warning path. Test by running `rtk init` in a fresh environment and verifying the hook rewrites commands correctly in the target agent.
