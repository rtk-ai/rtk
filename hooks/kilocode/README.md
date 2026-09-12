# Kilo Code Plugin

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- TypeScript plugin that intercepts `tool.execute.before` and mutates eligible Bash or shell commands in place.
- Delegates rewrite decisions to `rtk rewrite`; machine-readable output flags stay untouched.
- Locates RTK on Windows, Linux, and WSL, with `RTK_KILO_BIN` as an explicit override.
- Installed to `~/.config/kilo/plugin/rtk.ts` by `rtk init -g --agent kilocode`.
- Honors Kilo's `XDG_CONFIG_HOME` and `KILO_CONFIG_DIR` configuration-directory overrides.
- Set `RTK_KILO_BASH_PLUGIN=0` to disable rewrites for a session.
