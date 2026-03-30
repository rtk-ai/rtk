# Command Filter Modules

## Scope

**Command execution and output filtering.** Every module here calls an external CLI tool (`Command::new("some_tool")`), transforms its stdout/stderr to reduce token consumption, and records savings via `core/tracking`.

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

## Execution Flow: `runner::run_filtered()`

The shared wrapper in [`core/runner.rs`](../core/runner.rs) encapsulates the six-phase execution skeleton. Modules build the `Command` (custom arg logic), then delegate to `run_filtered()` for everything else.

```
 cmd.output()          Filter applied to         tee_and_hint()
      |                stdout or combined              |
      v                       |                        v
 +---------+  stdout  +-------+-------+  filtered  +-------+
 | Execute |--------->| filter_fn()   |----------->| Print |
 +---------+  stderr  +---------------+            +-------+
      |                                                |
      v                                                v
 +----------+                                    +---------+
 | raw =    |                                    | Track   |
 | stdout + |                                    | savings |
 | stderr   |                                    +---------+
 +----------+                                          |
                                                       v
                                                 +-----------+
                                                 | Ok(code)  |
                                                 | returned  |
                                                 +-----------+
```

**Six phases in order:**

1. **Execute** — `cmd.output()` captures stdout + stderr
2. **Filter** — `filter_fn` receives stdout-only or combined, returns compressed string
3. **Print** — filtered output printed; if tee enabled, appends recovery hint on failure
4. **Stderr passthrough** — when `filter_stdout_only`: stderr printed via `eprintln!()` unconditionally
5. **Track** — `timer.track()` records raw vs filtered for token savings
6. **Exit code** — returns `Ok(exit_code)` to caller; `main.rs` calls `process::exit(code)` once

**`RunOptions` builder:**

| Constructor | Behavior |
|-------------|----------|
| `RunOptions::default()` | Combined stdout+stderr to filter, no tee |
| `RunOptions::with_tee("label")` | Combined filtering + tee recovery |
| `RunOptions::stdout_only()` | Stdout-only to filter, stderr passthrough, no tee |
| `RunOptions::stdout_only().tee("label")` | Stdout-only + tee recovery |

**Example — filtered command (recommended):**

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

Exit code handling is **fully automatic** when using `run_filtered()` — the wrapper extracts the exit code (including Unix signal handling via 128+signal), tracks savings, and returns `Ok(exit_code)`. Module authors just return the result.

**Example — passthrough command (no filtering):**

```rust
pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    let status = resolved_command("mycmd").args(args)
        .stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit())
        .status().context("Failed to run mycmd")?;
    Ok(exit_code_from_status(&status, "mycmd"))
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

Modules with deviations (subcommand dispatch, parser trait systems, two-command fallback, synthetic output).


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
| `run_filtered()` (wrapper) | Automatic — no manual code needed | Built-in |

**When using `run_filtered()`**: exit code handling is fully automatic. The wrapper extracts the exit code, handles signals, and returns `Ok(exit_code)`. Module authors just return the wrapper's result — no exit code logic needed.

**When doing manual execution**: use `exit_code_from_output()` or `exit_code_from_status()` and return `Ok(exit_code)`. Never call `process::exit()`, never use `.code().unwrap_or(1)` (loses signal info).

### Filter Failure Passthrough

When filtering fails, fall back to raw output and warn on stderr. Never block the user.

### Tee Recovery

Modules that parse structured output (JSON, NDJSON, state machines) must call `tee::tee_and_hint()` so users can recover full output on failure.

### Stderr Handling

Modules must capture stderr and include it in the raw string passed to `timer.track()`, so token savings reflect total output.

### Tracking Completeness

All modules must call `timer.track()` on every path — success, failure, and fallback. Since modules return `Ok(exit_code)` instead of calling `process::exit()`, tracking always runs before the program exits.

### Verbose Flag

All modules accept `verbose: u8`. Use it to print debug info (command being run, savings %, filter tier). Do not accept and ignore it.


## Adding a New Command Filter

Adding a new filter or command requires changes in multiple places. For TOML-vs-Rust decision criteria, see [CONTRIBUTING.md](../../CONTRIBUTING.md#toml-vs-rust-which-one).

### Rust module (structured output, flag injection, state machines)

1. **Create module** in `src/cmds/<ecosystem>/mycmd_cmd.rs`:
   - Write the `filter_mycmd()` function (pure: `&str -> String`, no side effects)
   - Write `pub fn run(...) -> Result<i32>` using `runner::run_filtered()` — build the `Command`, choose `RunOptions`, delegate
   - Use `RunOptions::stdout_only()` when the filter parses structured stdout (JSON, NDJSON) — stderr would corrupt parsing
   - Use `RunOptions::default()` when filtering combined text output
   - Add `.tee("label")` when the filter parses structured output (enables raw output recovery on failure)
   - **Exit codes**: handled automatically by `run_filtered()` — just return its result
2. **Register module**:
   - Add `pub mod mycmd_cmd;` to the ecosystem's `mod.rs`
   - Add variant to `Commands` enum in `main.rs` with `#[arg(trailing_var_arg = true, allow_hyphen_values = true)]`
   - Add routing match arm in `main.rs`: `Commands::Mycmd { args } => mycmd_cmd::run(&args, cli.verbose)?,`
3. **Add rewrite pattern** — Entry in `src/discover/rules.rs` (PATTERNS + RULES arrays at matching index) so hooks auto-rewrite the command
4. **Write tests** — Real fixture, snapshot test, token savings >= 60% (see [testing rules](../../.claude/rules/cli-testing.md))
5. **Update docs** — Ecosystem README, CHANGELOG.md

### TOML filter (simple line-based filtering)

1. **Create filter** in [`src/filters/`](../filters/README.md)
2. **Add rewrite pattern** in `src/discover/rules.rs`
3. **Write tests** and **update docs**
