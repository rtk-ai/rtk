# rtk installer for Windows - https://github.com/rtk-ai/rtk
# Usage: irm https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.ps1 | iex

$ErrorActionPreference = "Stop"

$Repo = "rtk-ai/rtk"
$BinaryName = "rtk"
$InstallDir = if ($env:RTK_INSTALL_DIR) { $env:RTK_INSTALL_DIR } else { "$env:USERPROFILE\.local\bin" }

function Write-Info  { param($Msg) Write-Host "[INFO] $Msg" -ForegroundColor Green }
function Write-Warn  { param($Msg) Write-Host "[WARN] $Msg" -ForegroundColor Yellow }
function Write-Err   { param($Msg) Write-Host "[ERROR] $Msg" -ForegroundColor Red; exit 1 }

# Detect architecture
$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
switch ($Arch) {
    "X64"   { $Target = "x86_64-pc-windows-msvc" }
    "Arm64" { Write-Err "ARM64 Windows builds are not yet available. Use cargo install --git https://github.com/$Repo" }
    default { Write-Err "Unsupported architecture: $Arch" }
}

# Get latest release version
Write-Info "Fetching latest release..."
try {
    $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "rtk-installer" }
    $Version = $Release.tag_name
} catch {
    Write-Err "Failed to get latest version: $_"
}

if (-not $Version) {
    Write-Err "Failed to determine latest version"
}

Write-Info "Detected: Windows $Arch"
Write-Info "Target: $Target"
Write-Info "Version: $Version"

# Download
$DownloadUrl = "https://github.com/$Repo/releases/download/$Version/$BinaryName-$Target.zip"
$TempDir = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "rtk-install-$(Get-Random)")
$Archive = Join-Path $TempDir "$BinaryName.zip"

Write-Info "Downloading from: $DownloadUrl"
try {
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $Archive -UseBasicParsing
} catch {
    Write-Err "Failed to download binary: $_"
}

# Extract
Write-Info "Extracting..."
Expand-Archive -Path $Archive -DestinationPath $TempDir -Force

# Install
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$Source = Join-Path $TempDir "$BinaryName.exe"
$Destination = Join-Path $InstallDir "$BinaryName.exe"
Move-Item -Path $Source -Destination $Destination -Force

# Cleanup
Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue

Write-Info "Successfully installed $BinaryName to $Destination"

# Check PATH
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    Write-Warn "$InstallDir is not in your PATH."
    $AddToPath = Read-Host "Add to PATH? [Y/n]"
    if ($AddToPath -ne "n") {
        [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
        $env:Path = "$env:Path;$InstallDir"
        Write-Info "Added $InstallDir to user PATH. Restart your terminal for it to take effect."
    } else {
        Write-Warn "To add manually, run:"
        Write-Warn "  [Environment]::SetEnvironmentVariable('Path', `"`$env:Path;$InstallDir`", 'User')"
    }
}

# Verify
if (Get-Command $BinaryName -ErrorAction SilentlyContinue) {
    $VerOutput = & $BinaryName --version 2>&1
    Write-Info "Verification: $VerOutput"
} else {
    Write-Warn "Binary installed but not yet in PATH for this session. Restart your terminal, then run: $BinaryName --version"
}

Write-Host ""
Write-Info "Installation complete! Run '$BinaryName --help' to get started."
