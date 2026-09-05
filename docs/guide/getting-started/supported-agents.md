---
title: Supported Agents
description: How to integrate RTK with Claude Code, Cursor, Copilot, Cline, Windsurf, Codex, OpenCode, Hermes, Kilo Code, Antigravity, Factory Droid, and Mistral Vibe
sidebar:
  order: 3
---

# Supported Agents

RTK supports all major AI coding agents across 3 integration tiers.

## How it works

Each agent integration intercepts CLI commands before execution and rewrites them to their RTK equivalent. The agent runs `rtk cargo test` instead of `cargo test`, sees filtered output, and reads up to 90% fewer bash output bytes — without any change to your workflow.

All rewrite logic lives in the RTK binary (`rtk rewrite`). Agent hooks are thin delegates that parse the agent-specific JSON format and call `rtk rewrite` for the actual decision.

```
Agent runs "cargo test"
  -> Hook intercepts (PreToolUse / plugin event)
  -> Calls rtk rewrite "cargo test"
  -> Returns "rtk cargo test"
  -> Agent executes filtered command
  -> LLM reads up to 90% fewer bash output bytes
```

## Supported agents

| Agent | Integration tier | Can rewrite transparently? |
|-------|-----------------|---------------------------|
| Claude Code | Shell hook (`PreToolUse`) | Yes |
| VS Code Copilot Chat | Shell hook (`PreToolUse`) | Yes |
| GitHub Copilot CLI | Shell hook (`PreToolUse`) | Yes |
| Cursor | Shell hook (`preToolUse`) | Yes |
| Gemini CLI | Rust binary (`BeforeTool`) | Yes |
| OpenCode | TypeScript plugin (`tool.execute.before`) | Yes |
| OpenClaw | TypeScript plugin (`before_tool_call`) | Yes |
| Pi | TypeScript extension (`tool_call` event) | Yes |
| Oh My Pi (OMP) | TypeScript extension (`tool_call` event, shared with Pi) | Yes |
| Hermes | Python plugin (`terminal` command mutation) | Yes |
| Factory Droid | Shell hook (`PreToolUse`, matcher `Execute`) | Yes |
| Cline / Roo Code | Rules file (prompt-level) | N/A |
| Windsurf | Rules file (prompt-level) | N/A |
| Codex CLI | AGENTS.md instructions | N/A |
| Kilo Code | Rules file (prompt-level) | N/A |
| Google Antigravity | Rules file (prompt-level) | N/A |
| Mistral Vibe | Rust binary (`pre_tool`) | Yes |

## Installation by agent

### Claude Code

```bash
rtk init --global    # installs hook + patches settings.json
```

Restart Claude Code. Verify:

```bash
rtk init --show    # shows hook status
```

### Cursor

```bash
rtk init --global --agent cursor
```

Restart Cursor. The hook uses `preToolUse` with Cursor's `updated_input` format.

### GitHub Copilot (VS Code Chat + CLI)

```bash
rtk init --copilot            # project-scoped (.github/hooks/)
rtk init --global --copilot   # user-scoped (~/.copilot/hooks/, respects $COPILOT_HOME)
```

Project-scoped writes `.github/hooks/rtk-rewrite.json` — a single `PreToolUse` entry shared by both hosts, each getting transparent rewrite via `updatedInput` — plus the RTK block in `.github/copilot-instructions.md`. User-scoped writes the same hook config to `~/.copilot/hooks/rtk-rewrite.json` and the RTK block to `~/.copilot/copilot-instructions.md` (both respect `$COPILOT_HOME` if set).

Earlier `rtk` versions also registered a second, camelCase `preToolUse` entry for Copilot CLI's native schema. Copilot CLI treats `PreToolUse`/`preToolUse` as independent hooks and runs both sequentially for the same tool call — a redundant process spawn with no behavioral benefit, since Copilot CLI honors the single `PreToolUse` schema on its own. Re-run `rtk init --copilot` (or `--global --copilot`) to upgrade an existing install to the single-hook config.

