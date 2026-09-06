# Filtering Behavior Reference

This document describes how RTK transforms command output for each supported command. RTK filters, compresses, and restructures output to minimize LLM token consumption (typically achieving 60-90% savings).

> **Warning**: Filtering is lossy by design. If you need raw, unfiltered output, see [How to Get Raw Output](#how-to-get-raw-output) below.

## Quick Reference Table

| Command | What RTK Does | Data Lost/Transformed | Workaround |
|---|---|---|---|
| `rtk curl` | JSON responses converted to schema-only format; non-JSON truncated to 30 lines; lines >200 chars truncated | Full response bodies (values replaced with types); lines beyond 30th; characters beyond 200 per line | Use `rtk proxy curl`, or set `RTK_DISABLED=1` |
| `rtk grep` | Groups matches by file, shows `[file] path (count):` headers; truncates lines to `max_line_len` (default 150); limits total results to `grep_max_results` (default 200); limits per-file matches to `grep_max_per_file` (default 25) | Full line context beyond truncation point; matches beyond 200 total; per-file matches beyond 25 shown as `+N` | Use `rtk proxy grep`, or `RTK_DISABLED=1` |
| `rtk ls` | Converts `ls -la` output to compact format: dirs as `name/`, files as `name  size`; strips permissions, owners, dates; hides noise dirs (`node_modules`, `.git`, `target`, `__pycache__`, `.next`, `dist`, `build`, `.cache`, `.turbo`, `.vercel`) unless `-a`; shows file extension summary | File permissions, owners, group, timestamps, symlink targets (partially); noise directories; exact byte sizes (converted to human-readable) | Use `-a` flag to show noise dirs; use `rtk proxy ls` for raw output |
| `rtk read` | Strips comments and boilerplate based on filter level (`none`, `minimal`, `aggressive`); truncates large files; optionally shows line numbers | All comments (single-line and block) at minimal+ level; implementation bodies at aggressive level (replaced with `// ... implementation`); blank lines and formatting details | Use `--level none` to disable filtering; use `rtk proxy cat` for raw output |
| `rtk env` | Masks sensitive values (keys containing `key`, `secret`, `password`, `token`, `credential`, `auth`, `private`, `api_key`, `jwt`) showing only first/last 2 chars; truncates values >100 chars to 50-char preview; caps "Other" vars at 20; categorizes vars; shows PATH as split entries (first 5 + count) | Actual secret values (masked as `ab****yz`); characters beyond 50 for long values; "Other" environment variables beyond 20th | Use `--all` flag to unmask secrets; use `rtk proxy env` for raw output |
| `rtk cargo build` | Strips `Compiling`, `Downloading`, `Finished` lines; keeps errors, warnings, and final crate info | Individual compilation progress lines; download messages | Use `rtk proxy cargo build` for full output |
| `rtk cargo test` | Hides `test ... ok` lines and `running N tests`; aggregates passing suites into compact summary; shows only failures (truncated to 200 chars each, max 10); preserves compile errors | Individual passing test names; full failure stack traces beyond 200 chars; failures beyond 10th shown as `+N more` | Use `rtk proxy cargo test` or `rtk cargo test -- --nocapture` |
| `rtk cargo clippy` | Similar to build filtering: strips compiling lines, keeps warnings and errors; deduplicates and truncates long diagnostics | Full compiler backtraces; individual compilation progress | Use `rtk proxy cargo clippy` |
| `rtk cargo check` | Same filtering as `cargo build` | Same as build | Use `rtk proxy cargo check` |
| `rtk cargo install` | Strips dependency compilation lines; keeps `Installing`, `Installed`, `Replaced`, and error lines | All dependency build output; intermediate compilation status | Use `rtk proxy cargo install` |
| `rtk cargo nextest` | Similar to `cargo test` filtering | Same as test filtering | Use `rtk proxy cargo nextest` |
| `rtk git log` | Truncates to `max_lines`; formats commit entries compactly; can show short or long format | Full diff content; commit body beyond truncation limit | Use `rtk proxy git log` or `rtk git log --full` |
| `rtk git status` | Groups changes by status (modified, added, deleted, untracked); limits displayed files to `status_max_files` (default 15); limits untracked files to `status_max_untracked` (default 10) | File details beyond the displayed limits; full diff stats | Use `rtk proxy git status` |
| `rtk git diff` | Truncates to `max_lines`; focuses on changed hunks | Full file contents; changes beyond truncation limit | Use `rtk proxy git diff` |
| `rtk gt log` | Applies structured filtering to graphite/git-town style log output | Depends on short/long format selection | Use `rtk proxy gt log` |
| `rtk npm` | Filters install/output to show key results; strips verbose npm logs | Intermediate npm progress output; dependency tree details | Use `rtk proxy npm` |
| `rtk pnpm` | Parser-based filtering with tiered output; falls back to truncated passthrough (default 2000 chars) on parse failure | Full dependency resolution details; parse failures beyond `passthrough_max_chars` | Use `rtk proxy pnpm` |
| `rtk pytest` | Aggregates test results; shows failures compactly | Individual passing test output | Use `rtk proxy pytest` |
| `rtk vitest` | Parser-based with tiered filtering; passthrough fallback truncated to 2000 chars | Full test output beyond passthrough limit | Use `rtk proxy vitest` |
| `rtk playwright` | Parser-based with tiered filtering; passthrough fallback truncated to 2000 chars | Full test output beyond passthrough limit | Use `rtk proxy playwright` |
| `rtk tsc` | Extracts and structures TypeScript errors; truncates error messages to 120 chars; shows error code summary | Full compiler output; error context beyond 120 chars per line | Use `rtk proxy tsc` |
| `rtk ruff` | Formats lint output compactly | Full lint output details | Use `rtk proxy ruff` |
| `rtk mypy` | Filters type checking output | Full type checking details | Use `rtk proxy mypy` |
| `rtk next` | Filters Next.js build output; extracts route information (truncated to 30 chars) | Full build logs; route details beyond 30 chars | Use `rtk proxy next` |
| `rtk prisma` | Filters generate/migrate output compactly | Full Prisma CLI output | Use `rtk proxy prisma` |
| `rtk prettier` | Shows summary of formatting results; truncates file lists | Full list of checked files beyond summary | Use `rtk proxy prettier` |
| `rtk go test` | Filters Go test output | Individual passing test lines | Use `rtk proxy go test` |
| `rtk go build` | Filters Go build output | Compilation progress lines | Use `rtk proxy go build` |
| `rtk go vet` | Filters Go vet output | Full vet details | Use `rtk proxy go vet` |
| `rtk docker` | Filters container command output (ps, logs, etc.) | Full container details | Use `rtk proxy docker` |
| `rtk psql` | Filters SQL command output | Full query results beyond limits | Use `rtk proxy psql` |
| `rtk wget` | Filters download output | Full download progress | Use `rtk proxy wget` |
| `rtk rspec` | Filters Ruby test output | Individual passing test details | Use `rtk proxy rspec` |
| `rtk rake` | Filters Ruby task output | Full task execution details | Use `rtk proxy rake` |
| `rtk rubocop` | Filters Ruby lint output | Full lint details | Use `rtk proxy rubocop` |
| `rtk gh` | Filters GitHub CLI output | Full CLI response details | Use `rtk proxy gh` |
| `rtk dotnet` | Filters .NET build/test/restore output | Compilation and test progress | Use `rtk proxy dotnet` |
| `rtk golangci-lint` | Filters golangci-lint output | Full lint details | Use `rtk proxy golangci-lint` |

## Configuration Limits

These defaults are defined in `src/core/config.rs` and can be overridden via config file:

| Setting | Default | Description |
|---|---|---|
| `grep_max_results` | 200 | Maximum total matches shown by `rtk grep` |
| `grep_max_per_file` | 25 | Maximum matches shown per file by `rtk grep` |
| `status_max_files` | 15 | Maximum files shown in `rtk git status` |
| `status_max_untracked` | 10 | Maximum untracked files shown in `rtk git status` |
| `passthrough_max_chars` | 2000 | Maximum chars for parser passthrough fallback |

## Filter Levels (`rtk read`)

The `rtk read` command supports three filter levels:

| Level | Behavior | Tokens Saved | Best For |
|---|---|---|---|
| `none` | Returns file content unchanged | 0% | When you need exact content (config files, data) |
| `minimal` | Strips comments (single-line and block), keeps code structure, type definitions, constants, function signatures | 20-40% | General code reading, understanding structure |
| `aggressive` | Strips comments + collapses implementation bodies to `// ... implementation`; keeps only signatures, types, constants | 50-80% | High-level code overview, architecture review |

### Language Support

`rtk read` detects language from file extension and applies appropriate comment patterns:

| Language | Extensions | Comment Style |
|---|---|---|
| Rust | `.rs` | `//`, `/* */`, `///`, `/** */` |
| Python | `.py`, `.pyw` | `#`, `"""..."""` |
| JavaScript | `.js`, `.mjs`, `.cjs` | `//`, `/* */` |
| TypeScript | `.ts`, `.tsx` | `//`, `/* */` |
| Go | `.go` | `//`, `/* */` |
| C | `.c`, `.h` | `//`, `/* */` |
| C++ | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh` | `//`, `/* */` |
| Java | `.java` | `//`, `/* */` |
| Ruby | `.rb` | `#` |
| Shell | `.sh`, `.bash`, `.zsh` | `#` |
| Data (no comment stripping) | `.json`, `.yaml`, `.yml`, `.toml`, `.xml`, `.csv`, `.tsv`, `.graphql`, `.gql`, `.sql`, `.md`, `.txt`, `.env`, `.lock` | None — content returned as-is |

## How Filtering Works Internally

### Parser Tiers

For JS ecosystem commands (npm, pnpm, vitest, playwright), RTK uses a tiered parser system:

- **Tier 1-2**: Structured parsing — extracts specific fields, drops noise
- **Tier 3**: Partial parsing — extracts what it can, fills gaps
- **Fallback**: Passthrough with truncation to `passthrough_max_chars` (default 2000)

If parsing fails or exceeds the configured tier, output degrades to truncated passthrough automatically.

### Line Truncation

The `truncate()` utility (used across multiple commands) truncates strings to a character limit with `...` appended. Common limits:
- `rtk curl`: 200 chars per line
- `rtk cargo test` failures: 200 chars per failure
- `rtk tsc` errors: 120 chars per message/context line
- `rtk next` routes: 30 chars

### Secret Masking

`rtk env` masks values for any variable whose name contains (case-insensitive): `key`, `secret`, `password`, `token`, `credential`, `auth`, `private`, `api_key`, `apikey`, `access_key`, `jwt`.

Masking format: first 2 characters + `****` + last 2 characters. Values ≤4 chars show as `****`.

## How to Get Raw Output

### Option 1: Use `rtk proxy`

```bash
rtk proxy <command> [args...]
```

Executes the command without any filtering but still tracks usage metrics. This is the recommended way to get raw output while maintaining visibility in `rtk gain --history`.

```bash
rtk proxy curl https://api.example.com/data
rtk proxy git log --oneline -50
rtk proxy cargo test -- --nocapture
```

### Option 2: Exclude from Hook Rewrite

If using RTK's shell hook integration, you can exclude specific commands from being rewritten by adding them to the exclude list in your RTK configuration.

### Option 3: Use Native Command Directly

Bypass RTK entirely by calling the underlying command:

```bash
curl https://api.example.com/data    # instead of rtk curl
grep -rn "pattern" .                 # instead of rtk grep
cat file.rs                          # instead of rtk read file.rs
env                                  # instead of rtk env
```

### Option 4: `RTK_DISABLED=1` Environment Variable

Set `RTK_DISABLED=1` to disable RTK filtering for the current shell session:

```bash
export RTK_DISABLED=1
curl https://api.example.com/data    # Full output, no filtering
```

## When Filtering Breaks

### Common Issues

1. **Missing data in output**: RTK may have filtered out information you need. Check the Quick Reference table above to understand what each command drops.
2. **Truncated stack traces**: Test failures and build errors are truncated. Use `rtk proxy` to see full traces.
3. **Corrupted JSON**: If JSON output looks wrong, RTK may have incorrectly applied schema extraction. Report this as a bug.
4. **Masked secrets you need to see**: Use `rtk env --all` to show unmasked values.

### Reporting Filter Bugs

When reporting a filtering bug, include:

1. **Command run**: The exact `rtk <command> [args]` you executed
2. **Expected output**: What you expected to see (or a sample of raw output)
3. **Actual output**: What RTK produced
4. **Impact**: What information was lost and why it mattered
5. **RTK version**: Output of `rtk --version`

```bash
# Quick bug report template
rtk <command> [args] > /tmp/rtk-filtered.txt 2>&1
<command> [args] > /tmp/rtk-raw.txt 2>&1
diff /tmp/rtk-raw.txt /tmp/rtk-filtered.txt
```

Attach both files and the diff to your issue report.

### Known Filter Degradation

When RTK's structured parser cannot parse output (e.g., unexpected format from a new tool version), it falls back to truncated passthrough. A warning is emitted:

```
rtk: warning: output format changed, using passthrough (truncated to 2000 chars)
```

This means you're seeing partial output. Use `rtk proxy` as a workaround and file a bug report.
