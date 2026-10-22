---
title: Methods
description: The methods RTK composes pipelines from, and what each one costs and recovers
sidebar:
  order: 4
---

# Methods

A method transforms one block of text. Rules chain them with `use = [...]`;
the chain is checked at load so a method never receives a format it cannot
handle, and a method marked terminal (`toon`) is only accepted last.

`rtk methods list` prints this table.

## Built-in (hook + proxy sites)

| Method | Lossy | Recovery | Runs under `proxy.request` | What it does |
|---|---|---|---|---|
| `ansi` | no |  | tool_result (normalize) | strip escape sequences |
| `progress` | yes | `rtk recall` (hook site) | tool_result (normalize) | keep only the last frame of `\r`-animated output |
| `dedup` | no |  | tool_result (normalize) | collapse ≥3 identical adjacent lines to `[xN]` |
| `base64-elide` | yes | none — value-free blobs | tool_result | replace long base64 runs with a size note |
| `cap` | yes | `rtk recall <hash>` | no — needs recall | over 300 lines: keep head 50 + tail 200; full output stored |
| `toon` | yes | terminal · `rtk recall` (hook site) | no — needs recall | uniform array of flat objects → TOON; else minified JSON |
| `minify-json` | yes | `rtk recall` (hook site) | no — needs recall | compact re-serialisation |
| `web-md` | no |  | tool_result | HTML → markdown |
| `md-slim` | yes | none — read-noise | tool_result | drop badges, comments, blank runs; keeps link URLs |
| `grep-group` | yes | `rtk recall` (hook site) | no — needs recall | group `path:line:content` matches by file; every match kept |
| `lockfile` | yes | none | tool_result | lockfiles → `name@version` list |
| `strip-line-numbers` | yes | none; ships **off** | tool_result (normalize) | remove line-number prefixes from file reads |
| `truncate` | yes | recall | no — needs recall | structural line cap (not in any shipped chain) |
| `shape` | yes | recall | no — needs recall | normalize → classify layout → per-shape render |
| `rle` | yes | `rtk recall` (hook site) | no — needs recall | run-length collapse of adjacent identical lines |

Every lossless method is idempotent, so a block reaching both a hook rule and
a proxy rule is never damaged; every lossy method marks its output, and the
proxy skips blocks already marked. Methods marked "needs recall" are skipped
under `proxy.request` because the daemon cannot store the recall copy — see
[What the Proxy Compresses](../proxy/compression.md#your-own-methods-on-tool-results).

## Proxy-resident

Implemented in the [RTK Proxy](../proxy/overview.md) only.

| Method | Lossy | Recovery | Runs under `proxy.request` | What it does |
|---|---|---|---|---|
| `cache-align` | no | | cache | place the cache breakpoint; no-op if the client set one |
| `ccr` | yes | `rtk_retrieve` | tool_result | compress a large tool result already in the conversation to a preview and a marker the model can expand — [opt-in](../proxy/compression.md#large-tool-results-ccr) |
| `retire-pointer` | yes | the file on disk, `rtk_retrieve` | tool_use | replace a file body echoed through a write argument with a pointer to the file — [opt-in](../proxy/compression.md#large-tool-results-ccr) |

Naming it under `hook.*` leaves the chain without it, so the rule fires as a
no-op. `validate` reports **W011**, and **W012** when that empties the rule
completely; `rtk pipeline show` marks the rule `(inert: proxy-only)`.

## Chains — shipped composites, usable anywhere a method is

| Chain | = |
|---|---|
| `term` | `ansi` `progress` `dedup` `base64-elide` `cap` |
| `json` | `base64-elide` `toon` |
| `web` | `ansi` `base64-elide` `web-md` `toon` `md-slim` |
| `matches` | `grep-group` |

## Your own chains

```toml
[method.gentle]
chain = ["ansi", "dedup"]

[[hook.after_tool.shell]]
name = "gentle-term"
use  = ["gentle"]
```

Chains are also composed directly in rules — `use = ["ansi", "cap"]` runs one
then the other. Order is yours and is checked.

## Your own command filters

Custom command filters live where they always have: `~/.config/rtk/filters.toml`
(macOS `~/Library/Application Support/rtk/filters.toml`) and `.rtk/filters.toml`
in a project. Nothing about them changes in v1.0 — same keys, same inline
tests, same `rtk verify`.

```toml
schema_version = 1

[filters.mytool]
description          = "Output of my internal deploy tool"
match_command        = "^mytool\\b"
strip_ansi           = true
strip_lines_matching = ["^\\[INFO\\]", "^Downloading "]
max_lines            = 200
on_empty             = "(no output)"

[[tests.mytool]]
name = "strips info"
input = "[INFO] x\nreal line"
expected = "real line"
```

Then run `rtk trust` once. Filter files are trust-gated because they rewrite
what your agent sees; an untrusted (or edited-since-trust) file is skipped.

For a command RTK has no filter for, the `rtk-rewrite` rule rewrites
`mytool deploy` → `rtk mytool deploy` (it consults every filter's
`match_command`), and `rtk mytool deploy` runs the real command and applies your
filter. Filters are the rewrite's business: they are not methods and do not
appear in `use`, and the pipeline file does not define them (E016). The
decision log and `rtk pipeline explain --last` show the rewrite that happened.

## Your own methods in Rust

A `[method.*]` composes existing methods; a [plugin](./plugins.md) adds a
genuinely new transform — or a new task kind — written in Rust and built into
your `rtk`. It then resolves by name in `use = [...]` like any built-in.
