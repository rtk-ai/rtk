# MiniMax Code Hooks

`rtk init -g --agent minimax` installs `rtk-rewrite/` into MiniMax Code's local plugin directory
(`<data dir>/plugins/`, data dir = `$MINIMAX_DATA_DIR`, `$MAVIS_DATA_DIR`, or `~/.minimax`).

- `rtk-rewrite/.minimax-plugin/plugin.json`: plugin manifest (name must equal the directory name)
- `rtk-rewrite/hooks/hooks.json`: `PreToolUse`, matcher `bash` (MiniMax's native lowercase tool name), command `rtk hook minimax`
- `rtk-rewrite/icon.png`: required by MiniMax's manifest schema

The hook is the native `rtk hook minimax`; it replies with `hookSpecificOutput.updatedInput` and never a
`permissionDecision`. Handlers that use `args`, `shell`, `if` or `async` are skipped by MiniMax, so none are set.
Validate the package with `node tools/plugins/validate-plugin.mjs <dir>` from the minimax-code repo.
