# Hook System

> See also [docs/contributing/TECHNICAL.md](../../docs/contributing/TECHNICAL.md) for the full architecture overview | [hooks/](../../hooks/README.md) for deployed hook artifacts

## Scope

The **lifecycle management** layer for LLM agent hooks: install, uninstall, verify integrity, audit usage, and manage trust. This component creates and maintains the hook artifacts that live in `hooks/` (root), but does **not** execute rewrite logic itself — that lives in `discover/registry`.

Owns: `rtk init` installation flows (6 agents via `AgentTarget` enum, now including Mistral Vibe + 3 special modes: Gemini, Codex, OpenCode), SHA-256 integrity verification, hook version checking, audit log analysis, `rtk rewrite` CLI entry point, and TOML filter trust management.

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
Deny > Ask > Allow (explicit) > Default (ask)
```

Rules are loaded from all Claude Code `settings.json` files (project + global, including `.local` variants). Only `Bash(...)` rules are extracted; other scopes (Read, Write) are ignored.

Quoted `sh -c`, `bash -c`, `zsh -c` and `fish -c` scripts add a
second command-parsing boundary. RTK checks deny rules against both the outer
wrapper and the parsed inner segments. A wrapper rewrite is never auto-allowed:
its strongest RTK verdict is `Ask`. Ask-capable hosts prompt, while other
hosts retain their existing native permission flow. Unsupported or ambiguous
wrapper syntax is left unchanged for the host to evaluate.

Unambiguously-fish scripts (a fish-only keyword such as `end`, `begin`,
`switch`, `and`, `or`, or `not` at command position, with no POSIX
disambiguator anywhere, comments excluded before anything is classified) are
rewritten to `rtk run --shell fish -c '<script>'` (`discover/fish_script.rs`),
with the script's own commands rewritten inside the wrap so it costs no
savings. Like wrapper rewrites, the wrapped form is never auto-allowed — its
strongest verdict is `Ask` — and deny rules are checked first. The wrap is
skipped without a resolvable `fish` binary, on Windows, or when
`hooks.wrap_fish_scripts = false`; those cases take the decision path they
always did.

**Why it runs before the unattestable gate.** Every other rewrite refuses a
command the gate could not decompose. The wrap cannot wait for that gate and
still do its job: a host that evaluates the command string with a POSIX layer
fails to parse a fish script *before* RTK is consulted at all, so deferring
means the command is lost rather than merely unrewritten. What it does instead
is refuse everything that gate refuses — command and process substitution,
file-target redirects, and fish's own `(cmd)` substitution, which the shared
bash lexer reads as a subshell — reading the same comment-stripped text the
classification uses, so a quote in a comment cannot blind it. The only scripts
that reach the wrap are ones whose sole unattestable property is fish control
flow (`; and`, `if … end`), which the shared segmenter cannot split into
commands. For those:

- the script travels inside one quoted argument and RTK decides nothing about
  its contents beyond the rewrite rules it applies to them — those rules are
  the one edit it makes, so the argument carries the same commands rather than
  the same bytes;
- the verdict is `Ask`, never `Allow`, and a deny rule still wins outright;
- the residual exposure is a host that treats an `rtk`-prefixed command as
  pre-approved. Codex does: it renders `Ask` as a protocol-level `allow` and
  its own safe/dangerous classifiers do not unwrap `rtk` (see `hook_cmd.rs`).
  `hooks.wrap_fish_scripts = false` is the switch for a host where that
  trade is not wanted.

| Verdict | Trigger | rewrite_cmd exit | Hook behavior |
|---------|---------|-----------------|---------------|
| Deny | `permissions.deny` rule matched | 2 | Passthrough — host tool handles denial |
| Ask | `permissions.ask` rule matched | 3 | Rewrite + let host tool prompt user |
| Allow | `permissions.allow` rule matched | 0 | Rewrite + auto-allow |
| Default | No rule matched | 3 | Rewrite + let host tool prompt user |

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

### Implementation

- `permissions.rs` — loads deny/ask/allow rules, evaluates precedence, returns `PermissionVerdict`
- `rewrite_cmd.rs` — maps verdict to exit code (consumed by shell hook)
- `hook_cmd.rs` — maps decisions to each agent's JSON protocol, including Codex `updatedInput`
- `discover/shell_wrapper.rs` — shares conservative script-span parsing between rewrite and permission checks

## Exit Code Contract

Hook processors in `hook_cmd.rs` must return `Ok(())` on every path — success, no-match, parse error, and unexpected input. Returning `Err` propagates to `main()` and exits non-zero, which blocks the agent's command from executing. This violates the non-blocking guarantee documented in `hooks/README.md`.

## Adding New Functionality
To add support for a new AI coding agent: (1) add the hook installation logic to `src/hooks/init/` following the existing agent patterns, (2) if the agent requires a custom hook protocol (like Gemini's `BeforeTool` or Vibe's `pre_tool`), add a processor function in `hook_cmd.rs` and a matching `HookCommands::<Agent>` variant + `AgentTarget::<Agent>` enum entry in `main.rs`, (3) if the agent has installable permission surfaces (denylist / allowlist), wire them into `permissions.rs::check_command_for` via a new `Host::<Agent>` variant, and (4) update `integrity.rs` with the expected hash for the new hook file. Note that `hook_check.rs::maybe_warn()` only checks the Claude Code hook — other agents don't have an outdated-hook warning path. Test by running `rtk init` in a fresh environment and verifying the hook rewrites commands correctly in the target agent.
