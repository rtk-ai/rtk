# Discover — History Analysis & Command Rewrite

> Full rewrite pipeline diagram: [docs/contributing/TECHNICAL.md](../../docs/contributing/TECHNICAL.md#32-hook-interception-command-rewriting)

## What This Module Does

This module has two jobs:

1. **Rewrite commands** — Every LLM agent hook calls `rtk rewrite "git status"`. This module decides whether to rewrite it (`rtk git status`) or pass it through unchanged. This is the hot path — every command the LLM runs goes through here.

2. **Analyze history** — `rtk discover` scans past LLM sessions to find commands that *could have been* rewritten but weren't. Same classification logic, different consumer.

## How Command Rewriting Works

When a hook sends `cargo fmt --all && cargo test 2>&1 | tail -20`:

**Tokenization** — The lexer (`lexer.rs`) turns the raw string into typed tokens. It's a single-pass state machine that understands shell quoting, escapes, redirects, and operators. This is critical because naive string splitting breaks on quoted content like `git commit -m "fix && update"`.

```
"cargo test 2>&1 && git status"
→ [Arg("cargo"), Arg("test"), Redirect("2>&1"), Operator("&&"), Arg("git"), Arg("status")]
```

**Compound splitting** — The rewrite engine walks the tokens, splitting on `Operator` (`&&`, `||`, `;`) and typed `Pipe` tokens (`|`, `|&`). For normal pipelines, producers and intermediate stages stay raw, and only an argument-safe final stage marked `pipeline_final_safe` is rewritten. The initial safe set is ordinary `grep` and `rg` invocations; search pattern-file forms (`-f`/`--file`) defer because they can consume pipeline stdin as configuration. Stderr pipelines (`|&`) and pipelines containing opaque shell groups remain raw.

**Per-segment rewriting** — Each segment goes through:

1. Strip trailing redirects (`2>&1`, `>/dev/null`) — matched via lexer tokens, set aside, re-appended after rewriting
2. Short-circuit special cases — `head -20 file` → `rtk read file --max-lines 20`, `tail -n 5 file` → `rtk read file --tail-lines 5`. These can't go through generic prefix replacement because it would produce `rtk read -20 file` (wrong flag position)
3. Classify the command — strip env prefixes (`FOO="bar baz"`), normalize paths (`/usr/bin/grep` → `grep`), strip git global opts (`git -C /tmp` → `git`), then match against 60+ regex patterns from `rules.rs`
4. Apply the rewrite — find the matching rule, replace the command prefix with `rtk <cmd>`, re-prepend the env prefix, re-append the redirect suffix

**Guards along the way:**
- `RTK_DISABLED=1` in the env prefix → skip rewrite
- `gh` with `--json`/`--jq`/`--template` → skip (structured output, rtk would corrupt it)
- `cat` with flags other than `-n` → skip (different semantics than `rtk read`)
- `cat`/`head`/`tail` with `>` or `>>` → skip (write operation, not a read)
- Command in `hooks.exclude_commands` config → skip

**Result**: `rtk cargo fmt --all && rtk cargo test 2>&1 | tail -20`. Bash handles the `&&` and `|` at execution time — each `rtk` invocation is a separate process.

## Shared Lexer Toolkit

`lexer.rs` is layer 1 of RTK's command parsing (raw string → quote/operator-aware tokens and words) — layer 2, already-split argv → flags/values, is `core/arg_tokenizer.rs`. It's not private to this module: `hooks/permissions.rs`, `hooks/mod.rs`, and `main.rs`'s `rtk proxy` all build on it instead of re-scanning commands themselves.

| Function | Purpose | Used outside `discover/` by |
|---|---|---|
| `tokenize(cmd)` | Full shell-syntax tokens: quotes, escapes, operators, pipes, redirects, shellisms | — |
| `tokenize_with_newlines(cmd)` | Like `tokenize`, plus a `\n` `Operator` token per unquoted newline (a lone `\r` stays glued to its word, matching real bash) | — |
| `shell_split(cmd)` | Quote-aware split into argv-ready words (quotes stripped, escapes resolved) | `hooks/mod.rs::is_claude_hook_command`, `main.rs`'s `rtk proxy '...'` |
| `split_for_permissions(cmd)` | Segments a compound command for the **permission gate** — deliberately the most conservative of three segmenters (see its doc comment for the full comparison table) | `hooks/permissions.rs::check_command_with_rules` |
| `split_on_operators(cmd, stop_at_pipe)` | Segments for classification only — not safe for permission/security decisions | `registry.rs::split_command_chain` |
| `contains_unattestable_construct(cmd)` | True for command/process substitution or a file-target redirect — constructs the permission gate can't decompose and must never auto-allow | `hooks/permissions.rs::check_command_with_rules` |

The permission gate, discover/analytics classification, and rewrite each segment compound commands (`&&`, `;`, `|`, background `&`, subshells) slightly differently on purpose — the gate must never under-segment (a hidden command could evade a deny rule), while rewrite and analytics only need to reproduce or classify the command's actual shape. Don't reuse `split_on_operators` or `rewrite_compound`'s segmenting for a permission/security decision; use `split_for_permissions`.

## How History Analysis Works

`rtk discover` reads Claude Code and Pi JSONL session files. Claude uses `tool_use`/`tool_result` blocks, while modern Pi stores `toolCall`/`toolResult` messages; provider-specific parsers normalize both into the same command records. The module:

1. Extracts commands from the JSONL via the `SessionProvider` trait
2. Splits compound commands using the same lexer-based tokenization
3. Classifies each command against the same rules used for live rewriting
4. Aggregates results: which commands could have been rewritten, estimated token savings, adoption rate

The classification logic is shared between discover and rewrite — same patterns, same rules, different consumers.

## Env Prefix Handling

The `ENV_PREFIX` regex strips env variable assignments and `env` from the front of commands. `sudo` is deliberately not stripped, so sudo-prefixed commands stay unclassified and pass through unrewritten. It handles:
- Unquoted: `FOO=bar`
- Double-quoted with spaces: `FOO="bar baz"`
- Single-quoted: `FOO='bar baz'`
- Escaped quotes: `FOO="he said \"hello\""`
- Chained: `A="x y" B=1 env git status`

The prefix is stripped twice: once in `classify_command()` to match the underlying command against rules, and again in `rewrite_segment()` to extract it for re-prepending to the rewritten command.

## Adding a New Rewrite Rule

Add an entry to `rules.rs`. Each rule has:
- `pattern` — regex that matches the command (used by `RegexSet` for fast matching)
- `rtk_cmd` — the RTK command it maps to (e.g., `"rtk cargo"`)
- `rewrite_prefixes` — command prefixes to replace (e.g., `&["cargo"]`)
- `category`, `savings_pct` — metadata for discover reports
- `subcmd_savings`, `subcmd_status` — per-subcommand overrides

No other files need to change. The registry compiles the patterns at first use via `LazyLock`.
