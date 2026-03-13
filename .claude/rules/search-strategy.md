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
├── main.rs           ← Commands enum + routing (start here for any command)
├── git.rs            ← Git operations (log, status, diff)
├── runner.rs         ← Cargo commands (test, build, clippy, check)
├── gh_cmd.rs         ← GitHub CLI (pr, run, issue)
├── grep_cmd.rs       ← Code search output filtering
├── ls.rs             ← Directory listing
├── read.rs           ← File reading with filter levels
├── filter.rs         ← Language-aware code filtering engine
├── tracking.rs       ← SQLite token metrics
├── config.rs         ← ~/.config/rtk/config.toml
├── tee.rs            ← Raw output recovery on failure
├── utils.rs          ← strip_ansi, truncate, execute_command
├── init.rs           ← rtk init command
└── *_cmd.rs          ← All other command modules
```

## Common Search Patterns

### "Where is command X handled?"

```
# Step 1: Find the routing
Grep pattern="Gh\|Cargo\|Git\|Grep" path="src/main.rs" output_mode="content"

# Step 2: Follow to module
Read file_path="src/gh_cmd.rs"
```

### "Where is function X defined?"

```
Grep pattern="fn filter_git_log\|fn run\b" type="rust"
```

### "All command modules"

```
Glob pattern="src/*_cmd.rs"
# Then: src/git.rs, src/runner.rs for non-*_cmd.rs modules
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
2. Check existing `*_cmd.rs` for patterns to follow (e.g., `src/gh_cmd.rs`)
3. Check `src/utils.rs` for shared helpers before reimplementing
4. Check `tests/fixtures/` for existing fixture patterns

### Debugging filter output

1. Start with `src/<cmd>_cmd.rs` → find `run()` function
2. Trace filter function (usually `filter_<cmd>()`)
3. Check `lazy_static!` regex patterns in same file
4. Check `src/utils.rs::strip_ansi()` if ANSI codes involved

### Tracking/metrics issues

1. `src/tracking.rs` → `track_command()` function
2. `src/config.rs` → `tracking.database_path` field
3. `RTK_DB_PATH` env var overrides config

### Configuration issues

1. `src/config.rs` → `RtkConfig` struct
2. `src/init.rs` → `rtk init` command
3. Config file: `~/.config/rtk/config.toml`
4. Filter files: `~/.config/rtk/filters/` (global) or `.rtk/filters/` (project)

## TOML Filter DSL Navigation

```
Glob pattern=".rtk/filters/*.toml"         # Project-local filters
Glob pattern="src/filter_*.rs"             # TOML filter engine
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
