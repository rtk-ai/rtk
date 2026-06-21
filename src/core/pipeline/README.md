# Filter Pipeline

Generic, layered output filtering wrapped around each command's own filter: the
pipeline strips decorative chrome **before** the command's filter, collapses
consecutive repeats (`dedup`) **after** it, and exposes a `truncate` dial the
filter reads for its item caps.

## Concepts

- **Node layer** — a generic transformation pass run around the custom filter
  (decorative pre-custom; dedup post-custom in `run`). Each lives in its own file
  with a whole-string (captured) and per-line (streaming) form:
  - `decorative` — chrome removal (ANSI, blank runs, box-drawing). Safe pre-custom.
  - `dedup` — collapse consecutive repeats into `[×N] line`. Default **off**. Runs
    **post-custom** in `run` (after the command's filter, so it can't corrupt a
    parser). The streaming path still wraps it pre-custom, so there it's wired
    only for the parser-less fallback.
- **Dial layer** — not a pass; a global level the command's own renderer reads.
  - `truncate` — scales the item caps (`core::truncate::caps()`): each command
    keeps deciding *which* items to cap, but reads a level-scaled value instead of
    the `CAP_*` const. `reasonable` = today; `high` ÷2; `light` ×2; `none` = no cap.
- **`Routing`** (`mod.rs`) — a per-command, **code-level** policy of which node
  layers run. Not user-configurable. A command opts out with `Routing { decorative: false }`.
- **`Levels`** (`levels.rs`) — the **user-configurable** aggressivity per layer
  (`DecorativeLevel`/`DedupLevel`/`TruncateLevel`), resolved once and cached. Every
  level has a `None` variant = layer off.
- **custom filter** — the command's own `cmds/` filter. Always the terminal step.

## Two execution modes

`Pipeline::with_routing(routing)` then either:

- `run(raw, custom)` — **captured**: apply enabled layers to the whole output,
  then call `custom`. Used by `runner` for `run_filtered` / `run_filtered_with_exit`.
- `stream(inner)` — **streamed**: wrap the command's `StreamFilter` so enabled
  layers run per-line before it. Used by `runner` for `run_streamed`. Only
  line-oriented layers have a streaming form; whole-output layers cannot stream.

In both, the raw output kept for tee/tracking is the untouched original — layers
only affect what the custom filter (and the user) sees.

## Where it is wired

The pipeline is applied centrally in `core::runner`, so every command routed
through `runner` inherits it:

- captured paths (`run_filtered`, `run_filtered_with_exit`) → `Pipeline::run`
- streamed path (`run_streamed`) → `Pipeline::stream`

Commands that bypass `runner` (direct `stream::exec_capture` /
`stream::run_streaming`) do not go through the pipeline.

## Global fallback

Unsupported commands (no `cmds/` handler, no TOML filter) reach `run_fallback`
in `main.rs`. Routing order: **cmds → TOML → global fallback**. A TOML match
runs its own filter, then `route_toml_output` sends the result through the
decorative + dedup layers (global levels; not the truncate dial or per-group).
The fallback:

- **terminal stdout** → passthrough (inherit stdio) so interactive apps and
  color work.
- **excluded command** (`is_excluded`) → passthrough untouched, so raw-output
  commands (`cat`, `head`, …) stay byte-exact.
- **otherwise (piped)** → stream through the pipeline with an `Identity` custom
  filter (`FALLBACK_ROUTING` wires decorative + dedup; dedup is off unless a
  dedup level is configured). No command-specific filtering.
- when every routed layer resolves to off, `is_noop()` short-circuits to native
  exec (byte-identical output/exit/stream ordering).

The exclude list is a built-in `const` set in `levels.rs`, extended by the user
via `[layers].exclude`.

## Level resolution (`levels.rs`)

Resolved once per process (cached in a `OnceLock`) to keep config off the hot
path. `main.rs` calls `set_group()` with the running command's folder group
(from `GROUPS`/`group_for_command`) *before* the first level read. Precedence,
highest first:

1. group env — `RTK_<GROUP>_<LAYER>_LEVEL` (hyphens in the group → underscores,
   e.g. `golangci-lint` → `RTK_GOLANGCI_LINT_<LAYER>_LEVEL`)
2. group config — `[layers.<group>]`
3. global env — `RTK_<LAYER>_LEVEL`
4. global config — `[layers]`
5. built-in default

`GROUPS` maps each `cmds/` folder to its commands (the per-group config surface).
The runtime can't see a command's source folder, so the mapping is explicit; the
`groups_match_subcommands` test (in `main.rs`) guards it against typos/renames,
and an unlisted command falls through to the global `[layers]`.

## Adding a layer

1. New file `pipeline/<layer>.rs` with its level enum (incl. a `None` = off
   variant) + whole-string and (if line-oriented) per-line forms.
2. Add a field to `Routing` if a command must be able to opt out in code.
3. Apply it in `Pipeline::run` (and `stream` if it has a per-line form), in
   canonical order. Node layers that need clean (parsed) input run *after* the
   custom step, like `dedup`.
4. If user-tunable, add a field to `Levels` + `LayersConfig` (+ `GroupLayers` for
   per-group) and resolve it in `levels.rs::resolve`.