Uninstall:

```bash
rtk init --uninstall --copilot
rtk init --uninstall --global --copilot
```

Removes only RTK's hook file (and, for project, the RTK block in `copilot-instructions.md`). Other files in `.github/hooks/` or `~/.copilot/hooks/` and your own instruction content are untouched.

### Gemini CLI

```bash
rtk init --global --gemini
```

### OpenCode

```bash
rtk init --global --opencode
```

Creates `~/.config/opencode/plugins/rtk.ts`. Uses the `tool.execute.before` hook.

### Pi

```bash
# Project-local (default)
rtk init --agent pi

# Global — all projects
rtk init --agent pi --global
```

Creates `.pi/extensions/rtk.ts` (local) or `~/.pi/agent/extensions/rtk.ts` (global). Pi auto-discovers extensions from both paths on startup. The global path follows `PI_CODING_AGENT_DIR` when set; OMP uses that same variable. When the paths alias, RTK records the agent(s) it installed in an adjacent hidden `.rtk-agents` state file so shared-file warnings are based on ownership rather than path equality alone.

Installation updates only the current or a known historical RTK extension. If the managed path contains modified or unrelated content, RTK asks before overwriting it; use `--auto-patch` to approve without prompting or `--no-patch` to leave it unchanged. A declined protected update, including `--no-patch`, exits nonzero so automation can detect that no install occurred. `--dry-run` reports the prompt without changing files. LF and CRLF line endings are treated as the same stock extension.

Uninstall:

```bash
rtk init --uninstall --agent pi
rtk init --uninstall --agent pi --global
```

Removes only the current or known historical stock Pi extension. If the file contains modified RTK content, uninstall asks before removing it; `--auto-patch` approves and copies the file to `rtk.ts.bak` first, `--no-patch` aborts with a manual-removal message, and `--dry-run` previews the prompt; unreadable content is left in place and causes a normal uninstall to exit nonzero, while `--dry-run` reports it and succeeds. Unrelated content is left in place. LF and CRLF stock files are both recognized.

### Oh My Pi (OMP)

```bash
# Project-local (default)
rtk init --agent omp

# Global — all projects
rtk init --agent omp --global
```

Creates `.omp/extensions/rtk.ts` (local) or `~/.omp/agent/extensions/rtk.ts` (global). OMP loads the same extension file as Pi through its `legacy-pi-compat` layer, so both agents share `rtk.ts`. The global path follows `PI_CODING_AGENT_DIR`, which OMP also honors for its agent directory.

When the Pi and OMP targets in either project or global scope resolve to one file, RTK records whether Pi, OMP, or both agents were installed for that alias in an adjacent hidden `.rtk-agents` state file. A valid sidecar is authoritative. Missing or unreadable state is treated as uncertain: RTK warns and proceeds without using a heuristic to claim sole ownership. Uninstalling a definitively shared project or global file asks before removing it, so confirm that neither agent should use it first (or pass `--auto-patch` to approve). An uncertain legacy or corrupt-state uninstall warns and proceeds; a declined definitive shared uninstall, including `--no-patch`, exits nonzero. `--dry-run` remains a successful preview. RTK currently targets OMP's default profile and `.omp` project directory; named OMP profiles and custom `PI_CONFIG_DIR` locations are not auto-detected.

Installation updates only the current or a known historical RTK extension. If the managed path contains modified or unrelated content, RTK asks before overwriting it; use `--auto-patch` to approve without prompting or `--no-patch` to leave it unchanged. A declined protected update, including `--no-patch`, exits nonzero so automation can detect that no install occurred. `--dry-run` reports the prompt without changing files. LF and CRLF line endings are treated as the same stock extension.

Uninstall:

```bash
rtk init --uninstall --agent omp
rtk init --uninstall --agent omp --global
```

