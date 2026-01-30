# rtk - Rust Token Killer

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**High-performance CLI proxy to minimize LLM token consumption.**

rtk filters and compresses command outputs before they reach your LLM context, saving 60-90% of tokens on common operations.

## Token Savings (30-min Claude Code Session)

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
| `docker ps` | 3× | 900 | 180 | -80% |
| **Total** | | **~101,000** | **~22,000** | **-78%** |

> Estimates based on medium-sized TypeScript/Rust projects. Actual savings vary by project size.

## Installation

### Quick Install (Linux/macOS)
```bash
curl -fsSL https://raw.githubusercontent.com/pszymkowiak/rtk/master/install.sh | sh
```

### Homebrew (macOS) - Coming Soon
<!--
```bash
brew tap pszymkowiak/rtk
brew install rtk
```
-->

### Cargo
```bash
cargo install rtk
```

### Debian/Ubuntu
```bash
curl -LO https://github.com/pszymkowiak/rtk/releases/latest/download/rtk_0.3.1-1_amd64.deb
sudo dpkg -i rtk_0.3.1-1_amd64.deb
```

### Fedora/RHEL
```bash
curl -LO https://github.com/pszymkowiak/rtk/releases/latest/download/rtk-0.3.1-1.x86_64.rpm
sudo rpm -i rtk-0.3.1-1.x86_64.rpm
```

### Manual Download
Download binaries from [Releases](https://github.com/pszymkowiak/rtk/releases):
- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-gnu.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

## Quick Start

```bash
# Initialize rtk for Claude Code
rtk init --global    # Add to ~/CLAUDE.md (all projects)
rtk init             # Add to ./CLAUDE.md (this project)
```

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
rtk diff file1 file2            # Ultra-condensed diff
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
```

### Data
```bash
rtk json config.json            # Structure without values
rtk deps                        # Dependencies summary
rtk env -f AWS                  # Filtered env vars

# Token Savings Analytics
rtk gain                        # Summary stats (default view)
rtk gain --graph                # With ASCII graph of last 30 days
rtk gain --history              # With recent command history (10)
rtk gain --quota --tier 20x     # Monthly quota analysis (pro/5x/20x)

# Temporal Breakdowns (NEW in v0.4.0)
rtk gain --daily                # Day-by-day breakdown (all days)
rtk gain --weekly               # Week-by-week breakdown
rtk gain --monthly              # Month-by-month breakdown
rtk gain --all                  # All breakdowns combined

# Export Formats
rtk gain --all --format json    # JSON export for APIs/dashboards
rtk gain --all --format csv     # CSV export for Excel/analysis
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

1. **Smart Filtering**: Removes noise (comments, whitespace, boilerplate)
2. **Grouping**: Aggregates similar items (files by directory, errors by type)
3. **Truncation**: Keeps relevant context, cuts redundancy
4. **Deduplication**: Collapses repeated log lines with counts

## Configuration

rtk reads from `CLAUDE.md` files to instruct Claude Code to use rtk automatically:

```bash
rtk init --show    # Show current configuration
rtk init           # Create local CLAUDE.md
rtk init --global  # Create ~/CLAUDE.md
```

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

## Documentation

- **[AUDIT_GUIDE.md](docs/AUDIT_GUIDE.md)** - Complete guide to token savings analytics, temporal breakdowns, and data export
- **[CLAUDE.md](CLAUDE.md)** - Claude Code integration instructions and project context
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - Technical architecture and development guide

## License

MIT License - see [LICENSE](LICENSE) for details.

## Contributing

Contributions welcome! Please open an issue or PR on GitHub.
