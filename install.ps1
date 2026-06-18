# rtk installer for Windows - https://github.com/rtk-ai/rtk
# Usage:
#   irm https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.ps1 | iex
#
# Installs the native rtk.exe to %USERPROFILE%\.local\bin so the Claude Code
# `rtk hook claude` PreToolUse hook works exactly like macOS/Linux: commands are
# rewritten automatically, no WSL required.
#
# Optional environment variables:
#   $env:RTK_VERSION      Pin a specific release, e.g. "v0.42.4"
#   $env:RTK_INSTALL_DIR  Override install directory (default: ~/.local/bin)

$ErrorActionPreference = 'Stop'

$Repo        = 'rtk-ai/rtk'
$BinaryName  = 'rtk.exe'
$Target      = 'x86_64-pc-windows-msvc'
$InstallDir  = if ($env:RTK_INSTALL_DIR) { $env:RTK_INSTALL_DIR } else { Join-Path $HOME '.local\bin' }

function Write-Info  { param($m) Write-Host "[INFO] $m"  -ForegroundColor Green }
function Write-Warn  { param($m) Write-Host "[WARN] $m"  -ForegroundColor Yellow }
function Write-Err   { param($m) Write-Host "[ERROR] $m" -ForegroundColor Red; exit 1 }

function Get-LatestVersion {
    # Resolve the latest tag via the /releases/latest redirect (no API rate limit).
    try {
        $resp = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" `
            -MaximumRedirection 0 -ErrorAction SilentlyContinue
    } catch {
        $resp = $_.Exception.Response
    }
    $location = $null
    if ($resp -and $resp.Headers) {
        $location = $resp.Headers['Location']
        if ($location -is [array]) { $location = $location[0] }
    }
    if ($location -and $location -match '/tag/([^/\s]+)') {
        return $Matches[1]
    }
    # Fallback to the REST API.
    Write-Warn 'Redirect lookup failed, falling back to GitHub API...'
    $api = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    if ($api.tag_name) { return $api.tag_name }
    Write-Err 'Failed to get latest version (set $env:RTK_VERSION=vX.Y.Z to pin)'
}

function Install-Rtk {
    if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64' -and $env:PROCESSOR_ARCHITEW6432 -ne 'AMD64') {
        Write-Warn "Detected non-AMD64 architecture ($env:PROCESSOR_ARCHITECTURE). Only x86_64 Windows builds are published; continuing anyway."
    }

    $version = if ($env:RTK_VERSION) {
        Write-Info "Using pinned version: $env:RTK_VERSION"
        $env:RTK_VERSION
    } else {
        Get-LatestVersion
    }

    Write-Info "Target:  $Target"
    Write-Info "Version: $version"

    $downloadUrl = "https://github.com/$Repo/releases/download/$version/rtk-$Target.zip"
    $tempDir     = Join-Path ([System.IO.Path]::GetTempPath()) "rtk-install-$([System.IO.Path]::GetRandomFileName())"
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
    $archive = Join-Path $tempDir 'rtk.zip'

    Write-Info "Downloading from: $downloadUrl"
    try {
        Invoke-WebRequest -Uri $downloadUrl -OutFile $archive
    } catch {
        Write-Err "Failed to download binary: $_"
    }

    Write-Info 'Extracting...'
    Expand-Archive -Path $archive -DestinationPath $tempDir -Force

    $extracted = Join-Path $tempDir $BinaryName
    if (-not (Test-Path $extracted)) {
        Write-Err "Archive did not contain $BinaryName"
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item -Path $extracted -Destination (Join-Path $InstallDir $BinaryName) -Force

    Remove-Item -Recurse -Force $tempDir

    Write-Info "Successfully installed $BinaryName to $InstallDir\$BinaryName"
}

function Test-PathEntry {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $normalized = $InstallDir.TrimEnd('\')
    if ($userPath -and ($userPath -split ';' | ForEach-Object { $_.TrimEnd('\') }) -contains $normalized) {
        return $true
    }
    return $false
}

function Confirm-Installation {
    $exe = Join-Path $InstallDir $BinaryName
    if (Test-Path $exe) {
        $ver = & $exe --version
        Write-Info "Verification: $ver"
    } else {
        Write-Warn 'Binary installed but not found at expected path.'
        return
    }

    if (-not (Test-PathEntry)) {
        Write-Warn "$InstallDir is not on your user PATH."
        Write-Warn 'Add it (new terminals will pick it up):'
        Write-Warn "  [Environment]::SetEnvironmentVariable('Path', `"`$([Environment]::GetEnvironmentVariable('Path','User'));$InstallDir`", 'User')"
    }
}

Write-Info "Installing rtk for Windows..."
Install-Rtk
Confirm-Installation

Write-Host ''
Write-Info "Installation complete. Next steps:"
Write-Host '  1. Open a NEW terminal (so PATH refreshes), then verify:  rtk --version'
Write-Host '  2. Register the Claude Code hook (full auto-rewrite):      rtk init -g'
Write-Host '  3. Restart Claude Code, then test:                         git status'
Write-Host ''
Write-Info 'Native Windows now has the same auto-rewrite hook as macOS/Linux. WSL is no longer required.'
