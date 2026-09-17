---
title: Installation
description: Install RTK via curl, Homebrew, winget, Cargo, or from source, and verify the correct version
sidebar:
  order: 1
---

# Installation

## Name collision warning

Two unrelated projects share the name `rtk`. Make sure you install the right one:

- **Rust Token Killer** (`rtk-ai/rtk`) — this project, a token-saving CLI proxy
- **Rust Type Kit** (`reachingforthejack/rtk`) — a different tool for generating Rust types

The easiest way to verify you have the correct one: run `rtk gain`. It should display the savings dashboard. If it returns "command not found", you either have the wrong package or RTK is not installed.

## Check before installing

```bash
rtk --version   # should print: rtk x.y.z
rtk gain        # should show the savings dashboard
```

If both commands work, RTK is already installed. Skip to [Project initialization](#project-initialization).

## Quick install (Linux and macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh | sh
```

Installs to `~/.local/bin`. Add it to your PATH if it is not there already:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc   # or ~/.zshrc
```

## Homebrew (macOS and Linux)

```bash
brew install rtk-ai/tap/rtk
```

## winget (Windows)

```powershell
winget install rtk-ai.rtk
```

## Cargo

:::caution[Name collision risk]
`cargo install rtk` may install **Rust Type Kit** instead of Rust Token Killer — two unrelated projects share the same crate name. Use the explicit Git URL to guarantee the correct package:
:::

```bash
cargo install --git https://github.com/rtk-ai/rtk --branch master rtk
```

## Pre-built binaries (Windows, Linux, macOS)

Download from [GitHub releases](https://github.com/rtk-ai/rtk/releases):

- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-musl.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

**Windows users**: Extract the zip and place `rtk.exe` in a directory on your PATH. Run RTK from Command Prompt, PowerShell, or Windows Terminal — do not double-click the `.exe` (it prints usage and exits immediately). See [Windows](#windows) below — the hook system works natively, no WSL required.

## Apache-2.0 edition

The same `rtk` binary built without the BUSL-1.1 compression engine — the hook rewrite system and every command filter, under Apache-2.0 alone. Pick this if your policy does not allow a source-available licence in the build.

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh | RTK_EDITION=oss sh
brew install rtk-ai/tap/rtk-oss
cargo install --git https://github.com/rtk-ai/rtk --branch master rtk --no-default-features
```

Release assets are named `rtk-oss-<target>`, with `rtk-oss` Debian and RPM packages alongside. The binary is still called `rtk`, so hooks and scripts are unchanged — but the two editions conflict and cannot be installed together.

See [Editions and Licensing](../resources/editions.md) for what the edition omits and where the licence boundary falls.

## Windows

RTK works fully on native Windows. Since **v0.37.2** the auto-rewrite hook runs as a native binary command (`rtk hook claude`) — no Unix shell, bash, or jq required — so commands are rewritten transparently in Command Prompt, PowerShell, and Windows Terminal, exactly as on Linux and macOS.

### Native Windows (manual install)

Prefer [winget](#winget-windows) if you can — it handles PATH for you.

```powershell
# 1. Download and extract rtk-x86_64-pc-windows-msvc.zip from releases
# 2. Add rtk.exe to your PATH (e.g. C:\Users\<you>\.local\bin)
# 3. Initialize - installs the native binary hook
rtk init -g
```

**Upgrading from an older install?** If you set RTK up before v0.37.2 you may still have the legacy `rtk-rewrite.sh` shell hook, which does need a Unix shell. Re-run `rtk init -g` to migrate to the native binary hook.

**Prerequisites**: some filters shell out to [ripgrep](https://github.com/BurntSushi/ripgrep) (`rg`). Install it and keep it on your PATH to avoid `Binary 'rg' not found on PATH` warnings:

```powershell
winget install BurntSushi.ripgrep.MSVC
```

### WSL

[WSL](https://learn.microsoft.com/en-us/windows/wsl/install) also works and behaves exactly like Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/master/install.sh | sh
rtk init -g
```

| Feature | Native Windows | WSL |
|---|---|---|
| Filters (cargo, git, …) | Full | Full |
| Auto-rewrite hook | Yes (native binary) | Yes |
| `rtk init -g` | Hook mode | Hook mode |
| `rtk gain` / analytics | Full | Full |

## Verify installation

```bash
rtk --version   # rtk x.y.z
rtk gain        # savings dashboard
```

If `rtk gain` fails but `rtk --version` succeeds, you installed Rust Type Kit by mistake. Uninstall it first:

```bash
cargo uninstall rtk
```

Then reinstall using one of the methods above.

## Project initialization

For a single project, `rtk init` adds the RTK block to `./CLAUDE.md` and a `.rtk/filters.toml` template, without installing a hook:

```bash
rtk init
```

To install the Claude Code hook (registered in `~/.claude/settings.json`, applies to every project):

```bash
rtk init --global
```

## Shell Completion

Enable tab-completion for `rtk` subcommands:

```bash
# bash — current shell, or add the source line to ~/.bashrc
source <(rtk completions bash)

# zsh — current shell, or add the source line to ~/.zshrc
source <(rtk completions zsh)

# fish — current shell, or copy the script to ~/.config/fish/completions/rtk.fish
rtk completions fish | source

# powershell — add this line to your $PROFILE
rtk completions powershell | Out-String | Invoke-Expression

# elvish — copy the script to ~/.config/elvish/lib/rtk-completions.elv and eval it
rtk completions elvish
```

Supported shells: `bash`, `zsh`, `fish`, `powershell`, `elvish`.

## Uninstall

```bash
rtk init -g --uninstall    # remove hook, RTK.md, and settings.json entry
cargo uninstall rtk         # remove binary (if installed via Cargo)
brew uninstall rtk          # remove binary (if installed via Homebrew)
winget uninstall rtk-ai.rtk # remove binary (if installed via winget)
```
