# Grok Build Integration

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- **Codex-style prompt guidance** — no programmatic PreToolUse rewrite
- Primary artifact: managed RTK block in `~/.grok/AGENTS.md` (recognized global instruction file)
- Optional sidecar: `~/.grok/RTK.md` (human-readable; **not** auto-loaded by Grok)
- Grok only auto-loads recognized top-level names under `~/.grok/` (`AGENTS.md`, `Claude.md`, …) — **not** arbitrary `*.md` such as bare `RTK.md`
- Grok does **not** apply Claude-style `updatedInput` mutation; automatic shell rewrite is not supported

## Install / Uninstall

```bash
# Install (global only)
rtk init -g --agent grok

# Uninstall
rtk init -g --uninstall --agent grok
```

**Global only.** Artifacts:

| Path | Purpose |
|------|---------|
| `~/.grok/AGENTS.md` | Managed `<!-- rtk-instructions -->` block (auto-loaded global rules) |
| `~/.grok/RTK.md` | Sidecar copy of awareness body (not auto-loaded) |

**AGENTS write** uses the same managed-block upsert as Copilot/Claude: creates or updates the RTK block while preserving other user content.

**Migration:** Install and uninstall remove a legacy hybrid PreToolUse file `~/.grok/hooks/rtk.json` if it invokes `rtk hook grok`. Other files under `~/.grok/hooks/` are left alone.

Restart Grok Build (or start a new session) after install or uninstall so global rules reload. Confirm with:

```bash
grok inspect
```

You should see `~/.grok/AGENTS.md` listed among project instructions.

## How it works

1. User runs `rtk init -g --agent grok`.
2. RTK upserts stock awareness into `~/.grok/AGENTS.md` and writes a matching `~/.grok/RTK.md` sidecar.
3. Grok includes `AGENTS.md` in context for TUI / headless / ACP sessions.
4. The model is instructed to prefix shell commands with `rtk` (e.g. `rtk git status`).

There is **no** `rtk hook grok` command. Savings are **soft** (model-dependent), same class as Codex CLI integration.

## Why not only `~/.grok/RTK.md`?

Grok’s project-rules docs list only recognized instruction filenames. Custom top-level names (including `RTK.md`) are not discovered. Codex works better with a soft install because it always loads `AGENTS.md` and resolves `@RTK.md`; RTK’s Grok install therefore writes the instruction body into `AGENTS.md` directly (Grok does not document `@file` includes).

## Why not a PreToolUse rewrite hook?

Grok’s documented PreToolUse API only supports:

```json
{"decision": "allow"}
{"decision": "deny", "reason": "..."}
```

It does not document Claude-style `updatedInput` mutation. Live validation showed allow + mutation still runs the original command, while deny hard-blocks without retry. Codex-style awareness is therefore the supported integration.

## Related

- Codex (same awareness pattern): [`hooks/codex/`](../codex/README.md)
- Awareness source: [`rtk-awareness.md`](rtk-awareness.md)
