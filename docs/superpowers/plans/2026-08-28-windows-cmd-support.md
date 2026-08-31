# Windows CMD Support Implementation Plan

## Global Constraints

- Target Desktop Windows 10 and 11; non-Windows invocation returns a clear unsupported-platform error.
- Preserve CMD exit codes, stateful/control-flow semantics, stdout/stderr separation, encoding, redirections, and machine-consumed output before pursuing token savings.
- `rtk cmd <expression...>` defaults to `cmd.exe /D /S /C`; one argument is raw CMD syntax and multiple arguments are reconstructed with CMD-safe quoting.
- Native `/C` is normalized; `/K` and no-argument sessions are unfiltered passthrough.
- Parsing is span-preserving and CMD-specific: double quotes quote, single quotes do not, caret escapes operators, and `%VAR%`/`!VAR!` remain shell-expanded.
- Eligible noisy segments use a hidden RTK runner; unsafe, opaque, redirected, batch, or unsupported constructs fail open to native CMD.
- Every CMD built-in and alias has explicit catalog metadata and an adapter strategy; naturally terse or stateful operations may use a documented identity strategy.
- Lossy output is recoverable through tee artifacts and never exceeds raw output.
- Tests are written and observed failing before production changes.

## Task 1: CMD Parser and Built-in Catalog

Create a Windows command subsystem under `src/cmds/windows/` with a span-preserving CMD lexer/parser and a checked-in built-in catalog.

- Recognize top-level `&`, `&&`, `||`, pipes, redirections, parentheses, double quotes, caret escapes, `%VAR%`, delayed expansion, `@`, CRLF, drive changes, and batch/control constructs.
- Represent simple segments and their exact source spans so safe segments can be replaced without reformatting the rest of the expression.
- Classify output-consuming pipelines, output redirections, batch/control groups, delayed-expansion-sensitive input, and malformed input as opaque/fail-open.
- Catalog all intrinsic and extension commands reported by Desktop CMD, including aliases, stateful/control/query/mutation/interactive mode, and an explicit adapter strategy.
- Add coverage validation for duplicate aliases and missing strategies.
- Add table-driven parser/catalog tests first and run focused tests plus `cargo test --all`.
- Commit the task and write the SDD report.

## Task 2: Public CLI and CMD Orchestration

Add the public `rtk cmd` route and hidden segment execution route.

- Parse one raw expression or reconstruct multiple arguments with CMD-safe quoting.
- Execute through the resolved `cmd.exe` using `/D /S /C`; normalize leading `/C` case-insensitively.
- Preserve `/K` and no-argument interactive behavior as passthrough.
- Rewrite only eligible parser segments to the quoted absolute RTK executable plus the hidden runner; preserve operators and untouched source spans.
- Keep stateful/control built-ins in the parent shell and fail open for opaque input.
- Preserve exit codes and avoid double-counting compound orchestration in savings analytics.
- Add CLI parsing and Windows end-to-end tests first, including echo plus dir, stateful cd/set chains, `&&`/`||`, Unicode/spaces, variables, redirection, batch input, and failures.
- Commit the task and write the SDD report.

## Task 3: Built-in Adapters and Filters

Implement an explicit adapter for every CMD built-in and alias.

- Add structured filters for noisy display modes: `dir`, display-form `set`, `help`, `assoc`, and `ftype`.
- Give `dir` native CMD argument semantics for `/A`, `/B`, `/S`, `/O`, `/T`, wildcards, and paths; do not forward slash flags to `rtk ls`.
- Preserve exact requested content for `echo`, `type` when exact or binary, mutation forms, stateful/control commands, interactive modes, redirects, and downstream-consumed output.
- Use command-specific identity adapters with reasons for naturally terse or unsafe-to-filter commands.
- Prefer invariant/structured parsing; unknown locale/layout falls back to raw output.
- Ensure lossy filters use tee recovery, never-worse guarding, correct stdout/stderr, and native exit codes.
- Add fixtures for every adapter strategy and real Windows integration tests first.
- Commit the task and write the SDD report.

## Task 4: Manifest, Documentation, and Desktop Gates

Finish the first stable increment and prepare later command-catalog releases.

- Add a checked-in Desktop Windows 10/11 external-command manifest with availability and recognized/raw status for later incremental adapters; runtime/builds do not fetch the network.
- Add validation that every built-in and external catalog entry has unique names/aliases and an explicit strategy/status.
- Document `rtk cmd`, examples, built-in filtering, raw fallback behavior, interactive limitations, and the existing `rtk proxy cmd.exe` escape hatch.
- Add hosted Windows smoke coverage and self-hosted Windows 10/11 release-gate workflow jobs for parser, catalog, exit-code, and side-effect parity suites.
- Run formatting checks, clippy, the full test suite, Windows smoke tests, and representative end-to-end commands.
- Commit the task and write the SDD report.
