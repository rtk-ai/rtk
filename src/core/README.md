# Core Infrastructure

> See also [docs/contributing/TECHNICAL.md](../../docs/contributing/TECHNICAL.md) for the full architecture overview

## Scope

Domain-agnostic building blocks that name **no third-party tool, hook, or agent**: a module that references "git", "cargo", "claude", or any other external tool by name does not belong here. rtk's own command line (`rtk`, `rtk proxy`) may appear, because rtk is not a third-party tool. Core is a leaf in the dependency graph — it is consumed by all other components but imports from none of them.

Owns: bash's reserved words, shell command-line lexing, configuration loading, token tracking persistence, TOML filter engine, tee output recovery, display formatting, explicit shell/direct command construction, telemetry, and shared utilities.

Does **not** own: command-specific filtering logic (that's `cmds/`), hook lifecycle management (that's `src/hooks/`), or analytics dashboards (that's `analytics/`).

## Purpose
Core infrastructure shared by all RTK command modules. Every filter, tracker, and command handler depends on these modules. No inward dependencies — leaf in the dependency graph (no circular imports possible).

## Shell Lexer (`cmdline/lexer.rs`)

`cmdline/lexer.rs` is the first step of RTK's command parsing: raw string → quote/operator-aware tokens, words and segments. The second step, already-split argv → flags and values, is `arg_tokenizer.rs`. The lexer holds no rewrite rule and no classification logic, so every component that has to read a shell command builds on it instead of re-scanning.

| Function | Purpose | Used by |
|---|---|---|
| `tokenize(cmd)` | Full shell-syntax tokens: words, quotes, escapes, operators, pipes, redirects, shellisms, the `Sep` runs between words and each unquoted `Newline` | `discover/registry.rs` |
| `shell_split(cmd)` | Quote-aware split into argv-ready words (quotes stripped, escapes resolved) | `hooks/mod.rs::is_claude_hook_command`, `main.rs`'s `rtk proxy '...'`, `discover/registry.rs` |
| `split_for_permissions(cmd)` | Segments a compound command for the **permission gate** — deliberately the most conservative segmenter (see its doc comment for the full comparison table) | `hooks/permissions.rs::check_command_with_rules`, `discover/registry.rs` |
| `split_for_classify(cmd)` | Segments for classification only, where `read_grammar` reads command text, as the rewrite does — not safe for permission/security decisions | `discover/registry.rs::split_command_chain` |
| `contains_unattestable_construct(cmd)` | True for command/process substitution, quoting the lexer reads differently from bash (`$'\''`), or a file-target redirect — constructs the permission gate can't decompose and must never auto-allow | `hooks/permissions.rs::check_command_with_rules`, `hooks/decision.rs`, `hooks/hook_cmd.rs` (Codex payloads), `discover/mod.rs` |

There is one token type, `Token { kind, value, offset }`, and every `value` is a slice of the input: tokens tile it, so each byte belongs to exactly one token and the values concatenated give the input back. `Sep` carries the exact spaces and tabs between two words and `Newline` the `\n` it sits on. Bash ends a line at `\n` only, so a `\r` is an ordinary word byte and `\r\n` is the last byte of a word followed by a `Newline`. The lexer always emits both kinds; a consumer that reads a newline as a blank says so through its segment policy, not through a different lex. `Token::is_blank` covers both for consumers that only want words and operators.

Words split on bash's `$IFS` only: space, tab and newline. Vertical tab, form feed, carriage return and non-breaking space are word bytes, so `is_ifs`, `split_ifs`, `trim_ifs`, `trim_ifs_start` and `trim_ifs_end` stand in for `char::is_whitespace`, `split_whitespace` and `str::trim` everywhere a command line is read. The rule patterns in `discover/rules.rs` and the built-in filters' `match_command` spell a separator `[ \t\n]` and end a command word at `(?:[ \t\n]|$)`, never at `\s` or `\b` (a rule also ends one at a metacharacter glued to it, since classification reads segments with their redirects kept). A `Word` is a run of adjacent tokens that are not blanks, with its span, and `words(input, tokens)` is the one function that finds them. A blank inside word text, as `read_grammar` reads it, separates no words: `a=(x y)`, `@(a|b c)` and `${x:-a b}` are one word each; `shell_split` resolves each one's quotes and escapes. `split_ifs` splits text blind to quoting, so it finds words only in text that carries none, and `squeeze_blanks` joins its runs with single spaces for a caller matching a pattern written with single spaces. `tokenize_at` lexes a piece of a larger input with offsets into that input. A command ends where its last word does, and an escaped or quoted blank belongs to that word (`head\ ` names the program `head␠`), so the rewrite path finds a command's ends from its tokens, never with `trim_ifs`: `tokenize_trimmed` lexes a command line and returns it without the blanks at either end, with its tokens. `QuoteScan` is the one flat quote-state machine: the tokenizer, word resolution and every other quote-aware scan walk it.

`pub(crate)` scanning helpers are shared with the rewriter's own token walks, which must read quoting and substitutions exactly as the segmenter does: `QuoteScan`, `SubstitutionDepth`, `words`, `resolve_words`, `content_bounds`, `squeeze_blanks`, `tokenize_at`, `tokenize_trimmed`, `split_ifs`, `ansi_c_quote_defeats_lexer` and `redirect_has_file_target` (all used by `discover/registry.rs`); `words` also gives `discover/mod.rs` a command's first words, and `squeeze_blanks` also normalises both sides of `hooks/permissions.rs::command_matches_pattern`. A change to any of them changes where the rewriter and the permission gate see a word or a segment end.

The permission gate, discover/analytics classification, and rewrite all agree on where a command begins and ends. The gate's `segment(cmd, Policy::PERMISSIONS)` cuts a segment at every newline (and at an unquoted `\r`, which bash reads as a word byte, so that no command can hide behind one), descends into `$( )`, and excises redirects. `split_for_classify` and `rewrite_compound` both read the line through `read_grammar`, split where it reads an operator, a pipe, `&` or a subshell's bracket as command text, stay out of substitutions, and keep redirects. `split_for_permissions`'s doc comment carries the full comparison, and `discover/registry.rs`'s `segmenter_agreement` tests hold each remaining difference to a stated reason. Those differences still matter at the call site: the gate must never under-segment, because a segment it never sees is a command its rules never check, so don't reuse `split_for_classify` or `rewrite_compound`'s segmenting for a permission/security decision — use `split_for_permissions`.

Classification and the rewrite read more of bash's grammar than `segment` does, through `read_grammar`: inside `[[ … ]]` and an arithmetic command `(( … ))` nothing is a command, so their `&&`, `||`, `;` and brackets end nothing, and a `case` pattern is never a command, so its words are never rewritten and only an `esac` where a pattern or a reserved word would start closes the `case`. After a compound command's last word (`fi`, `done`, `esac`, `}`, `]]`, `)`, `))`) and after the name of a `for`, `select`, `function` or `coproc`, bash reads a reserved word and no command, so `if (true) then [[ … ]]` and `for x do (( … ))` hold expressions. Word text is one word's, never a command: an extglob group (a `(` right after a bare `!`, `@`, `*`, `?` or `+` in a word), an array literal (`NAME=(` or `NAME+=(`) and a `${ }`, each read to the bracket that closes it (a `${ }` to its first `}`, as bash's `parse_matched_pair` reads it), in a command, a `case` pattern or a `[[ ]]` expression alike, so nothing inside one is rewritten and nothing inside one ends a command, a pattern or an expression. `time` is reserved only where a pipeline starts: after `|`, `|&` or `coproc` it is the program `time`. A substitution holds a command list of its own, which `read_grammar` reads to the `)` that ends it, past a `)` that a `case` pattern or a subshell inside it takes; `SubstitutionDepth`, which the segmenter and the rewrite's own walk share, only counts brackets. A `((` where a command or a reserved word starts, or after `for`, is arithmetic when the `)` that closes its second `(` is followed at once by another, as bash's `parse_dparen` decides; otherwise it is two subshells, one inside the other (`((ls) )`), and with no such `)` at all the rest of the line is read as the expression. `segment` keeps its private `CaseTracker`, which only tells a pattern's opening `(` from a subshell, and splits at every `&&`, `||` and `)` inside those constructs: a segment the gate checks that runs nothing costs a rule check, never a missed command.

