# Grok Build TUI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Programmatic PreToolUse hook at `~/.grok/hooks/rtk.json` invokes `rtk hook grok`
- Hook emits `{"decision":"deny","reason":"use rtk <cmd> instead..."}` on a rewritable command — Grok's wire protocol has no `updatedInput`, so transparent rewrite is not possible at the hook layer
- `rtk-awareness.md` is installed as `~/.grok/GROK.md` and instructs the agent to self-prefix commands with `rtk`, treating the hook as a fail-safe
- Installed by `rtk init -g --grok`
