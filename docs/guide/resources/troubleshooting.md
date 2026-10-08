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
   rtk init --global --opencode  # Claude Code + OpenCode plugin
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

### Hook not working (no auto-rewrite)

**Symptom:** On native Windows, commands are not auto-rewritten. An older `rtk init -g` printed "Falling back to --claude-md mode".

**Cause:** Before v0.37.2, `rtk init -g` registered no hook on native Windows: it fell back to injecting the full RTK instructions into `~/.claude/CLAUDE.md`. A setup made then still has no hook.

**Fix:** Upgrade to v0.37.2 or later, where `rtk init -g` registers the auto-rewrite hook on Windows as a native binary command (`rtk hook claude`). No Unix shell, bash, or jq is required. Re-run `rtk init -g`: it replaces the CLAUDE.md block with an `@RTK.md` reference and, once you confirm, adds `rtk hook claude` to `settings.json`. A legacy `~/.claude/hooks/rtk-rewrite.sh` hook, if one exists, is deleted along with its `.rtk-hook.sha256` and its `settings.json` entry.

Answer `y` when it asks to patch `settings.json`. Outside a terminal it cannot ask and defaults to `N`, so use `--auto-patch` there:

```powershell
rtk init -g                # answer y at the settings.json prompt
rtk init -g --auto-patch   # or: patch settings.json without asking
```

The default `N` leaves no hook registered: any legacy `rtk-rewrite.sh` entry is removed and `rtk hook claude` is not added. The `RTK hook registered (global).` banner prints either way; the line that confirms the patch is `settings.json: hook added` (or `settings.json: hook already present` when an earlier run added it). Restart Claude Code, then confirm:

```powershell
rtk init --show
```

It should report `[ok] Hook: rtk hook claude (native binary command)`.

[WSL](https://learn.microsoft.com/en-us/windows/wsl/install) also works and behaves like Linux if you prefer it.

### Node.js tools not found

**Symptom:**
```
rtk vitest --run
Error: program not found
```

**Cause:** On Windows, Node.js tools are installed as `.CMD`/`.BAT` wrappers. Older RTK versions couldn't find them.

**Fix:** Update to RTK v0.23.1+:
```bash
cargo install --git https://github.com/rtk-ai/rtk --branch master
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

Minimum required Rust version: 1.91 (edition 2024 needs Cargo 1.85 or newer).

## OpenCode not using RTK

```bash
rtk init --global --opencode
# restart OpenCode
rtk init --show    # should show "OpenCode: plugin installed"
```

## `cargo install rtk` installs the wrong package

If Rust Type Kit is published to crates.io under the name `rtk`, `cargo install rtk` may install the wrong one.

Always use the explicit URL, pinned to the release branch:

```bash
cargo install --git https://github.com/rtk-ai/rtk --branch master
```

## Does RTK break Claude's prompt cache?

No. RTK filters command output once, at execution time. The filtered result is written into the
conversation history and never changes afterwards, and prompt caching matches on a stable prefix
— RTK does not rewrite anything the cache has already seen.

Smaller tool results also make caching cheaper: cache writes bill at 1.25x and cache reads at
0.1x the input rate, so fewer tokens in means less to write once and less to re-read every turn.

To see your own cache write and read volumes next to RTK's savings:

```bash
rtk cc-economics
```

`rtk gain` reports token savings only; the cache breakdown lives in `rtk cc-economics`.

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
