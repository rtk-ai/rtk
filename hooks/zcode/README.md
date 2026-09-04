# ZCode Agent Integration

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code.

## Specifics

- Prompt-level guidance only; ZCode reads user-level `AGENTS.md` at task start.
- `rtk-awareness.md` tells ZCode Agent to prefix shell commands with `rtk`.
- Installed to `~/.zcode/AGENTS.md` by `rtk init -g --agent zcode`.
