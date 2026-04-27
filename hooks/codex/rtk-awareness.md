# RTK - Rust Token Killer (Codex CLI)

RTK is a token-optimized CLI proxy for shell commands. Use it deliberately:
save context on noisy output without hiding exact details needed for the task.

## Decision Policy

Use RTK when the shell command is likely to emit noisy or large output and the
filtered output still preserves the signal needed for implementation,
debugging, or verification.

Do not blindly prefix every command with `rtk`. In particular, do not rewrite
shell builtins such as `test -d`, `[`/`]`, `printf`, `pwd`, or tiny probes.

## Prefer RTK For

```bash
rtk git status
rtk git diff
rtk git show
rtk gh pr view <num>
rtk gh run view <id>
rtk grep <pattern> <path>
rtk find <path> -name <pattern>
rtk read <file>
rtk cargo test
rtk pytest
rtk vitest run
rtk playwright test
rtk tsc
rtk lint
rtk docker logs <container>
rtk kubectl logs <pod>
rtk curl <url>
```

For noisy repo scripts that RTK cannot rewrite directly, use generic wrappers:

```bash
rtk test bun run test
rtk err bun run typecheck
rtk err bun run lint
rtk err bun run build
rtk err bun run validate:local:agent
```

## Use Raw Commands For

- Exact file snippets or exact formatting where every line/character matters.
- Machine-readable JSON that will be piped into `jq`, saved, or consumed later.
- Small probes such as `pwd`, `command -v`, `printf`, `date`, or `test -d`.
- Interactive or long-running commands such as dev servers and watchers.
- Secret/env inspection, binary/media output, or user-requested raw/full output.

## Hook Behavior

Codex hooks cannot rewrite Bash input in place yet. If a raw command is blocked
with `Rerun that as: ...`, rerun the exact suggested `rtk ...` command unless it
conflicts with explicit user instructions or a safety rule.

## Escape Hatches

```bash
rtk proxy <cmd>       # track invocation but bypass filtering
RTK_DISABLED=1 <cmd>  # skip hook rewrite for one command
rtk gain              # token savings analytics
rtk gain --history    # recent command savings history
rtk hook-audit        # hook rewrite metrics when RTK_HOOK_AUDIT=1 is enabled
rtk --version
which rtk
```