Removes only the current or known historical stock OMP extension. If the file has been modified after install, uninstall asks before removing it; `--auto-patch` approves and copies the file to `rtk.ts.bak` first, `--no-patch` aborts with a message, and `--dry-run` previews the prompt; unreadable content is left in place and causes a normal uninstall to exit nonzero, while `--dry-run` reports it and succeeds. Unrelated content is left in place. When Pi and OMP paths in either scope resolve to the same file and a valid sidecar records both agents, uninstall asks before removing the shared file; use `--auto-patch` to approve or `--no-patch` to keep it. Missing or unreadable ownership state warns and proceeds without definitive shared-file protection. A declined definitive shared uninstall exits nonzero so scripts can detect that the file remains.

### OpenClaw

```bash
openclaw plugins install ./openclaw
```

Plugin in the `openclaw/` directory. Uses the `before_tool_call` hook, delegates to `rtk rewrite`.

### Hermes

```bash
rtk init --agent hermes
```

Creates `~/.hermes/plugins/rtk-rewrite/` and enables it through `plugins.enabled` in the Hermes config. Hermes loads Python plugins, so the plugin entrypoint is Python, but it is only a thin adapter. It mutates the Hermes `terminal` tool `command` before execution and delegates all rewrite decisions to Rust through `rtk rewrite`. The repository source and tests for that adapter live in `hooks/hermes/`; only installed runtime files use the `~/.hermes/plugins/rtk-rewrite/` path.

The plugin fails open. If `rtk` is missing at load time, the hook is not registered. If `rtk rewrite` errors, the tool is not `terminal`, the payload has no string `command`, or the plugin raises an exception, Hermes runs the original command unchanged. The same `rtk rewrite` limitations apply: already-prefixed `rtk` commands, compound shell commands, heredocs, and commands without filters are not rewritten.

### Factory Droid

```bash
rtk init -g --agent droid    # user-scoped (~/.factory/hooks.json)
rtk init --agent droid       # project-scoped (.factory/hooks.json, commit to share)
```

Installs a `PreToolUse` hook (matcher `Execute`) into Droid's canonical `hooks.json` — falling back to the `hooks` key of `settings.json` only when that file already carries live `PreToolUse` hooks. Respects `$FACTORY_HOME_OVERRIDE`.

RTK honors Droid's own permission lists, never another agent's settings. Commands matching an explicit `commandDenylist` or `commandBlocklist` entry — read from all four settings scopes (`~/.factory/settings.json`, `~/.factory/settings.local.json`, `.factory/settings.json`, `.factory/settings.local.json`) — are left untouched so Droid's native confirmation or block fires on the original command. Every other command is rewritten via `updatedInput` with **no** permission decision: Droid's native flow (allowlist, autonomy level, other hooks) decides on the rewritten command. To auto-run rewritten read-only commands, add `rtk`-prefixed entries (e.g. `rtk git status`) to your `commandAllowlist`.

Uninstall:

```bash
rtk init --uninstall -g --agent droid
rtk init --uninstall --agent droid
```

Removes only RTK's hook entry; other hooks and settings are untouched.

### Cline / Roo Code

```bash
rtk init --agent cline    # creates .clinerules in current project
```

Cline reads `.clinerules` as custom instructions. RTK adds guidance telling Cline to prefer `rtk <cmd>` over raw commands.

### Windsurf

```bash
rtk init --global --agent windsurf    # creates .windsurfrules in current project
```

### Codex CLI

```bash
rtk init --codex           # project-scoped (AGENTS.md)
rtk init --global --codex  # user-global (~/.codex/AGENTS.md)
```

### Kilo Code

```bash
rtk init --agent kilocode    # creates .kilocode/rules/rtk-rules.md in current project
```

Kilo Code reads `.kilocode/rules/` as custom instructions. RTK adds guidance telling Kilo Code to prefer `rtk <cmd>` over raw commands.

### Google Antigravity

```bash
rtk init --agent antigravity    # creates .agents/rules/antigravity-rtk-rules.md in current project
```

Antigravity reads `.agents/rules/` as custom instructions. RTK adds guidance telling Antigravity to prefer `rtk <cmd>` over raw commands.

