# OMP (Oh My Pi) Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Design Intent

RTK's OMP extension is a **rewrite-only token optimizer**. It mutates bash commands to their
`rtk`-prefixed equivalents, saving 60–90% context tokens.

**Permission gating is intentionally out of scope.** RTK does not block, confirm, or audit
commands — that concern belongs to a dedicated permission extension. This separation keeps
RTK's hook fast, predictable, and composable with other OMP extensions.

## Specifics

- TypeScript extension using OMP's `ExtensionAPI` (loaded via the `hooks/pre/` discovery path)
- Subscribes to `tool_call` event, narrows to `bash` tool via `toolName` check
- Calls `rtk rewrite` via `pi.exec`; mutates `event.input.command` in-place if rewrite differs
- All error paths return `undefined` (pass through); RTK never blocks execution
- Version guard at load time: checks `rtk >= 0.23.0`; warns and registers no-op if too old or missing
- Installed to `.omp/hooks/pre/rtk.ts` (project-local) or `~/.omp/agent/hooks/pre/rtk.ts` (global)

## Architecture

OMP's extension runner loads `hooks/pre/*.ts` files as extension modules at startup. The
`ExtensionToolWrapper` passes tool input by reference, so mutating `event.input.command`
inside the handler directly modifies the params that OMP executes — the command is rewritten
transparently before the bash tool runs.

This is the same architecture as the Pi extension. OMP and Pi share a common extension API
lineage (Earendil Works).

## Uninstall

```bash
# Remove project-local install (run from the project root)
rtk init --uninstall --agent omp
# → removes .omp/hooks/pre/rtk.ts

# Remove global install
rtk init --uninstall --agent omp --global
# → removes ~/.omp/agent/hooks/pre/rtk.ts
```

Uninstall is idempotent — re-running when nothing is installed is a no-op.
Only the extension file is managed by install/uninstall.

## Testing

```bash
# Load the extension directly without installing
# (OMP loads extensions from hooks/pre/ automatically)

# Verify rewrites are active — ask the agent to run a command, then check history
rtk gain --history   # should show rtk-prefixed commands with savings %

# Test RTK_DISABLED passthrough
RTK_DISABLED=1 omp -p --mode text --no-session "Run: git status"
# → commands pass through unchanged; no rewrites in rtk gain --history
```

## Design Notes

- All filtering logic lives in `rtk rewrite` (the Rust registry), not in this file
- Exit codes 0 and 3 both mean "rewrite and allow"; they are handled identically
- Uses `pi.exec` for subprocess management — consistent with OMP's extension API
- Local interfaces (no `import type`) for maximum portability across OMP versions
