---
title: Configuration
description: Customize RTK behavior via config.toml, environment variables, and per-project filters
sidebar:
  order: 4
---

# Configuration

## Config file location

| Platform | Path |
|----------|------|
| Linux | `~/.config/rtk/config.toml` |
| macOS | `~/Library/Application Support/rtk/config.toml` |

```bash
rtk config            # show current configuration
rtk config --create   # create config file with defaults
```

## Full config structure

```toml
[tracking]
enabled = true              # enable/disable token tracking
history_days = 90           # retention in days (auto-cleanup)
database_path = "/custom/path/history.db"   # optional override

[display]
colors = true               # colored output
emoji = true                # use emojis in output
max_width = 120             # maximum output width

[filters]
# These apply to file-reading commands (ls, find, grep, cat/rtk read).
# Paths matching these patterns are excluded from output, keeping noise low.
ignore_dirs = [".git", "node_modules", "target", "__pycache__", ".venv", "vendor"]
ignore_files = ["*.lock", "*.min.js", "*.min.css"]

[tee]
enabled = true              # save raw output on failure
mode = "failures"           # "failures" (default), "always", "never"
max_files = 20              # rotation: keep last N files
max_file_size = 1048576     # 1 MB in bytes
# directory = "/custom/tee/path"  # optional override

[telemetry]
enabled = true              # anonymous daily ping — see Telemetry & Privacy for full details

[hooks]
exclude_commands = []       # commands to never auto-rewrite

# Per-tool rules (optional, repeatable). See "Per-tool rules" below.
[[tools]]
match = { command = "npm", subcommand = "run", args_contains = ["build"] }
env = { CI = "1" }          # inject env before the command runs
```

For full details on what is collected, opt-out options, and GDPR rights, see [Telemetry & Privacy](../resources/telemetry.md).

## Environment variables

| Variable | Description |
|----------|-------------|
| `RTK_DISABLED=1` | Disable RTK for a single command (`RTK_DISABLED=1 git status`) |
| `RTK_TEE_DIR` | Override the tee directory |
| `RTK_TELEMETRY_DISABLED=1` | Disable telemetry |
| `RTK_HOOK_AUDIT=1` | Enable hook audit logging |
| `SKIP_ENV_VALIDATION=1` | Skip env validation (useful with Next.js) |

## Tee system

When a command fails, RTK saves the full raw output to a local file and prints the path:

```
FAILED: 2/15 tests
[full output: ~/.local/share/rtk/tee/1707753600_cargo_test.log]
```

Your AI assistant can then read the file if it needs more detail, without re-running the command.

| Setting | Default | Description |
|---------|---------|-------------|
| `tee.enabled` | `true` | Enable/disable |
| `tee.mode` | `"failures"` | `"failures"`, `"always"`, `"never"` |
| `tee.max_files` | `20` | Rotation: keep last N files |
| Min size | 500 bytes | Outputs shorter than this are not saved |
| Max file size | 1 MB | Truncated above this |

## Excluding commands from auto-rewrite

Prevent specific commands from being rewritten by the hook:

```toml
[hooks]
exclude_commands = ["git rebase", "git cherry-pick", "docker exec"]
```

Patterns match against the full command after stripping env prefixes (`VAR=val`), so `"psql"` excludes both `psql -h localhost` and `PGPASSWORD=x psql -h localhost`.

Subcommand patterns work too: `"git push"` excludes `git push origin main` but not `git status`.

An entry names a tool RTK has a filter for, and covers the wrapper, interpreter and path spellings
of it. Before matching, RTK peels those off the command and matches what is left, so
`"playwright"` excludes `playwright test`, `npx playwright test` and `pnpm exec playwright test`
alike; `"pytest"` also covers `python3 -m pytest tests/`, and `"phpunit"` covers
`vendor/bin/phpunit` and `php vendor/bin/phpunit`.

Three spellings are not peeled yet, and still rewrite despite a matching entry:

