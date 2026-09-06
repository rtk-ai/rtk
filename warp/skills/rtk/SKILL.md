---
name: rtk
description: >-
  Use RTK (Rust Token Killer) to reduce LLM token consumption by 60-90% on
  shell commands. Invoke this skill when running any shell command that produces
  verbose output — git, cargo, go, pnpm, docker, kubectl, grep, ls, curl, etc.
  RTK filters, groups, and deduplicates output so the context stays small.
  Always prefix supported commands with `rtk` (e.g. `rtk git status`).
---

# RTK — Rust Token Killer

RTK is a CLI proxy that filters command output before it reaches the LLM context, saving 60-90% of tokens.

## Golden Rule

**Always prefix commands with `rtk`.** If RTK has a dedicated filter, it uses it. If not, it passes through unchanged. This means RTK is always safe to use.

In command chains with `&&`, prefix each command:

```bash
# Correct
rtk git add . && rtk git commit -m "msg" && rtk git push
```

## Commands

### Git (59-80% savings)
```
rtk git status          rtk git log
rtk git diff            rtk git show
rtk git add             rtk git commit -m "msg"
rtk git push            rtk git pull
rtk git branch          rtk git fetch
rtk git stash           rtk git worktree
```

### GitHub CLI (26-87% savings)
```
rtk gh pr view <num>    rtk gh pr checks
rtk gh run list         rtk gh issue list
```

### Build & Compile (70-90% savings)
```
rtk cargo build         rtk cargo check
rtk cargo clippy        rtk tsc
rtk lint                rtk prettier --check
rtk next build          rtk ruff check
```

### Test (90-99% savings)
```
rtk cargo test          rtk vitest run
rtk playwright test     rtk pytest
rtk go test             rtk test <cmd>
```

### Files & Search (60-75% savings)
```
rtk ls <path>           rtk read <file>
rtk grep <pattern>      rtk find <pattern>
```

### Infrastructure (65-85% savings)
```
rtk docker ps           rtk docker images
rtk docker logs <c>     rtk kubectl get
rtk kubectl logs        rtk curl <url>
```

### Meta
```
rtk gain                # Token savings stats
rtk gain --history      # Command history with savings
rtk discover            # Find missed RTK usage
rtk proxy <cmd>         # Run without filtering (debug)
```

## Installation Verification

```bash
rtk --version   # Should show rtk X.Y.Z
rtk gain        # Must show token stats (not "command not found")
```

If `rtk gain` fails, the wrong package (Rust Type Kit) may be installed. Fix:
```bash
cargo uninstall rtk
cargo install --git https://github.com/rtk-ai/rtk
```
