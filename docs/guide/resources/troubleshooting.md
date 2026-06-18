---
title: Troubleshooting
description: Common RTK issues and how to fix them
sidebar:
  order: 2
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

## RTK on Windows

### Double-clicking rtk.exe does nothing

**Symptom:** You double-click `rtk.exe`, a terminal flashes and closes instantly.

**Cause:** RTK is a command-line tool. With no arguments, it prints usage and exits. The console window opens and closes before you can read anything.

**Fix:** Open a terminal first, then run RTK from there:
- Press `Win+R`, type `cmd`, press Enter
- Or open PowerShell or Windows Terminal
- Then run: `rtk --version`

### Hook not working (no auto-rewrite) on Windows

**Symptom:** Commands like `git status` are not being rewritten to `rtk git status` on native Windows.

**Cause:** The Claude Code hook is the native `rtk hook claude` subcommand — it works on native Windows, so this is almost always a registration or PATH issue, not a platform limitation.

**Fix:**
1. Confirm `rtk.exe` is on your PATH: open a **new** terminal and run `rtk --version`.
2. Register the hook: `rtk init -g` (it writes `rtk hook claude` into `~/.claude/settings.json`).
3. Verify it is recognized: `rtk init --show` should report `[ok] Hook: rtk hook claude`.
4. **Restart Claude Code** so it reloads `settings.json`.
5. Sanity-check the hook directly:
   ```powershell
   '{"tool_name":"Bash","tool_input":{"command":"git status"}}' | rtk hook claude
   # Expect JSON whose updatedInput.command is "rtk git status"
   ```

WSL also works and behaves exactly like Linux, but it is no longer required for Claude Code auto-rewrite.

### Node.js tools not found

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
