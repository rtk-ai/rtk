# Oh My Pi Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- TypeScript extension module, not a shell hook or rules file
- Sets its OMP extension display label to `RTK`
- Installs to `./.omp/extensions/rtk.ts` with `rtk init --omp`, or to `~/.omp/agent/extensions/rtk.ts` with `rtk init -g --omp`
- Intercepts OMP `tool_call` events for the `bash` tool and delegates rewrite decisions to `rtk rewrite`
- Requires Bun runtime (uses `Bun.which` and `Bun.spawn`); OMP currently ships with Bun
- Multi-extension chaining: OMP dispatches `tool_call` handlers sequentially. Downstream handlers observe the RTK-rewritten `event.input.command` only when RTK actually rewrites it
