# rtk - Rust Token Killer

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**High-performance CLI proxy to minimize LLM token consumption.**

[Website](https://www.rtk-ai.app) | [GitHub](https://github.com/rtk-ai/rtk) | [Install](INSTALL.md)

rtk filters and compresses command outputs before they reach your LLM context, saving 60-90% of tokens on common operations.

## ⚠️ Important: Name Collision Warning

**There are TWO different projects named "rtk":**

1. ✅ **This project (Rust Token Killer)** - LLM token optimizer
   - Repos: `rtk-ai/rtk`
   - Purpose: Reduce LLM token consumption in AI coding agents

2. ❌ **reachingforthejack/rtk** - Rust Type Kit (DIFFERENT PROJECT)
   - Purpose: Query Rust codebase and generate types
   - **DO NOT install this one if you want token optimization**

**How to verify you have the correct rtk:**
```bash
rtk --version   # Should show "rtk 0.22.2"
rtk gain        # Should show token savings stats
```

If `rtk gain` doesn't exist, you installed the wrong package. See installation instructions below.

## Token Savings (30-min AI Agent Session)

Typical session without rtk: **~150,000 tokens**
With rtk: **~45,000 tokens** → **70% reduction**

| Operation | Frequency | Standard | rtk | Savings |
|-----------|-----------|----------|-----|---------|
| `ls` / `tree` | 10× | 2,000 | 400 | -80% |
| `cat` / `read` | 20× | 40,000 | 12,000 | -70% |
| `grep` / `rg` | 8× | 16,000 | 3,200 | -80% |
| `git status` | 10× | 3,000 | 600 | -80% |
| `git diff` | 5× | 10,000 | 2,500 | -75% |
| `git log` | 5× | 2,500 | 500 | -80% |
| `git add/commit/push` | 8× | 1,600 | 120 | -92% |
| `npm test` / `cargo test` | 5× | 25,000 | 2,500 | -90% |
| `ruff check` | 3× | 3,000 | 600 | -80% |
| `pytest` | 4× | 8,000 | 800 | -90% |
| `go test` | 3× | 6,000 | 600 | -90% |
| `docker ps` | 3× | 900 | 180 | -80% |
| **Total** | | **~118,000** | **~23,900** | **-80%** |

> Estimates based on medium-sized TypeScript/Rust projects. Actual savings vary by project size.

## Installation

### ⚠️ Pre-Installation Check (REQUIRED)

**ALWAYS verify if rtk is already installed before installing:**

```bash
rtk --version        # Check if installed
rtk gain             # Verify it's the Token Killer (not Type Kit)
which rtk            # Check installation path
```

If already installed and `rtk gain` works, **DO NOT reinstall**. Skip to Quick Start.

### Homebrew (macOS/Linux)

```bash
brew install rtk
```

### Quick Install (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

> **Note**: rtk installs to `~/.local/bin` by default. If this directory is not in your PATH, add it:
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # or ~/.zshrc
> ```

After installation, **verify you have the correct rtk**:
```bash
rtk gain  # Must show token savings stats (not "command not found")
```

### Alternative: Manual Installation

```bash
# From rtk-ai upstream (maintained by pszymkowiak)
cargo install --git https://github.com/rtk-ai/rtk

# OR if published to crates.io
cargo install rtk
```

⚠️ **WARNING**: `cargo install rtk` from crates.io might install the wrong package (Type Kit instead of Token Killer). Always verify with `rtk gain` after installation.

### Alternative: Pre-built Binaries

Download from [rtk-ai/releases](https://github.com/rtk-ai/rtk/releases):
- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-gnu.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

## Quick Start

```bash
# 1. Verify installation
rtk gain  # Must show token stats, not "command not found"

# 2. Initialize for your AI agent (RECOMMENDED: hook-first mode)
rtk init --global
# → Auto-detects Claude Code or OpenCode
# → Installs hook/plugin + creates slim rules file
# → Follow printed instructions to complete setup

# 3. Test it works
rtk git status  # Should show ultra-compact output
rtk init --show # Verify hook is installed and executable

# Alternative modes:
# rtk init --global --claude-md  # Legacy: full injection (137 lines)
# rtk init                       # Local project only (./CLAUDE.md)
```

**New in v0.9.5**: Hook-first installation eliminates ~2000 tokens from the agent's context while maintaining full RTK functionality through transparent command rewriting.

## Global Flags

```bash
-u, --ultra-compact    # ASCII icons, inline format (extra token savings)
-v, --verbose          # Increase verbosity (-v, -vv, -vvv)
```

## Commands

### Files
```bash
rtk ls .                        # Token-optimized directory tree
rtk read file.rs                # Smart file reading
rtk read file.rs -l aggressive  # Signatures only (strips bodies)
rtk smart file.rs               # 2-line heuristic code summary
rtk find "*.rs" .               # Compact find results
rtk grep "pattern" .            # Grouped search results
```

### Git
```bash
rtk git status                  # Compact status
rtk git log -n 10               # One-line commits
rtk git diff                    # Condensed diff
rtk git add                     # → "ok ✓"
rtk git commit -m "msg"         # → "ok ✓ abc1234"
rtk git push                    # → "ok ✓ main"
rtk git pull                    # → "ok ✓ 3 files +10 -2"
```

### Commands
```bash
rtk test cargo test             # Show failures only (-90% tokens)
rtk err npm run build           # Errors/warnings only
rtk summary <long command>      # Heuristic summary
rtk log app.log                 # Deduplicated logs
rtk gh pr list                   # Compact PR listing
rtk gh pr view 42                # PR details + checks summary
rtk gh issue list                # Compact issue listing
rtk gh run list                  # Workflow run status
rtk wget https://example.com    # Download, strip progress bars
rtk config                       # Show config (--create to generate)
rtk ruff check                   # Python linting (JSON, 80% reduction)
rtk pytest                       # Python tests (failures only, 90% reduction)
rtk pip list                     # Python packages (auto-detect uv, 70% reduction)
rtk go test                      # Go tests (NDJSON, 90% reduction)
rtk golangci-lint run            # Go linting (JSON, 85% reduction)
```

### Data & Analytics
```bash
rtk json config.json            # Structure without values
rtk deps                        # Dependencies summary
rtk env -f AWS                  # Filtered env vars

# Token Savings Analytics (includes execution time metrics)
rtk gain                        # Summary stats with total exec time
rtk gain --graph                # With ASCII graph of last 30 days
rtk gain --history              # With recent command history (10)
rtk gain --quota --tier 20x     # Monthly quota analysis (pro/5x/20x)

# Temporal Breakdowns (includes time metrics per period)
rtk gain --daily                # Day-by-day with avg execution time
rtk gain --weekly               # Week-by-week breakdown
rtk gain --monthly              # Month-by-month breakdown
rtk gain --all                  # All breakdowns combined

# Export Formats (includes total_time_ms and avg_time_ms fields)
rtk gain --all --format json    # JSON export for APIs/dashboards
rtk gain --all --format csv     # CSV export for Excel/analysis
```

> 📖 **API Documentation**: For programmatic access to tracking data (Rust library usage, CI/CD integration, custom dashboards), see [docs/tracking.md](docs/tracking.md).

### Discover — Find Missed Savings

Scans your AI agent session history (Claude Code or OpenCode) to find commands where rtk would have saved tokens. Use it to:
- **Measure what you're missing** — see exactly how many tokens you could save
- **Identify habits** — find which commands you keep running without rtk
- **Spot new opportunities** — see unhandled commands that could become rtk features

```bash
rtk discover                    # Current project, last 30 days
rtk discover --all              # All agent projects
rtk discover --all --since 7    # Last 7 days across all projects
rtk discover -p aristote        # Filter by project name (substring)
rtk discover --format json      # Machine-readable output
```

Example output:
```
RTK Discover -- Savings Opportunities
====================================================
Scanned: 142 sessions (last 30 days), 1786 Bash commands
Already using RTK: 108 commands (6%)

MISSED SAVINGS -- Commands RTK already handles
----------------------------------------------------
Command              Count    RTK Equivalent        Est. Savings
git log                434    rtk git               ~55.9K tokens
cargo test             203    rtk cargo             ~49.9K tokens
ls -la                 107    rtk ls                ~11.8K tokens
gh pr                   80    rtk gh                ~10.4K tokens
----------------------------------------------------
Total: 986 commands -> ~143.9K tokens saveable

TOP UNHANDLED COMMANDS -- open an issue?
----------------------------------------------------
Command              Count    Example
git checkout            84    git checkout feature/my-branch
cargo run               32    cargo run -- gain --help
----------------------------------------------------
-> github.com/rtk-ai/rtk/issues
```

### Containers
```bash
rtk docker ps                   # Compact container list
rtk docker images               # Compact image list
rtk docker logs <container>     # Deduplicated logs
rtk kubectl pods                # Compact pod list
rtk kubectl logs <pod>          # Deduplicated logs
rtk kubectl services             # Compact service list
```

### JavaScript / TypeScript Stack
```bash
rtk lint                         # ESLint grouped by rule/file
rtk lint biome                   # Supports other linters too
rtk tsc                          # TypeScript errors grouped by file
rtk next build                   # Next.js build compact output
rtk prettier --check .           # Files needing formatting
rtk vitest run                   # Test failures only
rtk playwright test              # E2E results (failures only)
rtk prisma generate              # Schema generation (no ASCII art)
rtk prisma migrate dev --name x  # Migration summary
rtk prisma db-push               # Schema push summary
```

### Python & Go Stack
```bash
# Python
rtk ruff check                   # Ruff linter (JSON, 80% reduction)
rtk ruff format                  # Ruff formatter (text filter)
rtk pytest                       # Test failures with state machine parser (90% reduction)
rtk pip list                     # Package list (auto-detect uv, 70% reduction)
rtk pip install <package>        # Install with compact output
rtk pip outdated                 # Outdated packages (85% reduction)

# Go
rtk go test                      # NDJSON streaming parser (90% reduction)
rtk go build                     # Build errors only (80% reduction)
rtk go vet                       # Vet issues (75% reduction)
rtk golangci-lint run            # JSON grouped by rule (85% reduction)
```

## Examples

### Standard vs rtk

**Directory listing:**
```
# ls -la (45 lines, ~800 tokens)
drwxr-xr-x  15 user  staff    480 Jan 23 10:00 .
drwxr-xr-x   5 user  staff    160 Jan 23 09:00 ..
-rw-r--r--   1 user  staff   1234 Jan 23 10:00 Cargo.toml
...

# rtk ls (12 lines, ~150 tokens)
📁 my-project/
├── src/ (8 files)
│   ├── main.rs
│   └── lib.rs
├── Cargo.toml
└── README.md
```

**Git operations:**
```
# git push (15 lines, ~200 tokens)
Enumerating objects: 5, done.
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...

# rtk git push (1 line, ~10 tokens)
ok ✓ main
```

**Test output:**
```
# cargo test (200+ lines on failure)
running 15 tests
test utils::test_parse ... ok
test utils::test_format ... ok
...

# rtk test cargo test (only failures, ~20 lines)
FAILED: 2/15 tests
  ✗ test_edge_case: assertion failed at src/lib.rs:42
  ✗ test_overflow: panic at src/utils.rs:18
```

## How It Works

```
  Without rtk:

  ┌──────────┐  git status     ┌──────────┐  git status  ┌──────────┐
  │   LLM    │ ─────────────── │  shell   │ ──────────── │   git    │
  │  Agent   │                 │          │              │  (CLI)   │
  └──────────┘                 └──────────┘              └──────────┘
        ▲                                                      │
        │              ~2,000 tokens (raw output)              │
        └──────────────────────────────────────────────────────┘

  With rtk:

  ┌──────────┐  git status     ┌──────────┐  git status  ┌──────────┐
  │   LLM    │ ─────────────── │   RTK    │ ──────────── │   git    │
  │  Agent   │                 │  (proxy) │              │  (CLI)   │
  └──────────┘                 └──────────┘              └──────────┘
        ▲                           │  ~2,000 tokens raw       │
        │                           └──────────────────────────┘
        │  ~200 tokens (filtered)   filter · group · dedup · truncate
        └───────────────────────────────────────────────────────
```

Four strategies applied per command type:

1. **Smart Filtering**: Removes noise (comments, whitespace, boilerplate)
2. **Grouping**: Aggregates similar items (files by directory, errors by type)
3. **Truncation**: Keeps relevant context, cuts redundancy
4. **Deduplication**: Collapses repeated log lines with counts

## Configuration

### Installation Modes

| Command | Scope | Hook | RTK.md | CLAUDE.md | Tokens in Context | Use Case |
|---------|-------|------|--------|-----------|-------------------|----------|
| `rtk init -g` | Global | ✅ | ✅ (10 lines) | @RTK.md | ~10 | **Recommended**: All projects, automatic |
| `rtk init -g --claude-md` | Global | ❌ | ❌ | Full (137 lines) | ~2000 | Legacy compatibility |
| `rtk init -g --hook-only` | Global | ✅ | ❌ | Nothing | 0 | Minimal setup, hook-only |
| `rtk init` | Local | ❌ | ❌ | Full (137 lines) | ~2000 | Single project, no hook |

```bash
rtk init --show         # Show current configuration
rtk init -g             # Install hook + RTK.md (recommended)
rtk init -g --claude-md # Legacy: full injection into CLAUDE.md
rtk init                # Local project: full injection into ./CLAUDE.md
```

### Installation Flags

**Settings.json Control (Claude Code only)**:
```bash
rtk init -g                 # Default: prompt to patch [y/N]
rtk init -g --auto-patch    # Patch settings.json without prompting
rtk init -g --no-patch      # Skip patching, show manual instructions
```

**Mode Control**:
```bash
rtk init -g --claude-md     # Legacy: full 137-line injection (no hook)
rtk init -g --hook-only     # Hook only, no RTK.md
```

**Uninstall**:
```bash
rtk init -g --uninstall     # Remove all RTK artifacts
```

**What is settings.json?**
Claude Code's configuration file that registers the RTK hook. The hook transparently rewrites commands (e.g., `git status` → `rtk git status`) before execution. Without this registration, the hook won't run. OpenCode uses a different mechanism (auto-loaded plugins) that doesn't require settings.json.

**Backup Behavior**:
RTK creates `~/.claude/settings.json.bak` before making changes. If something breaks, restore with:
```bash
cp ~/.claude/settings.json.bak ~/.claude/settings.json
```

**Migration**: If you previously used `rtk init -g` with the old system (137-line injection), simply re-run `rtk init -g` to automatically migrate to the new hook-first approach.

example of 3 days session:
```bash
📊 RTK Token Savings
════════════════════════════════════════

Total commands:    133
Input tokens:      30.5K
Output tokens:     10.7K
Tokens saved:      25.3K (83.0%)

By Command:
────────────────────────────────────────
Command               Count      Saved     Avg%
rtk git status           41      17.4K    82.9%
rtk git push             54       3.4K    91.6%
rtk grep                 15       3.2K    26.5%
rtk ls                   23       1.4K    37.2%

Daily Savings (last 30 days):
────────────────────────────────────────
01-23 │███████████████████                      6.4K
01-24 │██████████████████                       5.9K
01-25 │                                         18
01-26 │████████████████████████████████████████ 13.0K
```

### Custom Database Path

By default, RTK stores tracking data in `~/.local/share/rtk/history.db`. You can override this:

**Environment variable** (highest priority):
```bash
export RTK_DB_PATH="/path/to/custom.db"
```

**Config file** (`~/.config/rtk/config.toml`):
```toml
[tracking]
database_path = "/path/to/custom.db"
```

Priority: `RTK_DB_PATH` env var > `config.toml` > default location.

### Tee: Full Output Recovery

When RTK filters command output, LLM agents lose failure details (stack traces, assertion messages) and may re-run the same command 2-3 times. The **tee** feature saves raw output to a file so the agent can read it without re-executing.

**How it works**: On command failure, RTK writes the full unfiltered output to `~/.local/share/rtk/tee/` and prints a one-line hint:
```
✓ cargo test: 15 passed (1 suite, 0.01s)
[full output: ~/.local/share/rtk/tee/1707753600_cargo_test.log]
```

The agent reads the file instead of re-running the command — saving tokens.

**Default behavior**: Tee only on failures (exit code != 0), skip outputs < 500 chars.

**Config** (`~/.config/rtk/config.toml`):
```toml
[tee]
enabled = true          # default: true
mode = "failures"       # "failures" (default), "always", or "never"
max_files = 20          # max files to keep (oldest rotated out)
max_file_size = 1048576 # 1MB per file max
# directory = "/custom/path"  # override default location
```

**Environment overrides**:
- `RTK_TEE=0` — disable tee entirely
- `RTK_TEE_DIR=/path` — override output directory

**Supported commands**: cargo (build/test/clippy/check/install/nextest), vitest, pytest, lint (eslint/biome/ruff/pylint/mypy), tsc, go (test/build/vet), err, test.

## Auto-Rewrite Hook/Plugin (Recommended)

The most effective way to use rtk is with the **auto-rewrite hook** (Claude Code) or **plugin** (OpenCode). Instead of relying on rules file instructions (which subagents may ignore), this transparently intercepts Bash commands and rewrites them to their rtk equivalents before execution.

**Result**: 100% rtk adoption across all conversations and subagents, zero token overhead in context.

### How It Works

RTK auto-detects your AI coding agent and installs the appropriate integration:

| Agent | Mechanism | Config Location |
|-------|-----------|-----------------|
| **Claude Code** | PreToolUse hook (bash script) | `~/.claude/hooks/rtk-rewrite.sh` + `settings.json` |
| **OpenCode** | Plugin (TypeScript) | `~/.config/opencode/plugins/rtk-rewrite.ts` |

When the agent is about to execute a Bash command like `git status`, the hook/plugin rewrites it to `rtk git status` before the command reaches the shell. The agent never sees the rewrite — it's transparent.

```
  Agent types:  git status
                      │
               ┌──────▼──────────────────────┐
               │  Hook/Plugin registered     │
               │  (auto-detected per agent)  │
               └──────┬──────────────────────┘
                      │
               ┌──────▼──────────────────────┐
               │  rtk-rewrite (.sh or .ts)    │
               │  "git status"                │
               │    →  "rtk git status"       │  transparent rewrite
               └──────┬──────────────────────┘
                      │
               ┌──────▼──────────────────────┐
               │  RTK (Rust binary)           │
               │  executes real git status    │
               │  filters output              │
               └──────┬──────────────────────┘
                      │
  Agent receives:  "3 modified, 1 untracked ✓"
                   ↑ not 50 lines of raw git output
```

### Quick Install (Automated)

```bash
rtk init -g
# Auto-detects Claude Code or OpenCode and installs:
#
# Claude Code:
#   → Hook: ~/.claude/hooks/rtk-rewrite.sh
#   → Context: ~/.claude/RTK.md (10 lines)
#   → Prompts to patch ~/.claude/settings.json
#
# OpenCode:
#   → Plugin: ~/.config/opencode/plugins/rtk-rewrite.ts
#   → Context: ~/.config/opencode/RTK.md (10 lines)
#   → No settings.json needed (plugins auto-load)

# Verify installation
rtk init --show  # Shows hook/plugin status for both agents
```

#### Claude Code-Specific Options

```bash
rtk init -g                 # Default: prompts for consent [y/N]
rtk init -g --auto-patch    # Patch settings.json without prompting (CI/CD)
rtk init -g --no-patch      # Skip patching, print manual JSON snippet
```

**What is settings.json?**
Claude Code's hook registry. RTK adds a PreToolUse hook entry that triggers command rewriting. Without this registration, the hook won't run. RTK backs up the file before changes (`settings.json.bak`).

**Restart Required**: After installation, restart your AI agent, then test with `git status`.

### Manual Install (Fallback)

#### Claude Code

```bash
# 1. Copy the hook script
mkdir -p ~/.claude/hooks
cp hooks/rtk-rewrite.sh ~/.claude/hooks/rtk-rewrite.sh
chmod +x ~/.claude/hooks/rtk-rewrite.sh

# 2. Add to ~/.claude/settings.json under hooks.PreToolUse:
```

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/rtk-rewrite.sh"
          }
        ]
      }
    ]
  }
}
```

#### OpenCode

```bash
# Copy the plugin (auto-loaded from plugins directory)
mkdir -p ~/.config/opencode/plugins
cp hooks/rtk-rewrite.ts ~/.config/opencode/plugins/rtk-rewrite.ts
# No config file changes needed — OpenCode auto-loads plugins
```

### Per-Project Install

The hook/plugin files are included in this repository under `hooks/`. To use in another project, copy the appropriate file for your agent.

### Commands Rewritten

| Raw Command | Rewritten To |
|-------------|-------------|
| `git status/diff/log/add/commit/push/pull/branch/fetch/stash` | `rtk git ...` |
| `gh pr/issue/run` | `rtk gh ...` |
| `cargo test/build/clippy` | `rtk cargo ...` |
| `cat <file>` | `rtk read <file>` |
| `rg/grep <pattern>` | `rtk grep <pattern>` |
| `ls` | `rtk ls` |
| `vitest/pnpm test` | `rtk vitest run` |
| `tsc/pnpm tsc` | `rtk tsc` |
| `eslint/pnpm lint` | `rtk lint` |
| `prettier` | `rtk prettier` |
| `playwright` | `rtk playwright` |
| `prisma` | `rtk prisma` |
| `ruff check/format` | `rtk ruff ...` |
| `pytest` | `rtk pytest` |
| `pip list/install/outdated` | `rtk pip ...` |
| `go test/build/vet` | `rtk go ...` |
| `golangci-lint run` | `rtk golangci-lint run` |
| `docker ps/images/logs` | `rtk docker ...` |
| `kubectl get/logs` | `rtk kubectl ...` |
| `curl` | `rtk curl` |
| `pnpm list/ls/outdated` | `rtk pnpm ...` |

Commands already using `rtk`, heredocs (`<<`), and unrecognized commands pass through unchanged.

### Alternative: Suggest Hook (Non-Intrusive, Claude Code Only)

If you prefer the agent to **suggest** rtk usage rather than automatically rewriting commands, use the **suggest hook** pattern instead. This emits a system reminder when rtk-compatible commands are detected, without modifying the command execution. (Currently only available for Claude Code.)

**Comparison**:

| Aspect | Auto-Rewrite | Suggest Hook |
|--------|-------------|--------------|
| **Strategy** | Intercepts and modifies command before execution | Emits system reminder when rtk-compatible command detected |
| **Effect** | Agent never sees the original command | Agent receives hint to use rtk, decides autonomously |
| **Adoption** | 100% (forced) | ~70-85% (depends on agent adherence to instructions) |
| **Use Case** | Production workflows, guaranteed savings | Learning mode, auditing, user preference for explicit control |
| **Overhead** | Zero (transparent rewrite) | Minimal (reminder message in context) |

**When to use suggest over rewrite**:
- You want to audit which commands the agent chooses to run
- You're learning rtk patterns and want visibility into the rewrite logic
- You prefer the agent to make explicit decisions rather than transparent rewrites
- You want to preserve exact command execution for debugging

#### Suggest Hook Setup (Claude Code)

**1. Create the suggest hook script**

```bash
mkdir -p ~/.claude/hooks
cp .claude/hooks/rtk-suggest.sh ~/.claude/hooks/rtk-suggest.sh
chmod +x ~/.claude/hooks/rtk-suggest.sh
```

**2. Add to `~/.claude/settings.json`**

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/rtk-suggest.sh"
          }
        ]
      }
    ]
  }
}
```

The suggest hook detects the same commands as the rewrite hook but outputs a `systemMessage` instead of `updatedInput`, informing the agent that an rtk alternative exists.

## Uninstalling RTK

**Complete Removal (Global Only)**:
```bash
rtk init -g --uninstall

