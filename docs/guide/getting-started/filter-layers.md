---
title: Filter Layers
description: Tune RTK's generic filter pipeline — decorative, dedup, and truncate layers, globally or per command group, via config or environment variables.
sidebar:
  order: 5
---

# Filter Layers

Before each command's own filter runs, RTK passes the output through a small
pipeline of **generic layers**. Each command's bespoke filter always runs last;
the layers handle cross-cutting cleanup that applies everywhere — to Rust
command filters, TOML filters, and unsupported commands alike.

`decorative` and `dedup` apply to TOML-filtered commands too (run on the
filter's output, so they never change what the TOML rules matched). `truncate`
and per-group config stay Rust-command-only — a TOML filter's caps are explicit
in the filter, and TOML commands have no ecosystem group.

The mental model is simple: **a layer has a level.** You pick how hard each
layer pushes; `none` turns a layer off. Defaults are tuned to match RTK's
historical behavior, so you only touch this if you want more (or less)
compression.

## The layers

| Layer | What it does | Levels | Default |
|-------|--------------|--------|---------|
| `decorative` | Removes terminal chrome | `none` / `light` / `reasonable` / `high` | `reasonable` |
| `dedup` | Collapses repeated lines into `[×N] line` | `none` / `exact` / `normalized` | `none` (off) |
| `truncate` | Scales the per-command "show N items, +M more" caps | `none` / `light` / `reasonable` / `high` | `reasonable` |

Every layer accepts **`none`** to turn it off.

### decorative — chrome removal

Lossless cleanup applied to every command's output (and to otherwise-unsupported
commands via the global fallback).

| Level | Removes |
|-------|---------|
| `none` | nothing (layer off) |
| `light` | ANSI color codes only |
| `reasonable` (default) | ANSI + trailing whitespace + collapses blank-line runs |
| `high` | + drops pure box-drawing / separator lines (content-bearing rows kept) |

> Disabling `decorative` never breaks a command's own filter: the few filters
> that need ANSI removed to parse (e.g. `next`, `mypy`, `glab`, `rake`) strip it
> themselves regardless of this setting. `decorative` only controls the *generic*
> cosmetic pass.

### dedup — collapse repeats

Collapses consecutive identical lines into `[×N] line`. **Off by default.** When
enabled it runs on a command's *filtered output* (post-parse, so it never
corrupts a parser) and on the global fallback for unsupported commands.

| Level | Behavior |
|-------|----------|
| `none` (default) | no collapsing |
| `exact` | collapse byte-identical consecutive lines |
| `normalized` | mask volatile tokens (numbers, hex, timestamps) first, then collapse near-identical lines |

### truncate — item caps

Each command caps how many items it lists ("show 10 errors, +15 more"). The
`truncate` level scales those caps. **Higher = more compression (fewer items).**

| Level | Effect on caps |
|-------|----------------|
| `none` | no cap — show everything (can exceed raw, since the filter still adds its summary) |
| `light` | looser caps (×2 — show more) |
| `reasonable` (default) | today's per-command caps, unchanged |
| `high` | tighter caps (÷2 — fewer items) |

## Configuring globally

In `~/.config/rtk/config.toml`:

```toml
[layers]
decorative = "reasonable"   # none | light | reasonable | high
dedup      = "none"         # none | exact | normalized
truncate   = "reasonable"   # none | light | reasonable | high
exclude    = []             # extra commands to leave completely unfiltered
```

Or per-invocation via environment variables:

```bash
RTK_DECORATIVE_LEVEL=high rtk cargo build
RTK_TRUNCATE_LEVEL=none   rtk pip list      # show every item
RTK_DEDUP_LEVEL=normalized rtk <command>
```

## Configuring per command group

Layers can be overridden per **command group** (the rtk command's ecosystem
folder). A group key applies to every command in that group:

```toml
[layers]                 # global defaults (and the fallback)
truncate = "reasonable"

[layers.js]              # all JS/TS commands
dedup = "exact"

[layers.git]
decorative = "high"
```

Or via env, `RTK_<GROUP>_<LAYER>_LEVEL` (hyphens in a group become underscores):

```bash
RTK_PYTHON_TRUNCATE_LEVEL=high rtk pytest
RTK_GO_DECORATIVE_LEVEL=none   rtk go build
```

### Groups

| Group | Commands |
|-------|----------|
| `git` | git, gh, glab, gt, diff |
| `rust` | cargo, err, test |
| `js` | pnpm, npm, npx, jest, vitest, prisma, tsc, next, lint, prettier, playwright |
| `python` | ruff, pytest, mypy, pip |
| `go` | go, golangci-lint |
| `dotnet` | dotnet |
| `jvm` | gradlew |
| `cloud` | aws, psql, docker, kubectl, curl, wget |
| `ruby` | rake, rubocop, rspec |
| `system` | ls, tree, read, smart, json, deps, env, find, log, summary, grep, wc, format, pipe |

A command not in any group (or an RTK meta-command like `gain`) simply uses the
global `[layers]`.

## Precedence

Highest wins:

1. group env — `RTK_<GROUP>_<LAYER>_LEVEL`
2. group config — `[layers.<group>]`
3. global env — `RTK_<LAYER>_LEVEL`
4. global config — `[layers]`
5. built-in default

An unrecognized value (typo) is ignored and falls through to the next level.

## Turning RTK off for a command

`decorative = "none"` and friends disable *layers*, not the command's own
filter. To get fully raw output from a supported command, use proxy mode or
disable RTK for that invocation:

```bash
rtk proxy cargo build      # run with zero filtering, still tracked
RTK_DISABLED=1 cargo build # bypass RTK entirely
```

When **every** layer is off for an unsupported (fallback) command, RTK executes
it natively — byte-for-byte identical output, exit code, and stream ordering.

## Excluding commands from filtering

Raw-output commands stay byte-exact and are never filtered: `cat`, `head`,
`tail`, `base64`, `xxd`, `hexdump`, `od`, `strings`, `dd`. Add your own:

```toml
[layers]
exclude = ["mytool", "dump-binary"]
```

Matching is by command basename, so `/usr/bin/cat` and `cat` both match.
