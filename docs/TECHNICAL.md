# RTK Technical Documentation

> **Start here** for a guided tour of how RTK works end-to-end.
>
> - [CONTRIBUTING.md](../CONTRIBUTING.md) — Design philosophy, PR process, branch naming, testing requirements
> - [ARCHITECTURE.md](../ARCHITECTURE.md) — Deep reference: filtering taxonomy, performance benchmarks, architecture decisions
> - Each folder has its own `README.md` with implementation details and file descriptions

---

## 1. Project Vision

LLM-powered coding agents (Claude Code, Copilot, Cursor, etc.) consume tokens for every CLI command output they process. Most command outputs contain boilerplate, progress bars, ANSI escape codes, and verbose formatting that wastes tokens without providing actionable information.

RTK sits between the agent and the CLI, filtering outputs to keep only what matters. This achieves 60-90% token savings per command, reducing costs and increasing effective context window utilization. RTK is a single Rust binary with no runtime dependencies beyond the compiled binary itself, adding less than 10ms overhead per command.

---

## 2. Architecture Overview

```
User / LLM Agent
       |
       v
+--------------------------------------------------+
|  LLM Agent Hook                                  |
|  hooks/{claude,copilot,cursor,...}/               |
|  Intercepts: "git status" -> "rtk git status"    |
+-------------------------+------------------------+
                          |
                          v
+--------------------------------------------------+
|  RTK CLI (main.rs)                               |
|                                                  |
|  +-------------+    +-----------------+          |
|  | Clap Parser | -> | Command Routing |          |
|  | (Commands   |    | (match on enum) |          |
|  |  enum)      |    +--------+--------+          |
|  +-------------+             |                   |
|                    +---------+---------+         |
|                    v         v         v         |
|             +----------+ +--------+ +----------+|
|             |Rust Filter| |TOML DSL| |Passthru  ||
|             |(cmds/**)  | |Filter  | |(fallback)||
|             +-----+----+ +----+---+ +----+-----+|
|                   |           |           |      |
|                   +-----+-----+-----------+      |
|                         v                        |
|              +---------------------+             |
|              |   Token Tracking    |             |
|              |   (core/tracking)   |             |
|              |   SQLite DB         |             |
|              +---------------------+             |
+--------------------------------------------------+
```

**Design principles:**
- Single-threaded, no async (startup < 10ms)
- Graceful degradation: filter failure falls back to raw output
- Exit code propagation: RTK never swallows non-zero exits
- Transparent proxy: unknown commands pass through unchanged

---

## 3. End-to-End Flow

This is the full lifecycle of a command through RTK, from LLM agent to filtered output.

### 3.1 Hook Installation (`rtk init`)

The user runs `rtk init` to set up hooks for their LLM agent. This:

1. Writes a thin shell hook script (e.g., `~/.claude/hooks/rtk-rewrite.sh`)
2. Stores its SHA-256 hash for integrity verification
3. Patches the agent's settings file (e.g., `settings.json`) to register the hook
4. Writes RTK awareness instructions (e.g., `RTK.md`) for prompt-level guidance

RTK supports 7 agents, each with its own installation mode. The hook scripts are embedded in the binary and written at install time.

> **Details**: [`src/hooks/README.md`](../src/hooks/README.md) covers all installation modes, configuration files, and the uninstall flow.

### 3.2 Hook Interception (Command Rewriting)

When an LLM agent runs a command (e.g., `git status`):

1. The agent fires a `PreToolUse` event (or equivalent) containing the command as JSON
2. The hook script reads the JSON, extracts the command string
3. The hook calls `rtk rewrite "git status"` as a subprocess
4. `rtk rewrite` consults the command registry and returns `rtk git status`
5. The hook sends a response telling the agent to use the rewritten command
6. If anything fails (jq missing, rtk not found, no match), the hook exits silently -- the raw command runs unchanged

All rewrite logic lives in Rust (`src/discover/registry.rs`). Hooks are thin delegates that handle agent-specific JSON formats.

> **Details**: [`hooks/README.md`](../hooks/README.md) covers each agent's JSON format, the rewrite registry, compound command handling, and the `RTK_DISABLED` override.

#### Rewrite Pipeline

The rewrite pipeline is how RTK intercepts and rewrites commands. The call chain is:

