# Pi Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Design Intent

RTK's Pi extension is a **rewrite-only token optimizer**. It mutates bash commands to their
`rtk`-prefixed equivalents, saving 60–90% context tokens.

**Permission gating is intentionally out of scope.** RTK does not block, confirm, or audit
commands — that concern belongs to a dedicated permission extension (e.g. one that gates
`rm -rf`, `sudo`, etc.). This separation keeps RTK's hook fast, predictable, and composable
with other Pi extensions.

## Specifics

- TypeScript extension using Pi's `ExtensionAPI` (not a shell hook, no `zx` dependency)
- Subscribes to `tool_call` event, narrows to `bash` tool via `isToolCallEventType`
- Calls `rtk rewrite` via `pi.exec`; mutates `event.input.command` in-place if rewrite differs
- All error paths return `undefined` (pass through); RTK never blocks execution
- Version guard at load time: checks `rtk >= 0.23.0`; warns and registers no-op if too old or missing
- Installed to `.pi/extensions/rtk.ts` by `rtk init --agent pi` (project-local) or `~/.pi/agent/extensions/rtk.ts` by `rtk init --agent pi --global`

## Uninstall

```bash
# Remove project-local install (run from the project root)
rtk init --uninstall --agent pi
# → removes .pi/extensions/rtk.ts

# Remove global install
rtk init --uninstall --agent pi --global
# → removes ~/.pi/agent/extensions/rtk.ts
```

Uninstall is idempotent — re-running when nothing is installed is a no-op.
Only the extension file is managed by install/uninstall.

## Testing

```bash
# Load the extension directly without installing
pi -e ./hooks/pi/rtk.ts

# Verify rewrites are active — ask the agent to run a command, then check history
rtk gain --history   # should show rtk-prefixed commands with savings %

# Test RTK_DISABLED passthrough
RTK_DISABLED=1 pi -e ./hooks/pi/rtk.ts
# → commands pass through unchanged; no rewrites in rtk gain --history

# Test version guard — temporarily shadow rtk with a stub that prints "rtk 0.22.0"
# → extension logs a warning at startup and registers a no-op; pi starts normally
```

## Design Notes

- All filtering logic lives in `rtk rewrite` (the Rust registry), not in this file
- Exit codes 0 and 3 both mean "rewrite and allow"; they are handled identically
- Uses `pi.exec` for subprocess management — consistent with Pi's extension API
