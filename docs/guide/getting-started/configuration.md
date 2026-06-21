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
# directory = "/custom/tee/path"  # optional override

[telemetry]
enabled = true              # anonymous daily ping — see Telemetry & Privacy for full details

[hooks]
exclude_commands = []       # commands to never auto-rewrite

[layers]
decorative = "reasonable"   # chrome removal: "none" | "light" | "reasonable" | "high"
dedup = "none"              # collapse repeated lines: "none" | "exact" | "normalized"
truncate = "reasonable"     # item caps "show N, +M more": "none" | "light" | "reasonable" | "high"
exclude = []                # extra commands to leave unfiltered (raw passthrough)

# Optional per-group overrides (group = command ecosystem). See Filter Layers.
# [layers.js]
# dedup = "exact"
```

Every layer accepts `"none"` to turn it off. The default keeps RTK's current behavior. See **[Filter Layers](./filter-layers.md)** for the full reference.

For full details on what is collected, opt-out options, and GDPR rights, see [Telemetry & Privacy](../resources/telemetry.md).

## Environment variables

| Variable | Description |
|----------|-------------|
| `RTK_DISABLED=1` | Disable RTK for a single command (`RTK_DISABLED=1 git status`) |
| `RTK_DECORATIVE_LEVEL` | Decorative level for this invocation (`none`/`light`/`reasonable`/`high`) |
| `RTK_DEDUP_LEVEL` | Dedup level for this invocation (`none`/`exact`/`normalized`) |
| `RTK_TRUNCATE_LEVEL` | Item-cap level for this invocation (`none`/`light`/`reasonable`/`high`) |
| `RTK_<GROUP>_<LAYER>_LEVEL` | Per-group override, e.g. `RTK_JS_TRUNCATE_LEVEL` (see [Filter Layers](./filter-layers.md)) |
| `RTK_TEE_DIR` | Override the tee directory |
| `RTK_TELEMETRY_DISABLED=1` | Disable telemetry |
| `RTK_HOOK_AUDIT=1` | Enable hook audit logging |
| `SKIP_ENV_VALIDATION=1` | Skip env validation (useful with Next.js) |

## Filter layers

Before each command's own filter, RTK runs generic layers — `decorative` (chrome
removal), `dedup` (collapse repeats), and the `truncate` dial (item caps). Each
has levels from `none` (off) to `high` (most compression), set globally in
`[layers]` above or per command group via `[layers.<group>]` /
`RTK_<GROUP>_<LAYER>_LEVEL`.

| Layer | Levels | Default |
|-------|--------|---------|
| `decorative` | none / light / reasonable / high | reasonable |
| `dedup` | none / exact / normalized | none (off) |
| `truncate` | none / light / reasonable / high | reasonable |

Quick overrides:

```bash
RTK_DECORATIVE_LEVEL=high rtk cargo build
RTK_TRUNCATE_LEVEL=none   rtk pip list        # show everything
RTK_JS_DEDUP_LEVEL=exact  rtk vitest          # per-group
```

See **[Filter Layers](./filter-layers.md)** for the full reference: every level,
the group list, precedence, and examples.

### Excluding commands

Raw-output commands stay byte-exact and are never filtered: `cat`, `head`,
`tail`, `base64`, `xxd`, `hexdump`, `od`, `strings`, `dd`. Add your own under
`[layers].exclude`; matching is by command basename.

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

Patterns match against the full command after stripping env prefixes (`sudo`, `VAR=val`), so `"psql"` excludes both `psql -h localhost` and `PGPASSWORD=x psql -h localhost`.

Subcommand patterns work too: `"git push"` excludes `git push origin main` but not `git status`.

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

## Per-project filters

Create `.rtk/filters.toml` in your project root to add custom filters or override built-ins. See [`src/filters/README.md`](https://github.com/rtk-ai/rtk/blob/master/src/filters/README.md) for the full TOML DSL reference.
