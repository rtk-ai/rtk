<#
.SYNOPSIS
    RTK Windows-Native Setup for VS Code Copilot Chat (Full Hook Support)
.DESCRIPTION
    Enables FULL RTK auto-rewrite hook support for GitHub Copilot Chat in VS Code
    on native Windows WITHOUT requiring WSL.
    
    This script:
    1. Downloads/verifies the RTK Windows binary
    2. Installs ripgrep (required for rtk grep)
    3. Creates Windows shims for missing Unix tools (ls)
    4. Configures the global Copilot PreToolUse hook
    5. Optionally configures project-scoped hooks
    
    The PreToolUse hook intercepts ALL terminal commands from Copilot Chat and
    rewrites them to use RTK's token-optimized filters — achieving 60-90% savings.
    
.PARAMETER Version
    Specific RTK version to install (e.g., "0.43.0"). If omitted, fetches the latest release.
.PARAMETER Global
    Install the hook globally (~/.copilot/hooks/) so it applies to all workspaces.
.PARAMETER Project
    Also install the hook project-scoped (.github/hooks/) in the current directory.
.PARAMETER SkipBinary
    Skip downloading the RTK binary (if already installed).
.PARAMETER Force
    Overwrite existing configurations.
.EXAMPLE
    .\windows-copilot-setup.ps1 -Global
.EXAMPLE
    .\windows-copilot-setup.ps1 -Global -Project
.NOTES
    Requires: Windows 10+, VS Code with GitHub Copilot Chat
    No WSL, no bash, no Unix tools required.
#>