## Bash reserved words (`cmdline/bash_grammar.rs`)

`RESERVED_WORDS` is bash's own grammar, not tool data: one row for each of the 22 words bash 5.3 lists under "Reserved Words" (`compgen -k`), giving where the word after it stands (`next`: where a command starts, as after `then`; where only a reserved word starts, as after `fi`, `done`, `esac`, `}` and `]]`; or a plain word, as after `case` or `for`), whether that word may be a name after which only a reserved word starts (`names`: `for`, `select`, `function`, `coproc`), the options bash reads right after it (`options`: `time -p --`, each optional, in that order) and its `Role` in the compound command it belongs to. `pipeline_follows` says whether a pipeline, and not only a command, may start after it: bash reads `time` as reserved only there, so not after `coproc`. A word `Opens` one (`if`, `case`, the loops, `[[`, and `function` and `coproc`, whose body is the command after them), `Continues` it into its next part (`then`, `elif`, `else`, `do`, `in`), `Closes` it (`fi`, `esac`, `done`, `}`, `]]`), or is a pipeline `Prefix` that belongs to none (`time`).

A reader that has to know whether a word is reserved, or what it does, calls `reserved_word(text)` rather than keeping a list of its own. The lookup matches the word as written, so a quoted or escaped spelling (`"if"`, `\if`) is an ordinary word, as bash reads it. Bash reads a reserved word only where a command or a reserved word could start (and `in`/`do` after a `case`, `for` or `select` header), so a caller looks up a word only in such a position. A test derives every row's role from the productions of bash's `parse.y`.

