# RTK Troubleshooting Guide

## Problem: "rtk gain" command not found

### Symptom
```bash
$ rtk --version
rtk 1.0.0  # (or similar)

$ rtk gain
rtk: 'gain' is not a rtk command. See 'rtk --help'.
```

### Root Cause
You installed the **wrong rtk package**. You have **Rust Type Kit** (reachingforthejack/rtk) instead of **Rust Token Killer** (rtk-ai/rtk).

### Solution

**1. Uninstall the wrong package:**
```bash
cargo uninstall rtk
```

**2. Install the correct one (Token Killer):**

#### Quick Install (Linux/macOS)
```bash
curl -fsSL https://github.com/rtk-ai/rtk/blob/master/install.sh | sh
```

#### Alternative: Manual Installation
```bash
cargo install --git https://github.com/rtk-ai/rtk
```

**3. Verify installation:**
```bash
rtk --version
rtk gain  # MUST show token savings stats, not error
```

If `rtk gain` now works, installation is correct.

---

## Problem: Confusion Between Two "rtk" Projects

### The Two Projects

| Project | Repository | Purpose | Key Command |
|---------|-----------|---------|-------------|
| **Rust Token Killer** ✅ | rtk-ai/rtk | LLM token optimizer for Claude Code | `rtk gain` |
| **Rust Type Kit** ❌ | reachingforthejack/rtk | Rust codebase query and type generator | `rtk query` |

### How to Identify Which One You Have

```bash
# Check if "gain" command exists
rtk gain

# Token Killer → Shows token savings stats
# Type Kit → Error: "gain is not a rtk command"
```

---

## Problem: cargo install rtk installs wrong package

### Why This Happens
If **Rust Type Kit** is published to crates.io under the name `rtk`, running `cargo install rtk` will install the wrong package.

### Solution
**NEVER use** `cargo install rtk` without verifying.

**Always use explicit repository URLs:**

```bash
# CORRECT - Token Killer
cargo install --git https://github.com/rtk-ai/rtk

# OR install from fork
git clone https://github.com/rtk-ai/rtk.git
cd rtk && git checkout feat/all-features
cargo install --path . --force
```

**After any installation, ALWAYS verify:**
```bash
rtk gain  # Must work if you want Token Killer
```

---

## Problem: RTK not working in Claude Code

### Symptom
Claude Code doesn't seem to be using rtk, outputs are verbose.

### Checklist

**1. Verify rtk is installed and correct:**
```bash
rtk --version
rtk gain  # Must show stats
```

**2. Initialize rtk for Claude Code:**
```bash
# Global (all projects)
rtk init --global

# Per-project
cd /your/project
rtk init
```

**3. Verify CLAUDE.md file exists:**
```bash
# Check global
cat ~/.claude/CLAUDE.md | grep rtk

# Check project
cat ./CLAUDE.md | grep rtk
```

**4. Install auto-rewrite hook (recommended for automatic RTK usage):**

**Option A: Automatic (recommended)**
```bash
rtk init -g
# → Installs hook + RTK.md automatically
# → Follow printed instructions to add hook to ~/.claude/settings.json
# → Restart Claude Code

# Verify installation
rtk init --show  # Should show "✅ Hook: executable, with guards"
```

**Option B: Manual (fallback)**
```bash
# Copy hook to Claude Code hooks directory
mkdir -p ~/.claude/hooks
cp .claude/hooks/rtk-rewrite.sh ~/.claude/hooks/
chmod +x ~/.claude/hooks/rtk-rewrite.sh
```

Then add to `~/.claude/settings.json` (replace `~` with full path):
```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "/Users/yourname/.claude/hooks/rtk-rewrite.sh"
          }
        ]
      }
    ]
  }
}
```

**Note**: Use absolute path in `settings.json`, not `~/.claude/...`

### Search Priority Policy

**Search priority (mandatory): rgai > rg > grep.**

- Use `rtk rgai` first for semantic/intention-based discovery.
- Use `rtk grep` for exact/regex matching.
- `rtk grep` internally follows `rg -> grep` backend fallback.
- If you need fully raw output for debugging, use `rtk proxy <cmd>`.

---

## Problem: "command not found: rtk" after installation

### Symptom
```bash
$ cargo install --path . --force
   Compiling rtk v0.7.1
    Finished release [optimized] target(s)
  Installing ~/.cargo/bin/rtk

$ rtk --version
zsh: command not found: rtk
```

### Root Cause
`~/.cargo/bin` is not in your PATH.

### Solution

**1. Check if cargo bin is in PATH:**
```bash
echo $PATH | grep -o '[^:]*\.cargo[^:]*'
```

**2. If not found, add to PATH:**

For **bash** (`~/.bashrc`):
```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

For **zsh** (`~/.zshrc`):
```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

For **fish** (`~/.config/fish/config.fish`):
```fish
set -gx PATH $HOME/.cargo/bin $PATH
```

**3. Reload shell config:**
```bash
source ~/.bashrc  # or ~/.zshrc or restart terminal
```

**4. Verify:**
```bash
which rtk
rtk --version
rtk gain
```

---

## Problem: Compilation errors during installation

### Symptom
```bash
$ cargo install --path .
error: failed to compile rtk v0.7.1
```

### Solutions

**1. Update Rust toolchain:**
```bash
rustup update stable
rustup default stable
```

**2. Clean and rebuild:**
```bash
cargo clean
cargo build --release
cargo install --path . --force
```

**3. Check Rust version (minimum required):**
```bash
rustc --version  # Should be 1.70+ for most features
```

**4. If still fails, report issue:**
- GitHub: https://github.com/rtk-ai/rtk/issues

---

## Need More Help?

**Report issues:**
- Fork-specific: https://github.com/rtk-ai/rtk/issues
- Upstream: https://github.com/rtk-ai/rtk/issues

**Run the diagnostic script:**
```bash
# From the rtk repository root
bash scripts/check-installation.sh
```

This script will check:
- ✅ RTK installed and in PATH
- ✅ Correct version (Token Killer, not Type Kit)
- ✅ Available features (pnpm, vitest, next, etc.)
- ✅ Claude Code integration (CLAUDE.md files)
- ✅ Auto-rewrite hook status

The script provides specific fix commands for any issues found.
