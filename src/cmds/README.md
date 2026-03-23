# Command Filter Modules

## Scope

**Command execution and output filtering** — this is the core value RTK delivers. Every module here calls an external CLI tool (`Command::new("some_tool")`), transforms its stdout/stderr to reduce token consumption, and records savings via `core/tracking`.

Owns: all command-specific filter logic, organized by ecosystem (git, rust, js, python, go, dotnet, cloud, system). Cross-ecosystem routing (e.g., `lint_cmd` detecting Python and delegating to `ruff_cmd`) is an intra-component concern.

Does **not** own: the TOML DSL filter engine (that's `core/toml_filter`), hook interception (that's `hooks/`), or analytics dashboards (that's `analytics/`). This component **writes** to the tracking DB; analytics **reads** from it.

Boundary rule: a module belongs here if and only if it executes an external command and filters its output. Infrastructure that serves multiple modules without calling external commands belongs in `core/`.

## Purpose
All command-specific filter modules that execute CLI commands and transform their output to minimize LLM token consumption. Each module follows a consistent pattern: execute the underlying command, filter its output through specialized parsers, track token savings, and propagate exit codes.

## Organization
Commands are organized by ecosystem:

| Directory | Ecosystem | Commands |
|-----------|-----------|----------|
| `git/` | Git and VCS | git, gh (GitHub CLI), gt (Graphite), diff |
| `rust/` | Rust | cargo (build, test, clippy, check), generic runner |
| `js/` | JavaScript/TypeScript/Node | npm, pnpm, vitest, lint, tsc, next, prettier, playwright, prisma |
| `python/` | Python | ruff, pytest, mypy, pip |
| `go/` | Go | go (test, build, vet), golangci-lint |
| `dotnet/` | .NET | dotnet (build, test, format), TRX/binlog parsers |
| `cloud/` | Cloud and Infrastructure | aws, docker/kubectl, curl, wget, psql |
| `system/` | System and Generic Utilities | ls, tree, read, grep, find, wc, env, json, log, deps, summary, format, smart |

## Common Pattern

Every command module follows this structure:

```rust
pub fn run(args: MyArgs, verbose: u8) -> Result<()> {
    // 1. Start timer for tracking
    let timer = tracking::TimedExecution::start();

    // 2. Execute the underlying command
    let output = resolved_command("mycmd")
        .args(&args.to_cmd_args())
        .output()
        .context("Failed to execute mycmd")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    // 3. Filter the output (with fallback to raw on error)
    let filtered = filter_output(&stdout)
        .unwrap_or_else(|e| {
            eprintln!("rtk: filter warning: {}", e);
            raw.clone()  // Passthrough on failure
        });

    // 4. Tee raw output on failure (for LLM re-read)
    let exit_code = output.status.code().unwrap_or(1);
    if let Some(hint) = tee::tee_and_hint(&raw, "mycmd", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    // 5. Track token savings to SQLite
    timer.track("mycmd args", "rtk mycmd args", &raw, &filtered);

    // 6. Propagate exit code
    if !output.status.success() {
        std::process::exit(exit_code);
    }
    Ok(())
}
```

Key aspects of this pattern:
- **`TimedExecution`**: Records elapsed time, estimates tokens (`ceil(chars / 4.0)`), writes to SQLite
- **`resolved_command()`**: Finds the command in PATH, handles aliases
- **Fallback**: Filter errors fall back to raw output (never block the user)
- **Tee recovery**: On failure, saves raw output to disk with a hint line for LLM re-read
- **Exit code**: Always propagated via `std::process::exit(code)` for CI/CD reliability

## Token Savings by Category

| Category | Commands | Typical Savings | Strategy |
|----------|----------|----------------|----------|
| Test Runners | vitest, pytest, cargo test, go test, playwright | 90-99% | Show failures only, aggregate passes |
| Build Tools | cargo build, npm, pnpm, dotnet | 70-90% | Strip progress bars, summarize errors |
| VCS | git status/log/diff/show | 70-80% | Compact commit hashes, stat summaries |
| Linters | eslint/biome, ruff, tsc, mypy, golangci-lint | 80-85% | Group by file/rule, strip context |
| Package Managers | pip, cargo install, pnpm list | 75-80% | Remove decorative output, compact trees |
| File Operations | ls, find, grep, cat/head/tail | 60-75% | Tree format, grouped results, truncation |
| Infrastructure | docker, kubectl, aws, terraform | 75-85% | Essential info only |

## Cross-Command Dependencies

- `lint_cmd` routes to `mypy_cmd` or `ruff_cmd` when detecting Python projects
- `format_cmd` routes to `prettier_cmd` or `ruff_cmd` depending on the formatter detected
- `gh_cmd` imports markdown filtering helpers from `git`

## Cross-Cutting Behavior Contracts

These behaviors must be uniform across all command modules. Full audit details in `docs/ISO_ANALYZE.md`.

### Exit Code Propagation

Modules must capture the underlying command's exit code, propagate it via `std::process::exit()` only on failure, and return `Ok(())` on success. When the process is killed by signal (`.code()` returns `None`), default to exit code 1.

### Filter Failure Passthrough

When filtering fails, fall back to raw output and warn on stderr. Never block the user.

### Tee Recovery

Modules that parse structured output (JSON, NDJSON, state machines) must call `tee::tee_and_hint()` so users can recover full output on failure.

### Stderr Handling

Modules must capture stderr and include it in the raw string passed to `timer.track()`, so token savings reflect total output.

### Tracking Completeness

All modules must call `timer.track()` on every path — success, failure, and fallback. Never exit before tracking.

### Verbose Flag

All modules accept `verbose: u8`. Use it to print debug info (command being run, savings %, filter tier). Do not accept and ignore it.

### Gaps (to be fixed)

**Exit code** — 5 different patterns coexist, should be reviewed for uniform behavior:
- `vitest_cmd.rs`, `tsc_cmd.rs`, `psql_cmd.rs` — exit unconditionally, even on success
- `lint_cmd.rs` — swallows signal kills silently
- `golangci_cmd.rs` — maps signal kill to exit 130 (correct but unique)

**Filter passthrough** — silent passthrough, no warning:
- `gh_cmd.rs`, `pip_cmd.rs`, `container.rs`, `dotnet_cmd.rs` — `run_passthrough()` skips filtering without warning
- `pnpm_cmd.rs`, `playwright_cmd.rs` — 3-tier degradation but no tee recovery on final tier

**Tee recovery** — currently 12/38 modules implement tee. Missing from high-risk modules:
- `pnpm_cmd.rs`, `playwright_cmd.rs` — 3-tier parsers, no tee
- `gh_cmd.rs` — aggressive markdown filtering, no tee
- `ruff_cmd.rs`, `golangci_cmd.rs` — JSON parsers, no tee
- `psql_cmd.rs` — has tee but exits before calling it on error path

**Stderr handling** — 3 patterns coexist. Some modules combine stderr into raw (correct), others print via `eprintln!()` and exclude from tracking (inflates savings %). See `docs/ISO_ANALYZE.md` section 4.

**Tracking** — exit before track on error path:
- `ls.rs`, `tree.rs` — lost metrics on failure
- `container.rs` — inconsistent across subcommands

**Verbose** — accept parameter but ignore it:
- `container.rs` — all internal functions prefix `_verbose`
- `diff_cmd.rs` — `_verbose` unused

## Adding a New Command Filter

1. Create `<cmd>_cmd.rs` in the appropriate ecosystem subdirectory
2. Follow the common pattern above (timer, execute, filter with fallback, tee, track, exit code)
3. Use `lazy_static!` for all regex patterns
4. Add the command to the `Commands` enum in `main.rs`
5. Create a test fixture in `tests/fixtures/` from real command output
6. Write snapshot test (`assert_snapshot!`) and token savings test (verify >= 60% reduction)
7. Run `cargo fmt --all && cargo clippy --all-targets && cargo test`

See the Filter Development Checklist in `CLAUDE.md` and `.claude/rules/rust-patterns.md` for the full module structure.