```
hook shell → rewrite_cmd.rs → rewrite_command() → rewrite_compound() → rewrite_segment() → classify_command()
```

Traced step by step for `cargo fmt --all && cargo test 2>&1 | tail -20`:

```
LLM Agent: "cargo fmt --all && cargo test 2>&1 | tail -20"
  |
  |  Hook shell (hooks/claude/rtk-rewrite.sh)
  |  Reads JSON from agent, extracts command, calls `rtk rewrite "$CMD"`
  |  On failure (jq missing, rtk missing, old version): exit 0 (passthrough)
  |
  v
rewrite_cmd::run(cmd)                              [src/hooks/rewrite_cmd.rs]
  |  1. Load config → hooks.exclude_commands
  |  2. check_command(cmd) → Deny → exit(2)
  |  3. registry::rewrite_command(cmd, excluded)
  |     → None → exit(1)          (no RTK equivalent, passthrough)
  |     → Some + Allow → print, exit(0)
  |     → Some + Ask   → print, exit(3)
  |
  v
rewrite_command(cmd, excluded)                     [src/discover/registry.rs]
  |  Early exits:
  |  - Empty → None
  |  - Contains "<<" or "$((" (heredoc/arithmetic) → None
  |  - Simple "rtk ..." (no operators) → return as-is
  |  - Otherwise → rewrite_compound(cmd, excluded)
  |
  v
rewrite_compound(cmd, excluded)                    [src/discover/registry.rs]
  |
  |  Step 1 — Tokenize (lexer.rs)
  |  tokenize() produces typed tokens with byte offsets:
  |    Arg("cargo") Arg("fmt") Arg("--all")
  |    Operator("&&")
  |    Arg("cargo") Arg("test") Redirect("2>&1")
  |    Pipe("|")
  |    Arg("tail") Arg("-20")
  |
  |  Step 2 — Split on operators, rewrite each segment
  |  Operator (&&, ||, ;) → rewrite both sides
  |  Pipe (|) → rewrite left side only, keep right side raw
  |             exception: find/fd before pipe → skip rewrite
  |  Shellism (&) → rewrite both sides (background)
  |
  |  Calls rewrite_segment() per segment:
  |    segment 1: "cargo fmt --all"
  |    segment 2: "cargo test 2>&1"
  |    after pipe: "tail -20" kept raw
  |
  v
rewrite_segment(seg, excluded)                     [src/discover/registry.rs]
  |
  |  Step 3 — Strip trailing redirects
  |  strip_trailing_redirects() re-tokenizes the segment:
  |    "cargo test 2>&1" → cmd_part="cargo test", redirect=" 2>&1"
  |  (simple commands like "cargo fmt --all" → no redirect, suffix is "")
  |
  |  Step 4 — Already RTK → return as-is
  |
  |  Step 5 — Special cases (short-circuit before classification)
  |  head -N / --lines=N → rewrite_line_range() → "rtk read file --max-lines N"
  |  tail -N / -n N / --lines N → rewrite_line_range() → "rtk read file --tail-lines N"
  |  head/tail with unsupported flag (-c, -f) → None (skip rewrite)
  |  cat with incompatible flag (-A, -v, -e) → None (skip rewrite)
  |
  |  Step 6 — classify_command(cmd_part) [see below]
  |  → Supported → check excluded list → continue
  |  → Unsupported/Ignored → None (skip rewrite)
  |
  |  Step 7 — Build rewritten command
  |  a. Find matching rule from rules.rs
  |  b. Extract env prefix (ENV_PREFIX regex, second pass — first was in classify)
  |     e.g. "GIT_SSH_COMMAND=\"ssh -o ...\" git push" → prefix="GIT_SSH_COMMAND=..."
  |  c. Guard: RTK_DISABLED=1 in prefix → None
  |  d. Guard: gh with --json/--jq/--template → None
  |  e. Apply rule's rewrite_prefixes: "cargo fmt" → "rtk cargo fmt"
  |  f. Reassemble: env_prefix + rtk_cmd + args + redirect_suffix
  |
  v
classify_command(cmd)                              [src/discover/registry.rs]
  |  1. Check IGNORED_EXACT (cd, echo, fi, done, ...)
  |  2. Check IGNORED_PREFIXES (rtk, mkdir, mv, ...)
  |  3. Strip env prefix with ENV_PREFIX regex (for pattern matching only)
  |  4. Normalize absolute paths: /usr/bin/grep → grep
  |  5. Strip git global opts: git -C /tmp status → git status
  |  6. Guard: cat/head/tail with redirect (>, >>) → Unsupported (write, not read)
  |  7. Match against REGEX_SET (60+ compiled patterns from rules.rs)
  |  8. Extract subcommand → lookup custom savings/status overrides
  |  9. Return Classification::Supported { rtk_equivalent, category, savings, status }
  |
  v
Result: "rtk cargo fmt --all && rtk cargo test 2>&1 | tail -20"
  |
  |  Hook response
  |  Hook wraps result in agent-specific JSON, returns to LLM agent
  |
  v
LLM Agent executes rewritten command
  (bash handles && and |, each rtk invocation is a separate process)
```