# Removes (per detected agent):
#
# Claude Code:
#   - ~/.claude/hooks/rtk-rewrite.sh
#   - ~/.claude/RTK.md
#   - @RTK.md reference from ~/.claude/CLAUDE.md
#   - RTK hook entry from ~/.claude/settings.json
#
# OpenCode:
#   - ~/.config/opencode/plugins/rtk-rewrite.ts
#   - ~/.config/opencode/RTK.md
#   - @RTK.md reference from ~/.config/opencode/AGENTS.md

# Restart your AI agent after uninstall
```

**Restore from Backup** (Claude Code, if needed):
```bash
cp ~/.claude/settings.json.bak ~/.claude/settings.json
```

**Local Projects**: Manually remove RTK instructions from `./CLAUDE.md` or `./AGENTS.md`

**Binary Removal**:
```bash
# If installed via cargo
cargo uninstall rtk

# If installed via package manager
brew uninstall rtk          # macOS Homebrew
sudo apt remove rtk         # Debian/Ubuntu
sudo dnf remove rtk         # Fedora/RHEL
```

## Documentation

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - ⚠️ Fix common issues (wrong rtk installed, missing commands, PATH issues)
- **[INSTALL.md](INSTALL.md)** - Detailed installation guide with verification steps
- **[AUDIT_GUIDE.md](docs/AUDIT_GUIDE.md)** - Complete guide to token savings analytics, temporal breakdowns, and data export
- **[CLAUDE.md](CLAUDE.md)** - AI agent integration instructions and project context
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - Technical architecture and development guide
- **[SECURITY.md](SECURITY.md)** - Security policy, vulnerability reporting, and PR review process

## Troubleshooting

### Settings.json Patching Failed

**Problem**: `rtk init -g` fails to patch settings.json

**Solutions**:
```bash
# Check if settings.json is valid JSON
cat ~/.claude/settings.json | python3 -m json.tool

