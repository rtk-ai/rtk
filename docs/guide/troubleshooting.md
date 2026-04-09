---
title: Troubleshooting
description: Common RTK issues and how to fix them
sidebar:
  order: 8
---

# Troubleshooting

## `rtk gain` says "not a rtk command"

**Symptom:**
```bash
$ rtk gain
rtk: 'gain' is not a rtk command. See 'rtk --help'.
```

**Cause:** You installed **Rust Type Kit** (`reachingforthejack/rtk`) instead of **Rust Token Killer** (`rtk-ai/rtk`). They share the same binary name.

**Fix:**
```bash
cargo uninstall rtk
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh | sh
rtk gain    # should now show token savings stats
```

## How to tell which rtk you have

| If `rtk gain`... | You have |
|------------------|----------|
| Shows token savings dashboard | Rust Token Killer ✅ |
| Returns "not a rtk command" | Rust Type Kit ❌ |

## AI assistant not using RTK

**Symptom:** Claude Code (or another agent) runs `cargo test` instead of `rtk cargo test`.

**Checklist:**

1. Verify RTK is installed:
   ```bash
   rtk --version
   rtk gain
   ```

2. Initialize the hook:
   ```bash
   rtk init --global    # Claude Code
   rtk init --global --cursor    # Cursor
   rtk init --global --opencode  # OpenCode
   ```

3. Restart your AI assistant.

4. Verify hook status:
   ```bash
   rtk init --show
   ```

5. Check `settings.json` has the hook registered (Claude Code):
   ```bash
   cat ~/.claude/settings.json | grep rtk
   ```

## RTK not found after `cargo install`

**Symptom:**
```bash
$ rtk --version
zsh: command not found: rtk
```

**Cause:** `~/.cargo/bin` is not in your PATH.

**Fix:**

For bash (`~/.bashrc`) or zsh (`~/.zshrc`):
```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

For fish (`~/.config/fish/config.fish`):
```fish
set -gx PATH $HOME/.cargo/bin $PATH
```

Then reload:
```bash
source ~/.zshrc    # or ~/.bashrc
rtk --version
```

## RTK not working on Windows

**Symptom:**
```
rtk vitest --run
Error: program not found
```

**Cause:** On Windows, Node.js tools are installed as `.CMD`/`.BAT` wrappers. Older RTK versions couldn't find them.

**Fix:** Update to RTK v0.23.1+:
```bash
cargo install --git https://github.com/rtk-ai/rtk
rtk --version    # should be 0.23.1+
```

## Compilation error during installation

```bash
rustup update stable
rustup default stable
cargo clean
cargo build --release
cargo install --path . --force
```

Minimum required Rust version: 1.70+.

## OpenCode not using RTK

```bash
rtk init --global --opencode
# restart OpenCode
rtk init --show    # should show "OpenCode: plugin installed"
```

## `cargo install rtk` installs the wrong package

If Rust Type Kit is published to crates.io under the name `rtk`, `cargo install rtk` may install the wrong one.

Always use the explicit URL:

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

## Run the diagnostic script

From the RTK repository root:

```bash
bash scripts/check-installation.sh
```

Checks:
- RTK installed and in PATH
- Correct version (Token Killer, not Type Kit)
- Available features
- Claude Code integration
- Hook status

## Still stuck?

Open an issue: https://github.com/rtk-ai/rtk/issues
