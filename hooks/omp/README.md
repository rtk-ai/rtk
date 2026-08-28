# Oh My Pi (omp) Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Design Intent

RTK's omp extension is a **rewrite-only token optimizer** for Oh My Pi (omp), the
successor to the Pi coding agent. It mutates bash commands to their `rtk`-prefixed
equivalents, cutting up to 90% of the bash output that reaches the context.

omp and Pi expose the **same extension API**, so the extension file is identical to
[`hooks/pi/rtk.ts`](../pi/rtk.ts); only the install location differs (the omp agent
directory instead of the Pi one).

**Permission gating is intentionally out of scope** — RTK does not block, confirm,
or audit commands. That concern belongs to a dedicated permission extension.

## Specifics

- TypeScript extension using the shared `ExtensionAPI` (not a shell hook, no `zx`
  dependency)
- Subscribes to `tool_call`, narrows to `bash` via the `isBashToolCallEvent` guard
- Calls `rtk rewrite` via `pi.exec`; mutates `event.input.command` in-place when the
  rewrite differs
- All error paths return `undefined` (pass through); RTK never blocks execution
- Version guard at load time: checks `rtk >= 0.23.0`; warns and registers no-op if
  too old or missing
- Installed to `.omp/agent/extensions/rtk.ts` (project-local) by
  `rtk init --agent omp`, or `~/.omp/agent/extensions/rtk.ts` by
  `rtk init --agent omp --global`

## Install

```bash
# Global (all omp projects) — recommended
rtk init --agent omp --global --auto-patch

# Project-local
rtk init --agent omp --auto-patch
```

The omp agent directory honors `PI_CODING_AGENT_DIR` when set; otherwise it defaults
to `~/.omp/agent`.

## Uninstall

```bash
rtk init --uninstall --agent omp --global   # remove global install
rtk init --uninstall --agent omp           # remove project-local install
```

Uninstall is idempotent — re-running when nothing is installed is a no-op.

## Testing

```bash
# Load the extension directly without installing
omp -e ./hooks/pi/rtk.ts

# Verify rewrites are active — run a command, then check history
rtk gain --history   # should show rtk-prefixed commands with savings %
```
