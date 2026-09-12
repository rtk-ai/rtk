# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native Rust `PreToolUse` processor: `rtk hook codex`
- Transparently rewrites `tool_input.command` with Codex's `updatedInput` response
- Registers a `Bash` matcher in `.codex/hooks.json` (project) or `$CODEX_HOME/hooks.json` (global)
- Writes the shared awareness document selected by `awareness.level` to `RTK.md`, referenced from `AGENTS.md`
- Installed by `rtk init --codex` (project) or `rtk init -g --codex` (global)
- Uninstalled by adding `--uninstall` to the corresponding project or global command

Codex requires `permissionDecision: "allow"` in the hook response for `updatedInput` to take effect. Codex applies the replacement before its normal command approval and sandbox checks, so those native checks still run on the rewritten command. Codex's command safety classifier does not currently unwrap the `rtk` binary, so classification is based on the rewritten command. This can add prompts for known-safe commands or obscure signals for wrapped mutating commands such as `git push`.

RTK rewrites the documented Codex permission modes. Missing or unknown permission modes, no match, malformed JSON, unsupported commands, heredocs, substitutions, and file redirections fail open: the hook exits successfully without stdout and Codex executes the original command.

## History in workspace-write sandboxes

Command rewriting can work even when the sandbox prevents RTK from writing its
SQLite history database. If recording reports `SQLITE_CANTOPEN`, run
`rtk init --codex` outside the sandbox to see an optional Codex configuration
snippet using the actual database parent directory. The path follows
`RTK_DB_PATH`, then `tracking.database_path`, then the platform default.

To opt in, merge the printed `sandbox_workspace_write.writable_roots` entry into
your Codex `config.toml`, preserving existing entries. SQLite needs directory
access for journal/WAL files as well as the database itself. RTK only prints the
snippet; it does not change sandbox settings. Review custom paths before granting
access, and prefer a dedicated RTK data directory.

See the [Codex configuration reference](https://developers.openai.com/codex/config-reference/).