# Use manual patching
rtk init -g --no-patch  # Prints JSON snippet

# Restore from backup
cp ~/.claude/settings.json.bak ~/.claude/settings.json

# Check permissions
ls -la ~/.claude/settings.json
chmod 644 ~/.claude/settings.json
```

### Hook/Plugin Not Working After Install

**Problem**: Commands still not using RTK after `rtk init -g`

**Solutions**:
```bash
# Verify hook/plugin is registered
rtk init --show

# For Claude Code: check settings.json manually
cat ~/.claude/settings.json | grep rtk-rewrite

# For OpenCode: check plugin exists
ls ~/.config/opencode/plugins/rtk-rewrite.ts

# Restart your AI agent (critical step!)

# Test with a command
git status  # Should use rtk automatically
```

### Uninstall Didn't Remove Everything

**Problem**: RTK traces remain after `rtk init -g --uninstall`

**Manual Cleanup (Claude Code)**:
```bash
# Remove hook
rm ~/.claude/hooks/rtk-rewrite.sh

# Remove RTK.md
rm ~/.claude/RTK.md

# Remove @RTK.md reference
nano ~/.claude/CLAUDE.md  # Delete @RTK.md line

# Remove from settings.json
nano ~/.claude/settings.json  # Remove RTK hook entry