### Mistral Vibe

```bash
rtk init -g --agent vibe                # user-scoped (~/.vibe/hooks.toml)
rtk init -g --agent vibe --hook-only    # skip the ~/.vibe/prompts/rtk.md prompt file
```

Installs a `pre_tool` hook entry (`match = "bash"`, `command = "rtk hook vibe"`, `strict = false`) into `~/.vibe/hooks.toml`, following the contract at [docs.mistral.ai/vibe/code/cli/hooks](https://docs.mistral.ai/vibe/code/cli/hooks). Vibe invokes the native `rtk hook vibe` binary before every bash tool call; RTK reads Vibe's stdin JSON payload and emits `{"hook_specific_output": {"tool_input": {"command": "rtk ..."}}}` to rewrite the command in place. The Vibe UI surfaces `[rtk-rewrite] rtk: rewrote to \`…\`` via RTK's `system_message` field so the rewrite is visible.

Unlike Droid, Vibe does not yet expose a denylist / allowlist surface in `hooks.toml` for RTK to honor. RTK therefore rewrites every bash command it knows how to compress and defers to Vibe's own permission prompt on the rewritten command; commands RTK doesn't handle pass through unchanged. `strict = false` ensures a hook crash degrades to a warning rather than blocking the tool call.

Alongside the hook, RTK drops a system prompt at `~/.vibe/prompts/rtk.md` describing the RTK conventions to Vibe as a belt-and-suspenders fallback. Use `--hook-only` to skip it.

Install is global-only (Vibe's hook registry is user-scoped). Re-running the installer is a no-op; the RTK entry is detected by its `name = "rtk-rewrite"` field and never duplicated.

Uninstall:

```bash
rtk init -g --agent vibe --uninstall
```

Strips only RTK's `[[hooks]]` block and the `~/.vibe/prompts/rtk.md` file. Any other user-declared hooks in `hooks.toml` are preserved byte-for-byte. `hooks.toml` is removed only when the RTK entry was the sole content.

## Integration tiers explained

| Tier | Mechanism | How rewrites work |
|------|-----------|------------------|
| **Full hook** | Shell script or Rust binary, intercepts via agent API | Transparent — agent never sees the raw command |
| **Plugin** | TypeScript, JavaScript, or Python in agent's plugin system | Transparent, in-place mutation when the agent allows it |
| **Rules file** | Prompt-level instructions | Guidance only — agent is told to prefer `rtk <cmd>` |

Rules file integrations (Cline, Windsurf, Codex, Kilo Code, Antigravity) rely on the model following instructions. Full hook integrations (Claude Code, Cursor, Gemini) are guaranteed — the command is rewritten before the agent sees it. Plugin integrations (OpenCode, Pi) use in-place mutation via the agent's TypeScript extension API.

## Windows support

The shell hook (`rtk-rewrite.sh`) requires a Unix shell. On native Windows:

- `rtk init -g` automatically falls back to **CLAUDE.md injection mode** (prompt-level instructions)
- Filters work normally (`rtk cargo test`, `rtk git status`)
- Auto-rewrite does not work — the AI assistant is instructed to use RTK but commands are not intercepted

For full hook support on Windows, use [WSL](https://learn.microsoft.com/en-us/windows/wsl/install). Inside WSL, all agents with shell hook integration (Claude Code, Cursor, Gemini) work identically to Linux.

## Graceful degradation

Hooks never block command execution. If RTK is missing, the hook exits cleanly and the raw command runs unchanged:

- RTK binary not found: warning to stderr, exit 0
- Invalid JSON input: pass through unchanged
- RTK version too old: warning to stderr, exit 0
- Filter logic error: fallback to raw command output

## Override: disable RTK for one command

```bash
RTK_DISABLED=1 git status    # runs raw git status, no rewrite
```

Or exclude commands permanently in `~/.config/rtk/config.toml`:

```toml
[hooks]
exclude_commands = ["git rebase", "git cherry-pick"]
```
