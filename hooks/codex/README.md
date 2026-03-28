# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Awareness document is injected into `AGENTS.md` with an `@RTK.md` reference
- On macOS and Linux, `rtk init --codex` also installs `config.toml` and `hooks.json` so Codex can run `rtk hook codex` for `PreToolUse`
- Codex currently uses deny-and-retry rather than transparent rewrite because `updatedInput` is not supported yet
- On Windows, RTK falls back to prompt-only guidance because Codex lifecycle hooks are disabled upstream
- Global install goes to `${CODEX_HOME:-~/.codex}`
- Project installs only activate for trusted projects

## Test

```bash
bash hooks/codex/test-rtk-rewrite.sh
```
