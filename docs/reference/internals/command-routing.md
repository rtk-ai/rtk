---
title: Command Routing
description: How RTK parses, routes, and dispatches commands through the 6-phase execution lifecycle
sidebar:
  order: 1
---

# Command Routing

## 6-phase execution lifecycle

Every RTK command goes through six phases:

### Phase 1: Parse

Clap parses the CLI arguments into a typed `Commands` enum:

```
$ rtk git log --oneline -5 -v

Commands::Git { args: ["log", "--oneline", "-5"], verbose: 1 }
```

### Phase 2: Route

`main.rs` matches the enum variant and dispatches to the module:

```rust
match cli.command {
    Commands::Git { args, .. } => git::run(&args, verbose)?,
    Commands::Cargo { args, .. } => cargo::run(&args, verbose)?,
    Commands::Ls { args } => ls_cmd::run(&args)?,
    // ...
}
```

### Phase 3: Execute

The module spawns the underlying process:

```rust
std::process::Command::new("git")
    .args(["log", "--oneline", "-5"])
    .output()?
// stdout: "abc123 Fix bug\ndef456 Add feature\n..."
// exit_code: 0
```

### Phase 4: Filter

The module applies its filtering strategy to the captured output:

```rust
git::format_git_output(stdout, "log", verbose)
// Strategy: Stats Extraction
// Filtered: "5 commits, +142/-89" (96% reduction)
```

### Phase 5: Print

```rust
println!("{}", colored_output);
// eprintln! for debug output when verbose > 0
```

### Phase 6: Track

```rust
tracking::track(
    original_cmd: "git log --oneline -5",
    rtk_cmd:      "rtk git log --oneline -5",
    input:        &raw_output,    // 500 chars
    output:       &filtered,      // 20 chars
)
// SQLite INSERT: input_tokens=125, output_tokens=5, savings_pct=96.0
```

## Exit code preservation

RTK always propagates the exit code from the underlying tool:

```rust
if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("{}", stderr);
    std::process::exit(output.status.code().unwrap_or(1));
}
```

RTK exit codes: `0` = success, `1` = RTK internal error, `N` = preserved from underlying tool.

## Verbosity levels

| Flag | Behavior |
|------|----------|
| (none) | Compact output only |
| `-v` | + Debug messages (`eprintln!`) |
| `-vv` | + Command being executed |
| `-vvv` | + Raw output before filtering |

## Ultra-compact mode

`-u` / `--ultra-compact`: maximum compression — ASCII icons instead of words, single-line summaries.

```bash
rtk git push -u    # ✓ main  (vs "ok main" normally)
```

## Module organization

```
src/
  main.rs                   ← Commands enum + routing
  cmds/
    git/                    ← git, gh, gt, diff
    rust/                   ← cargo, runner (err/test)
    js/                     ← pnpm, vitest, tsc, next, playwright, prisma, lint
    python/                 ← ruff, pytest, mypy, pip
    go/                     ← go, golangci-lint
    dotnet/                 ← dotnet, binlog
    cloud/                  ← aws, docker/kubectl, curl, wget, psql
    system/                 ← ls, read, grep, find, json, env, log, deps, summary
    ruby/                   ← rake, rspec, rubocop
  core/                     ← utils, tracking, tee, config, toml_filter, filter
  hooks/                    ← init, rewrite, hook_cmd, verify, integrity
  analytics/                ← gain, cc_economics, ccusage
  discover/                 ← rtk discover, registry
```

Total: 64 modules (42 command + 22 infrastructure).
