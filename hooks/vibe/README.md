# RTK Hook for Mistral Vibe

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Design Intent

RTK's Mistral Vibe hook is a **rewrite-only token optimizer**. It intercepts Vibe tool calls (specifically `bash`/`run_shell_command`) and rewrites them to use RTK for 60-90% token savings.

**Permission gating is intentionally out of scope.** RTK does not block, confirm, or audit commands — that concern belongs to dedicated permission hooks. This separation keeps RTK's hook fast, predictable, and composable with other Vibe hooks.

## Specifics

- Shell script hook using Mistral Vibe's `pre_tool` hook system
- Intercepts `bash` and `run_shell_command` tool calls
- Calls `rtk rewrite` as a subprocess; returns rewritten command via JSON stdout
- All rewrite logic lives in RTK's Rust `rtk rewrite` command (single source of truth)
- Installed via `rtk init --agent vibe` which creates `.vibe/hooks.toml`

## Installation

```bash
rtk init --agent vibe
```

This creates or updates `~/.vibe/hooks.toml` with the RTK hook configuration. The hook script itself lives at `~/.vibe/hooks/rtk-hook-vibe.sh`.

## Uninstall

```bash
rtk init --uninstall --agent vibe
```

This removes the RTK hook entry from `~/.vibe/hooks.toml` and the hook script.

## How it works

Vibe's `pre_tool` hook receives JSON on stdin describing the tool call, including `tool_name` and `tool_input`. The RTK hook:

1. Checks if the tool is `bash` or `run_shell_command`
2. Extracts the command from `tool_input.command`
3. Calls `rtk rewrite <command>` to check for RTK equivalents
4. Returns JSON with either:
   - No output (exit 0) — pass through unchanged
   - JSON with `hook_specific_output.tool_input` — rewritten command

All error paths exit 0 with no output (fail-open), ensuring commands always execute.

## Fail-open behavior

The hook does not block command execution. If anything goes wrong, Vibe runs the original command unchanged:

- `jq` not installed: warning to stderr, exit 0
- `rtk` not available in PATH: warning to stderr, exit 0
- `rtk` version too old (< 0.23.0): warning to stderr, exit 0
- Invalid JSON input: pass through unchanged
- `rtk rewrite` crashes: hook exits 0 (subprocess error ignored)

## Limitations

- Only `bash` and `run_shell_command` tool calls are rewritten
- Commands skipped by `rtk rewrite` stay unchanged (already prefixed with `rtk`, compound shell commands, heredocs, etc.)
- Requires Vibe 2.21.0+ (when `pre_tool` hooks were introduced)

## JSON Format

### Input (stdin)

```json
{
  "session_id": "abc123",
  "parent_session_id": null,
  "transcript_path": "/path/to/transcript.jsonl",
  "cwd": "/path/to/project",
  "hook_event_name": "pre_tool",
  "tool_name": "bash",
  "tool_call_id": "def456",
  "tool_input": { "command": "git status" }
}
```

### Output (stdout, when rewritten)

```json
{
  "decision": "allow",
  "hook_specific_output": {
    "tool_input": { "command": "rtk git status" }
  }
}
```

### Output (no rewrite)

Empty stdout, exit code 0.