Key design decisions:
- **Lexer-based tokenization**: A single-pass state machine (`lexer.rs`) handles all shell constructs (quotes, escapes, redirects, operators). Used for both compound splitting and redirect stripping.
- **Segment-level rewriting**: Compound commands are split by operators, each segment rewritten independently. Bash recombines them at execution time.
- **Pipe semantics**: Only the left side of `|` is rewritten. The pipe consumer (grep, head, wc) runs raw. `find`/`fd` before a pipe is never rewritten (output format incompatible with xargs).
- **Double env prefix handling**: `classify_command()` strips env prefixes to match the underlying command against rules. `rewrite_segment()` extracts the same prefix separately to re-prepend it to the rewritten command.
- **Fallback contract**: If any segment fails to match, it stays raw. `rewrite_command()` returns `None` only when zero segments were rewritten.

### 3.3 CLI Parsing and Routing

Once the rewritten command reaches RTK:

1. **Telemetry**: `telemetry::maybe_ping()` fires a non-blocking daily usage ping
2. **Clap parsing**: `Cli::try_parse()` matches against the `Commands` enum
3. **Hook check**: `hook_check::maybe_warn()` warns if the installed hook is outdated (rate-limited to 1/day)
4. **Integrity check**: `integrity::runtime_check()` verifies the hook's SHA-256 hash for operational commands
5. **Routing**: A `match cli.command` dispatches to the specialized filter module

If Clap parsing fails (command not in the enum), the fallback path runs instead.

### 3.4 Filter Execution

RTK has two filter systems:

**Rust Filters**: Compiled modules in `src/cmds/` that execute the command, parse its output, and apply specialized transformations (regex, JSON, state machines).

**TOML DSL Filters**: Declarative filters in `src/filters/*.toml` that apply regex-based line filtering, truncation, and section extraction. Applied in `run_fallback()` when no Rust filter matches.

Each filter module follows the same pattern:
1. Start a timer (`TimedExecution::start()`)
2. Execute the underlying command (`std::process::Command`)
3. Apply filtering (strip boilerplate, group errors, truncate)
4. On filter error, fall back to raw output
5. Track token savings to SQLite
6. Propagate exit code

> **Details**: [`src/cmds/README.md`](../src/cmds/README.md) covers the common pattern, ecosystem organization, cross-command dependencies, and how to add new filters.

### 3.5 Fallback Path

When Clap parsing fails (unknown command):

1. Guard: check if the command is an RTK meta-command (`gain`, `init`, etc.) -- if so, show Clap error
2. Look up TOML DSL filters via `toml_filter::find_matching_filter()`
3. If TOML match: capture stdout, apply filter pipeline, track savings
4. If no match: pure passthrough with `Stdio::inherit`, track as 0% savings

```
Command received
  -> Clap parse succeeds?
     -> Yes: Route to Rust filter module
     -> No:  run_fallback()
              -> TOML filter match?
                 -> Yes: Capture stdout, apply filter, track savings
                 -> No:  Passthrough (inherit stdio, track 0% savings)
```

> **Details**: [`src/core/README.md`](../src/core/README.md) covers the TOML filter engine, filter pipeline stages, and trust-gated project filters.

### 3.6 Token Tracking

Every command execution records metrics to SQLite (`~/.local/share/rtk/tracking.db`):

