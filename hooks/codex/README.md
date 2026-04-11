# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Awareness document is injected inline into `AGENTS.md` with RTK-managed markers
- `rtk init --codex` also installs `.codex/config.toml` and `.codex/hooks.json` so Codex can run `rtk hook codex` for `PreToolUse`
- Codex currently uses deny-and-retry rather than transparent rewrite because `updatedInput` is not supported yet
- On Windows, native Codex hooks are enabled only when `codex --version` is `0.120.0+`; older builds fall back to prompt-only guidance
- Global install goes to `${CODEX_HOME:-~/.codex}`
- Project installs only activate for trusted projects

## Test

```bash
bash hooks/codex/test-rtk-rewrite.sh
```
