# RTK Installation Guide - For AI Coding Assistants

## ⚠️ Name Collision Warning

**There are TWO completely different projects named "rtk":**

1. ✅ **Rust Token Killer** (this project) - LLM token optimizer
   - Repos: `rtk-ai/rtk`
   - Has `rtk gain` command for token savings stats

2. ❌ **Rust Type Kit** (reachingforthejack/rtk) - DIFFERENT PROJECT
   - Rust codebase query tool and type generator
   - **DO NOT install if you want token optimization**

## Pre-Installation Check (REQUIRED)

**AI assistants should ALWAYS verify if RTK is already installed before attempting installation.**

```bash
# Check if RTK is installed
rtk --version

# CRITICAL: Verify it's the Token Killer (not Type Kit)
rtk gain    # Should show token savings stats, NOT "command not found"

# Check installation path
which rtk
```

If `rtk gain` works, you have the **correct** RTK installed. **DO NOT reinstall**. Skip to "Project Initialization".

If `rtk gain` fails but `rtk --version` succeeds, you have the **wrong** RTK (Type Kit). Uninstall and reinstall the correct one (see below).

## Installation (only if RTK not available or wrong RTK installed)

### Step 0: Uninstall Wrong RTK (if needed)

If you accidentally installed Rust Type Kit:

```bash
cargo uninstall rtk
```

### Quick Install (Linux/macOS)

```bash
curl -fsSL https://github.com/rtk-ai/rtk/blob/master/install.sh | sh
```

After installation, **verify you have the correct rtk**:
```bash
rtk gain  # Must show token savings stats (not "command not found")
```

### Alternative: Manual Installation

```bash
# From rtk-ai repository (NOT reachingforthejack!)
cargo install --git https://github.com/rtk-ai/rtk

# OR (if published and correct on crates.io)
cargo install rtk

# ALWAYS VERIFY after installation
rtk gain  # MUST show token savings, not "command not found"
```

⚠️ **WARNING**: `cargo install rtk` from crates.io might install the wrong package. Always verify with `rtk gain`.

## Project Initialization

**For each project where you want to use RTK:**

```bash
# Navigate to project directory
cd /path/to/your/project

# Initialize RTK for this project (creates ./CLAUDE.md)
rtk init

# OR initialize globally (creates ~/CLAUDE.md for all projects)
rtk init --global
```

## Installation Verification

```bash
# Basic test
rtk ls .

# Test with git
rtk git status

# Test with pnpm (fork only)
rtk pnpm list

# Test with Vitest (feat/vitest-support branch only)
rtk vitest run
```

## Essential Commands

### Files
```bash
rtk ls .              # Compact tree view
rtk read file.rs      # Optimized reading
rtk grep "pattern" .  # Grouped search results
```

### Git
```bash
rtk git status        # Compact status
rtk git log -n 10     # Condensed logs
rtk git diff          # Optimized diff
rtk git add .         # → "ok ✓"
rtk git commit -m "msg"  # → "ok ✓ abc1234"
rtk git push          # → "ok ✓ main"
```

### Pnpm (fork only)
```bash
rtk pnpm list         # Dependency tree (-70% tokens)
rtk pnpm outdated     # Available updates (-80-90%)
rtk pnpm install pkg  # Silent installation
```

### Tests
```bash
rtk test cargo test   # Failures only (-90%)
rtk vitest run        # Filtered Vitest output (-99.6%)
```

### Statistics
```bash
rtk gain              # Token savings
rtk gain --graph      # With ASCII graph
rtk gain --history    # With command history
```

## Validated Token Savings

### Production T3 Stack Project
| Operation | Standard | RTK | Reduction |
|-----------|----------|-----|-----------|
| `vitest run` | 102,199 chars | 377 chars | **-99.6%** |
| `git status` | 529 chars | 217 chars | **-59%** |
| `pnpm list` | ~8,000 tokens | ~2,400 | **-70%** |
| `pnpm outdated` | ~12,000 tokens | ~1,200-2,400 | **-80-90%** |

### Typical Claude Code Session (30 min)
- **Without RTK**: ~150,000 tokens
- **With RTK**: ~45,000 tokens
- **Savings**: **70% reduction**

## Troubleshooting

### RTK command not found after installation
```bash
# Check PATH
echo $PATH | grep -o '[^:]*\.cargo[^:]*'

# Add to PATH if needed (~/.bashrc or ~/.zshrc)
export PATH="$HOME/.cargo/bin:$PATH"

# Reload shell
source ~/.bashrc  # or source ~/.zshrc
```

### RTK command not available (e.g., vitest)
```bash
# Check branch
cd /path/to/rtk
git branch

# Switch to feat/vitest-support if needed
git checkout feat/vitest-support

# Reinstall
cargo install --path . --force
```

### Compilation error
```bash
# Update Rust
rustup update stable

# Clean and recompile
cargo clean
cargo build --release
cargo install --path . --force
```

## Support and Contributing

- **Troubleshooting**: See [TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) for common issues
- **GitHub issues**: https://github.com/rtk-ai/rtk/issues
- **Pull Requests**: https://github.com/rtk-ai/rtk/pulls

⚠️ **If you installed the wrong rtk (Type Kit)**, see [TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md#problem-rtk-gain-command-not-found)

## AI Assistant Checklist

Before each session:

- [ ] Verify RTK is installed: `rtk --version`
- [ ] If not installed → follow "Install from fork"
- [ ] If project not initialized → `rtk init`
- [ ] Use `rtk` for ALL git/pnpm/test/vitest commands
- [ ] Check savings: `rtk gain`

**Golden Rule**: AI coding assistants should ALWAYS use `rtk` as a proxy for shell commands that generate verbose output (git, pnpm, npm, cargo test, vitest, docker, kubectl).
