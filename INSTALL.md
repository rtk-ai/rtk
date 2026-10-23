# RTK Installation Guide - For AI Coding Assistants

## ⚠️ Name Collision Warning

**There are TWO completely different projects named "rtk":**

1. ✅ **Rust Token Killer** (this project) - LLM token optimizer
   - Repos: `rtk-ai/rtk`
   - Has `rtk gain` command showing the savings dashboard

2. ❌ **Rust Type Kit** (reachingforthejack/rtk) - DIFFERENT PROJECT
   - Rust codebase query tool and type generator
   - **DO NOT install if you want token optimization**

## Pre-Installation Check (REQUIRED)

**AI assistants should ALWAYS verify if RTK is already installed before attempting installation.**

```bash
# Check if RTK is installed
rtk --version

# CRITICAL: Verify it's the Token Killer (not Type Kit)
rtk help | head  # Should show the "high-performance CLI proxy", NOT "command not found"

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
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh | sh
```

After installation, **verify you have the correct rtk**:
```bash
rtk help | head # Should show the "high-performance CLI proxy", NOT "command not found"
```

### Quick Install (Windows)

```powershell
winget install rtk-ai.rtk
```

### Alternative: Manual Installation

```bash
# From rtk-ai repository (NOT reachingforthejack!)
cargo install --git https://github.com/rtk-ai/rtk rtk

# ALWAYS VERIFY after installation
rtk help | head # Should show the "high-performance CLI proxy", NOT "command not found"
```

⚠️ **WARNING**: `cargo install rtk` from crates.io might install the wrong package (Rust Type Kit). Use the Git URL above and verify with `rtk gain`.

## Apache-2.0 edition (open source only, commercial use allowed)

The same `rtk` binary built without the BUSL-1.1 compression engine: the hook
rewrite system and every command filter, under Apache-2.0 alone. Pick this if
your policy does not allow a source-available licence in the build.

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | RTK_EDITION=oss sh
brew install rtk-ai/tap/rtk-oss
cargo install --git https://github.com/rtk-ai/rtk rtk --no-default-features
```

Release assets are named `rtk-oss-<target>`, plus `rtk-oss_amd64.deb` and
`rtk-oss.x86_64.rpm`. It does not include `rtk pipeline`, PostToolUse
compression, or delegation tasks.

## Project Initialization

### Which mode to choose?

```
  Do you want RTK active across ALL Claude Code projects?
  │
  ├─ YES → rtk init -g              (recommended)
  │         Hook + RTK.md (~10 tokens in context)
  │         Commands auto-rewritten transparently
  │
  ├─ YES, minimal → rtk init -g --hook-only
  │         Hook only, nothing added to CLAUDE.md
  │         Zero tokens in context
  │
  └─ NO, single project → rtk init
            RTK block in ./CLAUDE.md + .rtk/filters.toml template
            No hook, no global effect
```

### Recommended: Global Hook-First Setup

**Best for: All projects, automatic RTK usage**

```bash
rtk init -g
# → Registers the native `rtk hook claude` PreToolUse hook
# → Creates ~/.claude/RTK.md (short awareness file)
# → Adds @RTK.md reference to ~/.claude/CLAUDE.md
# → Prompts: "Patch settings.json? [y/N]"
# → If yes: patches + creates backup (~/.claude/settings.json.bak)

# Automated alternatives:
rtk init -g --auto-patch    # Patch without prompting
rtk init -g --no-patch      # Print manual instructions instead

# Verify installation
rtk init --show  # Check the hook is registered
```

**Context cost**: the hook adds a short `RTK.md` to your context instead of a full command reference, and rewrites commands transparently at no per-command context cost.

**What is settings.json?**
Claude Code's hook registry. RTK adds a PreToolUse hook that rewrites commands transparently. Without this, Claude won't invoke the hook automatically.

```
  Claude Code          settings.json       rtk hook claude       RTK binary
       │                    │                     │                    │
       │  "git status"      │                     │                    │
       │ ──────────────────►│                     │                    │
       │                    │  PreToolUse trigger  │                    │
       │                    │ ───────────────────►│                    │
       │                    │                     │  rewrite command   │
       │                    │                     │  → rtk git status  │
       │                    │◄────────────────────│                    │
       │                    │  updated command     │                    │
       │                    │                                          │
       │  execute: rtk git status                                      │
       │ ─────────────────────────────────────────────────────────────►│
       │                                                               │  filter
       │  "3 modified, 1 untracked ✓"                                  │
       │◄──────────────────────────────────────────────────────────────│
```

**Backup Safety**:
RTK backs up existing settings.json before changes. Restore if needed:
```bash
cp ~/.claude/settings.json.bak ~/.claude/settings.json
```

### Alternative: Local Project Setup

**Best for: Single project without hook**

```bash
cd /path/to/your/project
rtk init  # Adds the RTK block to ./CLAUDE.md and a .rtk/filters.toml template
```

**Context cost**: instructions loaded only for this project

### Upgrading from Previous Version

#### From old 137-line CLAUDE.md injection (pre-0.22)

```bash
rtk init -g  # Automatically migrates to hook-first mode
# → Removes old 137-line block
# → Installs hook + RTK.md
# → Adds @RTK.md reference
```

#### From an `rtk-rewrite.sh` shell hook (pre-0.37.2)

Since v0.37.2 the hook is a native binary command (`rtk hook claude`) and needs no Unix shell or jq. Older installs used an `rtk-rewrite.sh` script in `~/.claude/hooks/`.

```bash
# Migrate to the native binary hook
rtk init --global