Its readers: the segmenter's `CaseTracker`, which follows `case … in … esac`; `read_grammar`, which finds where a command or a reserved word starts, the `[[ … ]]` expressions and the `case` patterns from the words' positions, roles and options; `discover/registry.rs`'s line classifier, where a line starting with a word that `ReservedWord::delimits_command_list` passes the whole block through; and `classify_command`, where a segment that `starts_with_grammar` (a reserved word, or the `((` of an arithmetic command, as `read_grammar` reads them, at the `CommandStart` the segment stands at, so that after `|` `time` is a program) is `Ignored`, so the rewrite never puts `rtk` in front of one.

## rtk's own command line (`cmdline/rtk.rs`)

`rtk_invocation(words)` is the one answer to "does this command run rtk?": `Some` when the first word is `rtk` once its quotes and escapes are removed (`resolve_word_text`, as bash reads the word before looking the command up), so `'rtk'` and `\rtk` run rtk as well. Its `proxy` field says the second word, read the same way, is `proxy`. The rewriter returns such a command as it is unless an operator joins another command to it, classification ignores it, and `discover` and `rtk session` count it as adoption unless it is `rtk proxy`.

## Segment edits (`cmdline/edit.rs`)

An `Edit` is one change to a command line: `Insert { at, text }` puts text in front of the byte at `at`, and `Replace { span, text }` replaces the bytes in `span`. `apply_edits(line, edits)` applies them in one pass and copies every byte outside an edit as written, so a caller edits only what it decided and never rebuilds the rest. The edits come in position order, and `apply_edits` returns `None` for an overlapping, reversed, out-of-range or off-boundary span rather than panic, so a bad edit leaves the line unchanged.

## TOML Filter Pipeline

The TOML DSL applies 8 stages in order:

