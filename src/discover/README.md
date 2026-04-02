# Discover — History Analysis & Command Rewrite

> Full rewrite pipeline diagram: [docs/TECHNICAL.md](../../docs/TECHNICAL.md#32-hook-interception-command-rewriting)

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

**Compound splitting** — The rewrite engine walks the tokens, splitting on `Operator` (`&&`, `||`, `;`) and `Pipe` (`|`). Each segment is rewritten independently. For pipes, only the left side is rewritten (the pipe consumer like `grep` or `head` runs raw). `find`/`fd` before a pipe is never rewritten because rtk's grouped output format breaks pipe consumers like `xargs`.

**Per-segment rewriting** — Each segment goes through:

1. Strip trailing redirects (`2>&1`, `>/dev/null`) — matched via lexer tokens, set aside, re-appended after rewriting
2. Short-circuit special cases — `head -20 file` → `rtk read file --max-lines 20`, `tail -n 5 file` → `rtk read file --tail-lines 5`. These can't go through generic prefix replacement because it would produce `rtk read -20 file` (wrong flag position)
3. Classify the command — strip env prefixes (`sudo`, `FOO="bar baz"`), normalize paths (`/usr/bin/grep` → `grep`), strip git global opts (`git -C /tmp` → `git`), then match against 60+ regex patterns from `rules.rs`
4. Apply the rewrite — find the matching rule, replace the command prefix with `rtk <cmd>`, re-prepend the env prefix, re-append the redirect suffix

**Guards along the way:**
- `RTK_DISABLED=1` in the env prefix → skip rewrite
- `gh` with `--json`/`--jq`/`--template` → skip (structured output, rtk would corrupt it)
- `cat` with flags other than `-n` → skip (different semantics than `rtk read`)
- `cat`/`head`/`tail` with `>` or `>>` → skip (write operation, not a read)
- Command in `hooks.exclude_commands` config → skip

**Result**: `rtk cargo fmt --all && rtk cargo test 2>&1 | tail -20`. Bash handles the `&&` and `|` at execution time — each `rtk` invocation is a separate process.

## How History Analysis Works

`rtk discover` reads Claude Code JSONL session files. Each file contains `tool_use`/`tool_result` pairs for every command the LLM ran. The module:

1. Extracts commands from the JSONL (via `SessionProvider` trait — currently only Claude Code)
2. Splits compound commands using the same lexer-based tokenization
3. Classifies each command against the same rules used for live rewriting
4. Aggregates results: which commands could have been rewritten, estimated token savings, adoption rate

The classification logic is shared between discover and rewrite — same patterns, same rules, different consumers.

## Env Prefix Handling

The `ENV_PREFIX` regex strips env variable assignments, `sudo`, and `env` from the front of commands. It handles:
- Unquoted: `FOO=bar`
- Double-quoted with spaces: `FOO="bar baz"`
- Single-quoted: `FOO='bar baz'`
- Escaped quotes: `FOO="he said \"hello\""`
- Chained: `A="x y" B=1 sudo git status`

The prefix is stripped twice: once in `classify_command()` to match the underlying command against rules, and again in `rewrite_segment()` to extract it for re-prepending to the rewritten command.

## Adding a New Rewrite Rule

Add an entry to `rules.rs`. Each rule has:
- `pattern` — regex that matches the command (used by `RegexSet` for fast matching)
- `rtk_cmd` — the RTK command it maps to (e.g., `"rtk cargo"`)
- `rewrite_prefixes` — command prefixes to replace (e.g., `&["cargo"]`)
- `category`, `savings_pct` — metadata for discover reports
- `subcmd_savings`, `subcmd_status` — per-subcommand overrides

No other files need to change. The registry compiles the patterns at first use via `lazy_static`.
