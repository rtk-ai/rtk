# RTK Installation Guide - For AI Coding Assistants

## Windows note

Native Windows support for the RTK binary exists, but this guide is still largely Unix-first in its install and hook examples.

Before following Windows instructions here, read:

- [WINDOWS.md](WINDOWS.md)

Short version:

- Native Windows: use the Windows binary or `cargo build --release`
- Linux/macOS: `install.sh` and hook-first setup are supported
- Windows hook-first Claude setup: not a complete native story yet
- If you want Bash-heavy docs and `.sh` scripts to work on Windows, install Git Bash first

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

PowerShell equivalent:

```powershell
rtk --version
rtk gain
Get-Command rtk
```

If `rtk gain` works, you have the **correct** RTK installed. **DO NOT reinstall**. Skip to "Project Initialization".

If `rtk gain` fails but `rtk --version` succeeds, you have the **wrong** RTK (Type Kit). Uninstall and reinstall the correct one (see below).

## Installation (only if RTK not available or wrong RTK installed)

### Step 0: Uninstall Wrong RTK (if needed)

If you accidentally installed Rust Type Kit:

```bash
cargo uninstall rtk
```

### Linux/macOS: Quick Install

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh | sh
```

After installation, **verify you have the correct rtk**:

```bash
rtk gain  # Must show token savings stats (not "command not found")
```

### Windows: Native Install Options

#### Option 1: Build from source

```powershell
cargo build --release
.\target\release\rtk.exe --version
.\target\release\rtk.exe gain
```

#### Option 2: Use the Windows release asset

Download:

- `rtk-x86_64-pc-windows-msvc.zip`

Then extract `rtk.exe`, add it to `PATH`, open a new terminal, and verify:

```powershell
rtk --version
rtk gain
```

### Manual Installation (all platforms)

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

## Platform split

- Linux/macOS:
  - hook-first setup via `rtk init -g` is the recommended path
- Windows:
  - local `rtk init` can still be useful
  - hook-first global setup is not a complete native-Windows flow yet
  - do not assume Bash hook instructions below work in plain PowerShell/cmd

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
            Local CLAUDE.md only (137 lines)
            No hook, no global effect
```

### Recommended: Global Hook-First Setup

**Best for: Linux/macOS projects, automatic RTK usage**

```bash
rtk init -g
# → Installs hook to ~/.claude/hooks/rtk-rewrite.sh
# → Creates ~/.claude/RTK.md (10 lines, meta commands only)
# → Adds @RTK.md reference to ~/.claude/CLAUDE.md
# → Prompts: "Patch settings.json? [y/N]"
# → If yes: patches + creates backup (~/.claude/settings.json.bak)

# Automated alternatives:
rtk init -g --auto-patch    # Patch without prompting
rtk init -g --no-patch      # Print manual instructions instead

# Verify installation
rtk init --show  # Check hook is installed and executable
```

**Token savings**: ~99.5% reduction (2000 tokens → 10 tokens in context)

**Windows warning**:

- This flow installs `rtk-rewrite.sh` and assumes Unix shell behavior.
- Native Windows users should not treat this section as a complete PowerShell/cmd setup guide.
- See [WINDOWS.md](WINDOWS.md) for the current native-Windows status.

**What is settings.json?**
Claude Code's hook registry. RTK adds a PreToolUse hook that rewrites commands transparently. Without this, Claude won't invoke the hook automatically.

```
  Claude Code          settings.json        rtk-rewrite.sh        RTK binary
       │                    │                     │                    │
       │  "git status"      │                     │                    │
       │ ──────────────────►│                     │                    │
       │                    │  PreToolUse trigger │                    │
       │                    │ ───────────────────►│                    │
       │                    │                     │  rewrite command   │
       │                    │                     │  → rtk git status  │
       │                    │◄────────────────────│                    │
       │                    │  updated command    │                    │
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

PowerShell equivalent:

```powershell
Copy-Item $HOME\.claude\settings.json.bak $HOME\.claude\settings.json -Force
```

### Alternative: Local Project Setup

**Best for: Single project without hook**

```bash
cd /path/to/your/project
rtk init  # Creates ./CLAUDE.md with full RTK instructions (137 lines)
```

**Token savings**: Instructions loaded only for this project

This is the safer native-Windows option today if you want project-local RTK instructions without relying on the Unix hook flow.

### Upgrading from Previous Version

#### From old 137-line CLAUDE.md injection (pre-0.22)

```bash
rtk init -g  # Automatically migrates to hook-first mode
# → Removes old 137-line block
# → Installs hook + RTK.md
# → Adds @RTK.md reference
```

#### From old hook with inline logic (pre-0.24) — ⚠️ Breaking Change

RTK 0.24.0 replaced the inline command-detection hook (~200 lines) with a **thin delegator** that calls `rtk rewrite`. The binary now contains the rewrite logic, so adding new commands no longer requires a hook update.

The old hook still works but won't benefit from new rules added in future releases.

```bash
# Upgrade hook to thin delegator
rtk init --global

# Verify the new hook is active
rtk init --show
# Should show: ✅ Hook: ... (thin delegator, up to date)
```

## Common User Flows

### First-Time User (Linux/macOS Recommended)

```bash
# 1. Install RTK
cargo install --git https://github.com/rtk-ai/rtk
rtk gain  # Verify (must show token stats)

