# Oh My Pi Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- TypeScript hook module (HookAPI), not a shell hook or rules file
- Installs to `./.omp/hooks/pre/rtk.ts` with `rtk init --agent omp`, or to `~/.omp/agent/hooks/pre/rtk.ts` with `rtk init -g --agent omp`
- Intercepts OMP `tool_call` events for the `bash` tool and delegates rewrite decisions to `rtk rewrite`
- Uses `ctx.ui.confirm()` to prompt the user when `rtk rewrite` returns an "ask" permission verdict (exit code 3)
- Deny verdicts (exit code 2) pass through unchanged (host tool handles denial)
- Fail-open: if `rtk` is unavailable or `rtk rewrite` fails, commands run raw unchanged
- Multi-hook chaining: OMP dispatches `tool_call` handlers sequentially. Downstream handlers observe the RTK-rewritten `event.input.command` when RTK rewrites it
