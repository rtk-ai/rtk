# RTK Windows-Native Full Support (No WSL Required)

This document describes how to achieve **100% RTK hook support** on native Windows
for GitHub Copilot Chat in VS Code — without WSL.

## Background

The RTK README states that WSL is required for full hook support. This is only true
for the **Claude Code shell hook** (`rtk-rewrite.sh`) which requires bash.

For **VS Code Copilot Chat**, RTK uses a completely different mechanism:
the `PreToolUse` hook — a native Rust binary (`rtk hook copilot`) that works on
any platform. This binary:

1. Receives JSON via stdin from VS Code's hook system
2. Rewrites the command using RTK's registry engine
3. Returns JSON via stdout with the rewritten command

No shell scripting, no bash, no WSL. Pure cross-platform binary execution.

## Architecture

```
  VS Code Copilot Chat                    Terminal
        │                                     ▲
        │ tool_input.command = "git status"   │ "rtk git status"
        ▼                                     │
  ┌─────────────────────┐              ┌──────┴──────┐
  │ PreToolUse Hook     │──stdin JSON──▶│ rtk.exe     │
  │ (rtk-rewrite.json)  │◀─stdout JSON─│ hook copilot│
  └─────────────────────┘              └─────────────┘
        │
        │ updatedInput.command = "rtk git status"
        ▼
  Terminal executes rewritten command
```

The hook config (`rtk-rewrite.json`) tells VS Code to pipe every terminal command
through `rtk hook copilot` before execution. The binary decides whether to rewrite
(supported commands) or pass through (unknown commands).

## Quick Setup

```powershell
# One-command setup (global — all workspaces)
.\scripts\windows-copilot-setup.ps1 -Global

# With project-scoped hooks too
.\scripts\windows-copilot-setup.ps1 -Global -Project

# Force overwrite existing config
.\scripts\windows-copilot-setup.ps1 -Global -Force
```

## Manual Setup

### 1. Install RTK Binary

```powershell
# Download latest release from GitHub
$dest = "$env:USERPROFILE\.local\bin"
New-Item -ItemType Directory -Path $dest -Force

# Get the latest release URL (or substitute a specific version tag)
$release = Invoke-RestMethod "https://api.github.com/repos/rtk-ai/rtk/releases/latest"
$asset = $release.assets | Where-Object { $_.name -like "*windows-msvc*" }
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile "$env:TEMP\rtk.zip"
Expand-Archive "$env:TEMP\rtk.zip" -DestinationPath $dest -Force

# Add to PATH permanently
$path = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($path -notlike "*$dest*") {
    [Environment]::SetEnvironmentVariable("PATH", "$dest;$path", "User")
}

# Verify
rtk --version  # Should show "rtk x.y.z"
```

### 2. Install ripgrep (for `rtk grep`)

```powershell
winget install BurntSushi.ripgrep.MSVC
```

### 3. Create ls shim (for `rtk ls`)

```powershell
Set-Content "$env:USERPROFILE\.local\bin\ls.cmd" '@echo off
if "%~1"=="" ( dir /b ) else ( dir /b %* )'
```

### 4. Configure the PreToolUse Hook

**Global** (all workspaces):
```powershell
$dir = "$env:USERPROFILE\.copilot\hooks"
New-Item -ItemType Directory -Path $dir -Force
Set-Content "$dir\rtk-rewrite.json" '{
  "hooks": {
    "PreToolUse": [
      {
        "type": "command",
        "command": "rtk hook copilot",
        "cwd": ".",
        "timeout": 5
      }
    ]
  }
}'
```

**Project-scoped** (single workspace):
```powershell
New-Item -ItemType Directory -Path ".github\hooks" -Force
Set-Content ".github\hooks\rtk-rewrite.json" '{
  "hooks": {
    "PreToolUse": [
      {
        "type": "command",
        "command": "rtk hook copilot",
        "cwd": ".",
        "timeout": 5
      }
    ]
  }
}'
```

### 5. Restart VS Code

The hook activates on next session start.

## Verification

```powershell
# Test hook interception
$json = '{"tool_name":"runTerminalCommand","tool_input":{"command":"git status"}}'
$json | rtk hook copilot
# Expected: {"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","updatedInput":{"command":"rtk git status"}}}

# Test filter execution
rtk git status       # Compact git output
rtk read file.txt    # Smart file reading
rtk grep pattern .   # Grouped search results
rtk ls .             # Directory listing
rtk find "*.rs" .    # File search

# Check analytics
rtk gain             # Token savings dashboard
```

## Supported Commands on Windows

| Command | RTK Rewrite | Windows Status |
|---------|-------------|----------------|
| `git *` | `rtk git *` | Full support (git for Windows) |
| `cat file` | `rtk read file` | Full support (built-in) |
| `head -N file` | `rtk read file --max-lines N` | Full support (built-in) |
| `tail -N file` | `rtk read file --tail-lines N` | Full support (built-in) |
| `grep/rg pattern` | `rtk grep pattern` | Full support (requires ripgrep) |
| `ls` | `rtk ls` | Full support (via ls.cmd shim) |
| `find pattern .` | `rtk find pattern .` | Full support (built-in) |
| `docker *` | `rtk docker *` | Works if Docker installed |
| `kubectl *` | `rtk kubectl *` | Works if kubectl installed |
| `cargo *` | `rtk cargo *` | Works if Rust installed |
| `npm run *` | `rtk npm run *` | Works if Node installed |
| `pytest` | `rtk pytest` | Works if Python installed |
| `go test` | `rtk go test` | Works if Go installed |

## How It Differs From WSL Mode

| Aspect | WSL | Windows Native |
|--------|-----|----------------|
| Hook mechanism | bash shell hook (`rtk-rewrite.sh`) | PreToolUse binary (`rtk hook copilot`) |
| Applies to | Claude Code bash tool calls | VS Code Copilot Chat terminal commands |
| Binary | Linux ELF | Windows PE (MSVC) |
| Dependencies | jq, bash | None (self-contained binary) |
| Catch rate | 100% of bash calls | 100% of terminal commands |
| Token savings | 60-90% | 60-90% (identical filters) |

## Troubleshooting

### Hook not triggering
- Restart VS Code completely (not just reload window)
- Verify hook config location: `~/.copilot/hooks/rtk-rewrite.json` (global) or `.github/hooks/rtk-rewrite.json` (project)
- Ensure `rtk.exe` is in PATH: `Get-Command rtk`

### `rtk grep` fails
- Install ripgrep: `winget install BurntSushi.ripgrep.MSVC`
- Restart terminal to pick up PATH changes

### `rtk ls` fails
- Create shim: `Set-Content "$env:USERPROFILE\.local\bin\ls.cmd" '@echo off\ndir /b %*'`
- Ensure `~/.local/bin` is in PATH

### Commands pass through without rewrite
- The hook only rewrites recognized commands. PowerShell cmdlets (Get-ChildItem, etc.) are not rewritten.
- Use Unix-style commands (`cat`, `ls`, `grep`) which the hook knows about.
- Check: `'{"tool_name":"runTerminalCommand","tool_input":{"command":"YOUR_CMD"}}' | rtk hook copilot`

### Audit logging
```powershell
$env:RTK_HOOK_AUDIT = "1"
# Hook writes to ~/.local/share/rtk/hook-audit.log
```
