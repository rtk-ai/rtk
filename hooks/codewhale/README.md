# Codewhale Hooks

> Part of [`hooks/`](../README.md)

## Specifics

- Uses the `rtk hook codewhale` Rust binary (not a shell script) — no `jq` dependency
- `tool_call_before` hook declared in `~/.codewhale/config.toml` (user-scoped) or `.codewhale/hooks.toml` (project-scoped), with `condition = { type = "tool_name", name = "bash" }`
- Reads the tool input from the `DEEPSEEK_TOOL_ARGS` environment variable — Codewhale passes tool context via env, not stdin
- Returns `{"decision":"allow","updatedInput":{"command":"rtk ..."}}` for transparent rewrite; `updatedInput` preserves every other input field and only replaces `command`
- Non-shell tool / empty command / malformed JSON / RTK-unknown command → passthrough (exit 0, empty stdout)
- RTK permission deny → `{"decision":"deny","reason":"..."}`
- `condition = { type = "tool_name", name = "bash" }` matches all three shell-tool spellings (`bash`, `Bash`, `exec_shell`), which Codewhale treats as aliases

## Notes

- The hook is a subcommand of the RTK binary (`rtk hook codewhale`); no separate script or plugin file is installed
- Install: `rtk init --agent codewhale` (project) or `rtk init -g --agent codewhale` (global)
- Project hooks are executable repository configuration: Codewhale runs `.codewhale/hooks.toml` only after workspace trust and exact-byte approval — `/hooks review`, then `/hooks approve <digest>`
- Codewhale hook contract reference: `docs/HOOKS.md` in the Codewhale repository
