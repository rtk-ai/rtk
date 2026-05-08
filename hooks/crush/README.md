# Crush + RTK Integration

RTK integrates natively with [Charmbracelet Crush](https://github.com/charmbracelet/crush) using its `PreToolUse` hook system — a Tier 1 (Native Hook) integration.

When you run `rtk init --agent crush`, RTK will:

1. **Deploy a PreToolUse hook** to `.crush/hooks/rtk-rewrite.sh` — intercepts every `bash` call, runs `rtk rewrite`, and returns the rewritten command via `updated_input`.
2. **Patch `crush.json`** (or `.crush.json` for global) — registers the hook under `hooks.PreToolUse` with matcher `^bash$`.
3. **Install a skill file** to `.agents/skills/rtk-awareness/SKILL.md` — provides the model with context on *why* its commands are being rewritten, as fallback documentation.

This ensures all bash commands executed by Crush are automatically routed through RTK — deterministic, no model awareness required.

## Files

- `rtk-rewrite.sh` — PreToolUse hook script (fail-open, delegates to `rtk rewrite`)
- `SKILL.md` — RTK awareness skill (explains token savings to the model)

## Architecture

```
Agent runs "cargo test"
  -> Crush fires PreToolUse hook (rtk-rewrite.sh)
  -> Calls rtk rewrite "cargo test"
  -> Returns {"updated_input": {"command": "rtk cargo test"}}
  -> Crush executes the rewritten command
  -> Model sees filtered output (60-90% fewer tokens)
```
