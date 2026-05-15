# Rovo Dev Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance via memory file (`AGENTS.md`) -- Rovo Dev natively
  reads `AGENTS.md` for project memory and `~/.rovodev/AGENTS.md` for
  user-global memory.
- `rtk-awareness.md` is **inlined** into `AGENTS.md` between
  `<!-- rtk-instructions -->` markers (Rovo Dev does not auto-expand `@file`
  references inside memory files, so the content must be embedded directly).
- Project-scope install (default): writes the RTK block into `./AGENTS.md`.
- Global install (`-g`): writes the RTK block into `~/.rovodev/AGENTS.md`
  so RTK awareness applies to every `acli rovodev` session for the user.
- Installed by `rtk init --agent rovodev` (project) or
  `rtk init -g --agent rovodev` (global).
- The block is idempotent — re-running `rtk init --agent rovodev` updates
  the block in place if the awareness content has changed.

## Why a memory file (and not a programmatic hook)?

Rovo Dev does support event hooks (configured via the `/hooks` slash
command), but its native `PreToolUse`-equivalent does not yet expose
`updatedInput` for shell commands the way Claude Code does. The most
reliable, transparent integration today is therefore prompt-level guidance
via `AGENTS.md`, which Rovo Dev always loads at session start.

## Testing

```bash
# Project-scope
rtk init --agent rovodev
grep rtk-instructions AGENTS.md

# Global-scope
rtk init -g --agent rovodev
grep rtk-instructions ~/.rovodev/AGENTS.md

# Uninstall (removes only the RTK block; preserves any other content)
rtk init --agent rovodev --uninstall      # local
rtk init -g --agent rovodev --uninstall   # global
```
