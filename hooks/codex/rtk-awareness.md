# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk`.

Prefer RTK-native forms for search, file reads, git, tests, and builds:

- `rtk grep`, `rtk read`, `rtk ls`, `rtk wc`
- `rtk git`, `rtk cargo`, `rtk pnpm`, `rtk pytest`, `rtk mvn`

Use `rtk proxy <cmd>` only when RTK has no equivalent. Even then, keep output bounded:

- `rtk proxy <cmd> >/tmp/cmd.log 2>&1; rtk read /tmp/cmd.log --tail-lines 40`
- `rtk proxy <cmd> >/tmp/cmd.log 2>&1; rtk grep -n "ERROR|FAIL" /tmp/cmd.log`
- `rtk proxy ./mvnw ... >/tmp/mvn.log 2>&1; rtk read /tmp/mvn.log --tail-lines 40`
- `rtk proxy poetry run api_test ... >/tmp/api_test.log 2>&1; rtk read /tmp/api_test.log --tail-lines 10`

Avoid bare `git` / `rg` / `sed` / `find` / `tail` commands and avoid inline Python when `rtk grep` or `rtk read` can do the job.

Preferred bounded patterns:

- Search with a result cap: `rtk grep --max 20 -n "needle" src`
- Read the first section of a file: `rtk read path/to/file --max-lines 120`
- Read the end of a log: `rtk read /tmp/build.log --tail-lines 40`
- Read a file with line numbers: `rtk read -n --level none path/to/file --max-lines 120`

Examples:

```bash
rtk git status
rtk cargo test
rtk npm run build
rtk pytest -q
rtk grep --max 20 -n "needle" src
rtk read path/to/file --max-lines 120
rtk proxy ./mvnw test >/tmp/mvn.log 2>&1; rtk read /tmp/mvn.log --tail-lines 40
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
