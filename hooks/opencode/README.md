# OpenCode Hooks

> Part of [`hooks/`](../README.md)

## Specifics

- TypeScript plugin (not a shell hook)
- Supports **OpenCode 2.0** (`execute.before` via `{ id, setup }`) and **OpenCode 1.x >= 1.18.29** (`tool.execute.before`), the dual-shape default export [OpenCode's own V2 docs prescribe](https://opencode.ai/v2/docs/build/plugins)
- Requires OpenCode **1.18.29 or newer**, the floor OpenCode's V2 plugin docs give for this dual-shape export. With an older OpenCode, `rtk init -g --opencode` writes nothing and leaves an existing plugin alone, when it can read `opencode --version`
- Thin delegating shim: calls `rtk hook opencode` as a subprocess, and the Rust side is the single source of truth for rewrite rules
- The Rust side judges the command against OpenCode's own permission rules and answers `{}` whenever the rewrite would change the verdict those rules give — OpenCode evaluates the final command itself, so RTK never lifts a deny, silences an ask, or blocks an allow (#4195). Rules are read from root and project `opencode.json`/`.jsonc` in the order OpenCode resolves them: the legacy `permission` map first, then the 2.x `permissions` list, whatever order the file declares them. Every file's root rules load before any file's agent rules, so a project `allow` cannot outrank a global agent `ask`. 2.x list actions match as globs against `shell`, so `sh*` covers it; a list rule whose action only names `bash` does not count, because OpenCode ignores it there. A legacy map key the two generations read differently (`shell` and `sh*` apply only on 2.x, `b*` only on 1.x) makes rtk leave the command as typed, because it cannot tell which generation it is serving. Global rules come before project rules. An agent defined as a markdown file in `agents/` or `agent/`, under `~/.config/opencode/` or any `.opencode/` from the working directory up, whose frontmatter declares `permission:` or `permissions:`, is not parsed here, so no rewrite happens for that agent and OpenCode applies its own rules.
- Uses standard `node:child_process.execFile` (compatible with OpenCode CLI, OpenCode Desktop/Electron, and Windows; zero Bun/zx dependencies)
- Passes the command as a single argv element, never through a shell
- Resolves rtk in this order, skipping any directory named `rtk`: `RTK_BIN` (an explicit override, so it wins over an rtk on `PATH`), then OpenCode's own `PATH` (`PATHEXT` on Windows), then `~/.cargo/bin`, `~/.local/bin` (`install.sh`'s default), `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin` and, on Windows, `%LOCALAPPDATA%\cargo\bin` and `%ProgramFiles%\rtk`. The rewrite stays a bare `rtk …`, so OpenCode judges the same string `rtk hook opencode` judged; to make every `rtk` in it resolve, the plugin appends rtk's directory to the end of the spawned shell's `PATH` through OpenCode 2.x's `ctx.shell.hook("create.before")` and OpenCode 1.x's `shell.env` hook. Appending changes nothing for any other command, and nothing at all when rtk is already on `PATH` (#4462)
- Passes OpenCode 2.x's `event.agent` as `rtk hook opencode --agent <name>`, so an `agent.<name>.permission` ask or deny is judged against the agent that runs the command. OpenCode 1.x sends no agent field, so that path stays root-only
- Two separate off switches, both named `RTK_DISABLED`. The per-command form is `RTK_DISABLED=1 git status`, an env prefix inside the command string; the Rust rewrite engine handles that one and the plugin passes the string through untouched. The plugin also reads `RTK_DISABLED=1` from OpenCode's own process environment, which switches rewriting off for the whole session
- Mutates command in-place (`event.input.command` in v2, `output.args.command` in v1) if the answered rewrite differs from the original
- Any failure — non-zero exit, timeout, non-JSON stdout, missing binary — passes the command through unchanged
- Installed to `~/.config/opencode/plugins/rtk.ts` by `rtk init -g --opencode`
- Tests: `node --test hooks/opencode/rtk.test.mjs`