1. **strip_ansi**: Remove ANSI escape codes if enabled
2. **replace**: Line-by-line regex substitutions (chainable, supports backreferences)
3. **match_output**: Short-circuit rules (if output matches pattern, return message; `unless` field prevents swallowing errors)
4. **strip/keep_lines**: Filter lines by regex (mutually exclusive)
5. **truncate_lines_at**: Truncate each line to N chars (unicode-safe)
6. **head/tail_lines**: Keep first N or last N lines (with omit message)
7. **max_lines**: Absolute line cap applied after head/tail
8. **on_empty**: Return message if result is empty after all stages

Three-tier filter lookup (first match wins):
1. `.rtk/filters.toml` (project-local, requires `rtk trust`)
2. `~/.config/rtk/filters.toml` (user-global)
3. Built-in filters concatenated by `build.rs` at compile time

## Source File Comment Stripping

`src/core/filter.rs` is a separate engine from the TOML DSL: it filters *source
files* (used by `rtk read`) rather than command output. At `-l minimal` it
strips comments using the per-language delimiters in
`Language::comment_patterns()`.

Python does not use that walk. It has no block comments — `"""` opens a
*string*, which may be a docstring or an ordinary value — so it gets a
string-aware path that removes `#` comments and leaves string contents alone.
Matching `"""` as a block delimiter misread both of these:

```python
QUERY = """          # contains """ without starting with it
SELECT 1
"""

"""Module doc."""    # opens and closes on one line
```

Docstrings are kept at `minimal`. `aggressive` has no string awareness: it
keeps a line inside a string when that line looks like an import or a
signature.

## Tracking Database Schema

```sql
CREATE TABLE commands (
  id INTEGER PRIMARY KEY,
  timestamp TEXT,              -- UTC ISO8601
  original_cmd TEXT,           -- "ls -la"
  rtk_cmd TEXT,                -- "rtk ls"
  project_path TEXT,           -- cwd (for project-scoped stats)
  input_tokens INTEGER,        -- estimated from raw output (bytes / 4, no tokenizer)
  output_tokens INTEGER,       -- estimated from filtered output (bytes / 4)
  saved_tokens INTEGER,        -- input - output
  savings_pct REAL,            -- (saved / input) * 100, i.e. reduction in bash output bytes
  exec_time_ms INTEGER         -- elapsed milliseconds
);

CREATE TABLE parse_failures (
  id INTEGER PRIMARY KEY,
  timestamp TEXT,
  raw_command TEXT,
  error_message TEXT,
  fallback_succeeded INTEGER   -- 1=yes, 0=no
);
```

Project-scoped queries use GLOB patterns (not LIKE) to avoid `_`/`%` wildcard issues in paths.

## Config Sections

```toml
[tracking]
enabled = true
history_days = 90
database_path = "/custom/path/to/tracking.db"  # Optional

[display]
colors = true
emoji = true
max_width = 120

[retriever]
mode = "sqlite"             # sqlite (default) | tee (legacy files) | disabled
max_entry_bytes = 10485760  # sqlite: 10 MiB per entry
max_entries = 200           # sqlite: FIFO cap
retention_days = 30         # sqlite: 0 disables age eviction
compression = true          # sqlite: gzip blobs (lossless)
# database_path = "/custom/recall.db"
tee_max_files = 20          # tee mode: rotation
tee_max_file_size = 1048576 # tee mode: per-file cap
# tee_directory = "/custom/tee/dir"

[telemetry]
enabled = true

[hooks]
exclude_commands = ["curl", "playwright"]  # Never auto-rewrite these

[limits]
grep_max_results = 200
grep_max_per_file = 25
status_max_files = 15
status_max_untracked = 10
passthrough_max_chars = 2000
```

## Shared Utilities (utils.rs)

Key functions available to all command modules:

| Function | Purpose |
|----------|---------|
| `truncate(s, max)` | Truncate string with `...` suffix |
| `strip_ansi(text)` | Remove ANSI escape/color codes |
| `resolved_command(name)` | Find command in PATH, returns `Command` |
| `tool_exists(name)` | Check if a CLI tool is available |
| `detect_package_manager()` | Detect pnpm/yarn/npm from lockfiles |
| `package_manager_exec(tool)` | Build `Command` using detected package manager |
| `ruby_exec(tool)` | Auto-detect `bundle exec` when `Gemfile` exists |
| `count_tokens(text)` | Estimate tokens: `ceil(chars / 4.0)` |

## Argument Tokenizer (arg_tokenizer.rs)

Shared classifier for an already-`--`-restored args slice (see `args_utils::restore_double_dash`) into flags, their values, and positionals. `tokenize_grammar(args, &GRAMMAR)` reads it under one tool's `Grammar`: the list of which flags take a value is per-tool, but the token-walking around it isn't. `tokenize(args)` is the structural-only entry point, for a caller asking which arguments are flags and where `--` is; it assumes **no flag takes a value**, so anything reading a value or counting free positionals needs `tokenize_grammar`.

**Use it for anything that decides what an argument is.** A new command filter, a new flag on an existing one, or a fix to how one is detected goes through `tokenize_grammar` — not `starts_with('-')`, not `args.iter().any(|a| a == "--flag")`. Those miss exactly what this module exists for: a flag's own value (`git log --grep -p` searches for the string "-p"), an attached value (`--flag=v`, `/bl:x`), a short cluster (`-rn`), and everything past `--`. Every bug the migration fixed was one of those four.

**A grammar is declared once, as data.** A tool's flags are `const` tables of `Flag`s (`Flag::short("n")`, `Flag::long("grep")`, `Flag::pair("u", "set-upstream-to")`, each `.takes(spec)` when it takes a value and bare when it takes none), and its `Grammar` lists those tables. A `Flag` has at most one short and one long spelling, paired as the tool pairs them, and a further spelling of the same kind is a `Flag` of its own (git's `-u` beside `-p`/`--patch`, grep's `--silent` beside `-q`/`--quiet`). A grammar declares every flag that takes a value, since those decide how the next argument tokenizes, and every flag of the tool that a handler checks tokens for, booleans included; a boolean takes no value, so declaring one leaves the tokens as they were. `tokenize_grammar` looks each flag token up in the grammar once and keeps the declared `Flag` in `Token::flag`, so code that asks whether a token is a declared flag, or how it takes a value, reads the token: `token.flag` for the declared `Flag`, `token.value_spec()` for its `ValueSpec`. A check for one flag compares that `Flag` with the flag's `const` (`token.is(&SET_UPSTREAM_TO)` matches `-u` and `--set-upstream-to` alike), and a check for a set of flags looks it up in a slice of those `const`s (`token.is_one_of(LOG_RAW_SHAPES)` for git, `GREP_SHAPES` for grep), so each spelling and each pairing is written once, and a token asked several questions is looked up once. Both compare flags by their spellings, which name one declaration per grammar, and every flag they are asked about must be declared in the grammar the token was read with, exactly as declared: a debug build panics on a flag the grammar does not declare or a copy that differs from the declaration in one field, which would otherwise never match and say nothing, and a release build answers `false`. So a predicate asking for a flag that only some grammars declare either takes tokens whose type names the grammar (git's `LogTokens` and `DiffTokens`) or states which grammar its tokens are read with. A `Short` token only matches a flag's short spelling and a `Long` token only its long one, so `-u` and `--u` never meet. The lookups over tokens (`has_flag`, `double_dash_flag_value`, …) take the grammar the tokens were read with and read only its naming rules (exact, or ASCII case folded under MSBuild), so the name they look for need not be declared.

Four rules, each of which cost a real bug before it was written down:

- **One grammar per tool and subcommand.** Never reuse a sibling's grammar wholesale because it looks close enough — `-u` means `-p` in `git log` but `--include-untracked` in `git stash show`, and `-T` is `--initial-tab` in grep but `--type-not` in rg. Transcribe from the tool's own `--help` and verify against the real binary; grep and rg share 13 of ~50 value-taking flags, so one merged table is wrong for both. Subcommands whose flags go through the same parser share one grammar (`git log` and `show` read theirs with `LOG_GRAMMAR`), and what distinct grammars share is a table: a subcommand after which every parent flag stays valid lists the parent's table next to its own (golangci-lint's `run` lists the global table), and grammars that differ in one flag list the shared tables and a table each for that flag (`git diff`'s `DIFF_GRAMMAR` lists the revision-walk tables `LOG_GRAMMAR` lists, with `--quiet` alone where `log` and `show` pair it with `-q`, and `git stash show`'s lists them after its own `-u`/`--include-untracked`, where the other two read `-u` as `-p`). Grammars that merely intersect get a table each.
- **Scope the lookup to the region the tool parses.** Everything past `--` is a pathspec or an argument forwarded to another program — `before_dashdash` gives the tool's own tokens. An MSBuild grammar keeps classifying past the boundary (it forwards rather than ending option parsing), which makes this explicit slice mandatory there, not optional.
- **Inject before the boundary.** RTK's own flags go at `injection_point`, never appended: dotnet parks anything after `--` in UnparsedTokens and git reads it as a pathspec, so an appended `--verify-no-changes` silently does nothing.
- **Detect and act with one rule.** Strip or inject using the detected token's `source_index`; re-matching the text lets the two disagree, which deleted a pathspec named `--no-compact` and swallowed a forwarded `--write`.

