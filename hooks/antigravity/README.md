# Google Antigravity Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native programmatic hook: Antigravity dispatches a `PreToolUse` lifecycle event and lets the handler rewrite the tool call, so RTK intercepts commands instead of asking the model to prefix them
- Installed by `rtk init --agent antigravity` (workspace) or `rtk init -g --agent antigravity` (global)
- Also installs `../rtk-awareness-full.md` to `rules/antigravity-rtk-rules.md` as a second layer for sessions where the hook is disabled

## Customization roots

Antigravity discovers customizations from a *customization root*, and both roots hold the same layout (`hooks.json`, `rules/`, `skills/`):

| Scope | Root |
| :--- | :--- |
| Workspace | `<workspace>/.agents/` |
| Global | `~/.gemini/config/` |

## `hooks.json`

Each top-level key is a **hook name**; Antigravity merges the handlers of every named hook for a given event. RTK owns the `rtk` key and leaves all others untouched:

```json
{
  "rtk": {
    "PreToolUse": [
      {
        "matcher": "run_command",
        "hooks": [
          { "type": "command", "command": "rtk hook antigravity", "timeout": 10 }
        ]
      }
    ]
  }
}
```

## Wire contract

`rtk hook antigravity` reads the payload on stdin and answers on stdout. Keys are camelCase (protojson).

**stdin**

```json
{
  "toolCall": { "name": "run_command", "args": { "CommandLine": "git status" } },
  "stepIdx": 12,
  "conversationId": "…",
  "workspacePaths": ["/path/to/workspace"]
}
```

**stdout**

| case | response |
| :--- | :--- |
| rewrite | `{"decision":"allow","overwrite":{"CommandLine":"rtk git status"}}` |
| deny | `{"decision":"deny","reason":"Blocked by RTK permission rule"}` |
| defer to the host's approval flow | `{"decision":"ask"}` |

`overwrite` is a shallow, top-level merge into the tool call's arguments, and the merged call is what actually executes and gets recorded. `decision` is required on every response; the accepted values are `allow`, `deny`, `ask` and `force_ask`.

The contract ships inside the `agy` binary. To read it first-hand:

```bash
strings $(which agy) | sed -n '/Lifecycle Hooks/,/PostInvocation Contract/p'
```

## Gotcha: permissions are checked after the rewrite

Antigravity evaluates `permissions.allow` against the **modified** tool call, so an existing rule for a bare command stops matching once RTK prefixes it. `command(git status)` alone is no longer enough — it needs `command(rtk git status)` too. Interactively this surfaces as an unexpected approval prompt; in headless or CI runs the tool call is denied outright. `rtk init --agent antigravity` prints this warning after installing.
