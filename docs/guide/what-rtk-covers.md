---
title: What RTK Optimizes
description: Commands and ecosystems automatically optimized by RTK with typical token savings
sidebar:
  order: 2
---

# What RTK Optimizes

Once RTK is installed with a hook, these commands are automatically intercepted and filtered. You run them normally — the hook rewrites them transparently before execution.

60+ commands across 9 ecosystems. Typical savings: 60-99%.

## Git

| Command | Savings | What changes |
|---------|---------|--------------|
| `git status` | 75-93% | Compact stat format, grouped by state |
| `git log` | 80-92% | Hash + author + subject only |
| `git diff` | 70% | Context reduced, headers stripped |
| `git show` | 70% | Same as diff |
| `git stash list` | 75% | Compact one-line per entry |

## GitHub CLI

| Command | Savings | What changes |
|---------|---------|--------------|
| `gh pr view` | 87% | Removes ASCII art and verbose metadata |
| `gh pr checks` | 79% | Status + name only, failures highlighted |
| `gh run list` | 82% | Compact workflow run summary |
| `gh issue view` | 80% | Body only, no decoration |

## Graphite (Stacked PRs)

| Command | Savings | What changes |
|---------|---------|--------------|
| `gt log` | 75% | Stack summary only |
| `gt status` | 70% | Current branch context |

## Cargo / Rust

| Command | Savings | What changes |
|---------|---------|--------------|
| `cargo test` | 90% | Failures only, passed tests suppressed |
| `cargo nextest` | 90% | Same as test |
| `cargo build` | 80% | Errors and warnings only |
| `cargo check` | 80% | Errors and warnings only |
| `cargo clippy` | 80% | Lint warnings grouped by file |

## JavaScript / TypeScript

| Command | Savings | What changes |
|---------|---------|--------------|
| `vitest run` | 94-99% | Failures only |
| `tsc` | 75% | Type errors grouped by file |
| `eslint` | 84% | Violations grouped by rule |
| `pnpm list` | 70-90% | Compact dependency tree |
| `pnpm outdated` | 70% | Package + current + latest only |
| `next build` | 80% | Route summary + errors only |
| `prisma migrate` | 75% | Migration status only |
| `playwright test` | 90% | Failures + trace links only |

## Python

| Command | Savings | What changes |
|---------|---------|--------------|
| `pytest` | 80-90% | Failures only |
| `ruff check` | 75% | Violations grouped by file |
| `mypy` | 75% | Type errors grouped by file |
| `pip install` | 70% | Installed packages only, progress stripped |

## Go

| Command | Savings | What changes |
|---------|---------|--------------|
| `go test` | 80-90% | Failures only |
| `golangci-lint run` | 75% | Violations grouped by file |
| `go build` | 75% | Errors only |

## Ruby

| Command | Savings | What changes |
|---------|---------|--------------|
| `rspec` | 80-90% | Failures only |
| `rubocop` | 75% | Offenses grouped by file |
| `rake` | 70% | Task output, build errors highlighted |

## .NET

| Command | Savings | What changes |
|---------|---------|--------------|
| `dotnet build` | 80% | Errors and warnings only |
| `dotnet test` | 85-90% | Failures only |
| `dotnet format` | 75% | Changed files only |

## Docker / Kubernetes

| Command | Savings | What changes |
|---------|---------|--------------|
| `docker ps` | 65% | Essential columns (name, image, status, port) |
| `docker images` | 60% | Name + tag + size only |
| `docker logs` | 70% | Deduplicated, last N lines |
| `docker compose up` | 75% | Service status, errors highlighted |
| `kubectl get pods` | 65% | Name + status + restarts only |
| `kubectl logs` | 70% | Deduplicated entries |

## Files and Search

| Command | Savings | What changes |
|---------|---------|--------------|
| `ls` | 80% | Tree format with file counts |
| `find` | 75% | Tree format |
| `grep` | 70% | Truncated lines, grouped by file |
| `diff` | 65% | Context reduced |
| `wc` | 60% | Compact counts |
| `cat` / `head` / `tail <file>` | 60-80% | Smart file reading via `rtk read` |
| `rtk smart <file>` | 85% | 2-line heuristic code summary (signatures only) |

## Cloud and Data

| Command | Savings | What changes |
|---------|---------|--------------|
| `aws` | 70% | JSON condensed, relevant fields only |
| `psql` | 65% | Query results without decoration |
| `curl` | 60% | Response body only, headers stripped |

## Global flags

These flags apply to all RTK commands and can push savings even higher:

| Flag | Description |
|------|-------------|
| `-u` / `--ultra-compact` | ASCII icons, inline format — extra token reduction on top of normal filtering |
| `-v` / `--verbose` | Show filtering details on stderr (`-v`, `-vv`, `-vvv` for increasing detail) |

```bash
# Ultra-compact: even smaller output
git log          # → already filtered by RTK
git log -u       # → ultra-compact variant (if using rtk directly)

# Debug: see what RTK is doing
RTK_DISABLED=0 git status -vvv
```

## Commands that are not rewritten

If a command isn't in the list above, RTK runs it through passthrough — the output reaches the LLM unchanged. You can explicitly track unsupported commands:

```bash
rtk proxy make install    # runs make install, tracks usage, no filtering
```

To check which commands were missed opportunities: `rtk discover`.