A flag's value is a `ValueSpec`, not a `bool`: one table per tool, answering every question the tokenizer has about a flag's value rather than one list per question, which is how two lists start drifting apart.

- `ValueSpec::value()` — `--flag=v` or `--flag v`, and a literal `--` is the boundary. The common case.
- `ValueSpec::attached_only()` — `--flag=v` only, the next argument is never the value (git's `-M`/`-U`/`-C`/`-B` take an optional attached number and nothing else).
- `ValueSpec::solo_only()` — a `Short` flag takes a separate value only when it is the whole argument: `git log -n 2` does, `git log -pn 2` does not. A `Flag` records the spec per spelling, and a long spelling is always the whole argument, so `Flag::pair("c", "config").takes(ValueSpec::solo_only())` records `solo_only()` for `-c` and `value()` for `--config`; `token.value_spec()` reads that one record. A flag with no short spelling declares `value()`: solo-only there panics in `Flag::takes`, which is a compile error in the `const` that declares the flag.
- `.claiming_dash_dash()` — lets a literal `--` be this flag's value. A per-tool split, not per-flag: grep and rg let any value-taking flag swallow it, git and cargo reject it whichever flag is asking.

The rest of a grammar is per tool, not per flag, and is fixed by the constructor, so a grammar cannot mix one dialect's naming with another's value attachment. `Grammar::posix(tables)` reads getopt: `-xyz` is a cluster of short flags, a value-taking one taking the rest of the cluster or the next argument, and an all-digit `-20` is one numeric token. `Grammar::msbuild(tables)` reads the MSBuild/dotnet dialect: `-flag`, `--flag` and `/flag` are all one `Long` token, a value attaches with `=` or `:`, and names fold ASCII case, so its flags are declared with a long spelling only; a short spelling panics in `Grammar::msbuild`, which is a compile error in the `const` that declares the grammar.

Each grammar has a test that checks it against an explicit table of the flags it declares and the spec each takes (`None` for a boolean flag), under every token kind, with `arg_tokenizer::assert_takes_value_table`. The same check fails on a spelling declared twice (compared under the grammar's naming rules, so case-folded under MSBuild): the first declaration wins, so a second one is dead data.

## Consumer Contracts

Core provides infrastructure that `cmds/` and other components consume. These contracts define expected usage.

### Command Construction (`shell`)

Use `shell::direct_command()` when the caller already has an argv vector. It
preserves argument boundaries and never expands globs, variables, redirects,
or operators. Use `shell::shell_command()` only for an intentional command
string, with an explicit shell when syntax is shell-specific. The platform
default remains `sh -c` on Unix and `cmd /C` on Windows for compatibility.

Never infer the command parser from `$SHELL`: agent hosts and terminal wrappers
can execute a different shell while preserving the user's login-shell value.
Callers that accept `--shell` must require the complete script as one quoted
argument instead of reconstructing it by joining parsed argv.

### Tracking (`TimedExecution`)

Consumers must call `timer.track()` on **all** code paths — success, failure, and fallback. Calling `std::process::exit()` before `track()` loses metrics. The raw string passed to `track()` should include both stdout and stderr to produce accurate savings percentages.

### Output recovery (`tee_and_hint` + recall store)

Consumers that parse structured output (JSON, NDJSON, state machines) should call `tee::tee_and_hint()` to persist raw output for LLM recovery on failure. It must be called before `std::process::exit()`.

For truncation recovery on **success** (e.g. a list capped at 20 items), use `tee::force_tee_hint()` (multi-line blocks) or `tee::force_tee_tail_hint(content, slug, offset)` (flat lists). All three persist the full output to the content-addressed recall store ([`retriever.rs`](retriever.rs)) and emit a runnable hint — `[full output: rtk recall <hash>]` or `[+N hidden: rtk recall <hash>]` — instead of burning tokens working around missing data.

The agent runs `rtk recall <hash>` to get back exactly what was elided. For `force_tee_tail_hint`, `offset` is the 1-based first hidden line (`header_lines + MAX_CAP + 1`); it is stored so the default recall returns only the hidden tail. Storage is byte-faithful (`BLOB` + lossless gzip); tune limits via the `[retriever]` config section.

### Truncation Caps (`truncate`)

`src/core/truncate.rs` defines four global cap policies — `CAP_ERRORS`, `CAP_WARNINGS`, `CAP_LIST`, `CAP_INVENTORY` — for the data classes RTK filters truncate. Each filter binds the right CAP to a local `const MAX_*` so the cap is one named jump away from the call site. These CAPs are the staging point for filter-level cap configuration (planned, not yet implemented): once the config surface lands, overriding `CAP_LIST` in `~/.config/rtk/config.toml` will tune every list filter in one place instead of editing 20+ files.

**Config policy.** Configured values are accepted as-is, including `0`, which means "summary only" — the filter still prints the count and the `[full output: …]` recovery hint, just no individual items. Caps are never refused and rtk never aborts on them, in keeping with the never-block-the-user fallback philosophy.

**Deviating from a cap.** A filter whose items are unusually verbose (multi-line entries, backtraces) may show fewer than its class cap. Use `truncate::reduced(cap, by)` rather than a bare `cap - by`: `reduced` returns `cap - by`, except when the reduction would empty the list (`by >= cap`), in which case it drops the deviation and uses the full `cap`. This guarantees a deviation can never hide every item, and — crucially — stays a `usize`-underflow-safe `const fn` once caps become runtime-configurable (a bare `CAP_WARNINGS - 5` would panic or wrap to "no truncation" if a user set `CAP_WARNINGS` below `5`). Never deviate with a bare literal or with `*`/`/` (those scale unboundedly). Each deviation needs a one-line comment stating why.

## Adding New Functionality
Place new infrastructure code here if it meets **all** of these criteria: (1) it has no dependencies on command modules or hooks, (2) it is used by two or more other modules, and (3) it provides a general-purpose utility rather than command-specific logic. Follow the existing pattern of lazy-initialized resources (`LazyLock` for regex, on-demand config loading) to preserve the <10ms startup target. Add `#[cfg(test)] mod tests` with unit tests in the same file.
