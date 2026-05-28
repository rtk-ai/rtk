# RTK for Jcode

Jcode currently supports prompt-level RTK integration through `prompt-overlay.md`.

## Installation

Project-scoped:

```bash
rtk init --agent jcode
```

Creates or updates:

```text
.jcode/prompt-overlay.md
```

Global:

```bash
rtk init -g --agent jcode
```

Creates or updates:

```text
$JCODE_HOME/prompt-overlay.md
```

If `JCODE_HOME` is unset, RTK uses:

```text
~/.jcode/prompt-overlay.md
```

## How it works

Jcode reads prompt overlay files and incorporates them into the agent instructions. RTK adds a marked instruction block telling Jcode to prefer `rtk <command>` when using its `bash` or `shell_exec` tool.

This is a prompt-level integration. It does not transparently rewrite commands before execution because current Jcode builds do not expose a before-tool hook with `updated_input.command` support.

## Future transparent rewrite support

If Jcode adds a `BeforeToolCall` hook that can mutate the `command` field, RTK can add `rtk hook jcode` and upgrade this integration to transparent command rewriting.
