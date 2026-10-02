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

**Compound splitting** — The rewrite engine walks the tokens, splitting on `Operator` (`&&`, `||`, `;`) and typed `Pipe` tokens (`|`, `|&`). For normal pipelines, intermediate stages stay raw. A final stage whose rule's `pipeline_safety` allows it (ordinary `grep` and `rg` invocations) is rewritten; search pattern-file forms (`-f`/`--file`) defer because they can consume pipeline stdin as configuration. The producer stage is rewritten when its rule's `pipeline_safety` allows it and every downstream stage is a display-only consumer (`cat`, `head`, non-following `tail`) with no file-target redirect (#3171). Stderr pipelines (`|&`) and pipelines containing opaque shell groups remain raw. A compound command whose parts are command lists and that a reserved word closes (those `ReservedWord::opens_command_lists` opens), when it is a pipeline's stage, with a pipe right before its opening word or right after its closing word and its redirections, writes its output into that pipe or reads its input from it, so every command in any of its lists is left as written; `lexer::read_nesting` finds those compounds, and in a block of lines one that a line opens and a later one closes spans the lines between. A pipeline inside one of its lists is read like any other.

**Per-segment rewriting** — The line is lexed once, into a `CompoundLex`, and each line of a block, segment and pipeline stage is a range of those tokens, from its first word to the end of its last: an escaped or quoted blank belongs to that word, so `head\ ` names the program `head␠`. A walk over a segment peels its transparent layers left to right on those same tokens: an env run (`FOO="bar baz"`), a built-in wrapper (`noglob`, `uv run`), a process wrapper (`timeout 5`), a user prefix. A user prefix is matched against whole words only: an entry that would end inside a word (`x 'a`) is refused, so the command after a prefix is always read from the same lex.

The command under the layers is then decided on. Its trailing redirects (`2>&1`, `>/dev/null`) are left out of the match. `head -20 file` and `tail -n 5 file` short-circuit to `rtk read file --head-lines 20` and `rtk read file --tail-lines 5`, because generic prefix replacement would produce `rtk read -20 file` (wrong flag position). Anything else is classified: env prefixes looked past, paths normalized (`/usr/bin/grep` → `grep`), git global opts stripped (`git -C /tmp` → `git`), then matched against 60+ regex patterns from `rules.rs`, and the matching rule's prefix becomes `rtk <cmd>`.

Each decision is one edit against the line (`core/cmdline/edit.rs`): `Replace` renames the decided command's span, and `Insert` puts `rtk ` in front of it. Every other byte is kept as written, so the layers, the redirects, the operators and the blanks between them come out as the author wrote them. The exception is `golangci-lint run`, whose edit runs to the end of the command and drops its trailing redirects: `golangci-lint run ./... 2>&1` becomes `rtk golangci-lint run ./...`.

**Guards along the way:**
- `RTK_DISABLED=1` in the assignment run → skip rewrite and warn (see [The RTK_DISABLED Warning](#the-rtk_disabled-warning))
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

## Assignment Runs

`FOO=bar`, `FOO="bar baz"`, `FOO='bar baz'` and `FOO="he said \"hello\""` are words of the command's own lex, so the walk (next section) takes each whole, quoted value included. A run of them, with `env` words in any mix (`A="x y" B=1 env git status`), is one layer, because bash reads it as one environment for one command. A `NAME=value` word is an assignment only where a simple command starts: behind a wrapper that runs a program, it is that program's name, and the rewrite leaves the command as written. `sudo` is deliberately not a family, so sudo-prefixed commands stay unclassified and pass through unrewritten.

`classify_command()` looks past the run to match the underlying command against rules, and the rewrite's walk peels it as a layer. Only the command after it is edited, so the run stays exactly as written.

## Wrapper Families

The tables live in `discover/families/`, one file per family, each a plain data table with its own accept/reject test: `env.rs` (the `env` run word), `uv_run.rs`, `precommand.rs` (`noglob`, `command`, `builtin`, `exec`, `nocorrect`), `process.rs` (`timeout`, `time`, `nice`, `nohup`) and `user.rs` (how a configured prefix is read). `ALL` lists the built-in families and is what the walker is handed. Each family's `kind` is a `LayerKind` the rewrite reads back:

- `Assignments`, `ReservedWord`: the layers the walker finds from bash's own grammar, with no family of their own (`env` is a run word labelled `Assignments`).
- `Precommand`: a word that runs its argument as the command it names. It is no program on its own (`rtk exec date` fails), so a command behind it that has no rewrite leaves the whole command as written.
- `RoutableWrapper`: a wrapper the rules also match whole (`uv run`), so a command behind it with no rewrite of its own falls through to the wrapper's rule.
- `ProcessWrapper`: the command behind it keeps its process identity, so `rtk` goes inside: `timeout 300 cargo test` becomes `timeout 300 rtk cargo test`.
- `UserPrefix`: an entry of `transparent_prefixes`.

A family's `Form` picks the walker's tier it is tried in, never its place in `ALL`; within a tier the first match wins. To add a wrapper, declare its `Family` in a file of `families/`, give it an accept/reject test through `peel_text(.., ALL, ..)`, and list it in `ALL`: the walker changes for none of them. A process wrapper's options are read with its own `Grammar`, and its operands are counted (`timeout`'s duration). An option the grammar does not declare drops the rewrite, because an undeclared option may consume the following word and make the wrong token look like the command. Shell syntax before the command (a redirect, a subshell, a glob) does the same, because the wrapper's argv cannot be read off the token list. A wrapper that already has a literal `rtk` word among its own words is no layer, since it wraps a command that runs rtk.

`stdbuf` is deliberately not a wrapper here: it exists to make the wrapped
command emit output incrementally, and routing through rtk buffers that output
until the child exits, so rewriting it would remove the only reason to type it.

Wrapping also changes who receives a signal. `timeout 300 rtk cargo test`
signals rtk rather than cargo, so `core::stream` relays SIGINT/SIGTERM to the
child and lets the normal filter-and-print path finish. Without that relay a
killed run prints nothing at all, which is strictly worse than the unwrapped
command.

## The Walk

`Walk::run` peels a command's layers left to right over the words and tokens of its one lex. A `Line` is the command's text with its words and tokens, each layer is a `Layer` holding the index of its first word, the index of the first word after it, its kind and the `Position` the walk stands at behind it, and everything the rewrite does with a layer is a span of that text. At each step the walker's tiers are tried in order (assignments, reserved words, keywords, wrappers, user prefixes) and the built-in tiers win whenever they match, so a reserved word is never a user prefix's to claim.

The walk ends with a `Stop` that says why:

- `Command`: at the command word, which runs to the end of the line, or past every word.
- `Nothing`: at a layer with nothing after it, which is no more the command than a wrapper of one.
- `Disabled`: at an assignment run that sets `RTK_DISABLED` (by that exact name) with a command behind it. An assignment run with no command behind it (`RTK_DISABLED=1; git status`) is a statement of its own and disables nothing.
- `Depth`: `MAX_PREFIX_DEPTH` layers deep with the command unseen.

`decide` then answers for the command under the layers: `Keep` (it already runs rtk), `Rewrite(edit)`, or `Excluded` (it is one `exclude_commands` names, as written, without its absolute path, or as the tool a rule routes it to: `python -m pytest` is `pytest`). Exclusion is decided before anything that depends on the context, so an excluded command is excluded wherever it sits. A first word that is an assignment or an option (a flag a wrapper does not declare, or `--`) names no command and decides nothing. A word behind a subshell's closing `)` in the same simple command is no command either, and `rewrite_compound` leaves it as written; a `case` pattern's `)` is not one.

## Fall-through

When `decide` settles nothing about the command a walk ends at, `Search::fall_back` reads the layers again, innermost first, each at the position and depth where it is peeled. A user prefix that starts at the layer is tried first, whatever the layer's kind, except at a reserved word, which is bash's and no prefix takes: `transparent_prefixes = ["nice -n 19 ionice"]` rewrites `nice -n 19 ionice git status` although the command under `nice`'s own peel, `ionice git status`, has no rewrite. The prefix is the answer, its own walk is searched the same way, and the layer is not also matched whole. A prefix that ends inside the words of a layer it covers, or that leaves nothing behind it, is skipped. Every layer any walk produces is a candidate, and a `(word, depth)` reached a second time is skipped, which keeps a `uv run uv run ... uv run` chain linear in its depth.

When no prefix matches, only a `RoutableWrapper` layer may be matched whole by a rule, and only when `fallthrough_refused` allows it. It refuses when the word right behind `uv run` is an assignment (a program's name there, which a whole-string match would hand to `uv run` as its command), when a walk of what the wrapper wraps stops at `RTK_DISABLED=`, and, under `exclude_commands`, when the command it reaches is `Decision::Excluded` or out of sight (`uv run timeout 5 pytest` is refused as `uv run pytest` is). An option right behind the wrapper that leaves the command unread (`uv run --with x pytest`) keeps the wrapper from being matched whole under exclusions while a user prefix that starts there is tried; a `--` is read through. Any other layer runs its command without changing which program runs, so a command with no rewrite of its own leaves it as written.

## The RTK_DISABLED Warning

A walk that ends at `Stop::Disabled`, or a fall-through search that reaches one, marks the command as refused on `RTK_DISABLED=`. The warning follows the walk's own stop: it is printed only when the command is refused, so a fall-through that wins draws none, and a line whose other commands were rewritten also warns about the refused one. `rewrite_command` runs the rewrite inside `deferred::capture`, prints the `RTK_DISABLED` warning first, and then the warnings held during the rewrite (configuration problems such as a refused prefix, then filter-trust messages). `rewrite_command_precompiled` prints nothing, as `rtk discover` asks it about commands from months of history.

## User Prefixes

`[hooks].transparent_prefixes` entries are configuration text, matched literally as words of the command they wrap: a prefix `foo bar` matches a command whose words start with `foo bar`, written with the same spacing, and ends where a word of the command ends. `normalize_transparent_prefixes` trims each entry with `trim_ifs` (bash splits words on space, tab and newline only, so a non-breaking space at either end stays), drops empty ones, orders the rest longest first so `docker exec mycontainer` wins over `docker`, and removes repeats. An entry that does not lex into whole words (`lexes_into_whole_words`: an unclosed quote or a trailing backslash) is refused with a warning, once per process that reads the configuration, because the quote it opens would run into the command after it and no command could be split where the entry claims to end. So is an entry that starts with a bash reserved word (`coproc`, `time`, `if`, ...), which bash reads as grammar where a command starts and never as a program's name. Behind a user prefix the walk stands at `AFTER_USER_PREFIX`: an assignment may follow, no pipeline starts.

## Adding a New Rewrite Rule

Add an entry to `rules.rs`. Each rule has:
- `pattern` — regex that matches the command (used by `RegexSet` for fast matching)
- `rtk_cmd` — the RTK command it maps to (e.g., `"rtk cargo"`)
- `rewrite_prefixes` — command prefixes to replace (e.g., `&["cargo"]`)
- `category`, `savings_pct` — metadata for discover reports
- `subcmd_savings`, `subcmd_status` — per-subcommand overrides

No other files need to change. The registry compiles the patterns at first use via `LazyLock`.