# Restore from backup
cp ~/.claude/settings.json.bak ~/.claude/settings.json
```

**Manual Cleanup (OpenCode)**:
```bash
# Remove plugin
rm ~/.config/opencode/plugins/rtk-rewrite.ts

# Remove RTK.md
rm ~/.config/opencode/RTK.md

# Remove @RTK.md reference
nano ~/.config/opencode/AGENTS.md  # Delete @RTK.md line
```

See **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** for more issues and solutions.

## For Maintainers

### Security Review Workflow

RTK implements a comprehensive 3-layer security review process for external PRs:

#### Layer 1: Automated GitHub Action
Every PR triggers `.github/workflows/security-check.yml`:
- **Cargo audit**: CVE detection in dependencies
- **Critical files alert**: Flags modifications to high-risk files (runner.rs, tracking.rs, Cargo.toml, workflows)
- **Dangerous pattern scanning**: Shell injection, network operations, unsafe code, panic risks
- **Dependency auditing**: Supply chain verification for new crates
- **Clippy security lints**: Enforces Rust safety best practices

Results appear in the PR's GitHub Actions summary.

#### Layer 2: AI Agent Skill
For comprehensive manual review, maintainers with [Claude Code](https://claude.ai/code) or OpenCode can use:

```bash
/rtk-pr-security <PR_NUMBER>
```

The skill performs:
- **Critical files analysis**: Detects modifications to shell execution, validation, or CI/CD files
- **Dangerous pattern detection**: Identifies shell injection, environment manipulation, exfiltration vectors
- **Supply chain audit**: Verifies new dependencies on crates.io (downloads, maintainer, license)
- **Semantic analysis**: Checks intent vs reality, logic bombs, code quality red flags
- **Structured report generation**: Produces security assessment with risk level and verdict

**Skill installation** (maintainers only):
```bash
# The skill is bundled in the rtk-pr-security directory
# Copy to your Claude skills directory:
cp -r ~/.claude/skills/rtk-pr-security ~/.claude/skills/
```

The skill includes:
- `SKILL.md` - Workflow automation and usage guide
- `critical-files.md` - RTK-specific file risk tiers with attack scenarios
- `dangerous-patterns.md` - Regex patterns with exploitation examples
- `checklist.md` - Manual review template

#### Layer 3: Manual Review
For PRs touching critical files or adding dependencies:
- **2 maintainers required** for Cargo.toml, workflows, or Tier 1 files
- **Isolated testing** recommended for high-risk changes
- Follow the checklist in SECURITY.md

See **[SECURITY.md](SECURITY.md)** for complete security policy and review guidelines.

## License

MIT License - see [LICENSE](LICENSE) for details.

## Contributing

Contributions welcome! Please open an issue or PR on GitHub.

**For external contributors**: Your PR will undergo automated security review (see [SECURITY.md](SECURITY.md)). This protects RTK's shell execution capabilities against injection attacks and supply chain vulnerabilities.

## Contact

- Website: https://www.rtk-ai.app
- Email: contact@rtk-ai.app
- Issues: https://github.com/rtk-ai/rtk/issues
