# Learn — CLI Correction Detection

> See also [docs/contributing/TECHNICAL.md](../../docs/contributing/TECHNICAL.md) for the full architecture overview

## Purpose

Analyzes Claude Code session history to detect recurring CLI mistakes — commands that fail then get corrected by the agent. Powers the `rtk learn` command, which identifies error patterns (unknown flags, wrong paths, missing args) and can auto-generate `.claude/rules/cli-corrections.md` to prevent them.

## Key Types

- **`ErrorType`** — `UnknownFlag`, `CommandNotFound`, `WrongSyntax`, `WrongPath`, `MissingArg`, `PermissionDenied`, `Other(String)`
- **`CorrectionPair`** — Raw detection: wrong command + right command + error output + confidence score
- **`CorrectionRule`** — Deduplicated pattern: wrong pattern + right pattern + occurrence count + base command

## Dependencies

- **Uses**: `discover::provider::ClaudeProvider` (session file discovery and command extraction), `lazy_static`/`regex` (error pattern matching), `serde_json` (JSON output)
- **Used by**: `src/main.rs` (routes `rtk learn` command)

## Detection Algorithm

1. Extract all commands from JSONL sessions via `ClaudeProvider`
2. Scan chronologically for fail-then-succeed pairs (same base command, first has error output, second succeeds)
3. Classify the error type using regex patterns on the error output
4. Assign confidence scores based on similarity and error clarity
5. Deduplicate into rules (merge identical wrong->right patterns, count occurrences)
6. Filter by `--min-confidence` and `--min-occurrences` thresholds
