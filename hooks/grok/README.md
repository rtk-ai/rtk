# Grok CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Uses the `rtk hook grok` Rust binary (not a shell script) — no `jq` dependency
- **Deny-with-suggestion** strategy (Grok does not honor Claude-style `updatedInput` as of 0.2.x)
- Accepts camelCase Grok envelopes (`toolName` / `toolInput`) and snake_case fallbacks
- Install target: `$GROK_HOME/hooks/rtk-rewrite.json` (default `~/.grok`; multi-account profiles set `GROK_HOME`)
- Awareness: `RTK.md` + `@RTK.md` (or absolute path) in `AGENTS.md` (Codex-style)

## Install / uninstall

```bash
rtk init -g --agent grok
rtk init -g --agent grok --uninstall
rtk init --show   # Claude default show; Grok status via files under $GROK_HOME
```

Respects `GROK_HOME` for multi-account installs — re-run init per account profile.

## Input (stdin JSON)

```json
{
  "hookEventName": "pre_tool_use",
  "sessionId": "…",
  "toolName": "run_terminal_command",
  "toolInput": { "command": "git status" }
}
```

Matcher installed by `rtk init`: `Bash|run_terminal_command`.

## Output (stdout JSON)

**Allow:**

```json
{ "decision": "allow" }
```

**Deny (rewrite suggestion):**

```json
{
  "decision": "deny",
  "reason": "RTK auto-rewrite (token-optimized). Re-run this exact command: `rtk git status`"
}
```

RTK does **not** emit Claude `updatedInput` / `permissionDecision` for Grok.

## Testing

```bash
printf '%s' '{"toolName":"run_terminal_command","toolInput":{"command":"git status"}}' \
  | rtk hook grok
# → decision=deny, reason contains `rtk git status`

printf '%s' '{"toolName":"run_terminal_command","toolInput":{"command":"rtk git status"}}' \
  | rtk hook grok
# → decision=allow

cargo test grok_
```

## Limitations

| Limit | Detail |
|-------|--------|
| No transparent rewrite | Grok ignores `updatedInput`; model must retry the suggested command |
| Shell tools only | Built-ins (`read_file`, `grep`, …) are not rewritten |
| Deny loop risk | If the model ignores the suggestion, the same raw command is denied again (MVP has no session cache) |
| Reload required | After install: reload hooks in Grok (`/hooks` → `r`) or restart the session |
| Fail-open | Parse/IO errors return `allow` (Never Block) |

## Migration from prototype hooks

If you previously installed a Python/shell prototype as `$GROK_HOME/hooks/rtk-rewrite.json`, `rtk init -g --agent grok` overwrites that RTK-managed file with the official binary hook (`rtk hook grok`). User-owned hook files in the same directory are left untouched.