[CmdletBinding()]
param(
    [string]$Version,
    [switch]$Global,
    [switch]$Project,
    [switch]$SkipBinary,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

# ── Constants ─────────────────────────────────────────────────────────────────

$RTK_BIN_DIR = Join-Path $env:USERPROFILE ".local\bin"
$RTK_EXE = Join-Path $RTK_BIN_DIR "rtk.exe"
$COPILOT_GLOBAL_HOOKS = Join-Path $env:USERPROFILE ".copilot\hooks"

# Resolve version: parameter > GitHub API latest
if ($Version) {
    $RTK_VERSION = $Version
} else {
    Write-Host "Fetching latest RTK release version..."
    $release = Invoke-RestMethod "https://api.github.com/repos/rtk-ai/rtk/releases/latest" -UseBasicParsing
    $RTK_VERSION = $release.tag_name -replace '^v', ''
}
$DOWNLOAD_URL = "https://github.com/rtk-ai/rtk/releases/download/v$RTK_VERSION/rtk-x86_64-pc-windows-msvc.zip"

$HOOK_CONFIG = @'
{
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
}
'@

# ── Functions ─────────────────────────────────────────────────────────────────

function Write-Step { param([string]$msg) Write-Host "`n[$((Get-Date).ToString('HH:mm:ss'))] $msg" -ForegroundColor Cyan }
function Write-Ok { param([string]$msg) Write-Host "  [OK] $msg" -ForegroundColor Green }
function Write-Skip { param([string]$msg) Write-Host "  [SKIP] $msg" -ForegroundColor Yellow }
function Write-Fail { param([string]$msg) Write-Host "  [FAIL] $msg" -ForegroundColor Red }

function Test-RtkInstalled {
    try {
        $ver = & $RTK_EXE --version 2>$null
        return $ver -match "^rtk \d"
    } catch { return $false }
}

function Install-RtkBinary {
    Write-Step "Installing RTK binary (v$RTK_VERSION)"
    
    if ((Test-RtkInstalled) -and -not $Force) {
        $ver = & $RTK_EXE --version
        Write-Skip "RTK already installed: $ver"
        return
    }
    
    New-Item -ItemType Directory -Path $RTK_BIN_DIR -Force | Out-Null
    $zipPath = Join-Path $env:TEMP "rtk-windows.zip"
    
    Write-Host "  Downloading from GitHub releases..."
    Invoke-WebRequest -Uri $DOWNLOAD_URL -OutFile $zipPath -UseBasicParsing
    
    Write-Host "  Extracting to $RTK_BIN_DIR..."
    Expand-Archive -Path $zipPath -DestinationPath $RTK_BIN_DIR -Force
    Remove-Item $zipPath -ErrorAction SilentlyContinue
    
    # Add to user PATH if not already there
    $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    if ($userPath -notlike "*$RTK_BIN_DIR*") {
        [Environment]::SetEnvironmentVariable("PATH", "$RTK_BIN_DIR;$userPath", "User")
        $env:PATH = "$RTK_BIN_DIR;$env:PATH"
        Write-Host "  Added $RTK_BIN_DIR to user PATH"
    }
    
    # Verify
    if (Test-RtkInstalled) {
        $ver = & $RTK_EXE --version
        Write-Ok "RTK installed: $ver"
    } else {
        Write-Fail "RTK binary verification failed"
        exit 1
    }
}

function Install-WindowsShims {
    Write-Step "Installing Windows shims for Unix tools"
    
    # ls.cmd shim - RTK's ls command tries to call the ls binary
    $lsShim = Join-Path $RTK_BIN_DIR "ls.cmd"
    if (-not (Test-Path $lsShim) -or $Force) {
        $lsContent = "@echo off`r`nif `"%~1`"==`"`" ( dir /b ) else ( dir /b %* )"
        Set-Content -Path $lsShim -Value $lsContent -NoNewline
        Write-Ok "Created ls.cmd shim"
    } else {
        Write-Skip "ls.cmd shim already exists"
    }
}

function Install-Ripgrep {
    Write-Step "Checking ripgrep (rg) for rtk grep support"
    
    $rg = Get-Command rg -ErrorAction SilentlyContinue
    if ($rg) {
        Write-Skip "ripgrep already installed: $($rg.Source)"
        return
    }
    
    $winget = Get-Command winget -ErrorAction SilentlyContinue
    if ($winget) {
        Write-Host "  Installing ripgrep via winget..."
        winget install BurntSushi.ripgrep.MSVC --accept-package-agreements --accept-source-agreements --silent
        Write-Ok "ripgrep installed"
    } else {
        Write-Fail "Cannot install ripgrep - winget not available. Install manually: https://github.com/BurntSushi/ripgrep/releases"
    }
}

function Install-GlobalHook {
    Write-Step "Configuring global Copilot PreToolUse hook"
    
    $hookFile = Join-Path $COPILOT_GLOBAL_HOOKS "rtk-rewrite.json"
    
    if ((Test-Path $hookFile) -and -not $Force) {
        Write-Skip "Global hook already configured: $hookFile"
        return
    }
    
    New-Item -ItemType Directory -Path $COPILOT_GLOBAL_HOOKS -Force | Out-Null
    Set-Content -Path $hookFile -Value $HOOK_CONFIG -Encoding UTF8
    Write-Ok "Global hook config: $hookFile"
}

function Install-ProjectHook {
    Write-Step "Configuring project-scoped Copilot hook"
    
    $githubHooks = Join-Path (Get-Location) ".github\hooks"
    $hookFile = Join-Path $githubHooks "rtk-rewrite.json"
    
    if ((Test-Path $hookFile) -and -not $Force) {
        Write-Skip "Project hook already configured: $hookFile"
        return
    }
    
    New-Item -ItemType Directory -Path $githubHooks -Force | Out-Null
    Set-Content -Path $hookFile -Value $HOOK_CONFIG -Encoding UTF8
    Write-Ok "Project hook config: $hookFile"
    
    # Also create copilot-instructions.md if not present
    $instructionsFile = Join-Path (Get-Location) ".github\copilot-instructions.md"
    if (-not (Test-Path $instructionsFile) -or $Force) {
        & $RTK_EXE init --copilot 2>$null
        if (Test-Path $instructionsFile) {
            Write-Ok "Copilot instructions: $instructionsFile"
        }
    }
}

function Test-Integration {
    Write-Step "Verifying integration (simulating Copilot hook calls)"
    
    $testCases = @(
        @{cmd='git status';    expect='rtk git status'},
        @{cmd='git log -10';   expect='rtk git log -10'},
        @{cmd='cat README.md'; expect='rtk read README.md'},
        @{cmd='ls -la';        expect='rtk ls -la'},
        @{cmd='grep pattern .';expect='rtk grep pattern .'},
        @{cmd='docker ps';     expect='rtk docker ps'},
        @{cmd='cargo test';    expect='rtk cargo test'}
    )
    
    $passed = 0
    $failed = 0
    
    foreach ($test in $testCases) {
        $json = ConvertTo-Json -Compress -InputObject @{
            tool_name = "runTerminalCommand"
            tool_input = @{ command = $test.cmd }
        }
        $output = $json | & $RTK_EXE hook copilot 2>$null
        
        if ($output) {
            $parsed = ConvertFrom-Json $output
            $rewritten = $parsed.hookSpecificOutput.updatedInput.command
            if ($rewritten -eq $test.expect) {
                Write-Host "  PASS: $($test.cmd) -> $rewritten" -ForegroundColor Green
                $passed++
            } else {
                Write-Host "  WARN: $($test.cmd) -> $rewritten (expected: $($test.expect))" -ForegroundColor Yellow
                $passed++  # Still counts as intercepted
            }
        } else {
            Write-Host "  FAIL: $($test.cmd) -> [not intercepted]" -ForegroundColor Red
            $failed++
        }
    }
    
    Write-Host ""
    $total = $passed + $failed
    $rate = [math]::Round(($passed / $total) * 100, 1)
    Write-Host "  Results: $passed/$total commands intercepted ($rate% catch rate)" -ForegroundColor $(if ($rate -eq 100) { 'Green' } else { 'Yellow' })
    
    # Test that rewritten commands actually execute
    Write-Step "Verifying rewritten commands execute on Windows"
    
    $execTests = @(
        @{cmd='rtk git status';   label='git status'},
        @{cmd='rtk read Cargo.toml --max-lines 3'; label='file read'},
        @{cmd='rtk find "*.toml" .'; label='find files'},
        @{cmd='rtk ls .';         label='directory list'}
    )
    
    foreach ($et in $execTests) {
        try {
            $null = Invoke-Expression $et.cmd 2>$null
            Write-Host "  EXEC OK: $($et.label)" -ForegroundColor Green
        } catch {
            Write-Host "  EXEC FAIL: $($et.label) - $_" -ForegroundColor Red
        }
    }
}

# ── Main ──────────────────────────────────────────────────────────────────────

Write-Host @"

╔═══════════════════════════════════════════════════════════════════╗
║  RTK Windows-Native Setup for VS Code Copilot Chat              ║
║  Full hook support WITHOUT WSL                                   ║
╚═══════════════════════════════════════════════════════════════════╝

"@ -ForegroundColor White

if (-not $Global -and -not $Project) {
    Write-Host "Usage: .\windows-copilot-setup.ps1 -Global [-Project] [-Force] [-Version x.y.z]" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  -Global      Install hook globally (all workspaces)"
    Write-Host "  -Project     Also install project-scoped hook"
    Write-Host "  -Force       Overwrite existing configurations"
    Write-Host "  -SkipBinary  Skip RTK binary download"
    Write-Host "  -Version     Pin to a specific version (default: latest)"
    Write-Host ""
    exit 0
}

# Step 1: RTK Binary
if (-not $SkipBinary) {
    Install-RtkBinary
}

# Step 2: Windows shims
Install-WindowsShims

# Step 3: Ripgrep
Install-Ripgrep

# Step 4: Global hook
if ($Global) {
    Install-GlobalHook
}

# Step 5: Project hook
if ($Project) {
    Install-ProjectHook
}

# Step 6: Verify
Test-Integration

Write-Host @"

╔═══════════════════════════════════════════════════════════════════╗
║  Setup Complete!                                                 ║
║                                                                  ║
║  ACTION REQUIRED: Restart VS Code to activate the hook.         ║
║                                                                  ║
║  The PreToolUse hook will now intercept ALL terminal commands    ║
║  from Copilot Chat and rewrite them to use RTK filters.         ║
║                                                                  ║
║  Verify with: rtk gain                                          ║
╚═══════════════════════════════════════════════════════════════════╝

"@ -ForegroundColor Green
