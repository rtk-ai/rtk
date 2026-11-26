# Claude Code Hooks

> Part of [`hooks/`](../README.md)

## Specifics

- `rtk-rewrite.sh` is the legacy shell hook, kept for reference and for the version check in `hook_check.rs`. Since v0.37.2 `rtk init -g` registers the native `rtk hook claude` command in `settings.json` instead, and deletes this script (with its `.rtk-hook.sha256`) when an older install left it
- Shell-based `PreToolUse` hook -- requires `jq` for JSON parsing
- Returns `updatedInput` JSON for transparent command rewrite (agent doesn't know RTK is involved)
- Exits silently (exit 0) on any failure: jq missing, rtk missing, rtk too old (< 0.23.0), no match
- Version guard checks `rtk --version` against minimum 0.23.0
- `../rtk-awareness.md` (or `-high` / `-full`, per `awareness.level` in config.toml) is the instructions file `rtk init -g` writes to `~/.claude/RTK.md` and references from `~/.claude/CLAUDE.md` as `@RTK.md`; a project-only `rtk init` puts it in `./CLAUDE.md` as a block
