# oh-my-pi Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

This integration is for `oh-my-pi` (`@oh-my-pi/pi-coding-agent`, binary `omp`). For `@earendil-works/pi-coding-agent` (binary `pi`), see [`../pi/`](../pi/).

## Design Intent

RTK's oh-my-pi extension is a **rewrite-only token optimizer**. It mutates bash commands to their
`rtk`-prefixed equivalents, saving 60–90% context tokens.

**Permission gating is intentionally out of scope.** RTK does not block, confirm, or audit
commands — that concern belongs to a dedicated permission extension (e.g. one that gates
`rm -rf`, `sudo`, etc.). This separation keeps RTK's hook fast, predictable, and composable
with other oh-my-pi extensions.

## Specifics

- TypeScript extension using oh-my-pi's `ExtensionAPI` (not a shell hook, no `zx` dependency)
- Subscribes to `tool_call` event, narrows to `bash` tool via `isToolCallEventType`
- Calls `rtk rewrite` via `pi.exec`; mutates `event.input.command` in-place if rewrite differs
- All error paths return `undefined` (pass through); RTK never blocks execution
- Version guard at load time: checks `rtk >= 0.40.0`; warns and registers no-op if too old or missing
- Installed to `.omp/extensions/rtk.ts` by `rtk init --agent omp` (project-local) or `~/.omp/agent/extensions/rtk.ts` by `rtk init --agent omp --global`

## Uninstall

```bash
# Remove project-local install (run from the project root)
rtk init --uninstall --agent omp
# → removes .omp/extensions/rtk.ts

# Remove global install
rtk init --uninstall --agent omp --global
# → removes ~/.omp/agent/extensions/rtk.ts
```

Uninstall is idempotent — re-running when nothing is installed is a no-op.
Only the extension file is managed by install/uninstall.

## Testing

```bash
# Load the extension directly without installing
omp -e ./hooks/omp/rtk.ts

# Verify rewrites are active — ask the agent to run a command, then check history
rtk gain --history   # should show rtk-prefixed commands with savings %

# Test RTK_DISABLED passthrough
RTK_DISABLED=1 omp -e ./hooks/omp/rtk.ts
# → commands pass through unchanged; no rewrites in rtk gain --history

# Test version guard — temporarily shadow rtk with a stub that prints "rtk 0.39.0"
# → extension logs a warning at startup and registers a no-op; omp starts normally
```

## Design Notes

- All filtering logic lives in `rtk rewrite` (the Rust registry), not in this file
- Exit codes 0 and 3 both mean "rewrite and allow"; they are handled identically
- Uses `pi.exec` for subprocess management — consistent with oh-my-pi's extension API
- oh-my-pi reuses the same `PI_CODING_AGENT_DIR` environment variable as the upstream pi integration; the default base directory differs (`.omp/agent` vs `.pi/agent`)
