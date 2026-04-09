# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk`.

Prefer RTK-native forms for search, file reads, git, tests, and builds:

- `rtk rg`, `rtk sed`, `rtk head`, `rtk tail`, `rtk ls`, `rtk wc`
- `rtk git`, `rtk cargo`, `rtk pnpm`, `rtk pytest`, `rtk mvn`

Use `rtk proxy <cmd>` only when RTK has no equivalent. Even then, keep output bounded:

- `rtk proxy <cmd> >/tmp/cmd.log 2>&1; rtk tail -n 40 /tmp/cmd.log`
- `rtk proxy <cmd> | rtk tail -n 40`
- `rtk proxy ./mvnw ... >/tmp/mvn.log 2>&1; rtk tail -n 40 /tmp/mvn.log`
- `rtk proxy poetry run api_test ... | rtk tail -n 10`

Avoid bare `git` / `rg` / `sed` / `find` / `tail` commands and avoid inline Python when `rtk rg` or `rtk sed` can do the job.

Examples:

```bash
rtk git status
rtk cargo test
rtk npm run build
rtk pytest -q
rtk rg -n "needle" src
rtk proxy ./mvnw test >/tmp/mvn.log 2>&1; rtk tail -n 40 /tmp/mvn.log
```

## Meta Commands

```bash
rtk gain            # Token savings analytics
rtk gain --history  # Recent command savings history
rtk proxy <cmd>     # Run raw command without filtering
```

## Verification

```bash
rtk --version
rtk gain
which rtk
```
