# RTK Plugin for Hermes

Rewrites Hermes `terminal` tool commands to RTK equivalents before execution, so Hermes receives compact command output without changing your workflow.

## Installation

```bash
rtk init --agent hermes
```

The installer writes the plugin to `~/.hermes/plugins/rtk-rewrite/` and enables it through `plugins.enabled` in the Hermes config. The repository copy lives in `hooks/hermes/`; don't use that repo path as the runtime install path.

## Development

Run the Hermes plugin tests from the repository root:

```bash
python3 -m unittest discover -s hooks/hermes
```

## How it works

Hermes loads plugins from Python, so the plugin entrypoint is Python. The Python code is only a thin Hermes adapter. It reads the Hermes terminal tool payload, calls `rtk rewrite` for the actual command decision, then mutates the terminal tool `command` before Hermes executes it.

All rewrite rules stay in Rust inside `rtk rewrite`. When RTK adds or changes command rewrite behavior, the Hermes plugin picks up that behavior by delegating to the RTK binary.

## Fail-open behavior

The plugin does not block command execution. If anything goes wrong, Hermes runs the original command unchanged.

If rtk is not available in PATH when Hermes loads the plugin, the plugin prints a warning and skips hook registration.

- `rtk` is missing from `PATH`
- `rtk rewrite` exits with an error
- Hermes sends a non-terminal tool call
- The tool payload has no string `command`
- The plugin raises an unexpected exception

## Limitations

- Only Hermes `terminal` tool calls are rewritten.
- Commands skipped by `rtk rewrite` stay unchanged, including commands already prefixed with `rtk`, compound shell commands, heredocs, and commands without an RTK filter.
- Shell hooks are not used for Hermes command rewriting. The integration depends on Hermes loading Python plugins and passing a mutable terminal tool payload.
