# RTK Plugin for DeepSeek Harness (DSH)

> Part of [`hooks/`](../README.md)

Rewrites DSH shell-tool commands to RTK equivalents before execution, so DSH receives compact command output without changing your workflow.

## Installation

```bash
rtk init -g --agent dsh
```

Global install only — DSH plugins are machine-level (installed into `$DSH_HOME/profiles/<profile>/`), so this only runs under `-g`. A project-local `rtk init --agent dsh` writes the `AGENTS.md` awareness block only and does not touch any DSH profile.

The installer, for every profile it finds under `$DSH_HOME/profiles/` (default `~/.dsh/profiles/`, override with `$DSH_HOME`):

- writes the plugin package to `<profile>/node_modules/rtk-rewrite/`
- appends one `insert` entry (`id: rtk-rewrite`) to `<profile>/cordis.patch.yml`, preserving every entry already there

No `package.json` edit, no `pnpm`/`dsh plugin add` call — see [Mechanism](#mechanism) for why.

## How it works

### The problem: DSH has no command-rewrite hook

DSH's `tools/pre-execute` event (`@deepseek-ai/dsh-tools`) is allow/deny-only by design — it cannot rewrite `exec.arguments`. There is no `PreToolUse`-style hook here the way there is for Claude Code, Codex, or Cursor.

### The seam: `ctx.shell`

Every bash/pwsh tool call and background job resolves through one cordis service, `ctx.shell` (`@deepseek-ai/dsh-tool-bash`'s `execute`/`resolve` calls). This plugin patches that service's `execute` method, so the command text is swapped for RTK's rewritten form immediately before the real executor (`dsh-bash-local` / `dsh-pwsh-local`) spawns it.

`ctx.shell` is a cordis-tracked service proxy — each *access* returns a fresh traceable wrapper, not the same object twice — so the patch is applied to the real instance behind it via `Symbol.for("cordis.original")`, not to whatever proxy a given access happens to produce.

### The decision: `rtk rewrite`

All rewrite logic lives in RTK's Rust binary. The plugin calls `rtk rewrite -- <command>` (the same single source of truth every other delegate — Hermes, Pi, OMP — calls) and only accepts exit 0 (allow-covered) or 3 (ask/partial-allow); see the exit-code table in `src/hooks/rewrite_cmd.rs`. Exit 1 (no rewrite) and 2 (deny) run the original command unchanged — a deny is left entirely to DSH's own permission system, never simulated here.

### Safety nets

1. **Fail open.** A missing `rtk` binary, non-zero/unaccepted exit code, timeout, or any unexpected exception falls back to the original command. This plugin is never the reason a command does not run.
2. **Whitelist.** A rewrite is only accepted when every `rtk <name>` prefix it inserts names a subcommand in a trusted list (`DEFAULT_COMMANDS` in `lib/index.js`, overridable via `config.commands`). Deliberately excludes host-sensitive aliases — e.g. PowerShell's `ls` is `Get-ChildItem` and returns an object stream, not text, so rewriting it to `rtk ls` would change what the command *returns*, not just how it is logged.
3. **Token-level guard.** A rewrite is only accepted when it is provably the original command with one or more extra literal `rtk`/`rtk.exe` tokens inserted — tokenized on shell operators (`&& || | ; &`) only. Any other difference (a changed, added, or removed argument) rejects the whole rewrite.
4. **Bypass switch.** `~/.dsh/rtk/off` (any content) disables the plugin globally without uninstalling it.

## Known limitations

- **Not a real pre-execution hook — a post-hoc substitution.** The rewrite happens inside `ctx.shell.execute`, after DSH has already recorded the tool call. DSH's session history, audit log, and UI card show the command exactly as the model wrote it; only the spawned process runs the RTK form. This is a consequence of DSH not yet implementing a `pre-tool-input-rewrite` proposal, not something this plugin can fix.
- **Only commands that go through `ctx.shell` are rewritten.** Foreground and background bash/pwsh tool calls do. A **persistent terminal (pty) session does not** — commands typed there run unmodified.
- **A deny (exit 2) is never simulated.** The plugin has no way to actually stop `ctx.shell.execute` from completing the call it already received, so it does not try; it runs the original command and lets DSH's own permission system do whatever it would otherwise do.

## Fail-open behavior

- `rtk` not found / not spawnable → original command runs unchanged
- `rtk rewrite` exits with anything other than 0 or 3 → original command runs unchanged
- `rtk rewrite` times out (default 3s) → original command runs unchanged
- the rewrite fails the token-level safety check or whitelist → original command runs unchanged
- any unexpected exception inside the plugin → original command runs unchanged

## Testing

```bash
node --test hooks/dsh/rtk-rewrite/rtk.test.mjs
```

Not run in CI (no Node/npm/pnpm setup step in `.github/workflows/ci.yml`), the same local-verification convention `hooks/opencode/rtk.test.mjs` and `hooks/pi/rtk.ts` follow.

## Uninstall

```bash
rtk init -g --agent dsh --uninstall
```

Removes `<profile>/node_modules/rtk-rewrite/` and the `id: rtk-rewrite` entry from `<profile>/cordis.patch.yml`, leaving every other entry in that file untouched.
