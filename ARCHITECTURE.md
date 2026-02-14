# rtk Architecture Documentation

> **rtk (Rust Token Killer)** - A high-performance CLI proxy that minimizes LLM token consumption through intelligent output filtering and compression.

This document provides a comprehensive architectural overview of rtk, including system design, data flows, module organization, and implementation patterns.

---

## Table of Contents

1. [System Overview](#system-overview)
2. [Command Lifecycle](#command-lifecycle)
3. [Module Organization](#module-organization)
4. [Filtering Strategies](#filtering-strategies)
5. [Shared Infrastructure](#shared-infrastructure)
6. [Token Tracking System](#token-tracking-system)
7. [Global Flags Architecture](#global-flags-architecture)
8. [Error Handling](#error-handling)
9. [Configuration System](#configuration-system)
10. [Module Development Pattern](#module-development-pattern)
11. [Build Optimizations](#build-optimizations)
12. [Extensibility Guide](#extensibility-guide)

---

## System Overview

### Proxy Pattern Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│                      rtk - Token Optimization Proxy                    │
└────────────────────────────────────────────────────────────────────────┘

User Input          CLI Layer           Router            Module Layer
──────────          ─────────           ──────            ────────────

$ rtk git log    ─→  Clap Parser   ─→   Commands    ─→   git::run()
  -v --oneline       (main.rs)          enum match
                     • Parse args                         Execute: git log
                     • Extract flags                      Capture output
                     • Route command                            ↓
                                                          Filter/Compress
                                                                 ↓
$ 3 commits      ←─  Terminal      ←─   Format      ←─   Compact Stats
  +142/-89           colored            optimized         (90% reduction)
                     output                                     ↓
                                                          tracking::track()
                                                                 ↓
                                                          SQLite INSERT
                                                          (~/.local/share/rtk/)
```

### Key Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| **CLI Parser** | main.rs | Clap-based argument parsing, global flags |
| **Command Router** | main.rs | Dispatch to specialized modules |
| **Module Layer** | src/*_cmd.rs, src/git.rs, etc. | Command execution + filtering |
| **Shared Utils** | utils.rs | Package manager detection, text processing |
| **Filter Engine** | filter.rs | Language-aware code filtering |
| **Tracking** | tracking.rs | SQLite-based token metrics |
| **Config** | config.rs, init.rs | User preferences, LLM integration |

### Design Principles

1. **Single Responsibility**: Each module handles one command type
2. **Minimal Overhead**: ~5-15ms proxy overhead per command
3. **Exit Code Preservation**: CI/CD reliability through proper exit code propagation
4. **Fail-Safe**: If filtering fails, fall back to original output
5. **Transparent**: Users can always see raw output with `-v` flags

---

## Command Lifecycle

### Six-Phase Execution Flow

```
┌────────────────────────────────────────────────────────────────────────┐
│                     Command Execution Lifecycle                        │
└────────────────────────────────────────────────────────────────────────┘

Phase 1: PARSE
──────────────
$ rtk git log --oneline -5 -v

Clap Parser extracts:
  • Command: Commands::Git
  • Args: ["log", "--oneline", "-5"]
  • Flags: verbose = 1
          ultra_compact = false

         ↓

Phase 2: ROUTE
──────────────
main.rs:match Commands::Git { args, .. }
  ↓
git::run(args, verbose)

         ↓

Phase 3: EXECUTE
────────────────
std::process::Command::new("git")
    .args(["log", "--oneline", "-5"])
    .output()?

Output captured:
  • stdout: "abc123 Fix bug\ndef456 Add feature\n..." (500 chars)
  • stderr: "" (empty)
  • exit_code: 0

         ↓

Phase 4: FILTER
───────────────
git::format_git_output(stdout, "log", verbose)

Strategy: Stats Extraction
  • Count commits: 5
  • Extract stats: +142/-89
  • Compress: "5 commits, +142/-89"

Filtered: 20 chars (96% reduction)

         ↓

Phase 5: PRINT
──────────────
if verbose > 0 {
    eprintln!("Git log summary:");  // Debug
}
println!("{}", colored_output);     // User output

Terminal shows: "5 commits, +142/-89 ✓"

         ↓

Phase 6: TRACK
──────────────
tracking::track(
    original_cmd: "git log --oneline -5",
    rtk_cmd: "rtk git log --oneline -5",
    input: &raw_output,    // 500 chars
    output: &filtered      // 20 chars
)

  ↓

SQLite INSERT:
  • input_tokens: 125 (500 / 4)
  • output_tokens: 5 (20 / 4)
  • savings_pct: 96.0
  • timestamp: now()

Database: ~/.local/share/rtk/history.db
```

### Verbosity Levels

```
-v (Level 1): Show debug messages
  Example: eprintln!("Git log summary:");

-vv (Level 2): Show command being executed
  Example: eprintln!("Executing: git log --oneline -5");

-vvv (Level 3): Show raw output before filtering
  Example: eprintln!("Raw output:\n{}", stdout);
```

---

## Module Organization

### Complete Module Map (30 Modules)

```
┌────────────────────────────────────────────────────────────────────────┐
│                        Module Organization                             │
└────────────────────────────────────────────────────────────────────────┘

Category          Module            Commands               Savings    File
──────────────────────────────────────────────────────────────────────────

GIT               git.rs            status, diff, log      85-99%     ✓
                                    add, commit, push
                                    branch, checkout

CODE SEARCH       grep_cmd.rs       grep                   60-80%     ✓
                  diff_cmd.rs       diff                   70-85%     ✓
                  find_cmd.rs       find                   50-70%     ✓

FILE OPS          ls.rs             ls                     50-70%     ✓
                  read.rs           read                   40-90%     ✓

EXECUTION         runner.rs         err, test              60-99%     ✓
                  summary.rs        smart (heuristic)      50-80%     ✓
                  local_llm.rs      smart (LLM mode)       60-90%     ✓

LOGS/DATA         log_cmd.rs        log                    70-90%     ✓
                  json_cmd.rs       json                   80-95%     ✓

JS/TS STACK       lint_cmd.rs       lint                   84%        ✓
                  tsc_cmd.rs        tsc                    83%        ✓
                  next_cmd.rs       next                   87%        ✓
                  prettier_cmd.rs   prettier               70%        ✓
                  playwright_cmd.rs playwright             94%        ✓
                  prisma_cmd.rs     prisma                 88%        ✓
                  vitest_cmd.rs     vitest                 99.5%      ✓
                  pnpm_cmd.rs       pnpm                   70-90%     ✓

CONTAINERS        container.rs      podman, docker         60-80%     ✓

VCS               gh_cmd.rs         gh                     26-87%     ✓

PYTHON            ruff_cmd.rs       ruff check/format      80%+       ✓
                  pytest_cmd.rs     pytest                 90%+       ✓
                  pip_cmd.rs        pip list/outdated      70-85%     ✓

GO                go_cmd.rs         go test/build/vet      75-90%     ✓
                  golangci_cmd.rs   golangci-lint          85%        ✓

NETWORK           wget_cmd.rs       wget                   85-95%     ✓

DEPENDENCIES      deps.rs           deps                   80-90%     ✓

ENVIRONMENT       env_cmd.rs        env                    60-80%     ✓

SYSTEM            init.rs           init                   N/A        ✓
                  gain.rs           gain                   N/A        ✓
                  config.rs         (internal)             N/A        ✓

SHARED            utils.rs          Helpers                N/A        ✓
                  filter.rs         Language filters       N/A        ✓
                  tracking.rs       Token tracking         N/A        ✓
```

**Total: 46 modules** (29 command modules + 17 infrastructure modules)

### Module Count Breakdown

- **Command Modules**: 29 (directly exposed to users)
- **Infrastructure Modules**: 17 (utils, filter, tracking, config, init, gain, etc.)
- **Git Commands**: 7 operations (status, diff, log, add, commit, push, branch/checkout)
- **JS/TS Tooling**: 8 modules (modern frontend/fullstack development)
- **Python Tooling**: 3 modules (ruff, pytest, pip)
- **Go Tooling**: 2 modules (go test/build/vet, golangci-lint)

---

## Filtering Strategies

### Strategy Matrix

```
┌────────────────────────────────────────────────────────────────────────┐
│                      Filtering Strategy Taxonomy                       │
└────────────────────────────────────────────────────────────────────────┘

Strategy            Modules              Technique               Reduction
──────────────────────────────────────────────────────────────────────────

1. STATS EXTRACTION
   ┌──────────────┐
   │ Raw: 5000    │  →  Count/aggregate  →  "3 files, +142/-89"  90-99%
   │ lines        │      Drop details
   └──────────────┘

   Used by: git status, git log, git diff, pnpm list

2. ERROR ONLY
   ┌──────────────┐
   │ stdout+err   │  →  stderr only      →  "Error: X failed"    60-80%
   │ Mixed        │      Drop stdout
   └──────────────┘

   Used by: runner (err mode), test failures

3. GROUPING BY PATTERN
   ┌──────────────┐
   │ 100 errors   │  →  Group by rule    →  "no-unused-vars: 23" 80-90%
   │ Scattered    │      Count/summarize     "semi: 45"
   └──────────────┘

   Used by: lint, tsc, grep (group by file/rule/error code)

4. DEDUPLICATION
   ┌──────────────┐
   │ Repeated     │  →  Unique + count   →  "[ERROR] ... (×5)"   70-85%
   │ Log lines    │
   └──────────────┘

   Used by: log_cmd (identify patterns, count occurrences)

5. STRUCTURE ONLY
   ┌──────────────┐
   │ JSON with    │  →  Keys + types     →  {user: {...}, ...}   80-95%
   │ Large values │      Strip values
   └──────────────┘

   Used by: json_cmd (schema extraction)

6. CODE FILTERING
   ┌──────────────┐
   │ Source code  │  →  Filter by level:
   │              │     • none       → Keep all               0%
   │              │     • minimal    → Strip comments        20-40%
   │              │     • aggressive → Strip bodies          60-90%
   └──────────────┘

   Used by: read, smart (language-aware stripping via filter.rs)

7. FAILURE FOCUS
   ┌──────────────┐
   │ 100 tests    │  →  Failures only    →  "2 failed:"         94-99%
   │ Mixed        │      Hide passing        "  • test_auth"
   └──────────────┘

   Used by: vitest, playwright, runner (test mode)

8. TREE COMPRESSION
   ┌──────────────┐
   │ Flat list    │  →  Tree hierarchy   →  "src/"             50-70%
   │ 50 files     │      Aggregate dirs      "  ├─ lib/ (12)"
   └──────────────┘

   Used by: ls (directory tree with counts)

9. PROGRESS FILTERING
   ┌──────────────┐
   │ ANSI bars    │  →  Strip progress   →  "✓ Downloaded"      85-95%
   │ Live updates │      Final result
   └──────────────┘

   Used by: wget, pnpm install (strip ANSI escape sequences)

10. JSON/TEXT DUAL MODE
   ┌──────────────┐
   │ Tool output  │  →  JSON when available  →  Structured data  80%+
   │              │      Text otherwise          Fallback parse
   └──────────────┘

   Used by: ruff (check → JSON, format → text), pip (list/show → JSON)

11. STATE MACHINE PARSING
   ┌──────────────┐
   │ Test output  │  →  Track test state  →  "2 failed, 18 ok"  90%+
   │ Mixed format │      Extract failures     Failure details
   └──────────────┘

   Used by: pytest (text state machine: test_name → PASSED/FAILED)

12. NDJSON STREAMING
   ┌──────────────┐
   │ Line-by-line │  →  Parse each JSON  →  "2 fail (pkg1, pkg2)" 90%+
   │ JSON events  │      Aggregate results   Compact summary
   └──────────────┘

   Used by: go test (NDJSON stream, interleaved package events)
```

### Code Filtering Levels (filter.rs)

```rust
// FilterLevel::None - Keep everything
fn calculate_total(items: &[Item]) -> i32 {
    // Sum all items
    items.iter().map(|i| i.value).sum()
}

// FilterLevel::Minimal - Strip comments only (20-40% reduction)
fn calculate_total(items: &[Item]) -> i32 {
    items.iter().map(|i| i.value).sum()
}

// FilterLevel::Aggressive - Strip comments + function bodies (60-90% reduction)
fn calculate_total(items: &[Item]) -> i32 { ... }
```

**Language Support**: Rust, Python, JavaScript, TypeScript, Go, C, C++, Java

**Detection**: File extension-based with fallback heuristics

---

## Python & Go Module Architecture

### Design Rationale

**Added**: 2026-02-12 (v0.15.1)
**Motivation**: Complete language ecosystem coverage beyond JS/TS

Python and Go modules follow distinct architectural patterns optimized for their ecosystems:

```
┌────────────────────────────────────────────────────────────────────────┐
│                 Python vs Go Module Design                             │
└────────────────────────────────────────────────────────────────────────┘

PYTHON (Standalone Commands)         GO (Sub-Enum Pattern)
──────────────────────────           ─────────────────────

Commands::Ruff { args }       ──────  Commands::Go {
Commands::Pytest { args }              Test { args },
Commands::Pip { args }                 Build { args },
                                       Vet { args }
                                     }
├─ ruff_cmd.rs                       Commands::GolangciLint { args }
├─ pytest_cmd.rs                     │
└─ pip_cmd.rs                        ├─ go_cmd.rs (sub-enum router)
                                     └─ golangci_cmd.rs

Mirrors: lint, prettier              Mirrors: git, cargo
```

### Python Stack Architecture

#### Command Implementations

```
┌────────────────────────────────────────────────────────────────────────┐
│                      Python Commands (3 modules)                       │
└────────────────────────────────────────────────────────────────────────┘

Module            Strategy              Output Format      Savings
─────────────────────────────────────────────────────────────────────────

ruff_cmd.rs       JSON/TEXT DUAL        • check → JSON    80%+
                                        • format → text

  ruff check:  JSON API with structured violations
    {
      "violations": [{"rule": "F401", "file": "x.py", "line": 5}]
    }
    → Group by rule, count occurrences

  ruff format: Text diff output
    "Fixed 12 files"
    → Extract summary, hide unchanged files

pytest_cmd.rs     STATE MACHINE         Text parser       90%+

  State tracking: IDLE → TEST_START → PASSED/FAILED → SUMMARY
  Extract:
    • Test names (test_auth_login)
    • Outcomes (PASSED ✓ / FAILED ✗)
    • Failures only (hide passing tests)

pip_cmd.rs        JSON PARSING          JSON API          70-85%

  pip list --format=json:
    [{"name": "requests", "version": "2.28.1"}]
    → Compact table format

  pip show <pkg>: JSON metadata
    {"name": "...", "version": "...", "requires": [...]}
    → Extract key fields only

  Auto-detect uv: If uv exists, use uv pip instead
```

#### Shared Infrastructure

**No Package Manager Detection**
Unlike JS/TS modules, Python commands don't auto-detect poetry/pipenv/pip because:
- `pip` is universally available (system Python)
- `uv` detection is explicit (binary presence check)
- Poetry/pipenv aren't execution wrappers (they manage virtualenvs differently)

**Virtual Environment Awareness**
Commands respect active virtualenv via `sys.executable` paths.

### Go Stack Architecture

#### Command Implementations

```
┌────────────────────────────────────────────────────────────────────────┐
│                       Go Commands (2 modules)                          │
└────────────────────────────────────────────────────────────────────────┘

Module            Strategy              Output Format      Savings
─────────────────────────────────────────────────────────────────────────

go_cmd.rs         SUB-ENUM ROUTER       Mixed formats     75-90%

  go test:  NDJSON STREAMING
    {"Action": "run", "Package": "pkg1", "Test": "TestAuth"}
    {"Action": "fail", "Package": "pkg1", "Test": "TestAuth"}

    → Line-by-line JSON parse (handles interleaved package events)
    → Aggregate: "2 packages, 3 failures (pkg1::TestAuth, ...)"

  go build: TEXT FILTERING
    Errors only (compiler diagnostics)
    → Strip warnings, show errors with file:line

  go vet:   TEXT FILTERING
    Issue detection output
    → Extract file:line:message triples

golangci_cmd.rs   JSON PARSING          JSON API          85%

  golangci-lint run --out-format=json:
    {
      "Issues": [
        {"FromLinter": "errcheck", "Pos": {...}, "Text": "..."}
      ]
    }
    → Group by linter rule, count violations
    → Format: "errcheck: 12 issues, gosec: 5 issues"
```

#### Sub-Enum Pattern (go_cmd.rs)

```rust
// main.rs enum definition
Commands::Go {
    #[command(subcommand)]
    command: GoCommand,
}

// go_cmd.rs sub-enum
pub enum GoCommand {
    Test { args: Vec<String> },
    Build { args: Vec<String> },
    Vet { args: Vec<String> },
}

// Router
pub fn run(command: &GoCommand, verbose: u8) -> Result<()> {
    match command {
        GoCommand::Test { args } => run_test(args, verbose),
        GoCommand::Build { args } => run_build(args, verbose),
        GoCommand::Vet { args } => run_vet(args, verbose),
    }
}
```

**Why Sub-Enum?**
- `go test/build/vet` are semantically related (core Go toolchain)
- Mirrors existing git/cargo patterns (consistency)
- Natural CLI: `rtk go test` not `rtk gotest`

**Why golangci-lint Standalone?**
- Third-party tool (not core Go toolchain)
- Different output format (JSON API vs text)
- Distinct use case (comprehensive linting vs single-tool diagnostics)

### Format Strategy Decision Tree

```
Output format known?
├─ Tool provides JSON flag?
│  ├─ Structured data needed? → Use JSON API
│  │    Examples: ruff check, pip list, golangci-lint
│  │
│  └─ Simple output? → Use text mode
│       Examples: ruff format, go build errors
│
├─ Streaming events (NDJSON)?
│  └─ Line-by-line JSON parse
│       Examples: go test (interleaved packages)
│
└─ Plain text only?
   ├─ Stateful parsing needed? → State machine
   │    Examples: pytest (test lifecycle tracking)
   │
   └─ Simple filtering? → Text filters
        Examples: go vet, go build
```

### Testing Patterns

#### Python Module Tests

```rust
// pytest_cmd.rs tests
#[test]
fn test_pytest_state_machine() {
    let output = "test_auth.py::test_login PASSED\ntest_db.py::test_query FAILED";
    let result = parse_pytest_output(output);
    assert!(result.contains("1 failed"));
    assert!(result.contains("test_db.py::test_query"));
}
```

#### Go Module Tests

```rust
// go_cmd.rs tests
#[test]
fn test_go_test_ndjson_interleaved() {
    let output = r#"{"Action":"run","Package":"pkg1"}
{"Action":"fail","Package":"pkg1","Test":"TestA"}
{"Action":"run","Package":"pkg2"}
{"Action":"pass","Package":"pkg2","Test":"TestB"}"#;

    let result = parse_go_test_ndjson(output);
    assert!(result.contains("pkg1: 1 failed"));
    assert!(!result.contains("pkg2")); // pkg2 passed, hidden
}
```

### Performance Characteristics

```
┌────────────────────────────────────────────────────────────────────────┐
│              Python/Go Module Overhead Benchmarks                      │
└────────────────────────────────────────────────────────────────────────┘

Command                 Raw Time    rtk Time    Overhead    Savings
─────────────────────────────────────────────────────────────────────────

ruff check              850ms       862ms       +12ms       83%
pytest                  1.2s        1.21s       +10ms       92%
pip list                450ms       458ms       +8ms        78%

go test                 2.1s        2.12s       +20ms       88%
go build (errors)       950ms       961ms       +11ms       80%
golangci-lint           4.5s        4.52s       +20ms       85%

Overhead Sources:
  • JSON parsing: 5-10ms (serde_json)
  • State machine: 3-8ms (regex + state tracking)
  • NDJSON streaming: 8-15ms (line-by-line JSON parse)
```

### Module Integration Checklist

When adding Python/Go module support:

- [x] **Output Format**: JSON API > NDJSON > State Machine > Text Filters
- [x] **Failure Focus**: Hide passing tests, show failures only
- [x] **Exit Code Preservation**: Propagate tool exit codes for CI/CD
- [x] **Virtual Env Awareness**: Python modules respect active virtualenv
- [x] **Error Grouping**: Group by rule/file for linters (ruff, golangci-lint)
- [x] **Streaming Support**: Handle interleaved NDJSON events (go test)
- [x] **Verbosity Levels**: Support -v/-vv/-vvv for debug output
- [x] **Token Tracking**: Integrate with tracking::track()
- [x] **Unit Tests**: Test parsing logic with representative outputs

---

## Shared Infrastructure

### Utilities Layer (utils.rs)

```
┌────────────────────────────────────────────────────────────────────────┐
│                       Shared Utilities Layer                           │
└────────────────────────────────────────────────────────────────────────┘

utils.rs provides common functionality:

┌─────────────────────────────────────────┐
│ truncate(s: &str, max: usize) → String  │  Text truncation with "..."
├─────────────────────────────────────────┤
│ strip_ansi(text: &str) → String         │  Remove ANSI color codes
├─────────────────────────────────────────┤
│ execute_command(cmd, args)              │  Shell execution helper
│   → (stdout, stderr, exit_code)         │  with error context
└─────────────────────────────────────────┘

Used by: All command modules (24 modules depend on utils.rs)
```

### Package Manager Detection Pattern

**Critical Infrastructure for JS/TS Stack (8 modules)**

```
┌────────────────────────────────────────────────────────────────────────┐
│                   Package Manager Detection Flow                       │
└────────────────────────────────────────────────────────────────────────┘

Detection Order:
┌─────────────────────────────────────┐
│ 1. Check: pnpm-lock.yaml exists?   │
│    → Yes: pnpm exec -- <tool>      │
│                                     │
│ 2. Check: yarn.lock exists?        │
│    → Yes: yarn exec -- <tool>      │
│                                     │
│ 3. Fallback: Use npx               │
│    → npx --no-install -- <tool>    │
└─────────────────────────────────────┘

Example (lint_cmd.rs:50-77):

let is_pnpm = Path::new("pnpm-lock.yaml").exists();
let is_yarn = Path::new("yarn.lock").exists();

let mut cmd = if is_pnpm {
    Command::new("pnpm").arg("exec").arg("--").arg("eslint")
} else if is_yarn {
    Command::new("yarn").arg("exec").arg("--").arg("eslint")
} else {
    Command::new("npx").arg("--no-install").arg("--").arg("eslint")
};

Affects: lint, tsc, next, prettier, playwright, prisma, vitest, pnpm
```

**Why This Matters**:
- **CWD Preservation**: pnpm/yarn exec preserve working directory correctly
- **Monorepo Support**: Works in nested package.json structures
- **No Global Installs**: Uses project-local dependencies only
- **CI/CD Reliability**: Consistent behavior across environments

---

## Token Tracking System

### SQLite-Based Metrics

```
┌────────────────────────────────────────────────────────────────────────┐
│                      Token Tracking Architecture                       │
└────────────────────────────────────────────────────────────────────────┘

Flow:

1. ESTIMATION (tracking.rs:235-238)
   ────────────
   estimate_tokens(text: &str) → usize {
       (text.len() as f64 / 4.0).ceil() as usize
   }

   Heuristic: ~4 characters per token (GPT-style tokenization)

         ↓

2. CALCULATION
   ───────────
   input_tokens  = estimate_tokens(raw_output)
   output_tokens = estimate_tokens(filtered_output)
   saved_tokens  = input_tokens - output_tokens
   savings_pct   = (saved / input) × 100.0

         ↓

3. RECORD (tracking.rs:48-59)
   ──────
   INSERT INTO commands (
       timestamp,      -- RFC3339 format
       original_cmd,   -- "git log --oneline -5"
       rtk_cmd,        -- "rtk git log --oneline -5"
       input_tokens,   -- 125
       output_tokens,  -- 5
       saved_tokens,   -- 120
       savings_pct,    -- 96.0
       exec_time_ms    -- 15 (execution duration in milliseconds)
   ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)

         ↓

4. STORAGE
   ───────
   Database: ~/.local/share/rtk/history.db

   Schema:
   ┌─────────────────────────────────────────┐
   │ commands                                │
   ├─────────────────────────────────────────┤
   │ id              INTEGER PRIMARY KEY     │
   │ timestamp       TEXT NOT NULL           │
   │ original_cmd    TEXT NOT NULL           │
   │ rtk_cmd         TEXT NOT NULL           │
   │ input_tokens    INTEGER NOT NULL        │
   │ output_tokens   INTEGER NOT NULL        │
   │ saved_tokens    INTEGER NOT NULL        │
   │ savings_pct     REAL NOT NULL           │
   │ exec_time_ms    INTEGER DEFAULT 0       │
   └─────────────────────────────────────────┘

   Note: exec_time_ms tracks command execution duration
   (added in v0.7.1, historical records default to 0)

         ↓

5. CLEANUP (tracking.rs:96-104)
   ───────
   Auto-cleanup on each INSERT:
   DELETE FROM commands
   WHERE timestamp < datetime('now', '-90 days')

   Retention: 90 days (HISTORY_DAYS constant)

         ↓

6. REPORTING (gain.rs)
   ────────
   $ rtk gain

   Query:
   SELECT
       COUNT(*) as total_commands,
       SUM(saved_tokens) as total_saved,
       AVG(savings_pct) as avg_savings,
       SUM(exec_time_ms) as total_time_ms,
       AVG(exec_time_ms) as avg_time_ms
   FROM commands
   WHERE timestamp > datetime('now', '-90 days')

   Output:
   ┌──────────────────────────────────────┐
   │ Token Savings Report (90 days)      │
   ├──────────────────────────────────────┤
   │ Commands executed:  1,234           │
   │ Average savings:    78.5%           │
   │ Total tokens saved: 45,678          │
   │ Total exec time:    8m50s (573ms)   │
   │                                      │
   │ Top commands:                       │
   │   • rtk git status    (234 uses)    │
   │   • rtk lint          (156 uses)    │
   │   • rtk test          (89 uses)     │
   └──────────────────────────────────────┘

   Note: Time column shows average execution
   duration per command (added in v0.7.1)
```

### Thread Safety

```rust
// tracking.rs:9-11
lazy_static::lazy_static! {
    static ref TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);
}
```

**Design**: Single-threaded execution with Mutex for future-proofing.
**Current State**: No multi-threading, but Mutex enables safe concurrent access if needed.

---

## Global Flags Architecture

### Verbosity System

```
┌────────────────────────────────────────────────────────────────────────┐
│                         Verbosity Levels                               │
└────────────────────────────────────────────────────────────────────────┘

main.rs:47-49
#[arg(short, long, action = clap::ArgAction::Count, global = true)]
verbose: u8,

Levels:
┌─────────┬──────────────────────────────────────────────────────┐
│ Flag    │ Behavior                                             │
├─────────┼──────────────────────────────────────────────────────┤
│ (none)  │ Compact output only                                  │
│ -v      │ + Debug messages (eprintln! statements)              │
│ -vv     │ + Command being executed                             │
│ -vvv    │ + Raw output before filtering                        │
└─────────┴──────────────────────────────────────────────────────┘

Example (git.rs:67-69):
if verbose > 0 {
    eprintln!("Git diff summary:");
}
```

### Ultra-Compact Mode

```
┌────────────────────────────────────────────────────────────────────────┐
│                       Ultra-Compact Mode (-u)                          │
└────────────────────────────────────────────────────────────────────────┘

main.rs:51-53
#[arg(short = 'u', long, global = true)]
ultra_compact: bool,

Features:
┌──────────────────────────────────────────────────────────────────────┐
│ • ASCII icons instead of words (✓ ✗ → ⚠)                            │
│ • Inline formatting (single-line summaries)                          │
│ • Maximum compression for LLM contexts                               │
└──────────────────────────────────────────────────────────────────────┘

Example (gh_cmd.rs:521):
if ultra_compact {
    println!("✓ PR #{} merged", number);
} else {
    println!("Pull request #{} successfully merged", number);
}
```

---

## Error Handling

### anyhow::Result<()> Propagation Chain

```
┌────────────────────────────────────────────────────────────────────────┐
│                      Error Handling Architecture                       │
└────────────────────────────────────────────────────────────────────────┘

Propagation Chain:

main() → Result<()>
  ↓
  match cli.command {
      Commands::Git { args, .. } => git::run(&args, verbose)?,
      ...
  }
  ↓ .context("Git command failed")
git::run(args: &[String], verbose: u8) → Result<()>
  ↓ .context("Failed to execute git")
git::execute_git_command() → Result<String>
  ↓ .context("Git process error")
Command::new("git").output()?
  ↓ Error occurs
anyhow::Error
  ↓ Bubble up through ?
main.rs error display
  ↓
eprintln!("Error: {:#}", err)
  ↓
std::process::exit(1)
```

### Exit Code Preservation (Critical for CI/CD)

```
┌────────────────────────────────────────────────────────────────────────┐
│                    Exit Code Handling Strategy                         │
└────────────────────────────────────────────────────────────────────────┘

Standard Pattern (git.rs:45-48, PR #5):

let output = Command::new("git").args(args).output()?;

if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("{}", stderr);
    std::process::exit(output.status.code().unwrap_or(1));
}

Exit Codes:
┌─────────┬──────────────────────────────────────────────────────┐
│ Code    │ Meaning                                              │
├─────────┼──────────────────────────────────────────────────────┤
│ 0       │ Success                                              │
│ 1       │ rtk internal error (parsing, filtering, etc.)        │
│ N       │ Preserved exit code from underlying tool            │
│         │ (e.g., git returns 128, lint returns 1)             │
└─────────┴──────────────────────────────────────────────────────┘

Why This Matters:
• CI/CD pipelines rely on exit codes to determine build success/failure
• Pre-commit hooks need accurate failure signals
• Git workflows require proper exit code propagation (PR #5 fix)

Modules with Exit Code Preservation:
• git.rs (all git commands)
• lint_cmd.rs (linter failures)
• tsc_cmd.rs (TypeScript errors)
• vitest_cmd.rs (test failures)
• playwright_cmd.rs (E2E test failures)
```

---

## Configuration System

### Two-Tier Configuration

```
┌────────────────────────────────────────────────────────────────────────┐
│                     Configuration Architecture                         │
└────────────────────────────────────────────────────────────────────────┘

1. User Settings (config.toml)
   ───────────────────────────
   Location: ~/.config/rtk/config.toml

   Format:
   [general]
   default_filter_level = "minimal"
   enable_tracking = true
   retention_days = 90

   Loaded by: config.rs (main.rs:650-656)

2. LLM Integration (CLAUDE.md)
   ────────────────────────────
   Locations:
   • Global: ~/.config/rtk/CLAUDE.md
   • Local:  ./CLAUDE.md (project-specific)

   Purpose: Instruct LLM (Claude Code) to use rtk prefix
   Created by: rtk init [--global]

   Template (init.rs:40-60):
   # CLAUDE.md
   Use `rtk` prefix for all commands:
   - rtk git status
   - rtk grep "pattern"
   - rtk read file.rs

   Benefits: 60-90% token reduction
```

### Initialization Flow

```
┌────────────────────────────────────────────────────────────────────────┐
│                      rtk init Workflow                                 │
└────────────────────────────────────────────────────────────────────────┘

$ rtk init [--global]
      ↓
Check existing CLAUDE.md:
  • --global? → ~/.config/rtk/CLAUDE.md
  • else      → ./CLAUDE.md
      ↓
      ├─ Exists? → Warn user, ask to overwrite
      └─ Not exists? → Continue
      ↓
Prompt: "Initialize rtk for LLM usage? [y/N]"
      ↓ Yes
Write template:
┌─────────────────────────────────────┐
│ # CLAUDE.md                         │
│                                     │
│ Use `rtk` prefix for commands:      │
│ - rtk git status                    │
│ - rtk lint                          │
│ - rtk test                          │
│                                     │
│ Benefits: 60-90% token reduction    │
└─────────────────────────────────────┘
      ↓
Success: "✓ Initialized rtk for LLM integration"
```

---

## Module Development Pattern

### Standard Module Template

```rust
// src/example_cmd.rs

use anyhow::{Context, Result};
use std::process::Command;
use crate::{tracking, utils};

/// Public entry point called by main.rs router
pub fn run(args: &[String], verbose: u8) -> Result<()> {
    // 1. Execute underlying command
    let raw_output = execute_command(args)?;

    // 2. Apply filtering strategy
    let filtered = filter_output(&raw_output, verbose);

    // 3. Print result
    println!("{}", filtered);

    // 4. Track token savings
    tracking::track(
        "original_command",
        "rtk command",
        &raw_output,
        &filtered
    );

    Ok(())
}

/// Execute the underlying tool
fn execute_command(args: &[String]) -> Result<String> {
    let output = Command::new("tool")
        .args(args)
        .output()
        .context("Failed to execute tool")?;

    // Preserve exit codes (critical for CI/CD)
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("{}", stderr);
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Apply filtering strategy
fn filter_output(raw: &str, verbose: u8) -> String {
    // Choose strategy: stats, grouping, deduplication, etc.
    // See "Filtering Strategies" section for options

    if verbose >= 3 {
        eprintln!("Raw output:\n{}", raw);
    }

    // Apply compression logic
    let compressed = compress(raw);

    compressed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_output() {
        let raw = "verbose output here";
        let filtered = filter_output(raw, 0);
        assert!(filtered.len() < raw.len());
    }
}
```

### Common Patterns

#### 1. Package Manager Detection (JS/TS modules)

```rust
// Detect lockfiles
let is_pnpm = Path::new("pnpm-lock.yaml").exists();
let is_yarn = Path::new("yarn.lock").exists();

// Build command
let mut cmd = if is_pnpm {
    Command::new("pnpm").arg("exec").arg("--").arg("eslint")
} else if is_yarn {
    Command::new("yarn").arg("exec").arg("--").arg("eslint")
} else {
    Command::new("npx").arg("--no-install").arg("--").arg("eslint")
};
```

#### 2. Lazy Static Regex (filter.rs, runner.rs)

```rust
lazy_static::lazy_static! {
    static ref PATTERN: Regex = Regex::new(r"ERROR:.*").unwrap();
}

// Usage: compiled once, reused across invocations
let matches: Vec<_> = PATTERN.find_iter(text).collect();
```

#### 3. Verbosity Guards

```rust
if verbose > 0 {
    eprintln!("Debug: Processing {} files", count);
}

if verbose >= 2 {
    eprintln!("Executing: {:?}", cmd);
}

if verbose >= 3 {
    eprintln!("Raw output:\n{}", raw);
}
```

---

## Build Optimizations

### Release Profile (Cargo.toml)

```toml
[profile.release]
opt-level = 3          # Maximum optimization
lto = true             # Link-time optimization
codegen-units = 1      # Single codegen unit for better optimization
strip = true           # Remove debug symbols
panic = "abort"        # Smaller binary size
```

### Performance Characteristics

```
┌────────────────────────────────────────────────────────────────────────┐
│                      Performance Metrics                               │
└────────────────────────────────────────────────────────────────────────┘

Binary:
  • Size: ~4.1 MB (stripped release build)
  • Startup: ~5-10ms (cold start)
  • Memory: ~2-5 MB (typical usage)

Runtime Overhead (estimated):
┌──────────────────────┬──────────────┬──────────────┐
│ Operation            │ rtk Overhead │ Total Time   │
├──────────────────────┼──────────────┼──────────────┤
│ rtk git status       │ +8ms         │ 58ms         │
│ rtk grep "pattern"   │ +12ms        │ 145ms        │
│ rtk read file.rs     │ +5ms         │ 15ms         │
│ rtk lint             │ +15ms        │ 2.5s         │
└──────────────────────┴──────────────┴──────────────┘

Note: Overhead measurements are estimates. Actual performance varies
by system, command complexity, and output size.

Overhead Sources:
  • Clap parsing: ~2-3ms
  • Command execution: ~1-2ms
  • Filtering/compression: ~2-8ms (varies by strategy)
  • SQLite tracking: ~1-3ms
```

### Compilation

```bash
# Development build (fast compilation, debug symbols)
cargo build

# Release build (optimized, stripped)
cargo build --release

# Check without building (fast feedback)
cargo check

# Run tests
cargo test

# Lint with clippy
cargo clippy --all-targets

# Format code
cargo fmt
```

---

## Extensibility Guide

### Adding a New Command

**Step-by-step process to add a new rtk command:**

#### 1. Create Module File

```bash
touch src/mycmd.rs
```

#### 2. Implement Module (src/mycmd.rs)

```rust
use anyhow::{Context, Result};
use std::process::Command;
use crate::tracking;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    // Execute underlying command
    let output = Command::new("mycmd")
        .args(args)
        .output()
        .context("Failed to execute mycmd")?;

    let raw = String::from_utf8_lossy(&output.stdout);

    // Apply filtering strategy
    let filtered = filter(&raw, verbose);

    // Print result
    println!("{}", filtered);

    // Track savings
    tracking::track("mycmd", "rtk mycmd", &raw, &filtered);

    Ok(())
}

fn filter(raw: &str, verbose: u8) -> String {
    // Implement your filtering logic
    raw.lines().take(10).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter() {
        let raw = "line1\nline2\n";
        let result = filter(raw, 0);
        assert!(result.contains("line1"));
    }
}
```

#### 3. Declare Module (main.rs)

```rust
// Add to module declarations (alphabetically)
mod mycmd;
```

#### 4. Add Command Enum Variant (main.rs)

```rust
#[derive(Subcommand)]
enum Commands {
    // ... existing commands ...

    /// Description of your command
    Mycmd {
        /// Arguments your command accepts
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}
```

#### 5. Add Router Match Arm (main.rs)

```rust
match cli.command {
    // ... existing matches ...

    Commands::Mycmd { args } => {
        mycmd::run(&args, verbose)?;
    }
}
```

#### 6. Test Your Command

```bash
# Build and test
cargo build
./target/debug/rtk mycmd arg1 arg2

# Run tests
cargo test mycmd::tests

# Check with clippy
cargo clippy --all-targets
```

#### 7. Document Your Command

Update CLAUDE.md:

```markdown
### New Commands

**rtk mycmd** - Description of what it does
- Strategy: [stats/grouping/filtering/etc.]
- Savings: X-Y%
- Used by: [workflow description]
```

### Design Checklist

When implementing a new command, consider:

- [ ] **Filtering Strategy**: Which of the 9 strategies fits best?
- [ ] **Exit Code Preservation**: Does your command need to preserve exit codes for CI/CD?
- [ ] **Verbosity Support**: Add debug output for `-v`, `-vv`, `-vvv`
- [ ] **Error Handling**: Use `.context()` for meaningful error messages
- [ ] **Package Manager Detection**: For JS/TS tools, use the standard detection pattern
- [ ] **Tests**: Add unit tests for filtering logic
- [ ] **Token Tracking**: Integrate with `tracking::track()`
- [ ] **Documentation**: Update CLAUDE.md with token savings and use cases

---

## Architecture Decision Records

### Why Rust?

- **Performance**: ~5-15ms overhead per command (negligible for user experience)
- **Safety**: No runtime errors from null pointers, data races, etc.
- **Single Binary**: No runtime dependencies (distribute one executable)
- **Cross-Platform**: Works on macOS, Linux, Windows without modification

### Why SQLite for Tracking?

- **Zero Config**: No server setup, works out-of-the-box
- **Lightweight**: ~100KB database for 90 days of history
- **Reliable**: ACID compliance for data integrity
- **Queryable**: Rich analytics via SQL (gain report)

### Why anyhow for Error Handling?

- **Context**: `.context()` adds meaningful error messages throughout call chain
- **Ergonomic**: `?` operator for concise error propagation
- **User-Friendly**: Error display shows full context chain

### Why Clap for CLI Parsing?

- **Derive Macros**: Less boilerplate (declarative CLI definition)
- **Auto-Generated Help**: `--help` generated automatically
- **Type Safety**: Parse arguments directly into typed structs
- **Global Flags**: `-v` and `-u` work across all commands

---

## Resources

- **README.md**: User guide, installation, examples
- **CLAUDE.md**: Developer documentation, module details, PR history
- **Cargo.toml**: Dependencies, build profiles, package metadata
- **src/**: Source code organized by module
- **.github/workflows/**: CI/CD automation (multi-platform builds, releases)

---

## Glossary

| Term | Definition |
|------|------------|
| **Token** | Unit of text processed by LLMs (~4 characters on average) |
| **Filtering** | Reducing output size while preserving essential information |
| **Proxy Pattern** | rtk sits between user and tool, transforming output |
| **Exit Code Preservation** | Passing through tool's exit code for CI/CD reliability |
| **Package Manager Detection** | Identifying pnpm/yarn/npm to execute JS/TS tools correctly |
| **Verbosity Levels** | `-v/-vv/-vvv` for progressively more debug output |
| **Ultra-Compact** | `-u` flag for maximum compression (ASCII icons, inline format) |

---

**Last Updated**: 2026-02-12
**Architecture Version**: 2.1
**rtk Version**: 0.15.1
