# RTK Plugin for Hermes

Rewrites Hermes `terminal` tool commands to RTK equivalents before execution, so Hermes receives compact command output without changing your workflow.

## Installation

```bash
rtk init --agent hermes
```

The installer writes the plugin to `~/.hermes/plugins/rtk-rewrite/` and enables it through `plugins.enabled` in the Hermes config.

## How it works

Hermes loads plugins from Python, so the plugin entrypoint is Python. The Python code is only a thin Hermes adapter. It reads the Hermes terminal tool payload, calls `rtk rewrite` for the actual command decision, then updates `command` in the original payload and returns a `modify` directive containing only the rewritten `command`. Hermes releases before v2026.8.16 execute the updated payload without reading `modify`; newer releases merge the returned directive before execution. Keeping the directive limited to `command` preserves other plugins' changes, such as `workdir`.

All rewrite rules stay in Rust inside `rtk rewrite`. When RTK adds or changes command rewrite behavior, the Hermes plugin picks up that behavior by delegating to the RTK binary.

The rewrite subprocess has no standard input and defaults to a 5-second timeout. Set `RTK_HERMES_TIMEOUT` to a positive, finite number of seconds (for example, `RTK_HERMES_TIMEOUT=10`) to override it. Missing, malformed, non-positive, or non-finite values fall back to 5 seconds. This controls the rewrite step only, not the terminal command's own timeout. Keep it below Hermes' `plugins.hook_callback_timeout` (30 seconds by default): the host may block the tool call if its callback limit is reached. Extremely large values may exceed the platform subprocess timeout limit and cause a warning with no rewrite.

## Fail-open behavior

The plugin itself does not request blocking. On rewrite failures it leaves the original command unchanged and returns no directive. Hermes can still block execution if its own callback timeout is reached.

If rtk is not available in PATH when Hermes loads the plugin, the plugin prints a warning and skips hook registration.

- `rtk` is missing from `PATH`
- `rtk rewrite` exits with an error
- Hermes sends a non-terminal tool call
- The tool payload has no string `command`
- The plugin raises an unexpected exception

## Limitations

- Only Hermes `terminal` tool calls are rewritten.
- Commands skipped by `rtk rewrite` stay unchanged, including commands already prefixed with `rtk`, compound shell commands, heredocs, and commands without an RTK filter.
- Shell hooks are not used for Hermes command rewriting. The integration depends on Hermes loading Python plugins and invoking `pre_tool_call`; both legacy payload mutation and newer modify directives are supported.
