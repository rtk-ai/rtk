# Amp Hooks

RTK's Amp integration is a native Amp plugin. It listens to Amp's `tool.call`
event, extracts shell commands with `shellCommandFromToolCall`, and asks
`rtk rewrite` for the replacement command. The plugin contains no command map;
the Rust rewrite registry remains the single source of truth.

## Installation

Install the plugin for the current project:

```sh
rtk init --agent amp
```

This writes `.amp/plugins/rtk/index.ts`. Install it for every Amp project on
the machine:

```sh
rtk init -g --agent amp
```

The global path is `$XDG_CONFIG_HOME/amp/plugins/rtk/index.ts`, or
`~/.config/amp/plugins/rtk/index.ts` when `XDG_CONFIG_HOME` is unset or empty
(`%USERPROFILE%\.config\amp\plugins\rtk\index.ts` on Windows). This is
Amp's machine-level system plugin directory. The command does not import or
push anything to an Amp personal or workspace hosted plugin repository.

RTK must be installed and available on the `PATH` of the environment where
Amp executes shell tools. Restart Amp after installation, or reload plugins
from Amp's command palette. `amp plugins list` shows the loaded plugin.

## Behavior

The plugin rewrites supported `shell_command` calls and the legacy `Bash` tool
just before Amp executes them. It preserves every other input field, including
the working directory and timeout. Missing RTK, unsupported commands, failed
or timed-out rewrites, empty or unchanged output, and any nonzero exit code
other than RTK's valid exit code 3 leave the original tool call unchanged.

Commands that already start with `rtk` are left alone. Set `RTK_DISABLED=1`
in the Amp executor environment to disable automatic rewriting. RTK's own
permission decision remains visible to Amp: exit code 3 carries a valid rewrite
and Amp applies its normal approval behavior.

The command is passed to `rtk rewrite` as one argument and is never evaluated
by the plugin. The original Amp tool executes the final command, so the plugin
does not retry or execute shell commands itself.

## Existing plugin files

`rtk init` is idempotent. The current stock plugin needs no approval. If
the target contains unrelated or modified plugin content, the default install
asks before replacing it; `--auto-patch` approves the replacement and
`--no-patch` leaves it unchanged and exits nonzero. An existing `index.js` also
requires approval before RTK adds `index.ts`, which Amp loads in preference to
`index.js`; uninstall leaves the JavaScript entry in place. A same-name
single-file plugin (`plugins/rtk.ts` or `plugins/rtk.js`) must be renamed manually
before installing the directory plugin, even with `--auto-patch`.

`--dry-run` reports the action without
creating directories or changing files. Uninstall removes only the current
stock RTK entry and leaves unrelated or modified content in place. Unreadable
or modified RTK entries cause a normal uninstall to exit nonzero; a dry-run
reports the refusal and succeeds.

```sh
rtk init --agent amp --show
rtk init --agent amp --uninstall
rtk init -g --agent amp --uninstall
```

The native API and plugin locations are described in Amp's
[plugin documentation](https://ampcode.com/docs/customize/plugins) and
[API reference](https://ampcode.com/docs/plugin-api).

## Tests

Run the Amp plugin tests with Bun:

```sh
bun test hooks/amp/rtk/index.test.ts
cargo test --test amp_init_test
```

They cover the registered hook, both valid rewrite exit codes, input
preservation, working-directory forwarding, malformed and unsupported calls,
missing or failed RTK, explicit disablement, shell syntax passed as data, and
timeouts. When RTK is available they also exercise the real rewrite command
and a temporary Git repository.
