# Forge (forgecode) Integration

## Scope

RTK integration for [Forge](https://github.com/antinomyhq/forge) (forgecode), the open-source
Rust coding agent at `~/repos/forgecode`.

Owns: the `rtk init --agent forge` installer (`src/hooks/init/forge.rs`).

Does **not** own: any forge-side code. The command-rewriting logic lives in the shared RTK
rewrite registry (`src/discover/registry.rs`).

## How It Works

Forge has **no external process-hook API**. Its extension points are:

- **In-process Rust hooks** — `Hook` / `EventHandle` handlers wired in
  `crates/forge_app/src/app.rs` (see `crates/forge_domain/src/hook.rs`), covering
  lifecycle events including `ToolcallStart`.
- **`AGENTS.md` custom instructions** — read from `~/.forge/AGENTS.md` (global), the git
  root, and the working directory (`crates/forge_services/src/instructions.rs`).

RTK therefore integrates at the **rules-file tier** (lowest maintenance, no code to break):

```
rtk init --agent forge     # RTK awareness block in ./AGENTS.md (project)
rtk init -g --agent forge  # RTK awareness block in ~/.forge/AGENTS.md (global)
rtk init --agent forge --uninstall   # remove the block
```

The awareness file teaches the agent to prefix commands with `rtk` (the `full` level
behavior), so `git status` becomes `rtk git status` and the shell tool output is filtered
by RTK's rewrite registry — up to 90% fewer output tokens reaching the LLM.

## Planned Tier Upgrade: In-Process Hook

Because forge is also a Rust project, the natural next step is a **full hook** inside forge
itself, mirroring how other agents call `rtk rewrite`:

1. Add a `RtkRewriteHandler` in forge implementing
   `EventHandle<EventData<ToolcallStartPayload>>` (`crates/forge_domain/src/hook.rs`).
2. On `ToolcallStart` where `tool_call.name == "shell"`, extract `arguments["command"]`,
   run `rtk rewrite <command>` (subprocess, 2s timeout, best-effort), and use the rewritten
   command when non-empty and different.
3. Wire it into the `Hook` chain in `crates/forge_app/src/app.rs` with
   `.on_toolcall_start(...)`.

Note: the current `ToolcallStartPayload` is passed to hooks by shared reference and the
orchestrator executes its own clone of the tool call (`crates/forge_app/src/orch.rs:122-135`),
so forge-side support for rewriting requires either (a) rewriting in
`ForgeShell::execute` (`crates/forge_services/src/tool_services/shell.rs`) — a single choke
point that also covers `!`-prefixed user shell commands — or (b) extending the hook API to
return a modified tool call. Option (a) is the smallest change.

## Contract

- Idempotent install: the marker block (`<!-- rtk-instructions v2 -->`) is upserted, never
  duplicated, and user content is preserved.
- Uninstall removes only the RTK-owned block.
- `--dry-run` is supported.
- Global install targets `~/.forge/AGENTS.md`, which forge reads via
  `Environment::global_agentsmd_path()`.
