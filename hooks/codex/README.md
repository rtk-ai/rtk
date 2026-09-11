# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native programmatic hook: Codex dispatches a `PreToolUse` lifecycle event and honours a handler that rewrites the tool call, so RTK intercepts commands instead of asking the model to prefix them
- Installed by `rtk init --codex` (project, `.codex/hooks.json`) or `rtk init -g --codex` (global, `$CODEX_HOME/hooks.json`, otherwise `~/.codex/hooks.json`)
- Also installs `../rtk-awareness-full.md` to `RTK.md`, referenced from `AGENTS.md` via `@RTK.md`, as a second layer for sessions where the hook is disabled

Confirm the hook engine is present on a given install with:

```bash
codex features list | grep hooks     # → hooks   stable   true
```

## `hooks.json`

Codex nests every event under a single top-level `hooks` key and merges all handlers registered for an event, so RTK appends one entry and leaves the rest of the file alone:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "rtk hook codex", "timeout": 10 }
        ]
      }
    ]
  }
}
```

## Wire contract

Codex models its hooks on Claude Code's, so the payload is the same snake_case shape RTK already parses. Captured live from Codex 0.153.4:

```json
{
  "session_id": "01a08578-…",
  "turn_id": "01a08578-…",
  "cwd": "/path/to/workspace",
  "hook_event_name": "PreToolUse",
  "permission_mode": "bypassPermissions",
  "tool_name": "Bash",
  "tool_input": { "command": "git status" },
  "tool_use_id": "exec-7716f8ce-…"
}
```

The response is where Codex diverges, and it validates strictly:

| case | response |
| :--- | :--- |
| rewrite | `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"RTK auto-rewrite","updatedInput":{"command":"rtk git status"}}}` |
| deny | the same envelope with `"permissionDecision":"deny"` and a non-empty `permissionDecisionReason` |
| no rewrite | empty stdout — Codex falls through to its native handling |

Two of Codex's validation rules are why this needs its own handler rather than reusing `rtk hook claude`:

- `updatedInput` is honoured only together with `permissionDecision: "allow"`; otherwise Codex rejects the response outright (*"PreToolUse hook returned updatedInput without permissionDecision:allow"*). `run_claude` omits that field on its Ask path.
- A deny without a non-empty `permissionDecisionReason` is likewise rejected.

`continue: false`, `stopReason` and `suppressOutput` are all rejected as unsupported, so the handler never emits them.

## Gotcha: a new hook has to be trusted once

Codex reviews hook definitions before running them and skips one that has not been trusted. After `rtk init --codex`, start Codex and approve the hook in its `/hooks` review — otherwise the integration looks installed but never fires.
