# Built-in Filters

> See also [docs/contributing/TECHNICAL.md](../../docs/contributing/TECHNICAL.md) for the full architecture overview

Each `.toml` file in this directory defines one filter and its inline tests.
Files are concatenated alphabetically by `build.rs` into a single TOML blob embedded in the binary.

## When to Use a TOML Filter

TOML filters strip noise lines — they don't reformat output. The filtered result must still look like real command output (see [Design Philosophy](../../CONTRIBUTING.md#design-philosophy)). For the full TOML-vs-Rust decision criteria, see [CONTRIBUTING.md](../../CONTRIBUTING.md#toml-vs-rust-which-one).

TOML works well for commands with **predictable, line-by-line text output** where regex filtering achieves 60%+ savings:
- Install/update logs (brew, composer, poetry) — strip `Using ...` / `Already installed` lines
- System monitoring (df, ps, systemctl) — keep essential rows, drop headers/decorations
- Simple linters (shellcheck, yamllint, hadolint) — strip context, keep findings
- Infra tools (terraform plan, helm, rsync) — strip progress, keep summary

For the full contribution checklist (including `discover/rules.rs` registration), see [src/cmds/README.md — Adding a New Command Filter](../cmds/README.md#adding-a-new-command-filter).

## Adding a filter

1. Copy any existing `.toml` file and rename it (e.g. `my-tool.toml`)
2. Update the three required fields: `description`, `match_command`, and at least one action field
3. Add `[[tests.my-tool]]` entries to verify the filter behaves correctly
4. Run `cargo test` — the build step validates TOML syntax and runs inline tests

## File format

```toml
[filters.my-tool]
description = "Short description of what this filter does"
match_command = "^my-tool\\b"          # regex matched against the full command string
strip_ansi = true                       # optional: strip ANSI escape codes first
strip_lines_matching = [               # optional: drop lines matching any of these regexes
  "^\\s*$",
  "^noise pattern",
]
squeeze_whitespace = false              # optional: collapse space/tab runs to one space (lossless, default off)
collapse_table_padding = false          # optional: collapse padded-column gaps to one separator (lossless, default off)
fold_repeats = false                    # optional: collapse consecutive identical lines to "line ×N" (lossless, default off)
max_lines = 40                          # optional: keep only the first N lines after filtering
on_empty = "my-tool: ok"               # optional: message to emit when output is empty after filtering

[[tests.my-tool]]
name = "descriptive test name"
input = "raw command output here"
expected = "expected filtered output"
```

## Available filter fields

| Field | Type | Description |
|-------|------|-------------|
| `description` | string | Human-readable description |
| `match_command` | regex | Matches the command string (e.g. `"^docker\\s+inspect"`) |
| `strip_ansi` | bool | Strip ANSI escape codes before processing |
| `filter_stderr` | bool | Capture and merge stderr into stdout before filtering (use for tools like liquibase that emit banners to stderr) |
| `strip_lines_matching` | regex[] | Drop lines matching any regex |
| `keep_lines_matching` | regex[] | Keep only lines matching at least one regex |
| `replace` | array | Regex substitutions (`{ pattern, replacement }`) |
| `match_output` | array | Short-circuit rules (`{ pattern, message }`) |
| `squeeze_whitespace` | bool | Collapse runs of spaces/tabs inside a line to a single space and strip trailing whitespace; leading indentation is preserved. **Lossless** — off by default. |
| `collapse_table_padding` | bool | For padded-column lines (2+ spaces between cells, or `\|`-separated with padding), reduce each inter-cell gap to a single separator (` \| ` or two spaces), keeping every cell verbatim. **Lossless** — off by default. |
| `fold_repeats` | bool | Collapse runs of consecutive identical lines into one line suffixed with ` ×N` (N >= 2). **Lossless** — the repeat count is recoverable from the suffix. Off by default. |
| `truncate_lines_at` | int | Truncate lines longer than N characters |
| `max_lines` | int | Keep only the first N lines |
| `tail_lines` | int | Keep only the last N lines (applied after other filters) |
| `on_empty` | string | Fallback message when filtered output is empty |

## Lossless whitespace/repeat transforms

`squeeze_whitespace`, `collapse_table_padding`, and `fold_repeats` all default
to `false` and never drop a line's *content* — only its whitespace geometry
(squeeze/collapse) or its repeat count representation (fold), both of which are
recoverable from the output. They run in that order, after `strip/keep_lines`
and before `truncate_lines_at`, so folding sees already-squeezed lines.

Reach for these on output that has no dedicated filter but is heavy on padded
columns or duplicate lines — `psql` tables, `ps`/`ps aux`, Python heredoc
tracebacks, `column -t`-formatted output:

```toml
[filters.psql-table]
description = "compact psql table output"
match_command = "^psql\\b"
squeeze_whitespace = true
collapse_table_padding = true
fold_repeats = true
```

Before:
```
 id | name    | status
----+---------+--------
  1 | alice   | active
  2 | bob     | active
  2 | bob     | active
```

After:
```
 id | name | status
----+---------+--------
  1 | alice | active
  2 | bob | active ×2
```

(`----+---------+--------` is untouched — a run of `-`/`+` has no 2+ space gap
and no whitespace-padded `|`, so it doesn't look like a padded-column line.)

## Naming convention

Use the command name as the filename: `terraform-plan.toml`, `docker-inspect.toml`, `mix-compile.toml`.
For commands with subcommands, prefer `<cmd>-<subcommand>.toml` over grouping multiple filters in one file.

## Build and runtime pipeline

How a `.toml` file goes from contributor → binary → filtered output.

```mermaid
flowchart TD
    A[["src/filters/my-tool.toml\n(new file)"]] --> B

    subgraph BUILD ["cargo build"]
        B["build.rs\n1. ls src/filters/*.toml\n2. sort alphabetically\n3. concat → BUILTIN_TOML"] --> C
        C{"TOML valid?\nDuplicate names?"} -->|"fail"| D[["Build fails\nerror points to bad file"]]
        C -->|"ok"| E[["OUT_DIR/builtin_filters.toml\n(generated)"]]
        E --> F["rustc embeds via include_str!"]
        F --> G[["rtk binary\nBUILTIN_TOML embedded"]]
    end

    subgraph TESTS ["cargo test"]
        H["test_builtin_filter_count\nassert_eq!(filters.len(), N)"] -->|"wrong count"| I[["FAIL"]]
        J["test_builtin_all_filters_present\nassert!(names.contains('my-tool'))"] -->|"name missing"| K[["FAIL"]]
        L["test_builtin_all_filters_have_inline_tests\nassert!(tested.contains(name))"] -->|"no tests"| M[["FAIL"]]
    end

    subgraph RUNTIME ["rtk my-tool args"]
        R["TomlFilterRegistry::load()\n1. .rtk/filters.toml\n2. ~/.config/rtk/filters.toml\n3. BUILTIN_TOML\n4. passthrough"] --> S
        S{"match_command\nmatches?"} -->|"no match"| T[["exec raw (passthrough)"]]
        S -->|"match"| U["exec command\ncapture stdout"]
        U --> V["11-stage pipeline\nstrip_ansi → replace → match_output\n→ strip/keep_lines → squeeze_whitespace\n→ collapse_table_padding → fold_repeats\n→ truncate → tail_lines → max_lines → on_empty"]
        V --> W[["print filtered output + exit code"]]
    end

    G --> H & J & L & R
```

## Filter lookup priority

```mermaid
flowchart LR
    CMD["rtk my-tool args"] --> P1
    P1{"1. .rtk/filters.toml\n(project-local)"}
    P1 -->|"match"| WIN["apply filter"]
    P1 -->|"no match"| P2
    P2{"2. ~/.config/rtk/filters.toml\n(user-global)"}
    P2 -->|"match"| WIN
    P2 -->|"no match"| P3
    P3{"3. BUILTIN_TOML\n(binary)"}
    P3 -->|"match"| WIN
    P3 -->|"no match"| P4[["exec raw (passthrough)"]]
```

First match wins. A project filter with the same name as a built-in shadows the built-in and triggers a warning:

```
[rtk] warning: filter 'make' is shadowing a built-in filter
```

## Custom filters and trust

You can add your own filters in two places (both use the format above):

- **Project-local** — `.rtk/filters.toml` (committed with a repo, applies in that project)
- **User-global** — `~/.config/rtk/filters.toml` (applies in every project)

Because a filter can rewrite or hide the command output an agent sees, **custom filter files are not applied until you trust them**. An untrusted (or edited) filter file is skipped **silently** on the command path — RTK never prints a warning around a rewritten command. You discover and enable untrusted filters through commands you run deliberately:

```bash
rtk trust       # lists each detected filter (labelled project/global) + a risk summary, then asks to confirm ([y/N], or --yes)
rtk untrust     # revokes trust
```

- Trust is recorded as a SHA-256 of the file's contents, so **editing a trusted file requires re-running `rtk trust`** — a content change invalidates trust.
- `rtk init` detects existing custom filters and lets you enable them — an interactive `[y/N]`, or `--trust-filters` / `--no-trust-filters` for scripts. It stays silent for an empty template (comments only) and, when run non-interactively, leaves filters disabled.
- Built-in filters (this directory) are compiled into the binary and always trusted; only the on-disk project and user-global files are gated.

> **Honest limitation:** this is consent + tamper-evidence, not a sandbox. An attacker who can write your filter file can usually also write the trust store (`~/.local/share/rtk/trusted_filters.json`) and bypass the gate. It defends the common case — a filter dropped in by a script, a dotfile sync, or an untrusted repo — not a same-user attacker who specifically targets RTK.
