# Pi Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Design Intent

RTK's Pi extension is a **rewrite-only token optimizer**. It mutates bash commands to their
`rtk`-prefixed equivalents, cutting up to 90% of the bash output that reaches the context.

**Permission gating is intentionally out of scope.** RTK does not block, confirm, or audit
commands — that concern belongs to a dedicated permission extension (e.g. one that gates
`rm -rf`, `sudo`, etc.). This separation keeps RTK's hook fast, predictable, and composable
with other Pi extensions.

## Specifics

- TypeScript extension using Pi's `ExtensionAPI` (not a shell hook, no `zx` dependency)
- Subscribes to `tool_call` event, narrows to `bash` tool via a local `isBashToolCallEvent` guard (avoids importing the package's value-exported `isToolCallEventType`, which pulls in its whole barrel at extension load)
- Calls `rtk rewrite` via `pi.exec`; mutates `event.input.command` in-place if rewrite differs
- All error paths return `undefined` (pass through); RTK never blocks execution
- Version guard at load time: checks `rtk >= 0.23.0`; warns and registers no-op if too old or missing
- Install updates only current or known historical stock content. For modified or unrelated content it asks before overwriting; `--auto-patch` approves and `--no-patch` leaves the file unchanged. A declined protected update, including `--no-patch`, exits nonzero; in `--dry-run` mode it reports the prompt without changing files or creating directories. LF and CRLF line endings are treated as the same stock content.
- Installed to `.pi/extensions/rtk.ts` by `rtk init --agent pi` (project-local) or `~/.pi/agent/extensions/rtk.ts` by `rtk init --agent pi --global`

The same extension is shared with Oh My Pi (OMP) through OMP's `legacy-pi-compat` layer. OMP installs it at `.omp/extensions/rtk.ts` (project-local) or `~/.omp/agent/extensions/rtk.ts` (global). The global OMP path follows `PI_CODING_AGENT_DIR`, which OMP also honors. When the Pi and OMP paths in either scope resolve to the same file, RTK records Pi/OMP ownership in an adjacent hidden `.rtk-agents` state file. A valid sidecar is authoritative; missing or unreadable state is treated as uncertain, warned about, and not used to claim sole ownership. A definitively shared project or global file prompts before removal (`--auto-patch` approves it); an uncertain legacy or corrupt-state uninstall warns and proceeds, while `--no-patch` still leaves definitively shared files in place with a nonzero exit. RTK currently targets OMP's default profile and `.omp` project directory, not named profiles or custom `PI_CONFIG_DIR` locations.

## Uninstall

```bash
# Remove project-local install (run from the project root)
rtk init --uninstall --agent pi
# → removes .pi/extensions/rtk.ts

# Remove global install
rtk init --uninstall --agent pi --global
# → removes ~/.pi/agent/extensions/rtk.ts
```

Uninstalling an absent extension is a no-op. Current and known historical stock files are removed, while modified RTK content is preserved unless the removal is approved: a normal uninstall asks first, `--auto-patch` approves it and copies the file to `rtk.ts.bak` before removing, and `--no-patch` fails with a manual-removal message; `--dry-run` previews the prompt. Unreadable content is left alone but causes a normal uninstall to fail nonzero; `--dry-run` reports it and succeeds. Unrelated content is left alone. When Pi and OMP paths in either scope resolve to the same file, a valid sidecar recording both agents makes uninstall ask before removing it; `--auto-patch` approves and `--no-patch` leaves it in place with a nonzero exit. Missing or unreadable ownership state produces a warning and proceeds without definitive shared-file protection. Only the extension file and RTK's adjacent ownership state are managed by install/uninstall.

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
