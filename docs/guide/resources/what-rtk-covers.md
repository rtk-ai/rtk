---
title: What RTK Optimizes
description: Commands and ecosystems automatically optimized by RTK
sidebar:
  order: 1
---

# What RTK Optimizes

Once RTK is installed with a hook, these commands are automatically intercepted and filtered. You run them normally — the hook rewrites them transparently before execution.

:::note
RTK reduces **bash output bytes** — the only thing it controls. Those bytes are one contributor to input tokens, and input tokens are only part of a bill that also counts output tokens. See [How RTK Savings Work](./savings-explained.md).
:::

## Git

| Command | What changes |
|---------|--------------|
| `git status` | Compact stat format, grouped by state |
| `git log` | Hash + author + subject only |
| `git diff` | Context reduced, headers stripped |
| `git show` | Same as diff |
| `git stash list` | Compact one-line per entry |

## GitHub CLI

| Command | What changes |
|---------|--------------|
| `gh pr view` | Removes ASCII art and verbose metadata |
| `gh pr checks` | Status + name only, failures highlighted |
| `gh run list` | Compact workflow run summary |
| `gh issue view` | Body only, no decoration |

## Graphite (Stacked PRs)

| Command | What changes |
|---------|--------------|
| `gt log` | Stack summary only |
| `gt status` | Current branch context |

## Cargo / Rust

| Command | What changes |
|---------|--------------|
| `cargo test` | Failures only, passed tests suppressed |
| `cargo nextest` | Same as test |
| `cargo build` | Errors and warnings only |
| `cargo check` | Errors and warnings only |
| `cargo clippy` | Lint warnings grouped by file |

## JavaScript / TypeScript

| Command | What changes |
|---------|--------------|
| `jest` | Failures only |
| `vitest` | Failures only |
| `tsc` | Type errors grouped by file |
| `eslint` | Violations grouped by rule |
| `pnpm list` | Compact dependency tree |
| `pnpm outdated` | Package + current + latest only |
| `next build` | Route summary + errors only |
| `prisma migrate` | Migration status only |
| `playwright test` | Failures + trace links only |

## Python

| Command | What changes |
|---------|--------------|
| `pytest` | Failures only |
| `ruff check` | Violations grouped by file |
| `mypy` | Type errors grouped by file |
| `pip install` | Installed packages only, progress stripped |

## Go

| Command | What changes |
|---------|--------------|
| `go test` | Failures only |
| `golangci-lint run` | Violations grouped by file |
| `go build` | Errors only |

## Ruby

| Command | What changes |
|---------|--------------|
| `rspec` | Failures only |
| `rubocop` | Offenses grouped by file |
| `rake` | Task output, build errors highlighted |

## .NET

| Command | What changes |
|---------|--------------|
| `dotnet build` | Errors and warnings only |
| `dotnet test` | Failures only |
| `dotnet format` | Changed files only |

## Docker / Kubernetes

| Command | What changes |
|---------|--------------|
| `docker ps` | Essential columns (name, image, status, port) |
| `docker images` | Name + tag + size only |
| `docker logs` | Deduplicated, last N lines |
| `docker compose up` | Service status, errors highlighted |
| `kubectl get pods` | Name + status + restarts only |
| `kubectl logs` | Deduplicated entries |

## Files and Search

| Command | What changes |
|---------|--------------|
| `ls` | Tree format with file counts |
| `find` | Tree format |
| `grep` | Truncated lines, grouped by file |
| `ast-grep` | Grouped by file, capped overflow; `--json` passes through |
| `diff` | Context reduced |
| `wc` | Compact counts |
| `cat` / `head` / `tail <file>` | Smart file reading via `rtk read` |
| `rtk smart <file>` | 2-line heuristic code summary (signatures only) |

## Cloud and Data

| Command | What changes |
|---------|--------------|
| `aws` | JSON condensed, relevant fields only |
| `psql` | Query results without decoration |
| `curl` | Response body only, headers stripped |

## Global flags

These flags apply to all RTK commands and must come before the subcommand:

| Flag | Description |
|------|-------------|
| `-v` / `--verbose` | Show filtering details on stderr (`-v`, `-vv`, `-vvv` for increasing detail) |

```bash
# Debug: see what RTK is doing
rtk -vvv git status
```

## Commands that are not rewritten

If a command isn't in the list above, RTK runs it through passthrough — the output reaches the LLM unchanged. You can explicitly track unsupported commands:

```bash
rtk bypass make install    # runs make install, tracks usage, no filtering
rtk bypass GOFLAGS=-mod=mod go test ./...    # leading NAME=value words go to the command's environment
```

To check which commands were missed opportunities: `rtk discover`.
