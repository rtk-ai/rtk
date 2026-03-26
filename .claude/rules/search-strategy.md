# Search Strategy — RTK Codebase Navigation

Efficient search patterns for RTK's Rust codebase.

## Priority Order

1. **Grep** (exact pattern, fast) → for known symbols/strings
2. **Glob** (file discovery) → for finding modules by name
3. **Read** (full file) → only after locating the right file
4. **Explore agent** (broad research) → last resort for >3 queries

Never use Bash for search (`find`, `grep`, `rg`) — use dedicated tools.

## RTK Module Map

```
src/
├── main.rs                    ← Commands enum + routing (start here for any command)
├── core/                      ← Shared infrastructure
│   ├── config.rs              ← ~/.config/rtk/config.toml
│   ├── tracking.rs            ← SQLite token metrics
│   ├── tee.rs                 ← Raw output recovery on failure
│   ├── utils.rs               ← strip_ansi, truncate, execute_command
│   ├── filter.rs              ← Language-aware code filtering engine
│   ├── toml_filter.rs         ← TOML DSL filter engine
│   ├── display_helpers.rs     ← Terminal formatting helpers
│   └── telemetry.rs           ← Analytics ping
├── hooks/                     ← Hook system
│   ├── init.rs                ← rtk init command
│   ├── rewrite_cmd.rs         ← rtk rewrite command
│   ├── hook_cmd.rs            ← Gemini/Copilot hook processors
│   ├── hook_check.rs          ← Hook status detection
│   ├── verify_cmd.rs          ← rtk verify command
│   ├── trust.rs               ← Project trust/untrust
│   └── integrity.rs           ← SHA-256 hook verification
├── analytics/                 ← Token savings analytics
│   ├── gain.rs                ← rtk gain command
│   ├── cc_economics.rs        ← Claude Code economics
│   ├── ccusage.rs             ← ccusage data parsing
│   └── session_cmd.rs         ← Session adoption reporting
├── cmds/                      ← Command filter modules
│   ├── git/                   ← git, gh, gt, diff
│   ├── rust/                  ← cargo, runner (err/test)
│   ├── js/                    ← npm, pnpm, vitest, lint, tsc, next, prettier, playwright, prisma
│   ├── python/                ← ruff, pytest, mypy, pip
│   ├── go/                    ← go, golangci-lint
│   ├── dotnet/                ← dotnet, binlog, trx, format_report
│   ├── cloud/                 ← aws, container (docker/kubectl), curl, wget, psql
│   ├── system/                ← ls, tree, read, grep, find, wc, env, json, log, deps, summary, format, local_llm
│   └── ruby/                  ← rake, rspec, rubocop
├── discover/                  ← Claude Code history analysis
├── learn/                     ← CLI correction detection
├── parser/                    ← Parser infrastructure
└── filters/                   ← 60 TOML filter configs
```

## Common Search Patterns

### "Where is command X handled?"

```
# Step 1: Find the routing
Grep pattern="Gh\|Cargo\|Git\|Grep" path="src/main.rs" output_mode="content"

# Step 2: Follow to module
Read file_path="src/cmds/git/gh_cmd.rs"
```

### "Where is function X defined?"

```
Grep pattern="fn filter_git_log\|fn run\b" type="rust"
```

### "All command modules"

```
Glob pattern="src/cmds/**/*_cmd.rs"
# Also: src/cmds/git/git.rs, src/cmds/rust/runner.rs, src/cmds/cloud/container.rs
```

### "Find all lazy_static regex definitions"

```
Grep pattern="lazy_static!" type="rust" output_mode="content"
```

### "Find unwrap() outside tests"

```
Grep pattern="\.unwrap()" type="rust" output_mode="content"
# Then manually filter out #[cfg(test)] blocks
```

### "Which modules have tests?"

```
Grep pattern="#\[cfg\(test\)\]" type="rust" output_mode="files_with_matches"
```

### "Find token savings assertions"

```
Grep pattern="count_tokens\|savings" type="rust" output_mode="content"
```

### "Find test fixtures"

```
Glob pattern="tests/fixtures/*.txt"
```

## RTK-Specific Navigation Rules

### Adding a new filter

1. Check `src/main.rs` for Commands enum structure
2. Check existing modules in `src/cmds/<ecosystem>/` for patterns to follow (e.g., `src/cmds/git/gh_cmd.rs`)
3. Check `src/core/utils.rs` for shared helpers before reimplementing
4. Check `tests/fixtures/` for existing fixture patterns

### Debugging filter output

1. Start with `src/cmds/<ecosystem>/<cmd>_cmd.rs` → find `run()` function
2. Trace filter function (usually `filter_<cmd>()`)
3. Check `lazy_static!` regex patterns in same file
4. Check `src/core/utils.rs::strip_ansi()` if ANSI codes involved

### Tracking/metrics issues

1. `src/core/tracking.rs` → `track_command()` function
2. `src/core/config.rs` → `tracking.database_path` field
3. `RTK_DB_PATH` env var overrides config

### Configuration issues

1. `src/core/config.rs` → `RtkConfig` struct
2. `src/hooks/init.rs` → `rtk init` command
3. Config file: `~/.config/rtk/config.toml`
4. Filter files: `~/.config/rtk/filters/` (global) or `.rtk/filters/` (project)

## TOML Filter DSL Navigation

```
Glob pattern=".rtk/filters/*.toml"         # Project-local filters
Glob pattern="src/core/toml_filter.rs"     # TOML filter engine
Grep pattern="FilterRule\|FilterConfig" type="rust"
```

## Anti-Patterns

❌ **Don't** read all `*_cmd.rs` files to find one function — use Grep first
❌ **Don't** use Bash `find src -name "*.rs"` — use Glob
❌ **Don't** read `main.rs` entirely to find a module — Grep for the command name
❌ **Don't** search `Cargo.toml` for dependencies with Bash — use Grep with `glob="Cargo.toml"`

## Dependency Check

```
# Check if a crate is already used (before adding)
Grep pattern="^regex\|^anyhow\|^rusqlite" glob="Cargo.toml" output_mode="content"

# Check if async is creeping in (forbidden)
Grep pattern="tokio\|async-std\|futures\|async fn" type="rust"
```
