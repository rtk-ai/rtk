# Discover — History Analysis & Command Rewrite

> Full rewrite pipeline diagram: [docs/contributing/TECHNICAL.md](../../docs/contributing/TECHNICAL.md#32-hook-interception-command-rewriting)

## What This Module Does

This module has two jobs:

1. **Rewrite commands** — Every LLM agent hook calls `rtk rewrite "git status"`. This module decides whether to rewrite it (`rtk git status`) or pass it through unchanged. This is the hot path — every command the LLM runs goes through here.

2. **Analyze history** — `rtk discover` scans past LLM sessions to find commands that *could have been* rewritten but weren't. Same classification logic, different consumer.

## How Command Rewriting Works

When a hook sends `cargo fmt --all && cargo test 2>&1 | tail -20`:

**Tokenization** — The lexer (`core/cmdline/lexer.rs`) turns the raw string into typed tokens. It's a single-pass state machine that understands shell quoting, escapes, redirects, and operators. This is critical because naive string splitting breaks on quoted content like `git commit -m "fix && update"`.

```
"cargo test 2>&1 && git status"
→ [Arg("cargo"), Arg("test"), Redirect("2>&1"), Operator("&&"), Arg("git"), Arg("status")]
```

**Compound splitting** — The rewrite engine walks the tokens, splitting on `Operator` (`&&`, `||`, `;`, and the `case` terminators `;;`, `;&`, `;;&`) and typed `Pipe` tokens (`|`, `|&`). For normal pipelines, intermediate stages stay raw. A final stage whose rule's `pipeline_safety` allows it (ordinary `grep` and `rg` invocations) is rewritten; search pattern-file forms (`-f`/`--file`) defer because they can consume pipeline stdin as configuration. The producer stage is rewritten when its rule's `pipeline_safety` allows it and every downstream stage is a display-only consumer (`cat`, `head`, non-following `tail`) with no file-target redirect (#3171). Stderr pipelines (`|&`) and pipelines containing opaque shell groups remain raw.

**Per-segment rewriting** — The line is lexed once, into a `CompoundLex`, and each line of a block, segment and pipeline stage is a range of those tokens, from its first word to the end of its last: an escaped or quoted blank belongs to that word, so `head\ ` names the program `head␠`. A walk over a segment peels its transparent layers left to right on those same tokens: an env run (`FOO="bar baz"`), a built-in wrapper (`noglob`, `uv run`), a process wrapper (`timeout 5`), a user prefix. A prefix matched as text can end inside a token, as a user prefix `x 'a` ends inside the quote it opens; the command after it is then read from a fresh lex of its own text.

The command under the layers is then decided on. Its trailing redirects (`2>&1`, `>/dev/null`) are left out of the match. `head -20 file` and `tail -n 5 file` short-circuit to `rtk read file --head-lines 20` and `rtk read file --tail-lines 5`, because generic prefix replacement would produce `rtk read -20 file` (wrong flag position). Anything else is classified: env prefixes looked past, paths normalized (`/usr/bin/grep` → `grep`), git global opts stripped (`git -C /tmp` → `git`), then matched against 60+ regex patterns from `rules.rs`, and the matching rule's prefix becomes `rtk <cmd>`.

Each decision is one edit against the line (`core/cmdline/edit.rs`): `Replace` renames the decided command's span, and `Insert` puts `rtk ` in front of it. Every other byte is kept as written, so the layers, the redirects, the operators and the blanks between them come out as the author wrote them.

**Guards along the way:**
- `RTK_DISABLED=1` in the env prefix → skip rewrite (under a routable wrapper such as `uv run`, the wrapper itself is still rewritten: `uv run RTK_DISABLED=1 git status` becomes `rtk uv run RTK_DISABLED=1 git status`)
- `gh` with `--json`/`--jq`/`--template` → skip (structured output, rtk would corrupt it)
- `cat` with flags other than `-n` → skip (different semantics than `rtk read`)
- `cat`/`head`/`tail` with `>` or `>>` → skip (write operation, not a read)
- Command in `hooks.exclude_commands` config → skip

**Result**: `rtk cargo fmt --all && rtk cargo test 2>&1 | tail -20`. Bash handles the `&&` and `|` at execution time — each `rtk` invocation is a separate process.

## Shell Lexer

Command segmentation, word splitting and quote scanning live in `core/cmdline/lexer.rs`; see [core/README.md](../core/README.md#shell-lexer-cmdlinelexerrs) for what it provides and who uses it.

## How History Analysis Works

`rtk discover` reads Claude Code JSONL session files. Each file contains `tool_use`/`tool_result` pairs for every command the LLM ran. The module:

1. Extracts commands from the JSONL (via `SessionProvider` trait — currently only Claude Code)
2. Splits compound commands where the rewrite does: both read the line through `read_grammar`, so a `[[ ]]` expression, an arithmetic command and a `case` pattern hold no command of their own
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

`classify_command()` looks past the prefix to match the underlying command against rules, and the rewrite's walk peels it as a layer. Only the command after it is edited, so the prefix stays exactly as written.

## Process Wrapper Handling

A process wrapper runs another command without changing which command runs, so the rewrite peels it and edits only the command it wraps, leaving the wrapper's text as written: `timeout 300 cargo test` becomes `timeout 300 rtk cargo test`. `PROCESS_WRAPPERS` in `registry.rs` describes each wrapper's own arguments — options that take a value, options that do not, values that may be attached to their option, and any positional argument the wrapper consumes before the command (`timeout`'s duration).

Two rules keep the peeling honest. An option the table does not describe drops
the rewrite, because an unknown option may consume the following word and make
the wrong token look like the command. Shell syntax before the command (a
redirect, a subshell, a glob) does the same, because the wrapper's argv can no
longer be read off the token list.

`stdbuf` is deliberately not a wrapper here: it exists to make the wrapped
command emit output incrementally, and routing through rtk buffers that output
until the child exits, so rewriting it would remove the only reason to type it.

Wrapping also changes who receives a signal. `timeout 300 rtk cargo test`
signals rtk rather than cargo, so `core::stream` relays SIGINT/SIGTERM to the
child and lets the normal filter-and-print path finish. Without that relay a
killed run prints nothing at all, which is strictly worse than the unwrapped
command.

## Adding a New Rewrite Rule

Add an entry to `rules.rs`. Each rule has:
- `pattern` — regex that matches the command (used by `RegexSet` for fast matching)
- `rtk_cmd` — the RTK command it maps to (e.g., `"rtk cargo"`)
- `rewrite_prefixes` — command prefixes to replace (e.g., `&["cargo"]`)
- `category`, `savings_pct` — metadata for discover reports
- `subcmd_savings`, `subcmd_status` — per-subcommand overrides

No other files need to change. The registry compiles the patterns at first use via `LazyLock`.
