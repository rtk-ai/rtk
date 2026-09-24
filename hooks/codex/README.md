# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native Rust `PreToolUse` processor: `rtk hook codex`
- Transparently rewrites `tool_input.command` with Codex's `updatedInput` response
- Registers a `Bash` matcher in `.codex/hooks.json` (project) or `$CODEX_HOME/hooks.json` (global); native Windows also registers `Shell` and `PowerShell`
- Writes the shared awareness document selected by `awareness.level` to `RTK.md`, referenced from `AGENTS.md`
- Installed by `rtk init --codex` (project) or `rtk init -g --codex` (global)
- Uninstalled by adding `--uninstall` to the corresponding project or global command

Codex requires `permissionDecision: "allow"` in the hook response for `updatedInput` to take effect. Codex applies the replacement before its normal command approval and sandbox checks, so those native checks still run on the rewritten command. Codex's command safety classifier does not currently unwrap the `rtk` binary, so classification is based on the rewritten command. This can add prompts for known-safe commands or obscure signals for wrapped mutating commands such as `git push`.

On native Windows, RTK emits `updatedInput` only for an explicit allow-rule match or the host's `bypassPermissions` mode. For default/ask decisions it emits no response, leaving Codex to run its native permission flow on the original command. This keeps the existing Windows approval boundary until rewritten-command classification is verified end to end. Elsewhere, the documented modes use the protocol-level `allow` described above. Missing or unknown permission modes, no match, malformed JSON, unsupported commands, heredocs, substitutions, and file redirections fail open: the hook exits successfully without stdout and Codex executes the original command.

The Codex handler uses RTK's shared hook decision pipeline through `Host::Codex` outside Windows. The Windows compatibility path retains the existing Claude-style explicit allow-rule check; RTK does not parse Codex execution rules, and Codex's native execution layer remains responsible for those rules.
An internal `AskRewrite` still emits the required protocol-level `allow` with
`updatedInput`; it is not recorded internally as an explicit permission grant.
This preserves transparent rewriting but does not remove the classifier
limitation described above.

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