# Verify the new hook is active
rtk init --show
# Should show: [ok] Hook: rtk hook claude (native binary command)
```

## Common User Flows

### First-Time User (Recommended)
```bash
# 1. Install RTK
cargo install --git https://github.com/rtk-ai/rtk rtk
rtk gain  # Verify (must show the savings dashboard)

# 2. Setup with prompts
rtk init -g
# → Answer 'y' when prompted to patch settings.json
# → Creates backup automatically

# 3. Restart Claude Code
# 4. Test: git status (should use rtk)
```

### CI/CD or Automation
```bash
# Non-interactive setup (no prompts)
rtk init -g --auto-patch

# Verify in scripts
rtk init --show | grep "Hook:"
```

### Conservative User (Manual Control)
```bash
# Get manual instructions without patching
rtk init -g --no-patch

# Review printed JSON snippet
# Manually edit ~/.claude/settings.json
# Restart Claude Code
```

### Temporary Trial
```bash
# Install hook
rtk init -g --auto-patch

# Later: remove everything
rtk init -g --uninstall

# Restore backup if needed
cp ~/.claude/settings.json.bak ~/.claude/settings.json
```

## Installation Verification

```bash
# Basic test
rtk ls .

# Test with git
rtk git status

# Test with pnpm
rtk pnpm list

# Test with Vitest
rtk vitest
```

## Uninstalling

### Complete Removal (Global Installations Only)

```bash
# Complete removal (global installations only)
rtk init -g --uninstall

# What gets removed:
#   - Hook: the `rtk hook claude` entry (and any legacy ~/.claude/hooks/rtk-rewrite.sh)
#   - Context: ~/.claude/RTK.md
#   - Reference: @RTK.md line from ~/.claude/CLAUDE.md
#   - Registration: RTK hook entry from settings.json

# Restart Claude Code after uninstall
```

**For Local Projects**: Manually remove RTK block from `./CLAUDE.md`

### Binary Removal

```bash
# If installed via cargo
cargo uninstall rtk

# If installed via package manager
brew uninstall rtk          # macOS Homebrew
winget uninstall rtk-ai.rtk # Windows winget
sudo apt remove rtk         # Debian/Ubuntu
sudo dnf remove rtk         # Fedora/RHEL
```

### Restore from Backup (if needed)

```bash
cp ~/.claude/settings.json.bak ~/.claude/settings.json
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

### Pnpm
```bash
rtk pnpm list     # Compact dependency tree
rtk pnpm outdated # Available updates
rtk pnpm install  # Silent installation
```

### Tests
```bash
rtk cargo test      # Filtered Cargo test output
rtk go test         # Filtered Go tests (NDJSON)
rtk jest            # Filtered Jest output
rtk vitest          # Filtered Vitest output
rtk playwright test # Filtered Playwright output
rtk pytest          # Filtered Python tests
rtk rake test       # Filtered Ruby tests
rtk rspec           # Filtered RSpec tests
rtk test <cmd> [args...]  # Generic test wrapper - failures only
```

### Statistics
```bash
rtk gain              # Savings dashboard
rtk gain --graph      # With ASCII graph
rtk gain --history    # With command history
```

## What RTK Filters

RTK compresses the output of a shell command before your agent reads it. What that looks like in practice:

| Operation | What RTK does to the output |
|-----------|-----------------------------|
| `vitest` / `jest` | Failures only; passing suites collapse to a count |
| `git status` | Compact stat format, grouped by state |
| `pnpm list` | Compact dependency tree |
| `pnpm outdated` | Package, current and target version only |
| `cargo test` | Failures only, with the assertion and location |

RTK reduces **bash output bytes**. That is the part RTK controls — it is not the same as reducing your bill by the same amount, because bash output is only one contributor to input tokens, and input tokens are only part of a bill that also counts output tokens.

See [How RTK Savings Work](docs/guide/resources/savings-explained.md) for the full explanation, including why the token counts RTK reports are estimates.

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
# Update to the latest release, then check the version
cargo install --git https://github.com/rtk-ai/rtk rtk --force
rtk --version
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

- **Website**: https://www.rtk-ai.app
- **Contact**: contact@rtk-ai.app
- **Troubleshooting**: See [Troubleshooting](docs/guide/resources/troubleshooting.md) for common issues
- **GitHub issues**: https://github.com/rtk-ai/rtk/issues
- **Pull Requests**: https://github.com/rtk-ai/rtk/pulls

⚠️ **If you installed the wrong rtk (Type Kit)**, see [Troubleshooting](docs/guide/resources/troubleshooting.md#rtk-gain-says-not-a-rtk-command)

## AI Assistant Checklist

Before each session:

- [ ] Verify RTK is installed: `rtk --version`
- [ ] If not installed → follow "Installation" above
- [ ] If project not initialized → `rtk init`
- [ ] Use `rtk` for ALL git/pnpm/test/vitest commands
- [ ] Check savings: `rtk gain`

**Golden Rule**: AI coding assistants should ALWAYS use `rtk` as a proxy for shell commands that generate verbose output (git, pnpm, npm, cargo test, vitest, docker, kubectl).
