# RTK Plugin for Hermes

Integrates RTK into [Hermes Agent](https://github.com/NousResearch/hermes-agent) via two hooks:

1. **`pre_tool_call`** — rewrites bare terminal commands to their `rtk` equivalents before execution.
2. **`transform_tool_result`** — filters high-token tool outputs before they enter the conversation context.

## Installation

```bash
rtk init --agent hermes
```

The installer writes the plugin to `~/.hermes/plugins/rtk-rewrite/` and enables it through `plugins.enabled` in the Hermes config. The repository copy lives in `hooks/hermes/`; don't use that repo path as the runtime install path.

## Development

Run the plugin tests from the repository root:

```bash
python3 -m pytest hooks/hermes/tests/ -v
```

---

## Hook 1 — `pre_tool_call`: command rewriting

When a Hermes worker calls the `terminal` tool with a bare command like `find`, `grep`, `cat`, or `ls`, this hook rewrites it to its `rtk` equivalent before execution — identical to the Claude Code `PreToolUse` hook, but applied inside Hermes so all backends (local, Docker, SSH, Modal) benefit without requiring the model to prefix commands manually.

### How it works

The plugin reads the Hermes `terminal` tool payload, calls `rtk rewrite <command>`, and mutates the `command` field if RTK provides a rewrite. All rewrite rules stay in Rust inside `rtk rewrite` — when RTK adds or changes rewrite behavior, the Hermes plugin picks it up automatically.

### Examples

| Worker command | Executed as |
|---|---|
| `find /home -name '*.py'` | `rtk find /home -name '*.py'` |
| `grep -rn foo /src` | `rtk grep -rn foo /src` |
| `cat large_file.yaml` | `rtk read large_file.yaml` |
| `ls -la /some/dir` | `rtk ls -la /some/dir` |
| `cd /foo && find . -name '*.cs'` | `cd /foo && rtk find . -name '*.cs'` |
| `python3 script.py` | `python3 script.py` *(unchanged)* |
| `rtk find ...` | `rtk find ...` *(no double-wrapping)* |

---

## Hook 2 — `transform_tool_result`: output filtering

Filters the output of high-token tools before it enters the conversation context. Each filter is independent and fails open — if parsing fails, the original output is returned unchanged.

### Filters

| Tool(s) | What is filtered | Limit |
|---|---|---|
| `terminal` | JSON-unwrap output field, deduplicate lines, strip blanks | 100 lines |
| `browser_navigate` | Strip `Layout*` accessibility-tree noise, deduplicate, truncate snapshot | 120 lines |
| `read_file`, `execute_code` | Deduplicate lines, strip blank lines | 100 lines |
| `write_file`, `patch` | Replace full unified diff with a 1-line summary | — |
| `search_files` | Truncate to first 50 results | 50 results |
| `kanban_show`, `kanban_list` | Keep only the last 10 events | 10 events |

### `write_file` / `patch` diff summary

A 50-line unified diff like:

```diff
--- a/src/Battle/Pig3DAnimator.cs
+++ b/src/Battle/Pig3DAnimator.cs
@@ -42,7 +42,6 @@
 ...
```

Is replaced with:

```
[patch] src/Battle/Pig3DAnimator.cs (2 hunks, +5/-3 lines)
```

This is the main fix for context overflow on patch-heavy tasks where multiple diffs accumulate in the conversation.

### `browser_navigate` noise stripping

Unity accessibility snapshots and browser pages often contain dozens of `LayoutBlock`, `LayoutInline`, `LayoutTableRow`, etc. lines that carry no semantic content. These are stripped before the snapshot enters the model context, typically reducing browser tool output by ~21%.

---

## Fail-open behavior

The plugin never blocks command execution or tool result delivery. In all of the following cases, Hermes proceeds with the original value unchanged:

- `rtk` is missing from `PATH` (hook not registered; one warning logged)
- `rtk rewrite` exits with an unexpected error
- A tool payload has no string `command`
- JSON parsing of a tool result fails
- The plugin raises an unexpected exception

---

## Limitations

- Only the `terminal` tool call is rewritten by `pre_tool_call`. Other tools (file reads, browser, etc.) are not affected.
- Shell hooks are not used for command rewriting. The integration depends on Hermes loading Python plugins and passing a mutable terminal tool payload.
- `transform_tool_result` filters apply to the tool output string only. Structured metadata fields (e.g. `exit_code`) are preserved.
