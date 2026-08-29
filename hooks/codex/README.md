# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native `PreToolUse` processor via `rtk hook codex`
- Transparent Bash command rewriting through `permissionDecision: "allow"` + `updatedInput`
- Hook registration in `.codex/hooks.json` (project) or `$CODEX_HOME/hooks.json` (global)
- `rtk-awareness.md` is injected into `AGENTS.md` with an `@RTK.md` reference
- Installed to `$CODEX_HOME` when set, otherwise `~/.codex/`, by `rtk init --codex`

Codex requires users to review and trust non-managed hooks through `/hooks` before
they run. Hook failures and unsupported commands produce no output, so the original
tool call continues unchanged.

Because Codex requires `permissionDecision: "allow"` when applying
`updatedInput`, RTK only rewrites a conservative set of single, non-mutating
commands transparently. Compound commands and commands that may change state pass
through unchanged so Codex retains its native approval behavior.

## Live verification

The opt-in smoke test starts a real Codex turn and may consume API credits. It
records the Codex version, asks Codex to issue a raw `git status --short`, and
uses an isolated tracking database to verify that `rtk git status --short`
actually executed:

```bash
rtk cargo build
RTK_CODEX_E2E=1 scripts/test-codex-hook-e2e.sh
```
