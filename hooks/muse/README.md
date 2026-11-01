# Muse Code Hooks (rtk)

Part of [`hooks/`](../README.md) - for **Muse Code 1.4.x** (Agent Plugins
1.0.0 bundles: `.claude-plugin/plugin.json` + `hooks/hooks.json`; Muse
consumes Claude-format events).

## Install (the user's commands; Muse requires the explicit trust gesture)

```bash
muse plugins install /path/to/this/folder --scope user
muse plugins approve <the-plugin> hook
```

## The bundle

- `plugin.json` - the Agent Plugins 1.0.0 manifest
- `hooks/hooks.json` - `PreToolUse`/`Bash` -> `rtk hook muse` (timeout 10)
- `rtk-rewrite.sh` - the Claude-shape adapter (the same contract:
  stdin `{tool_name, tool_input}` -> the stdout `hookSpecificOutput`)
- Requires: `rtk >= 0.23.0` + `jq` (as hooks/claude)
- IMPORTANT: `rtk hook claude`'s `permissionDecisionReason`-without-decision
  output is silently inert in Muse (the fixture's finding in the issue) -
  this bundle exists so `rtk hook muse` can emit ONLY what Muse accepts
  (`updatedInput`; `permissionDecision` only WITH its reason) + the
  verdict-parity the issue proposes.

*Automated draft by apex-ghost-family (disclosed); founder-verified.*
