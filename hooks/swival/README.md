# Swival Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Design Intent

RTK's Swival adapter is a **rewrite-only token optimizer**. It mutates the command Swival is
about to run into its `rtk`-prefixed equivalent, saving 60–90% context tokens.

**Permission gating is intentionally out of scope.** RTK never denies a command — it only ever
returns `allow`. Blocking, confirming, or auditing commands belongs to a dedicated middleware,
which keeps RTK's adapter fast, predictable, and composable with the rest of Swival's pipeline.

## Specifics

- Python script wired in as Swival's `command_middleware`, not a shell hook
- Reads a JSON request on stdin, writes a JSON response on stdout
- Calls `rtk rewrite` as a subprocess (2s timeout); returns the rewritten command when it differs
- Handles both `shell` mode (string command) and `argv` mode (list joined with `shlex.join`)
- All error paths return `{"action": "allow"}` (pass through); RTK never blocks execution
- Installed by `rtk init --agent swival` to `.rtk/swival-rtk-adapter.py` (project-local) or
  `~/.config/swival/rtk-adapter.py` (with `--global`), and `swival.toml` / `~/.config/swival/config.toml`
  gets `command_middleware` pointed at it

## Protocol

**Request** (stdin):

```json
{ "phase": "before", "mode": "shell", "command": "git status" }
```

**Response** (stdout) when rewritten:

```json
{ "action": "allow", "mode": "shell", "command": "rtk git status" }
```

When there is nothing to rewrite, the adapter responds with `{"action": "allow"}`.

## Uninstall

```bash
# Remove project-local install (run from the project root)
rtk init --uninstall --agent swival

# Remove the global install
rtk init --uninstall --global --agent swival
```

This deletes the adapter and removes the `command_middleware` entry RTK added, leaving any
value you set yourself untouched.
