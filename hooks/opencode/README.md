# OpenCode Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

OpenCode has two mutually incompatible plugin APIs. RTK ships one plugin
artifact per API and `rtk init -g --opencode` installs the matching one at
`~/.config/opencode/plugins/rtk.ts`:

| OpenCode | Plugin file | API | Event |
|----------|-------------|-----|-------|
| V1 (legacy) | [`rtk.ts`](rtk.ts) | named `Plugin` export returning a hook map | `tool.execute.before` |
| V2 | [`rtk-v2.ts`](rtk-v2.ts) | `export default Plugin.define({ id, setup })` | `ctx.tool.hook("execute.before", …)` |

Shared behavior:

- TypeScript plugins (V1 uses the Bun shell injected by OpenCode's plugin
  context, V2 uses `node:child_process`)
- Intercept shell tool calls, call `rtk rewrite` as a subprocess
- Read `rtk rewrite` stdout: exit code `0`/`3` carries a rewritten command,
  exit code `1` means "no rewrite"
- Silently ignore failures and pass the original command through
- Mutate the command in place if the rewrite differs from the original

## Version detection

`ensure_opencode_plugin_installed` picks the payload from the detected major
version, preferring `opencode --version` and falling back to the plugin package
OpenCode installs in its config directory (`@opencode/plugin` → V2,
`@opencode-ai/plugin` → V1). Undetected versions install the V1 file to
preserve prior behavior.
