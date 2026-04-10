# Antigravity Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Integration Paths

Antigravity is a Windsurf/Codeium fork. It does **not** support `hooks.json` with `preToolUse` entries. Two integration vectors are available:

### 1. Project-scoped rules (Cascade AI)

```bash
rtk init --agent antigravity
```

Installs `.agents/rules/antigravity-rtk-rules.md` in the project root. Antigravity's Cascade AI reads files matching `.agents/rules/**/*.md` (confirmed via `ruleEditor` custom editor registration in the Antigravity extension).

### 2. Claude Code hooks (transparent rewrite)

```bash
rtk init -g
```

Patches `~/.claude/settings.json` with a `PreToolUse` hook that transparently rewrites commands to use RTK. This works because Claude Code runs as an extension inside Antigravity and reads its own settings regardless of the host IDE.

## Specifics

- Antigravity registers a `ruleEditor` for `.agents/rules/**/*.md` — our rules file is picked up automatically
- Rules are prompt-level (Cascade follows the instruction to prefix commands with `rtk`)
- Claude Code hooks are programmatic (commands are rewritten before execution)
- For maximum token savings, use **both**: rules for Cascade + Claude Code hooks for Claude
- `rtk init --agent antigravity --global` is not supported (Antigravity has no hooks.json protocol)
