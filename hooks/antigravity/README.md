# Antigravity Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Modes

### Project-scoped (rules-based, default)

```bash
rtk init --agent antigravity
```

Installs `.agents/rules/antigravity-rtk-rules.md` in the project root. Antigravity reads this file for per-project instructions.

### Global (hook-based)

```bash
rtk init -g --agent antigravity
```

Installs programmatic hook to `~/.antigravity/hooks/rtk-rewrite.sh` and patches `~/.antigravity/hooks.json` with a `preToolUse` entry. This transparently rewrites commands to use RTK.

## Specifics

- Same delegating pattern as Cursor hook but targets `~/.antigravity/` config directory
- Returns `{}` (empty JSON) when no rewrite applies
- Requires `jq` and `rtk >= 0.23.0`
- Assumes Antigravity inherits Windsurf/VS Code hook conventions (`hooks.json` with `preToolUse` array)

## Assumptions (Unverified)

The global hook mode (`--global`) relies on three assumptions about Antigravity's
internal architecture. These are inferred from Antigravity's VS Code heritage but
**have not been verified against Antigravity's actual implementation**.

Project-scoped rules mode (`rtk init --agent antigravity`) works regardless of
these assumptions.

| ID | Assumption | Fallback |
|----|-----------|----------|
| A1 | Antigravity stores user config in `~/.antigravity/` | If wrong, `--global` installs to wrong path. Use project-scoped mode instead. |
| A2 | `~/.antigravity/hooks.json` uses `{"hooks": {"preToolUse": [...]}}` schema | If wrong, hook entry is ignored. Use project-scoped mode instead. |
| A3 | `preToolUse` hooks receive `{"tool_input": {"command": "..."}}` on stdin and return `{"permission": "allow", "updated_input": {"command": "..."}}` on stdout | If wrong, hook script output is ignored. Use project-scoped mode instead. |

Once Antigravity's hook API is publicly documented, these assumptions should be
verified and this section removed.
