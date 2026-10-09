# OpenCode Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- TypeScript plugin (not a shell hook)
- Supports **OpenCode 2.0** (`execute.before` via `{ id, setup }`) and **OpenCode 1.x >= 1.18.29** (`tool.execute.before`), the dual-shape default export [OpenCode's own V2 docs prescribe](https://opencode.ai/v2/docs/build/plugins)
- Requires OpenCode **1.18.29+ or 2.x**. Older V1 loaders call every export as `fn(input)`, which the object default is not, so there the plugin does not load; `rtk init -g --opencode` says so when it can read `opencode --version`. There is no separate file for those versions
- Thin delegating shim: calls `rtk hook opencode` as a subprocess, and the Rust side is the single source of truth for rewrite rules
- The Rust side judges the command against OpenCode's own permission rules and answers `{}` whenever the rewrite would change the verdict those rules give — OpenCode evaluates the final command itself, so RTK never lifts a deny, silences an ask, or blocks an allow (#4195). Rules are read from root and project `opencode.json`/`.jsonc`: the legacy `permission` map and `agent.<name>.permission` first, then the V2 `permissions` list, `permission.shell` and `agents.<name>.permissions`, whatever order they appear in the file. Global before project, agent last
- Uses standard `node:child_process.execFile` (compatible with OpenCode CLI, OpenCode Desktop/Electron, and Windows; zero Bun/zx dependencies)
- Passes the command as a single argv element, never through a shell
- Resolves rtk from the system `PATH` only (`PATHEXT` on Windows), and skips a PATH entry that is a directory. OpenCode's shell tool runs with OpenCode's own `PATH` and never sources `.profile`/`.bashrc`, so an rtk found outside `PATH` cannot be spawned as a bare `rtk …` — see #4462
- Probes the resolved binary once per session with `rtk hook opencode --help` (cached): exit 0 means the subcommand is there, exit 2 means it predates it, and a broken or wrong-arch binary fails to spawn. Not a version number — a develop build reports `rtk 0.49.0` and so does a release that lacks the subcommand
- Passes OpenCode 2.x's `event.agent` as `rtk hook opencode --agent <name>`, so an `agent.<name>.permission` ask or deny is judged against the agent that runs the command. OpenCode 1.x sends no agent field, so that path stays root-only
- Two separate off switches, both named `RTK_DISABLED`. The per-command form is `RTK_DISABLED=1 git status`, an env prefix inside the command string; the Rust rewrite engine handles that one and the plugin passes the string through untouched. The plugin also reads `RTK_DISABLED=1` from OpenCode's own process environment, which switches rewriting off for the whole session
- Mutates command in-place (`event.input.command` in v2, `output.args.command` in v1) if the answered rewrite differs from the original
- Any failure — non-zero exit, timeout, non-JSON stdout, missing binary — passes the command through unchanged
- Installed to `~/.config/opencode/plugins/rtk.ts` by `rtk init -g --opencode`
- Tests: `node --test hooks/opencode/rtk.test.mjs`