# 2. Setup with prompts
rtk init -g
# → Answer 'y' when prompted to patch settings.json
# → Creates backup automatically

# 3. Restart Claude Code
# 4. Test: git status (should use rtk)
```

### First-Time User (Windows Native)

```powershell
# Option A: build from source in this repository
cargo build --release

# Verify the binary you just built
.\target\release\rtk.exe --version
.\target\release\rtk.exe gain
```

Plain-language steps:

1. Open PowerShell in the RTK repository root.
2. Run `cargo build --release`.
3. Wait for the build to finish. This creates `rtk.exe` at:
   - `.\target\release\rtk.exe`
4. Verify that the binary starts correctly:
   - `.\target\release\rtk.exe --version`
5. Verify that you have the correct `rtk` project and not the other crate with the same name:
   - `.\target\release\rtk.exe gain`
6. If both commands work, RTK itself is usable on native Windows.
7. If you want RTK instructions in this project only, run:
   - `.\target\release\rtk.exe init`
8. This local `init` step updates the project-level `CLAUDE.md`. It is not the same as the Unix hook-first global setup.

If you want `rtk` available from any folder instead of running `.\target\release\rtk.exe` directly:

1. Copy `rtk.exe` to a permanent folder, for example:
   - `C:\Tools\rtk\rtk.exe`
2. Add that folder to your user `PATH`.
3. Close PowerShell and open a new one.
4. Verify:

```powershell
rtk --version
rtk gain
```

If you prefer the prebuilt release instead of building from source:

1. Download the Windows asset:
   - `rtk-x86_64-pc-windows-msvc.zip`
2. Extract `rtk.exe` to a permanent folder.
3. Add that folder to `PATH`.
4. Open a new PowerShell window.
5. Verify:

```powershell
rtk --version
rtk gain
```

If you want hook-based global rewriting on Windows, treat that as a separate portability task rather than following the Unix hook steps verbatim.

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

For native Windows, prefer reading the printed output as reference material instead of assuming the Unix path and shell commands are directly runnable.

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

# Test with pnpm (fork only)
rtk pnpm list

# Test with Vitest (feat/vitest-support branch only)
rtk vitest run
```

PowerShell/native Windows verification:

```powershell
rtk --version
rtk gain
rtk ls .
rtk read Cargo.toml
```

## Uninstalling

### Complete Removal (Global Installations Only)

```bash
# Complete removal (global installations only)
rtk init -g --uninstall

# What gets removed:
#   - Hook: ~/.claude/hooks/rtk-rewrite.sh
#   - Context: ~/.claude/RTK.md
#   - Reference: @RTK.md line from ~/.claude/CLAUDE.md
#   - Registration: RTK hook entry from settings.json

# Restart Claude Code after uninstall
```

**For Local Projects**: Manually remove RTK block from `./CLAUDE.md`

**Windows note**: this uninstall flow refers to the Unix hook-based setup. If you only used the native Windows binary, remove the binary or its containing folder from `PATH` instead.

### Binary Removal

```bash
# If installed via cargo
cargo uninstall rtk

# If installed via package manager
brew uninstall rtk          # macOS Homebrew
sudo apt remove rtk         # Debian/Ubuntu
sudo dnf remove rtk         # Fedora/RHEL
```

PowerShell example for a manually extracted Windows binary:

```powershell
Remove-Item C:\path\to\rtk.exe
```

### Restore from Backup (if needed)

```bash
cp ~/.claude/settings.json.bak ~/.claude/settings.json
```

PowerShell equivalent:

```powershell
Copy-Item $HOME\.claude\settings.json.bak $HOME\.claude\settings.json -Force
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

PowerShell equivalent:

```powershell
$env:Path -split ';' | Select-String '\.cargo'
Get-Command rtk
```

If `rtk.exe` is not found, reopen the terminal after updating `PATH`.

For a Windows-native install check, you can also run:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\check-installation.ps1
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

Windows-specific sanity checks:

```powershell
rustup show active-toolchain
cargo build
cargo test --test cli_tools_smoke
.\target\debug\rtk.exe --version
```

### Local Windows install helper

If you are in this repository and want to copy the local release build into a folder of your choice:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install-local.ps1
```

Optional custom install directory:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install-local.ps1 -InstallDir C:\Tools\rtk
```

## Support and Contributing

- **Website**: <https://www.rtk-ai.app>
- **Contact**: <contact@rtk-ai.app>
- **Troubleshooting**: See [TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) for common issues
- **GitHub issues**: <https://github.com/rtk-ai/rtk/issues>
- **Pull Requests**: <https://github.com/rtk-ai/rtk/pulls>

⚠️ **If you installed the wrong rtk (Type Kit)**, see [TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md#problem-rtk-gain-command-not-found)

## AI Assistant Checklist

Before each session:

- [ ] Verify RTK is installed: `rtk --version`
- [ ] If not installed → follow "Install from fork"
- [ ] If project not initialized → `rtk init`
- [ ] Use `rtk` for ALL git/pnpm/test/vitest commands
- [ ] Check savings: `rtk gain`

**Golden Rule**: AI coding assistants should ALWAYS use `rtk` as a proxy for shell commands that generate verbose output (git, pnpm, npm, cargo test, vitest, docker, kubectl).
