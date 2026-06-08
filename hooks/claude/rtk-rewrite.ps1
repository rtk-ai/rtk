# rtk-hook-version: 3
# RTK Codex / Claude Code hook — PowerShell rewrite script for Windows
#
# This is a thin delegating hook: all rewrite logic lives in `rtk rewrite`,
# which is the single source of truth (src/discover/registry.rs).
#
# Exit code protocol for `rtk rewrite`:
#   0 + stdout  Rewrite found, no deny/ask rule matched -> auto-allow
#   1           No RTK equivalent -> pass through unchanged
#   2           Deny rule matched -> pass through (let host handle natively)
#   3 + stdout  Ask rule matched -> rewrite but let host prompt the user

param()

$ErrorActionPreference = "Stop"

# Check rtk availability
$rtk = Get-Command rtk -ErrorAction SilentlyContinue
if (-not $rtk) {
    Write-Warning "[rtk] rtk is not installed or not in PATH. Hook cannot rewrite commands."
    Write-Warning "[rtk] Install: https://github.com/rtk-ai/rtk#installation"
    exit 0
}

# Version guard: rtk rewrite was added in 0.23.0
$cacheDir = if ($env:XDG_CACHE_HOME) { $env:XDG_CACHE_HOME } else { Join-Path $env:LOCALAPPDATA "rtk" }
$cacheFile = Join-Path $cacheDir "hook-version-ok"
if (-not (Test-Path $cacheFile)) {
    try {
        $versionOutput = & rtk --version 2>$null
        if ($versionOutput -match 'rtk\s+(\d+)\.(\d+)') {
            $major = [int]$Matches[1]
            $minor = [int]$Matches[2]
            if ($major -eq 0 -and $minor -lt 23) {
                Write-Warning "[rtk] rtk $($Matches[0]) is too old (need >= 0.23.0). Upgrade: cargo install rtk"
                exit 0
            }
        }
        New-Item -ItemType Directory -Force -Path $cacheDir | Out-Null
        New-Item -ItemType File -Force -Path $cacheFile | Out-Null
    } catch {
        # Cache failure is non-fatal
    }
}

# Read stdin (Claude Code / Codex sends JSON payload)
$rawInput = $input | Out-String
if ([string]::IsNullOrWhiteSpace($rawInput)) {
    exit 0
}

# Parse JSON payload
try {
    $payload = $rawInput | ConvertFrom-Json
} catch {
    Write-Warning "[rtk hook] Failed to parse JSON input: $_"
    exit 0
}

# Extract command
$cmd = $payload.tool_input.command
if ([string]::IsNullOrWhiteSpace($cmd)) {
    exit 0
}

# Delegate to rtk rewrite
try {
    $rewritten = & rtk rewrite $cmd 2>$null
    $exitCode = $LASTEXITCODE
} catch {
    $exitCode = 1
}

switch ($exitCode) {
    0 {
        # Rewrite found, no deny/ask rules -> auto-allow
        if ($cmd -eq $rewritten) {
            exit 0
        }
        # Build hook response
        $response = @{
            hookSpecificOutput = @{
                hookEventName = "PreToolUse"
                permissionDecision = "allow"
                permissionDecisionReason = "RTK auto-rewrite"
                updatedInput = @{
                    command = $rewritten
                }
            }
        }
        $response | ConvertTo-Json -Compress
        exit 0
    }
    1 {
        # No RTK equivalent -> pass through
        exit 0
    }
    2 {
        # Deny rule -> let host handle
        exit 0
    }
    3 {
        # Ask rule -> rewrite but omit permissionDecision
        $response = @{
            hookSpecificOutput = @{
                hookEventName = "PreToolUse"
                updatedInput = @{
                    command = $rewritten
                }
            }
        }
        $response | ConvertTo-Json -Compress
        exit 0
    }
    default {
        exit 0
    }
}
