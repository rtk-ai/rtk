---
name: rtk
description: >
  RTK (Rust Token Killer) token-optimized CLI proxy to reduce token usage during development. RTK removes noise from CLI command output.
  Optimize commands: ls, cat, grep, git, ruff check, pytest, ESLint, tsc, prettier, pip, docker, kubectl, curl, wget, json, deps, env, log, test, lint, build, run, proxy.
license: MIT
metadata:
  author: Damien Berezenko
---

# RTK - Rust Token Killer

RTK is a CLI proxy that reduces LLM token usage by filtering, grouping, truncating, and deduplicating noisy command output before it reaches the model. It is most useful on commands that produce lots of repetitive structure—diffs, test failures, build output, logs, and large file reads—so you can keep a tight development loop without losing the signal.

If the Claude Code hook is already enabled, common shell commands are transparently rewritten to their RTK equivalents. The commands below are the explicit forms worth knowing when you want predictable, high-signal output.

## Supported Commands

- **Files and code reading:** `rtk ls`, `rtk read`, `rtk smart`, `rtk find`, `rtk grep`, `rtk diff`
- **Git and GitHub:** `rtk git ...`, `rtk gh ...`
- **Tests and failures:** `rtk test <command>`, `rtk err <command>`, `rtk cargo test`, `rtk pytest`, `rtk go test`, `rtk vitest run`, `rtk playwright test`
- **Build and lint:** `rtk lint`, `rtk tsc`, `rtk next build`, `rtk prettier --check`, `rtk cargo build`, `rtk cargo clippy`, `rtk ruff check`, `rtk golangci-lint run`
- **Runtime, data, and logs:** `rtk log`, `rtk docker ...`, `rtk kubectl ...`, `rtk json`, `rtk curl`, `rtk env`, `rtk deps`, `rtk summary`
- **Analytics and debugging:** `rtk gain`, `rtk discover`, `rtk proxy`, `rtk init --show`

For commands like `rtk git ...` and `rtk ls ...`, RTK keeps the native command model and mainly changes how output is presented. That makes it easy to drop into existing workflows without learning a separate query language.

## Command Selection Guide

- Use `rtk read -l aggressive` when you need the file structure before implementation details.
- Use `rtk smart` when you want a very fast summary to decide whether a file is worth opening.
- Use `rtk grep` and `rtk find` when broad searches are returning too much duplicated context.
- Use `rtk git diff` and `rtk git log` when the raw Git output is longer than the decision you need to make.
- Use `rtk test <command>` for noisy test runners and `rtk err <command>` for noisy build tools.
- Use `rtk log`, `rtk docker logs`, or `rtk kubectl logs` when repeated lines dominate the output.
- Use `rtk proxy <command>` when exact raw output matters more than compression.
- Use `rtk gain` and `rtk discover` to verify RTK is actually saving tokens in practice.

## Practical Workflows

### 1) Read the structure first, then search precisely

Use `rtk read -l aggressive` when a file is too large or noisy to send raw. In aggressive mode, RTK keeps signatures and high-value structure, which is usually enough to decide whether a file matters before doing a full read.

```bash
rtk read src/server/router.rs -l aggressive
rtk grep "auth|middleware|rate_limit" src
rtk smart src/server/router.rs
```

This works best when you are mapping an unfamiliar module, tracing entry points, or narrowing a search before opening full files.

### 2) Collapse failing output down to actionable lines

Prefer `rtk test <command>` or `rtk err <command>` when the underlying tool is verbose, but you only need failures, errors, and warnings. If RTK compresses too aggressively for a specific debugging session, fall back to `rtk proxy` for raw passthrough while still keeping usage tracking.

```bash
rtk err npm run build
rtk test cargo test
rtk proxy cargo test -- --nocapture
```

Use this pattern for compiler noise, flaky test suites, CI reproduction, and long logs where the failure is small relative to the output.

## Most Useful CLI Flags and Options

### Output shaping

```bash
rtk read file.rs -l aggressive   # structure/signatures only
rtk read file.rs -l minimal      # lighter filtering
rtk read file.rs -l none         # essentially raw read
rtk -u git diff                  # ultra-compact output
rtk -v cargo test                # show more filtering detail
rtk --skip-env next build        # skip env validation for wrapped child process
```

- `-l, --level` is the most important option for code reading. `aggressive` is ideal for first-pass exploration; `minimal` is better when you still need local implementation detail.
- `-u, --ultra-compact` is useful when the command is structurally repetitive, and you want maximum compression.
- `-v, --verbose` helps when you need to understand what RTK is filtering.
- `--skip-env` is useful with tools like `next build`, `tsc`, `lint`, and `prisma` when env validation gets in the way of the command you actually want to inspect.

### High-value commands for complicated workflows

```bash
rtk test <command>               # generic test-output filter
rtk err <command>                # generic error/warning filter
rtk proxy <command>              # raw passthrough + tracking
rtk gain --history               # recent command history with savings
rtk gain --graph                 # 30-day savings graph
rtk gain --daily                 # daily breakdown
rtk gain --all --format json     # machine-readable export
rtk discover --all --since 7     # missed opportunities across projects
rtk init --show                  # verify hook/config status
```

- `rtk test <command>` and `rtk err <command>` are the most reusable wrappers when there is no dedicated RTK subcommand or you are working with a custom script.
- `rtk proxy <command>` is the escape hatch for debugging; use it when you need exact raw output without turning RTK off entirely.
- `rtk gain` and `rtk discover` matter once RTK is in regular use: `gain` proves whether it is saving context, and `discover` shows where raw commands are still leaking tokens.
- `rtk init --show` is the quickest way to check whether RTK is actually active in the environment instead of assuming the hook is working.

## Representative Before/After Examples

**Directory listing:**
```
# ls -la (45 lines, ~800 tokens)        # rtk ls (12 lines, ~150 tokens)
drwxr-xr-x  15 user staff 480 ...       my-project/
-rw-r--r--   1 user staff 1234 ...       +-- src/ (8 files)
...                                      |   +-- main.rs
                                         +-- Cargo.toml
```

**Git operations:**
```
# git push (15 lines, ~200 tokens)       # rtk git push (1 line, ~10 tokens)
Enumerating objects: 5, done.             ok main
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...
```

**Test output:**
```
# cargo test (200+ lines on failure)     # rtk test cargo test (~20 lines)
running 15 tests                          FAILED: 2/15 tests
test utils::test_parse ... ok               test_edge_case: assertion failed
test utils::test_format ... ok              test_overflow: panic at utils.rs:18
...
```

## When RTK Is Worth Reaching For

Use RTK when the raw command is high-volume, repetitive, or mostly boilerplate:

- large diffs and `git status` / `git log`
- test runners and compiler output
- codebase scans across many files
- infrastructure logs (`docker`, `kubectl`)
- machine-readable output that RTK can summarize cleanly (`json`, `curl`, dependency and env listings)

Skip it only when exact byte-for-byte output matters more than compression; in those cases, use `rtk proxy <command>`.
