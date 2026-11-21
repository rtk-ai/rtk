# Kiro IDE / CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Uses the `rtk hook kiro` Rust binary (not a shell script) — no `jq` dependency
- Dual mechanism: **steering file** (`.kiro/steering/rtk.md`, prompt-level guidance) as primary integration, plus an optional **PreToolUse hook** (`.kiro/hooks/rtk-rewrite.json`) for deny-with-suggestion reinforcement
- Hook uses **deny-with-suggestion**: exit code `2` plus the suggested `rtk` command on stderr, which Kiro forwards to the agent so it re-issues the command (Kiro's PreToolUse API does not support transparent command rewrite / `updatedInput`)
- The retry is idempotent: an already-`rtk` command never rewrites, so the second attempt passes through and the hook cannot loop
- Exits silently (exit 0, no output) on any failure: invalid JSON, missing command, no rewrite match, stdin > 1 MiB

## Scopes

Both artifacts are written under the same root:

| Artifact | `rtk init --agent kiro` | `rtk init --agent kiro --global` |
|----------|-------------------------|----------------------------------|
| Steering `steering/rtk.md` | `<repo>/.kiro/steering/` | `~/.kiro/steering/` (all projects) |
| Hook `hooks/rtk-rewrite.json` | `<repo>/.kiro/hooks/` | `~/.kiro/hooks/` |

Kiro CLI reads hooks from both `~/.kiro/hooks/` and the workspace. Kiro IDE reads hooks only from `.kiro/hooks/` in the open workspace, so with a global install the IDE gets the steering but not the hook; run `rtk init --agent kiro` in a repository to add the hook there.

## Mechanism

The Kiro PreToolUse hook is registered via a JSON config file at `hooks/rtk-rewrite.json` under the install root. When Kiro's shell execution tool is invoked, the hook triggers `rtk hook kiro`, which:

1. Reads the JSON payload from stdin (capped at 1 MiB)
2. Extracts the shell command from `tool_input.command`
3. Delegates to the shared rewrite decision flow (`decision::decide_for_agent`)
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
      "description": "Suggests rtk equivalents for shell commands.",
      "matcher": "execute_bash",
      "action": {
        "type": "command",
        "command": "rtk hook kiro",
        "timeout": 5
      }
    },
    {
      "name": "RTK Rewrite (shell)",
      "trigger": "PreToolUse",
      "description": "Suggests rtk equivalents for shell commands.",
      "matcher": "shell",
      "action": {
        "type": "command",
        "command": "rtk hook kiro",
        "timeout": 5
      }
    }
  ]
}
```

Kiro reports the shell tool as `execute_bash` or `shell` depending on the configuration, so the template matches both.

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | Hook schema version — `"v1"` |
| `hooks` | array | One entry per hook definition |
| `hooks[].name` | string | Display name shown by Kiro |
| `hooks[].trigger` | string | Hook event — `"PreToolUse"` |
| `hooks[].description` | string | Human-readable purpose |
| `hooks[].matcher` | string | Tool to match — `"execute_bash"` or `"shell"` |
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
| `tool_name` | string | Name of the tool being invoked (`executeBash`, `execute_bash`, `runCommand` or `shell`) |
| `tool_input` | object | Tool arguments; `command` is the shell command string |

### Output (stderr — hook → Kiro) — deny-with-suggestion

The hook writes nothing to stdout. When the command has an RTK equivalent, it exits `2` and emits a single stderr line, which Kiro forwards to the agent:

```
RTK: use `rtk git status` instead. Re-run the command with the `rtk` prefix.
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