- Input tokens (raw output size) and output tokens (filtered size)
- Savings percentage, execution time, project path
- 90-day automatic retention cleanup
- Token estimation: `ceil(chars / 4.0)` approximation

Analytics commands (`rtk gain`, `rtk cc-economics`, `rtk session`) query this database to produce dashboards and ROI reports.

> **Details**: [`src/analytics/README.md`](../src/analytics/README.md) covers the analytics modules, and [`src/core/README.md`](../src/core/README.md) covers the tracking database schema.

### 3.7 Tee Recovery

On command failure (non-zero exit code):

1. Raw unfiltered output is saved to `~/.local/share/rtk/tee/{epoch}_{slug}.log`
2. A hint line is printed: `[full output: ~/.../tee/1234_cargo_test.log]`
3. LLM agents can re-read the file instead of re-running the failed command

Tee is configurable (enabled/disabled, min size, max files, max file size) and never affects command output or exit code on failure.

> **Details**: [`src/core/README.md`](../src/core/README.md) covers tee configuration and the rotation strategy.

---

## 4. Folder Map

Start here, then drill down into each README for file-level details.

### `src/` — Rust source code

| Directory | What it does | What you'll find in its README |
|-----------|-------------|-------------------------------|
| `main.rs` | CLI entry point, `Commands` enum, routing match | _(no README — read the file directly)_ |
| [`core/`](../src/core/README.md) | Shared infrastructure | Tracking DB schema, config system, tee recovery, TOML filter engine, utility functions |
| [`hooks/`](../src/hooks/README.md) | Hook system | Installation flow (`rtk init`), integrity verification, rewrite command, trust model |
| [`analytics/`](../src/analytics/README.md) | Token savings analytics | `rtk gain` dashboard, Claude Code economics, ccusage parsing |
| [`cmds/`](../src/cmds/README.md) | **Command filters (9 ecosystems)** | Common filter pattern, cross-command routing, token savings table, **links to each ecosystem** |
| [`discover/`](../src/discover/README.md) | History analysis + rewrite registry | Rewrite patterns, session providers, compound command splitting |
| [`learn/`](../src/learn/README.md) | CLI correction detection | Error classification, correction pair detection, rule generation |
| [`parser/`](../src/parser/README.md) | Parser infrastructure | Canonical types (TestResult, LintResult, etc.), 3-tier format modes, migration guide |
| [`filters/`](../src/filters/README.md) | TOML filter configs | TOML DSL syntax, 8-stage pipeline, inline testing, naming conventions |

### `hooks/` — Deployed hook artifacts (root directory)

| Directory | Agent | What you'll find in its README |
|-----------|-------|-------------------------------|
| [`hooks/`](../hooks/README.md) | _(parent)_ | **All JSON formats**, rewrite registry overview, exit code contract, override controls |
| [`claude/`](../hooks/claude/README.md) | Claude Code | Shell hook mechanism, `PreToolUse` JSON, test script |
| [`copilot/`](../hooks/copilot/README.md) | GitHub Copilot | Rust binary hook, VS Code Chat vs Copilot CLI dual format |
| [`cursor/`](../hooks/cursor/README.md) | Cursor IDE | Shell hook, empty JSON response requirement |
| [`cline/`](../hooks/cline/README.md) | Cline / Roo Code | Rules file (prompt-level, no programmatic hook) |
| [`windsurf/`](../hooks/windsurf/README.md) | Windsurf / Cascade | Rules file (workspace-scoped) |
| [`codex/`](../hooks/codex/README.md) | OpenAI Codex CLI | Awareness document, AGENTS.md integration |
| [`opencode/`](../hooks/opencode/README.md) | OpenCode | TypeScript plugin, zx library, in-place mutation |

---

## 5. Hook System Summary

RTK supports the following LLM agents through hook integrations:

