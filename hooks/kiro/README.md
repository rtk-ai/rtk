# Kiro IDE / CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Uses the `rtk hook kiro` Rust binary (not a shell script) — no `jq` dependency
- Dual mechanism: **steering file** (`.kiro/steering/rtk.md`, prompt-level guidance) as primary integration, plus an optional **PreToolUse hook** (`.kiro/hooks/rtk-rewrite.json`) for deny-with-suggestion reinforcement
- Hook uses **deny-with-suggestion**: exit code `2` plus the suggested `rtk` command on stderr, which Kiro forwards to the agent so it re-issues the command (Kiro's PreToolUse API does not support transparent command rewrite / `updatedInput`)
- The retry is idempotent: an already-`rtk` command never rewrites, so the second attempt passes through and the hook cannot loop
- Exits silently (exit 0, no output) on any failure: invalid JSON, missing command, no rewrite match, stdin > 1 MiB
- Structured for future transparent rewrite if Kiro exposes an `updatedInput`-style field

## Scopes

The two artifacts do not share a scope:

| Artifact | `rtk init --agent kiro` | `rtk init --agent kiro --global` |
|----------|-------------------------|----------------------------------|
| Steering `steering/rtk.md` | `<repo>/.kiro/steering/` | `~/.kiro/steering/` (all projects) |
| Hook `hooks/rtk-rewrite.json` | `<repo>/.kiro/hooks/` | `<repo>/.kiro/hooks/` |

Kiro loads agent hooks **only** from `.kiro/hooks/` in the open workspace — a file placed in `~/.kiro/hooks/` is never read. So `--global` makes the steering apply everywhere, but the hook is always written into the repository the command ran from. Run `rtk init --agent kiro` in each repo where you want the hook reinforcement.

`rtk init --show` reports a leftover `~/.kiro/hooks/rtk-rewrite.json` as present but inert; it is safe to delete.

## Mechanism

The Kiro PreToolUse hook is registered via a JSON config file at `.kiro/hooks/rtk-rewrite.json`. When Kiro's shell execution tool is invoked, the hook triggers `rtk hook kiro`, which:

1. Reads the JSON payload from stdin (capped at 1 MiB)
2. Extracts the shell command from `tool_input.command`
3. Delegates to the shared rewrite decision flow (`decide_hook_action`)
4. If a rewrite exists: writes `RTK: use \`rtk <cmd>\` …` to stderr and exits `2`. Kiro blocks the raw command and feeds that text to the agent, which re-issues the `rtk` form
5. Otherwise: produces no output and exits `0`, so the original command runs unmodified

Why not `ask`? Kiro's `ask` decision runs the **original** command on approval — it costs a user confirmation and saves nothing, since there is no transparent-rewrite field. Deny-with-suggestion routes the correction to the agent instead of the user.

## Hook File Format

Kiro reads hook definitions as a `v1` document with a top-level `hooks` array. The template installed by `rtk init --agent kiro` is:

```json
{
  "version": "v1",
  "hooks": [
    {
      "name": "RTK Rewrite",
      "trigger": "PreToolUse",
      "description": "Sugere equivalentes rtk para comandos shell, economizando 60-90% de tokens.",
      "matcher": "execute_bash",
      "action": {
        "type": "command",
        "command": "rtk hook kiro",
        "timeout": 5
      }
    }
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | Hook schema version — `"v1"` |
| `hooks` | array | One entry per hook definition |
| `hooks[].name` | string | Display name shown by Kiro |
| `hooks[].trigger` | string | Hook event — `"PreToolUse"` |
| `hooks[].description` | string | Human-readable purpose |
| `hooks[].matcher` | string | Tool to match — `"execute_bash"` |
| `hooks[].action.type` | string | Action kind — `"command"` |
| `hooks[].action.command` | string | Command to run — `"rtk hook kiro"` |
| `hooks[].action.timeout` | number | Timeout in seconds |

## JSON Formats

### Input (stdin — Kiro → hook)

Kiro sends the session context, hook event, tool name, and tool input:

```json
{
  "session_id": "0f2c…",
  "hook_event_name": "PreToolUse",
  "tool_name": "executeBash",
  "tool_input": { "command": "git status" }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | string | Kiro session identifier (ignored by hook) |
| `hook_event_name` | string | Always `"PreToolUse"` for this hook |
| `tool_name` | string | Name of the tool being invoked (matched by `executeBash`) |
| `tool_input` | object | Tool arguments; `command` is the shell command string |

### Output (stderr — hook → Kiro) — deny-with-suggestion

The hook writes nothing to stdout. When the command has an RTK equivalent, it exits `2` and emits a single stderr line, which Kiro forwards to the agent:

```
RTK: use `rtk git status` (economiza 60-90% de tokens). Reemita o comando com o prefixo `rtk`.
```

| Channel | Value | Description |
|---------|-------|-------------|
| exit code | `2` | Kiro blocks the tool call and feeds stderr to the model |
| stderr | suggestion line | Names the `rtk` command and instructs the agent to re-issue it |
| stdout | *(empty)* | The hook never writes structured JSON |

The agent then re-issues `rtk git status`. That payload produces no rewrite (already `rtk`-prefixed), so the hook exits `0` and the command runs — the loop terminates after one round trip.

### Output — no rewrite

When no rewrite applies (command has no RTK equivalent, is already prefixed with `rtk`, contains unattestable constructs, heredoc, or on any error): **no output** and exit code 0. The original command executes unmodified.

## Exit Code Contract

`rtk hook kiro` exits `2` only when a rewrite exists. Every other path — including all errors, invalid input, and no-match cases — exits `0`, so a broken hook never blocks the user.

| Condition | Behavior |
|-----------|----------|
| Valid command with RTK equivalent | stderr: suggestion, exit 2 |
| No RTK equivalent | no output, exit 0 |
| Command already prefixed with `rtk` | no output, exit 0 |
| Unattestable construct / heredoc | no output, exit 0 |
| Invalid JSON input | parse note on stderr, exit 0 |
| Empty stdin | no output, exit 0 |
| Stdin exceeds 1 MiB | no output, exit 0 |
| `tool_name` is not a shell tool | no output, exit 0 |
