# Codex PreToolUse Adapter

This guide installs RTK for Codex Desktop or Codex CLI with a native
`PreToolUse` hook. The hook rewrites supported Bash commands through RTK before
Codex executes them.

## Quick setup

Use this on machines that may or may not already have RTK installed. The
commands reuse a working RTK installation when `rtk gain` succeeds, and install
RTK only when it is missing or the wrong `rtk` binary is on `PATH`.

```bash
set -eu

if ! command -v rtk >/dev/null 2>&1 || ! rtk gain >/dev/null 2>&1; then
  curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh | sh
fi

export PATH="$HOME/.local/bin:$PATH"

rtk init -g --codex
```

Restart Codex, enable or install the RTK plugin if Codex prompts for it, then
review and trust the RTK hook from `/hooks` or the Codex settings panel.

## What gets installed

`rtk init -g --codex` registers the RTK Codex plugin in the user-level Codex
environment:

- `$CODEX_HOME/plugins/rtk-codex/` or `~/.codex/plugins/rtk-codex/`
- `~/.agents/plugins/marketplace.json`
- A plugin-owned `PreToolUse` hook that runs `rtk hook codex`
- A bundled `$rtk` skill that explains RTK behavior and validation

For project-local setup, run `rtk init --codex` inside the project. That writes
the local plugin package under the project plugin directory and registers it in
the local marketplace.

## Behavior

- Handles Codex `Bash` tool calls only.
- Uses RTK's native `rtk hook codex` processor, so new rewrite rules are picked
  up by the installed RTK binary.
- Rewrites supported commands such as `git status` to `rtk git status`.
- Preserves the rest of Codex's original `tool_input`; only `command` is
  replaced.
- Produces no hook output when RTK has no rewrite, the payload is malformed, the
  tool is not Bash, the command is already RTK-prefixed, or the command contains
  a heredoc.
- Returns rewritten input only with `permissionDecision: "allow"`, matching the
  Codex hook contract.

## Verify

Check the RTK binary:

```bash
rtk --version
rtk gain
```

Check the Codex installation:

```bash
rtk init --show --codex
```

Preview a rewrite:

```bash
rtk hook check git status
```

After Codex has restarted and the hook is trusted, run a simple Bash command
such as:

```bash
git status
```

When the hook is active, Codex should execute the RTK-prefixed form and RTK
usage should appear in:

```bash
rtk gain
```

## Uninstall

For global setup:

```bash
rtk init -g --codex --uninstall
```

For project-local setup:

```bash
rtk init --codex --uninstall
```

Uninstall removes only RTK-managed plugin state, marketplace entries, and legacy
RTK-managed Codex guidance. Unrelated Codex plugins and marketplace entries are
preserved.