| Agent | Hook Type | Mechanism | Can Modify Command? |
|-------|-----------|-----------|---------------------|
| Claude Code | Shell hook | `PreToolUse` in `settings.json` | Yes (`updatedInput`) |
| GitHub Copilot (VS Code) | Rust binary | `rtk hook copilot` reads JSON | Yes (`updatedInput`) |
| GitHub Copilot CLI | Rust binary | `rtk hook copilot` reads JSON | No (deny + suggestion) |
| Cursor | Shell hook | `preToolUse` hook | Yes (`updated_input`) |
| Gemini CLI | Rust binary | `rtk hook gemini` reads JSON | Yes (`hookSpecificOutput`) |
| Cline/Roo Code | Rules file | Prompt-level guidance | N/A (prompt) |
| Windsurf | Rules file | Prompt-level guidance | N/A (prompt) |
| Codex CLI | Awareness doc | AGENTS.md integration | N/A (prompt) |
| OpenCode | TS plugin | `tool.execute.before` event | Yes (in-place mutation) |

> **Details**: [`hooks/README.md`](../hooks/README.md) has the full JSON schemas for each agent. [`src/hooks/README.md`](../src/hooks/README.md) covers installation, integrity verification, and the rewrite command.

---

## 6. Filter Pipeline Summary

### Rust Filters (cmds/**)

Compiled filter modules for complex transformations with 60-95% token savings.

> **Details**: [`src/cmds/README.md`](../src/cmds/README.md) and each ecosystem subdirectory README.

### TOML DSL Filters (src/filters/*.toml)

Declarative filters with an 8-stage pipeline: strip ANSI, regex replace, match output, strip/keep lines, truncate lines, head/tail, max lines, on-empty message. Loaded from three tiers: built-in (compiled), global (`~/.config/rtk/filters/`), project-local (`.rtk/filters/`, trust-gated).

> **Details**: [`src/core/README.md`](../src/core/README.md) covers the TOML filter engine.

---

## 7. Performance Constraints

| Metric | Target | Verification |
|--------|--------|--------------|
| Startup time | < 10ms | `hyperfine 'rtk git status' 'git status'` |
| Memory usage | < 5MB resident | `/usr/bin/time -v rtk git status` |
| Binary size | < 5MB stripped | `ls -lh target/release/rtk` |
| Token savings | 60-90% per filter | Snapshot + token count tests |

Achieved through:
- Zero async overhead (single-threaded, no tokio)
- Lazy regex compilation (`lazy_static!`)
- Minimal allocations (borrow over clone)
- No config file I/O on startup (loaded on-demand)

---

## 8. Testing

Tests live **in the module file itself** inside a `#[cfg(test)] mod tests` block (e.g., tests for `src/cmds/cloud/container.rs` go at the bottom of that same file).

### How to Write Tests

**1. Create a fixture from real command output** (not synthetic data):
```bash
kubectl get pods > tests/fixtures/kubectl_pods_raw.txt
```

**2. Write your test in the same module file** (`#[cfg(test)] mod tests`):
```rust
#[test]
fn test_my_filter() {
    let input = include_str!("../tests/fixtures/my_cmd_raw.txt");
    let output = filter_my_cmd(input);
    assert!(output.contains("expected content"));
    assert!(!output.contains("noise line"));
}
```

**3. Verify token savings** (60% minimum required):
```rust
#[test]
fn test_my_filter_savings() {
    let input = include_str!("../tests/fixtures/my_cmd_raw.txt");
    let output = filter_my_cmd(input);
    let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
    assert!(savings >= 60.0, "Expected >=60% savings, got {:.1}%", savings);
}
```

### Test Organization

```
tests/
├── fixtures/           # Real command output (never synthetic)
│   ├── git_log_raw.txt
│   ├── cargo_test_raw.txt
│   └── dotnet/         # Ecosystem-specific fixtures
└── integration_test.rs # Integration tests (#[ignore])
```

- **Unit tests**: `#[cfg(test)] mod tests` embedded in each module
- **Fixtures**: real command output in `tests/fixtures/`
- **Integration tests**: `#[ignore]` attribute, run with `cargo test --ignored`

> For testing requirements, pre-commit gate, and PR checklist, see [CONTRIBUTING.md — Testing](../CONTRIBUTING.md#testing).

---

## 9. Future Improvements

- **Extract cli.rs**: Move `Commands` enum, 13 sub-enums (`GitCommands`, `CargoCommands`, etc.), and `AgentTarget` from main.rs to a dedicated cli.rs module. This would reduce main.rs from ~2600 to ~1500 lines.
- **Split routing**: Extract the `match cli.command { ... }` block into a separate routing module.
- **Streaming filters**: For long-running commands, filter output line-by-line as it arrives instead of buffering.
