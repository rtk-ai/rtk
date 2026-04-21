# Amp Code CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance via awareness document -- no programmatic hook (Amp's `delegate` permission is exit-code-only and cannot rewrite commands)
- `rtk-awareness.md` is installed as `RTK.md` and referenced from `AGENTS.md` with an `@<abs-path>/RTK.md` line
- Installed to `$AMP_CONFIG_DIR` when set, otherwise `~/.config/amp/`, by `rtk init -g --agent amp`
