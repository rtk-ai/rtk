# Command Filter Modules

## Scope

**Command execution and output filtering.** Every module here calls an external CLI tool (`Command::new("some_tool")`), transforms its stdout/stderr to reduce the bytes the agent reads, and records the reduction via `core/tracking`.

Owns: all command-specific filter logic, organized by ecosystem (git, rust, js, python, go, dotnet, cloud, system). Cross-ecosystem routing (e.g., `lint_cmd` detecting Python and delegating to `ruff_cmd`) is an intra-component concern.

Does **not** own: the TOML DSL filter engine (that's `core/toml_filter`), hook interception (that's `hooks/`), or analytics dashboards (that's `analytics/`). This component **writes** to the tracking DB; analytics **reads** from it.

Boundary rule: a module belongs here if and only if it executes an external command and filters its output. Infrastructure that serves multiple modules without calling external commands belongs in `core/`.

## When to Write a Rust Module (vs TOML Filter)

Rust modules exist here because they need capabilities TOML filters don't have: parsing structured output (JSON, NDJSON), state machine parsing across phases, injecting CLI flags (`--format json`), cross-command routing, or **flag-aware filtering** — detecting user-requested verbose flags (e.g., `--nocapture`) and adjusting compression accordingly (see [Design Philosophy](../../CONTRIBUTING.md#design-philosophy) and [TOML vs Rust decision table](../../CONTRIBUTING.md#toml-vs-rust-which-one)).

**Ecosystem placement**: Match the command's language/toolchain. Use `system/` for language-agnostic commands. New ecosystem when 3+ related commands justify it.

For the full contribution checklist (including `discover/rules.rs` registration), see [Adding a New Command Filter](#adding-a-new-command-filter) below.

## Purpose
All command-specific filter modules that execute CLI commands and transform their output to minimize LLM token consumption. Each module follows a consistent pattern: execute the underlying command, filter its output through specialized parsers, track token savings, and propagate exit codes.

## Ecosystems

Each subdirectory has its own README with file descriptions, parsing strategies, and cross-command dependencies.

- **[`git/`](git/README.md)** — git, gh, gt, diff — `trailing_var_arg` parsing, gh markdown filtering, gt passthrough
- **[`rust/`](rust/README.md)** — cargo, runner (err/test) — Cargo sub-enum routing, runner dual-mode
- **[`js/`](js/README.md)** — npm, pnpm, vitest, lint, tsc, next, prettier, playwright, prisma — Package manager auto-detection, lint routing, cross-deps with python
- **[`python/`](python/README.md)** — ruff, pytest, mypy, pip — JSON check vs text format, state machine parsing, uv auto-detection
- **[`go/`](go/README.md)** — go test/build/vet, golangci-lint — NDJSON streaming, Go sub-enum pattern
- **[`dotnet/`](dotnet/README.md)** — dotnet, binlog, trx, format_report — DotnetCommands sub-enum, internal helper modules
- **[`cloud/`](cloud/README.md)** — aws, docker/kubectl, curl, wget, psql — Docker/Kubectl sub-enums, JSON forced output
- **[`system/`](system/README.md)** — ls, tree, read, grep, find, wc, env, json, log, deps, summary, format, smart — format_cmd routing, filter levels, language detection
- **[`ruby/`](ruby/README.md)** — rake/rails test, rspec, rubocop — JSON injection pattern, `ruby_exec()` bundle exec auto-detection

## Execution Flow

The shared wrappers in [`core/runner.rs`](../core/runner.rs) encapsulate the execution skeleton. Modules build the `Command` (custom arg logic), then choose one of three output contracts. All runners handle tracking, lossless recovery, and native exit-code propagation automatically.

```
 capture native output       parse semantic facts         render to budget
          |                           |                           |
          v                           v                           v
     +---------+  stdout/stderr  +------------+  AiDocument  +----------+
     | Spawn   |---------------->| AI parser  |------------->| Renderer |
     +---------+                 +------------+              +----+-----+
          |                                                        |
          | native status                         bounded output   v
          |                                                 +-------------+
          +------------------------------------------------>| Lossless    |
                                                            | emitter     |
                                                            +------+------+
                                                                   |
                                                        +----------+----------+
                                                        v                     v
                                                     stdout                tracking
```

### Choose the output contract first

Use this decision table for every new or migrated command route:

| Command output | Runner contract |
|----------------|-----------------|
| Safe semantic text | `run_ai_filtered(..., BudgetClass, parser, options)` |
| Existing migration-only string filter | `run_filtered(...)` |
| Structured, interactive, binary, streaming, sensitive, or unknown | `run_passthrough_with_reason(..., ExactReason)` |

New filtered routes **must not** use `run_filtered()`. That API preserves existing string filters while they are migrated; it is not the authoring interface for new work. Printing command output directly is also migration debt because it bypasses the shared output contract, recovery, and tracking metadata.

An AI parser returns an `AiDocument` containing status, facts, records, and any declared omissions. The shared renderer deterministically applies one of five budgets: `Acknowledgement` (128 estimated tokens), `State` (512), `Collection` (1,024), `Diagnostic` (2,048), or `Source` (4,096). If the parser fails, the runner converts the failure into a bounded, recoverable document instead of exposing an unbounded parser error or losing the native output.

Exact routes retain native I/O and record why semantic capture is unsafe through `ExactReason::{Structured, Interactive, Binary, Streaming, Sensitive, Unknown}`. In every contract, the child process's native exit behavior remains authoritative: filtering, fallback, and recovery never turn command failure into success or command success into failure.

### Filter modes

Captured and passthrough execution is implemented by `core::stream::run_streaming()` with one of five `FilterMode` variants. Production command-module wrappers select semantic capture, bounded stdout streaming, passthrough, or a narrowly retained legacy acknowledgement path; `Buffered` remains internal/test support. These modes explain existing internals, not an authoring menu. New routes select the semantic or exact contract above.

| FilterMode | How it works | Used by |
|------------|-------------|---------|
| **`CaptureOnly`** | Buffers stdout, then produces an `AiDocument` or a legacy string post-hoc. Stderr streams to the terminal in real time. | `run_ai_filtered()`; legacy `run_filtered()` |
| **`Buffered`** | Buffers stdout, applies a string filter, then prints the result. Stderr streams live. | Retained internal/test support; no current command-module runner selects it |
| **`Streaming`** | Feeds each stdout line to a legacy `StreamFilter`, emitting strings immediately and flushing after process exit. | Legacy `run_streamed()` compatibility paths and tests |
| **`StreamingStdout`** | Feeds bounded, lossy-decoded stdout lines to a semantic `StreamFilter`; stderr remains outside the parser and full producer byte counts are tracked. | Bounded high-volume search output |
| **`Passthrough`** | Inherits the parent TTY directly — no piping, buffering, or captured residual size. | `run_passthrough_with_reason()` |

### When to use which

| Scenario | Runner | FilterMode | Why |
|----------|--------|------------|-----|
| Parse safe semantic text into facts/records | `run_ai_filtered()` | CaptureOnly | Parser produces an `AiDocument`; shared rendering enforces its budget |
| Adapt an existing bounded filter while its parser is being replaced | `run_ai_from_filter()` | CaptureOnly | Adds semantic framing and recovery accounting without changing the specialized filter's selection logic |
| Preserve an existing string filter during migration | `run_filtered()` | CaptureOnly | Compatibility only; current wrapper does not select Buffered; do not use for new routes |
| Preserve an existing streamed string filter during migration | `run_streamed()` | Streaming | Compatibility only; do not use for new routes |
| New long-running or streaming output | `run_passthrough_with_reason(..., ExactReason::Streaming)` | Passthrough | Preserves timing, ordering, and native stream semantics |
| Bounded line-oriented semantic output | `run_streaming_with_line_cap(..., FilterMode::StreamingStdout)` | StreamingStdout | Drains arbitrarily long producer lines without retaining them unboundedly; use only with a parser that declares incomplete recovery |
| New interactive output | `run_passthrough_with_reason(..., ExactReason::Interactive)` | Passthrough | Preserves the native TTY and user interaction |
| Output must remain exact | `run_passthrough_with_reason()` | Passthrough | Preserves native I/O and records the exact-route reason |
| Custom direct output logic | Manual with `exec_capture()` | CaptureOnly | Remaining migration debt; record the exact/compatibility reason in `docs/validation/legacy-output-inventory.md` |

### Phases

1. **Spawn** — `run_streaming()` starts the child process with piped stdout/stderr (or inherited TTY for Passthrough)
2. **Parse** — semantic routes produce an `AiDocument`; legacy routes wrap their string result; parser errors become bounded recoverable documents
3. **Render** — semantic documents are rendered deterministically under their `BudgetClass`
4. **Emit** — the shared emitter either prints the bounded result or creates a private, complete recovery artifact; its final never-worse guard falls back to raw output if the compact form plus recovery would be larger
5. **Track** — output contract, exact reason, residual size, omissions, parser failure, and recovery metadata are recorded with the emitted text
6. **Exit code** — returns the native `exit_code` to the caller; `main.rs` calls `process::exit(code)` once

**`RunOptions` builder:**

| Constructor | Behavior |
|-------------|----------|
| `RunOptions::default()` | Combined stdout+stderr to filter, no tee |
| `RunOptions::with_tee("label")` | Combined filtering + tee recovery |
| `RunOptions::stdout_only()` | Stdout-only to filter, stderr passthrough, no tee |
| `RunOptions::stdout_only().tee("label")` | Stdout-only + tee recovery |

**Example — semantic filtered command (required for new routes):**

```rust
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("mycmd");
    cmd.args(args);

    runner::run_ai_filtered(
        cmd,
        "mycmd",
        &args.join(" "),
        BudgetClass::Collection,
        parse_mycmd_document,
        runner::RunOptions::stdout_only().tee("mycmd"),
    )
}
```

`parse_mycmd_document` returns `Result<AiDocument>`. Choose the narrowest budget that represents the command's semantic output; do not pre-render a string inside the parser.

**Example — legacy string filter (migration only):**

```rust
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("mycmd");
    for arg in args { cmd.arg(arg); }
    if verbose > 0 { eprintln!("Running: mycmd {}", args.join(" ")); }

    runner::run_filtered(
        cmd, "mycmd", &args.join(" "),
        filter_mycmd_output,
        runner::RunOptions::stdout_only().tee("mycmd"),
    )
}
```

Exit code handling is **fully automatic** for both shared captured runners — the wrapper extracts the native exit code (including Unix signal handling via 128+signal), tracks output, and returns `Ok(exit_code)`. Module authors just return the result.

**Legacy streaming filters (migration compatibility only):**

The existing `runner::run_streamed()` API emits filtered strings directly and does not implement the new semantic document contract. Keep it only while migrating existing line-by-line filters. Do not use it for a new command: new long-running or streaming behavior must call `run_passthrough_with_reason(..., ExactReason::Streaming)`, and interactive behavior must use `ExactReason::Interactive`.

The three legacy streaming abstractions are documented here only to maintain current routes until their command families are migrated:

**Level 1: `RegexBlockFilter`** — regex start pattern + indent continuation (3-5 lines)

For block-based errors where blocks start with a regex match and continue on indented lines. Handles skip prefixes, block counting, and summary automatically.

```rust
use crate::core::stream::{BlockStreamFilter, RegexBlockFilter};

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("mycmd");
    for arg in args { cmd.arg(arg); }

    let filter = RegexBlockFilter::new("mycmd", r"^error\[")
        .skip_prefixes(&["warning:", "note:"]);

    runner::run_streamed(
        cmd, "mycmd", &args.join(" "),
        Box::new(BlockStreamFilter::new(filter)),
        runner::RunOptions::with_tee("mycmd"),
    )
}
```

`RegexBlockFilter` provides: regex-based block start detection, indent-based continuation (space/tab), configurable line skipping via prefixes, and automatic summary (`"mycmd: 3 blocks in output"` or `"mycmd: no errors found"`).

**Level 2: `BlockHandler` trait** — custom block detection with state tracking

When you need custom block start/continuation logic or stateful parsing beyond regex + indent. Implement the `BlockHandler` trait and wrap in `BlockStreamFilter`.

```rust
use crate::core::stream::{BlockHandler, BlockStreamFilter};

struct MyHandler { error_count: usize }

impl BlockHandler for MyHandler {
    fn should_skip(&mut self, line: &str) -> bool { line.is_empty() }
    fn is_block_start(&mut self, line: &str) -> bool {
        if line.starts_with("FAIL") { self.error_count += 1; true } else { false }
    }
    fn is_block_continuation(&mut self, line: &str, _block: &[String]) -> bool {
        line.starts_with("  ") || line.starts_with("at ")
    }
    fn format_summary(&self, _exit_code: i32, _raw: &str) -> Option<String> {
        Some(format!("{} failures\n", self.error_count))
    }
}
```

See `cmds/rust/cargo_cmd.rs::CargoBuildHandler` and `cmds/js/tsc_cmd.rs::TscHandler` for production examples.

**Level 3: `StreamFilter` trait** — full line-by-line control

When block-based parsing doesn't fit (e.g., state machines, multi-phase output, line transforms). Implement `StreamFilter` directly.

```rust
use crate::core::stream::StreamFilter;

struct MyFilter { state: State }

impl StreamFilter for MyFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        // Return Some(text) to emit, None to suppress
        if line.contains("error") { Some(format!("{}\n", line)) } else { None }
    }
    fn flush(&mut self) -> String { String::new() }
    fn on_exit(&mut self, exit_code: i32, raw: &str) -> Option<String> { None }
}
```

See `cmds/rust/runner.rs::ErrorStreamFilter` for a complete reference implementation (state machine that tracks error blocks across lines).

**Example — passthrough command (no filtering):**

```rust
pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    runner::run_passthrough_with_reason(
        "mycmd",
        args,
        verbose,
        ExactReason::Structured,
    )
}
```

**Example — manual execution (custom logic):**

```rust
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let output = resolved_command("mycmd").args(args)
        .output().context("Failed to run mycmd")?;
    let exit_code = exit_code_from_output(&output, "mycmd");
    // ... custom filtering, tracking ...
    Ok(exit_code)
}
```

Manual execution and direct stdout are existing migration debt. Do not copy those patterns into new routes; preserve them only until the owning command family moves to the semantic or exact shared runner.


## Cross-Command Dependencies

- `lint_cmd` routes to `mypy_cmd` or `ruff_cmd` when detecting Python projects
- `format_cmd` routes to `prettier_cmd` or `ruff_cmd` depending on the formatter detected
- `gh_cmd` imports `compact_diff()` from `git` for diff formatting (markdown helpers are defined in `gh_cmd` itself)

## Cross-Cutting Behavior Contracts

These behaviors must be uniform across all command modules. Full audit details in `docs/ISO_ANALYZE.md`.

### Exit Code Propagation

All module `run()` functions return `Result<i32>` where the `i32` is the underlying command's exit code. `main.rs` calls `std::process::exit(code)` once at the single exit point — **modules never call `process::exit()` directly**.

| Return value | Meaning | Who exits |
|--------------|---------|-----------|
| `Ok(0)` | Command succeeded | `main.rs` exits 0 |
| `Ok(N)` | Command failed with code N | `main.rs` exits N |
| `Err(e)` | RTK itself failed (not the command) | `main.rs` prints error, exits 1 |

**How exit codes are extracted:**

| Execution style | Helper | Signal handling |
|----------------|--------|-----------------|
| `cmd.output()` (filtered) | `exit_code_from_output(&output, "tool")` | 128+signal on Unix |
| `cmd.status()` (passthrough) | `exit_code_from_status(&status, "tool")` | 128+signal on Unix |
| `run_ai_filtered()` (semantic wrapper) | Automatic — no manual code needed | Built-in |
| `run_filtered()` / `run_streamed()` (legacy wrappers) | Automatic during migration | Built-in |
| `run_passthrough_with_reason()` (exact wrapper) | Automatic from native status | Built-in |

**When using a shared semantic or exact runner**: exit code handling is fully automatic. The wrapper extracts the native exit code, handles signals, and returns `Ok(exit_code)`. Module authors just return the wrapper's result — no exit code logic needed. The same remains true for retained legacy wrappers during migration.

**When doing manual execution**: use `exit_code_from_output()` or `exit_code_from_status()` and return `Ok(exit_code)`. Never call `process::exit()`, never use `.code().unwrap_or(1)` (loses signal info).

### Filter Failure Passthrough

When filtering fails, fall back to raw output and warn on stderr. Never block the user.

### Tee Recovery

Existing legacy modules that already parse structured output use `tee::tee_and_hint()` so users can recover full output on failure. Do not copy that pattern into a new route: new structured output stays exact via `run_passthrough_with_reason(..., ExactReason::Structured)`, while safe semantic text uses the shared `AiDocument` emitter and its recovery contract.

### Internal Truncation Recovery

When a filter caps a list at N items (e.g. `take(20)`), the remaining items must be accessible via a tee hint. **Never show `"… +N more"` without a recovery path** — the agent has no way to retrieve the hidden content.

**Choosing the right hint:**

| Content type | Function | Condition |
|---|---|---|
| Flat list — one item = one line in the tee | `force_tee_tail_hint(content, slug, MAX + 1)` | PR lists, error lines, file paths — anything where each item is a single-line string |
| Multi-line blocks | `force_tee_hint(content, slug)` | Test failures, build error blocks — items that span multiple lines so a line offset is meaningless |

**Cap values come from `src/core/truncate.rs`.** Pick the `CAP_*` matching your data class (`CAP_ERRORS`, `CAP_WARNINGS`, `CAP_LIST`, `CAP_INVENTORY`) and bind it to a local `const MAX_XXX: usize = CAP_Y;`. Derive `take(MAX_XXX)`, `> MAX_XXX`, and the offset `MAX_XXX + 1` from the local. These CAPs will later become the configuration surface for per-filter cap tuning (user-overridable via config) — keep all truncation values routed through them so that hook lands as a single switch rather than a codebase-wide hunt. A filter that genuinely needs to deviate uses **`truncate::reduced(CAP_Y, n)`** (e.g. `reduced(CAP_WARNINGS, 5)`) so it still tracks the global when reconfigured — never a bare literal, never `cap - n` (underflows once caps are runtime-configurable), and never `*`/`/` (those scale unboundedly). `reduced` falls back to the full cap if the reduction would empty the list. Each deviation needs a one-line comment stating why; if there's no real reason, just use the plain CAP. See `src/core/README.md` ("Truncation Caps") for the full rationale.

**The tee content must match what `tail` produces.** For `force_tee_tail_hint`, build the tee from the same formatted values shown in the output — not raw/intermediate data. If the filter reformats items before displaying them, pre-build a `Vec<String>` of formatted lines and use it for both the display loop and the tee.

### Stderr Handling

Modules must capture stderr and include it in the raw string passed to `timer.track()`, so token savings reflect total output.

### Tracking Completeness

Shared semantic and exact runners record tracking on every path — success, failure, and fallback. Retained manual or legacy modules must still call `timer.track()` before returning. Since modules return `Ok(exit_code)` instead of calling `process::exit()`, tracking always runs before the program exits.

### Verbose Flag

All modules accept `verbose: u8`. Use it to print debug info (command being run, savings %, filter tier). Do not accept and ignore it.


## Adding a New Command Filter

Adding a new filter or command requires changes in multiple places. For TOML-vs-Rust decision criteria, see [CONTRIBUTING.md](../../CONTRIBUTING.md#toml-vs-rust-which-one).

### Rust module (semantic text or exact native output)

1. **Create module** in `src/cmds/<ecosystem>/mycmd_cmd.rs`:
   - Classify the route before writing a parser. Safe semantic text uses `run_ai_filtered(...)`; structured, interactive, binary, streaming, sensitive, or unknown output uses `run_passthrough_with_reason(..., ExactReason)`.
   - For safe semantic text, write a pure parser that returns `Result<AiDocument>` with status, facts, records, and any declared omissions. Do not render a string in the parser.
   - Write `pub fn run(...) -> Result<i32>` using `runner::run_ai_filtered()` — build the `Command`, choose the narrowest `BudgetClass`, choose `RunOptions`, and delegate.
   - Use `RunOptions::stdout_only()` only when stderr must stay outside the safe semantic parser; use `RunOptions::default()` when combined human-readable text is the semantic input.
   - Populate the complete semantic document and let the shared renderer and lossless emitter enforce the budget, exact omission counts, recovery, and never-worse guard.
   - For a new streaming route, use `run_passthrough_with_reason(..., ExactReason::Streaming)`. For an interactive route, use `ExactReason::Interactive`; select the corresponding reason for other exact categories.
   - **Exit codes**: handled automatically by `run_ai_filtered()` and `run_passthrough_with_reason()` — just return the result.
   - **Migration rule**: do not introduce `run_filtered()`, `run_streamed()`, direct stdout, or a new string-returning filter. Those patterns are retained only for existing routes awaiting migration.
2. **Register module**:
   - Ecosystem `mod.rs` files use `automod::dir!()` — any `.rs` file in the directory becomes a public module automatically. No manual `pub mod` needed, but be aware: WIP or helper files will also be exposed. Only commit command-ready modules.
   - Add variant to `Commands` enum in `main.rs` with `#[arg(trailing_var_arg = true, allow_hyphen_values = true)]`
   - Add routing match arm in `main.rs`: `Commands::Mycmd { args } => mycmd_cmd::run(&args, cli.verbose)?,`
3. **Add rewrite pattern** — Entry in `src/discover/rules.rs` (PATTERNS + RULES arrays at matching index) so hooks auto-rewrite the command
4. **Write tests** — Real fixture, snapshot test, >= 20% reduction in bash output (measured with RTK's token estimator, see [testing rules](../../.claude/rules/cli-testing.md))
5. **Update docs** — Ecosystem README (CHANGELOG.md is auto-generated by release-please)

### TOML filter (simple line-based filtering)

1. **Create filter** in [`src/filters/`](../filters/README.md)
2. **Add rewrite pattern** in `src/discover/rules.rs`
3. **Write tests** and **update docs**
