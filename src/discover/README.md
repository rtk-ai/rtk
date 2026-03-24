# Discover — Claude Code History Analysis

> See also [docs/TECHNICAL.md](../../docs/TECHNICAL.md) for the full architecture overview

## Purpose

Scans Claude Code JSONL session files to identify commands that could benefit from RTK filtering. Powers the `rtk discover` command, which reports missed savings opportunities and adoption metrics.

Also provides the **command rewrite registry** — the single source of truth for all 70+ patterns used by every LLM agent hook to decide which commands to rewrite.

## Key Types

- **`Classification`** — Result of `classify_command()`: `Supported { rtk_equivalent, category, savings_pct, status }`, `Unsupported { base_command }`, or `Ignored`
- **`RtkStatus`** — `Existing` (dedicated handler), `Passthrough` (external_subcommand), `NotSupported`
- **`SessionProvider`** trait — abstraction for session file discovery (currently only `ClaudeProvider`)
- **`ExtractedCommand`** — command string + output length + error flag extracted from JSONL

## Dependencies

- **Uses**: `walkdir` (session file discovery), `lazy_static`/`regex` (pattern matching), `serde_json` (JSONL parsing)
- **Used by**: `src/hooks/rewrite_cmd.rs` (imports `registry::classify_command` for `rtk rewrite`), `src/learn/` (imports `provider::ClaudeProvider` for session extraction), `src/main.rs` (routes `rtk discover` command)

## Registry Architecture

`registry.rs` (2270 lines) is the largest file in the project. It contains:

1. **Pattern matching** — Compiled regexes in `lazy_static!` matching command prefixes (e.g., `^git\s+(status|log|diff|...)`)
2. **Compound splitting** — `split_command_chain()` handles `&&`, `||`, `;`, `|`, `&` operators with shell quoting awareness
3. **RTK_DISABLED detection** — `has_rtk_disabled_prefix()` / `strip_disabled_prefix()` for per-command override
4. **Category averages** — `category_avg_tokens()` estimates output tokens when real data unavailable

The registry is used by both `rtk discover` (analysis) and `rtk rewrite` (live rewriting). Same patterns, different consumers.