| Entry | Command | Why |
|---|---|---|
| `"head"`, `"tail"` | `head -20 f`, `tail -n 5 f` | The line-range form takes a fast path that returns before the exclusion is consulted ([#2823](https://github.com/rtk-ai/rtk/issues/2823)). Without a line range, `head f` is excluded normally. |
| `"gradlew"`, `"mvn"` | `gradlew.bat build`, `mvnw.cmd test` | Path stripping splits on `/`, so a `.bat`/`.cmd` spelling never reduces to the tool name. `./gradlew` and `gradlew` are both excluded ([#3617](https://github.com/rtk-ai/rtk/pull/3617)). |
| `"golangci-lint"` | `golangci run ./...` | `golangci run` is one of the rule's own aliases and is kept whole, so it does not match the `golangci-lint` entry. Exclude `"golangci"` as well to cover it. |

A tool RTK has no filter of its own for is matched as typed, because RTK only sees the wrapper:
with `["my-tool"]`, `npx my-tool` still rewrites to `rtk npx my-tool`. Exclude `"npx"` to stop
that.

The arguments are kept when peeling, so an anchored pattern still narrows the way you wrote it:
`"^ls$"` excludes a bare `ls` without swallowing `ls -la`. Matching stays exact — `"go"` never
excludes `golangci-lint`, subcommand patterns stay literal (`"git push"` does not widen to all of
`git`), and an entry never leaks to a different tool that happens to share an RTK filter: `"read"`
does not exclude `cat`, and `"eslint"` does not exclude `biome`.

Patterns starting with `^` are treated as regex:

```toml
[hooks]
exclude_commands = ["^curl", "^wget", "git rebase"]
```

Invalid regex patterns fall back to prefix matching.

Or for a single invocation:

```bash
RTK_DISABLED=1 git rebase main
```

## Telemetry

RTK sends one anonymous ping per day (23h interval). No personal data, no file paths, no command content.

Data sent: device hash, version, OS, architecture, command count/24h, top commands, savings %.

To opt out:

```bash
# Via environment variable
export RTK_TELEMETRY_DISABLED=1

# Via config.toml
[telemetry]
enabled = false
```

## Per-tool rules

Some commands need adjusted handling — for example, interactive builders that hang when
their output is captured through a pipe instead of a terminal. The `[[tools]]` array lets
you attach behavior to a matched command without recompiling.

```toml
[[tools]]
match = { command = "npm", subcommand = "run", args_contains = ["build"] }
env = { CI = "1" }            # run the builder one-shot so it exits cleanly

[[tools]]
match = { command = "vite" }
capture = "pty"               # run under a pseudo-terminal
strip_ansi = true             # strip color/cursor codes from captured output (default for pty)
```

### Matching

| Field | Meaning |
|-------|---------|
| `command` | Required. The command basename, e.g. `"npm"`, `"ng"`. |
| `subcommand` | Optional. The first non-flag argument, e.g. `"run"` or `"build"`. |
| `args_contains` | Optional. Every listed token must appear in the args. Use it to target one script, e.g. `["build"]` matches `npm run build` but not `npm run test`. |

Rules are evaluated top to bottom; the first matching rule applies.

### Actions

| Field | Default | Meaning |
|-------|---------|---------|
| `env` | `{}` | Environment variables set on the child before it runs. The lightest fix for builders that hang on a pipe but run one-shot under `CI=1` (Angular, Vite, many JS toolchains). Applies in all capture modes. |
| `capture` | `"pipe"` | `"pipe"` (normal) or `"pty"`. A pseudo-terminal makes the child behave as in a real terminal — it runs one-shot and exits — which avoids hangs caused by a long-lived helper process (e.g. esbuild) holding the captured pipe open. PTY support is built unless RTK is compiled with `--no-default-features`. |
| `strip_ansi` | `true` when `capture = "pty"`, else `false` | Remove ANSI color/cursor/spinner sequences from captured output so the filtered result stays clean. |

### When to use which

- **Prefer `env = { CI = "1" }`** for npm/Angular/Vite-style builders — it's free (no extra
  capture machinery) and makes the tool exit cleanly on its own.
- **Use `capture = "pty"`** for tools that have no such non-interactive switch and only
  terminate when attached to a terminal.

## Custom filters

Add your own filters (or override built-ins) in either location:

- **Project-local** — `.rtk/filters.toml` in your project root (committed with the repo)
- **User-global** — `~/.config/rtk/filters.toml` (applies to every project)

See [`src/filters/README.md`](https://github.com/rtk-ai/rtk/blob/master/src/filters/README.md) for the full TOML DSL reference.

### Trusting custom filters

Because a filter can rewrite what your AI assistant sees, custom filter files are **not applied until you trust them**. An untrusted (or edited) filter file is skipped silently on the command path. You review and manage trust with explicit commands:

```bash
rtk trust      # shows each filter and asks to confirm (--yes to skip the prompt)
rtk untrust    # revokes trust
```

`rtk init` also detects existing filters and lets you enable them — interactively, or non-interactively with `--trust-filters` / `--no-trust-filters`. Trust is tied to the file's contents (SHA-256), so editing a trusted file requires re-running `rtk trust`.

> **Upgrading:** earlier versions applied `~/.config/rtk/filters.toml` without trust. After upgrading, the user-global file is gated like project filters — if you already relied on a global filter, run `rtk trust` once to re-enable it.
