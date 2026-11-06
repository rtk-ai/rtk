# OpenCode Hooks

> Part of [`hooks/`](../README.md)

## Specifics

- TypeScript plugin (not a shell hook)
- Supports **OpenCode 2.0** (`execute.before` via `{ id, setup }`) and **OpenCode 1.x >= 1.18.29** (`tool.execute.before`), the dual-shape default export [OpenCode's own V2 docs prescribe](https://opencode.ai/v2/docs/build/plugins)
- Requires OpenCode **1.18.29 or newer**, the floor OpenCode's V2 plugin docs give for this dual-shape export. With an older OpenCode, `rtk init -g --opencode` writes nothing and leaves an existing plugin alone, when it can read `opencode --version`
- Thin delegating shim: calls `rtk hook opencode` as a subprocess, and the Rust side is the single source of truth for rewrite rules
- The Rust side judges the command against OpenCode's own permission rules and answers `{}` whenever the rewrite would change the verdict those rules give — OpenCode evaluates the final command itself, so RTK never lifts a deny, silences an ask, or blocks an allow (#4195). Rules are read from the global directory (`$XDG_CONFIG_HOME/opencode`, or `~/.config/opencode`), where `config.json`, `opencode.json` and `opencode.jsonc` are all merged in that order, then from the project `opencode.json`/`.jsonc`. Within a file, rules load in the order OpenCode resolves them: the legacy `permission` map first, then the 2.x `permissions` list, whatever order the file declares them. Every file's root rules load before any file's agent rules, so a project `allow` cannot outrank a global agent `ask`. 2.x list actions match as globs against `shell`, so `sh*` covers it; a list rule whose action only names `bash` does not count, because OpenCode ignores it there. A legacy map key the two generations read differently (`shell` and `sh*` apply only on 2.x, `b*` only on 1.x) makes rtk leave the command as typed, because it cannot tell which generation it is serving. Global rules come before project rules. An agent defined as a markdown file in `agents/` or `agent/`, under the global directory or any `.opencode/` from the working directory up, whose frontmatter declares `permission:` or `permissions:`, is not parsed here, so no rewrite happens for that agent and OpenCode applies its own rules.
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
