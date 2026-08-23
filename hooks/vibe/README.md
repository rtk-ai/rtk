# Mistral Vibe Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Uses the `rtk hook vibe` Rust binary (not a shell script) -- no `jq` dependency
- `pre_tool` hook declared in `~/.vibe/hooks.toml` (user-scoped) with `match = "bash"` and `strict = false`
- Reads Vibe's stdin JSON payload (`tool_name`, `tool_input.command`, `hook_event_name`, `session_id`)
- Returns `hook_specific_output.tool_input.command` for transparent rewrite plus a `system_message` for UI visibility
- Non-bash tool / empty command / malformed JSON / RTK-unknown command → passthrough (exit 0, empty stdout)
- RTK permission deny → `{"decision":"deny","reason":"..."}`
- Alongside the hook, a system prompt at `~/.vibe/prompts/rtk.md` is installed as a belt-and-suspenders fallback (skip with `--hook-only`)
- Installed globally via `rtk init -g --agent vibe`; there is no project-scoped variant

## Notes

- This directory intentionally holds only this README — the hook is a subcommand of the RTK binary (`rtk hook vibe`), not a standalone script or plugin file, so nothing is deployed here
- Vibe hook contract reference: https://docs.mistral.ai/vibe/code/cli/hooks
