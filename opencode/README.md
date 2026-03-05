# RTK — OpenCode Integration

Use [RTK (Rust Token Killer)](https://github.com/rtk-ai/rtk) with [OpenCode](https://github.com/sst/opencode) to save 60-90% of tokens on common dev operations.

## How It Works

RTK already supports Claude Code via a `PreToolUse` hook. OpenCode uses a different plugin system — `tool.execute.before` hooks inside `.ts`/`.js` plugin files.

This integration provides:

- **`plugins/rtk-rewrite.ts`** — An OpenCode plugin that intercepts bash tool calls and rewrites them to their rtk equivalents using `rtk rewrite` (the same single-source-of-truth rewrite engine used by the Claude Code hook).
- **`rtk-awareness.md`** — A slim set of instructions to append to your `AGENTS.md` so the LLM knows about rtk meta-commands (`rtk gain`, `rtk discover`, etc.).
- **`setup.sh`** — One-command installer that copies the plugin and patches AGENTS.md.

## Quick Start

### Automated

```bash
# Global install (all OpenCode projects):
./opencode/setup.sh

# Per-project install:
./opencode/setup.sh --local
```

### Manual

1. **Copy the plugin** to your OpenCode plugins directory:

   ```bash
   # Global
   mkdir -p ~/.config/opencode/plugins
   cp opencode/plugins/rtk-rewrite.ts ~/.config/opencode/plugins/

   # Or per-project
   mkdir -p .opencode/plugins
   cp opencode/plugins/rtk-rewrite.ts .opencode/plugins/
   ```

2. **Add RTK awareness to AGENTS.md** (optional but recommended — lets the LLM use meta-commands like `rtk gain`):

   ```bash
   cat opencode/rtk-awareness.md >> ~/.config/opencode/AGENTS.md
   # or
   cat opencode/rtk-awareness.md >> AGENTS.md
   ```

3. **Restart OpenCode** and run a command — e.g. `git status` — to verify rewriting works.

## Requirements

- [rtk](https://github.com/rtk-ai/rtk) >= 0.23.0 on your PATH
- [OpenCode](https://github.com/sst/opencode)

The plugin checks for rtk at startup and disables itself gracefully if rtk is missing or too old.

## Commands Rewritten

The plugin delegates to `rtk rewrite`, which is the same registry used by the Claude Code hook. All of these are rewritten transparently:

| Raw Command | Rewritten To |
|---|---|
| `git status/diff/log/add/commit/push/pull/...` | `rtk git ...` |
| `gh pr/issue/run` | `rtk gh ...` |
| `cargo test/build/clippy` | `rtk cargo ...` |
| `cat <file>` | `rtk read <file>` |
| `rg/grep <pattern>` | `rtk grep <pattern>` |
| `ls` | `rtk ls` |
| `vitest/pnpm test` | `rtk vitest run` |
| `tsc/pnpm tsc` | `rtk tsc` |
| `eslint/pnpm lint` | `rtk lint` |
| `ruff check/format` | `rtk ruff ...` |
| `pytest` | `rtk pytest` |
| `go test/build/vet` | `rtk go ...` |
| `docker ps/images/logs` | `rtk docker ...` |
| `kubectl get/logs` | `rtk kubectl ...` |
| `curl` | `rtk curl` |

Commands already using `rtk`, heredocs (`<<`), and unrecognized commands pass through unchanged.

## Comparison with Claude Code Integration

| Aspect | Claude Code | OpenCode |
|---|---|---|
| Hook mechanism | `PreToolUse` shell hook in `settings.json` | `tool.execute.before` TypeScript plugin |
| Rewrite engine | `rtk rewrite` (Rust binary) | Same `rtk rewrite` binary |
| Setup command | `rtk init --global` | `./opencode/setup.sh` |
| Context file | `CLAUDE.md` / `RTK.md` | `AGENTS.md` |
| Plugin location | `~/.claude/hooks/` | `~/.config/opencode/plugins/` |

## Uninstalling

```bash
# Global
rm ~/.config/opencode/plugins/rtk-rewrite.ts

# Per-project
rm .opencode/plugins/rtk-rewrite.ts
```

Then manually remove the RTK section from your `AGENTS.md` if desired.
