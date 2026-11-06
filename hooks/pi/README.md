# Pi Hooks

> Part of [`hooks/`](../README.md)

## Design Intent

RTK's Pi extension is a **rewrite-only token optimizer**. It mutates bash commands to their
`rtk`-prefixed equivalents, so filtered output reaches the context.

**Permission gating is intentionally out of scope.** RTK does not block, confirm, or audit
commands — that concern belongs to a dedicated permission extension (e.g. one that gates
`rm -rf`, `sudo`, etc.). This separation keeps RTK's hook fast, predictable, and composable
with other Pi extensions.

## Specifics

- TypeScript extension using Pi's `ExtensionAPI` (not a shell hook, no `zx` dependency)
- Subscribes to `tool_call` event, narrows to `bash` tool
- Calls `rtk rewrite` via `pi.exec`; mutates `event.input.command` in-place if rewrite differs
- All error paths return `undefined` (pass through); RTK never blocks execution on its own
- Version guard at load time: checks `rtk >= 0.23.0`; warns and registers no-op if too old or missing
- Install updates only current or known historical stock content. For modified or unrelated content it asks before overwriting; `--auto-patch` approves and `--no-patch` leaves the file unchanged. A declined protected update, including `--no-patch`, exits nonzero; in `--dry-run` mode it reports the prompt without changing files or creating directories. LF and CRLF line endings are treated as the same stock content.
- Installed to `.pi/extensions/rtk.ts` by `rtk init --agent pi` (project-local) or `~/.pi/agent/extensions/rtk.ts` by `rtk init --agent pi --global`

The same extension is shared with Oh My Pi (OMP) through OMP's `legacy-pi-compat` layer. OMP installs it at `.omp/extensions/rtk.ts` (project-local) or `~/.omp/agent/extensions/rtk.ts` (global). The global OMP path follows `PI_CODING_AGENT_DIR`, which OMP also honors. When the Pi and OMP paths in either scope resolve to the same file, RTK records which agents it installed in an adjacent hidden `.rtk-agents` state file. Uninstalling one agent prompts before removing the file when the record names the other (`--auto-patch` approves it; `--no-patch` leaves it in place with a nonzero exit). A record RTK starts over an extension that already existed is marked partial, since RTK cannot tell who installed that file. Whenever RTK cannot tell who uses the file, because the record is missing or cannot be read, or is partial and does not name the other agent, uninstall warns and proceeds rather than claim sole ownership. When uninstall asks about, or refuses to remove, an extension with local changes, it says whether the other agent loads the same file or may load it, since removing it removes it for both. RTK currently targets OMP's default profile and `.omp` project directory, not named profiles or custom `PI_CONFIG_DIR` locations.

## Uninstall

```bash
# Remove project-local install (run from the project root)
rtk init --uninstall --agent pi
# → removes .pi/extensions/rtk.ts

# Remove global install
rtk init --uninstall --agent pi --global
# → removes ~/.pi/agent/extensions/rtk.ts
```

Uninstalling an absent extension is a no-op. Current and known historical stock files are removed, while modified RTK content is preserved unless the removal is approved: a normal uninstall asks first, since the removal discards those changes; `--auto-patch` approves, a declined removal, including `--no-patch`, fails nonzero with a manual-removal message, and `--dry-run` previews the prompt. Unreadable content is left alone but causes a normal uninstall to fail nonzero; `--dry-run` reports it and succeeds. Unrelated content is left alone. When Pi and OMP paths in either scope resolve to the same file and its ownership record names OMP, uninstall asks before removing it; `--auto-patch` approves and `--no-patch` leaves it in place with a nonzero exit. When RTK cannot tell who uses the file, it warns and proceeds instead. Only the extension file and RTK's adjacent ownership state are managed by install/uninstall.
