<# 
.SYNOPSIS
    RTK (Rust Token Killer) - Windows Installer Script

.DESCRIPTION
    Downloads and installs the latest RTK release from GitHub.
    Supports both x64 and ARM64 architectures.
    Verifies SHA256 checksum for security.
    Installs to %LOCALAPPDATA%\rtk\bin and adds to User PATH.

.NOTES
    Author: RTK Team
    Version: 1.0.0
    Repository: https://github.com/rtk-ai/rtk
#>

param(
    [switch]$Force,
    [switch]$NoPath,
    [string]$Version = "latest",
    [string]$InstallDir = "$env:LOCALAPPDATA\rtk\bin"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Colors for output
$Red = [ConsoleColor]::Red
$Green = [ConsoleColor]::Green
$Yellow = [ConsoleColor]::Yellow
$Cyan = [ConsoleColor]::Cyan
$Default = [ConsoleColor]::Gray

function Write-Color([string]$Message, [ConsoleColor]$Color = $Default) {
    Write-Host $Message -ForegroundColor $Color
}

function Write-ErrorColor([string]$Message) {
    Write-Color "[ERROR] $Message" $Red
}

function Write-Success([string]$Message) {
    Write-Color "[OK] $Message" $Green
}

function Write-WarningColor([string]$Message) {
    Write-Color "[WARN] $Message" $Yellow
}

function Write-Info([string]$Message) {
    Write-Color "[INFO] $Message" $Cyan
}

# Detect architecture
function Get-Architecture {
    $arch = [Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITECTURE")
    if ($arch -eq "AMD64") { return "x86_64" }
    if ($arch -eq "ARM64") { return "aarch64" }
    if ($arch -eq "x86") { return "x86_64" } # 32-bit on 64-bit Windows
    return "x86_64" # Default fallback
}

# Get latest release info from GitHub API
function Get-LatestRelease {
    $apiUrl = "https://api.github.com/repos/rtk-ai/rtk/releases/latest"
    Write-Info "Fetching latest release info from GitHub..."
    
    try {
        $response = Invoke-RestMethod -Uri $apiUrl -Headers @{ "Accept" = "application/vnd.github.v3+json" }
        return $response
    } catch {
        Write-ErrorColor "Failed to fetch release info: $_"
        throw
    }
}

# Get specific release info
function Get-ReleaseVersion([string]$version) {
    if ($version -eq "latest") {
        return Get-LatestRelease
    }
    
    $apiUrl = "https://api.github.com/repos/rtk-ai/rtk/releases/tags/v$version"
    Write-Info "Fetching release v$version from GitHub..."
    
    try {
        $response = Invoke-RestMethod -Uri $apiUrl -Headers @{ "Accept" = "application/vnd.github.v3+json" }
        return $response
    } catch {
        Write-ErrorColor "Failed to fetch release v${version}: $_"
        throw
    }
}

# Find the correct asset for Windows
function Find-WindowsAsset([object]$release, [string]$arch) {
    $assetName = "rtk-${arch}-pc-windows-msvc.zip"
    $asset = $release.assets | Where-Object { $_.name -eq $assetName }
    
    if (-not $asset) {
        # Try alternative naming
        $asset = $release.assets | Where-Object { $_.name -like "rtk-${arch}-pc-windows-msvc*" }
    }
    
    if (-not $asset) {
        Write-ErrorColor "No matching asset found for architecture: $arch"
        Write-Info "Available assets:"
        $release.assets | ForEach-Object { Write-Info "  - $($_.name)" }
        throw "No compatible asset found"
    }
    
    return $asset
}

# Download file with progress
function Download-File([string]$Url, [string]$OutputPath) {
    Write-Info "Downloading from $Url ..."
    try {
        $wc = New-Object System.Net.WebClient
        $wc.DownloadFile($Url, $OutputPath)
        Write-Success "Downloaded to $OutputPath"
    } catch {
        Write-ErrorColor "Download failed: $_"
        throw
    }
}

# Verify SHA256 checksum
function Verify-Checksum([string]$FilePath, [string]$ExpectedHash) {
    Write-Info "Verifying SHA256 checksum..."
    $sha256 = Get-FileHash -Path $FilePath -Algorithm SHA256
    $actualHash = $sha256.Hash.ToLower()
    $expectedHash = $ExpectedHash.ToLower()
    
    if ($actualHash -ne $expectedHash) {
        Write-ErrorColor "Checksum mismatch!"
        Write-ErrorColor "Expected: $expectedHash"
        Write-ErrorColor "Actual:   $actualHash"
        throw "Checksum verification failed"
    }
    
    Write-Success "Checksum verified"
}

# Extract zip file
function Extract-Zip([string]$ZipPath, [string]$Destination) {
    Write-Info "Extracting to $Destination ..."
    try {
        Expand-Archive -Path $ZipPath -DestinationPath $Destination -Force
        Write-Success "Extracted successfully"
    } catch {
        Write-ErrorColor "Extraction failed: $_"
        throw
    }
}

# Add to User PATH
function Add-ToPath([string]$PathToAdd) {
    $currentPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    if ($currentPath -like "*$PathToAdd*") {
        Write-Info "Path already in User PATH"
        return
    }
    
    if ([string]::IsNullOrEmpty($currentPath)) {
        $newPath = $PathToAdd
    } else {
        $newPath = "$currentPath;$PathToAdd"
    }
    
    [Environment]::SetEnvironmentVariable("PATH", $newPath, "User")
    Write-Success "Added $PathToAdd to User PATH"
    Write-WarningColor "Note: You need to restart your terminal/shell for PATH changes to take effect."
}

# Check if running as admin (not required but good to know)
function Check-Admin {
    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# Main installation flow
function Install-RTK {
    Write-Color "==============================================================" $Cyan
    Write-Color "                    RTK Windows Installer                     " $Cyan
    Write-Color "             Rust Token Killer - Token Optimizer              " $Cyan
    Write-Color "==============================================================" $Cyan
    Write-Host ""
    
    $arch = Get-Architecture
    Write-Info "Detected architecture: $arch"
    
    if (Check-Admin) {
        Write-WarningColor "Running as Administrator - installing to User PATH (not System PATH)"
    }
    
    # Get release info
    $release = Get-ReleaseVersion -version $Version
    Write-Success "Found release: $($release.tag_name) - $($release.name)"
    
    # Find asset
    $asset = Find-WindowsAsset -release $release -arch $arch
    Write-Success "Found asset: $($asset.name) ($([math]::Round($asset.size / 1MB, 2)) MB)"
    
    # Create temp directory
    $tempDir = [System.IO.Path]::GetTempPath()
    $zipPath = Join-Path $tempDir "rtk-$arch-windows.zip"
    
    # Download
    Download-File -Url $asset.browser_download_url -OutputPath $zipPath
    
    # Verify checksum if available
    $checksumAsset = $release.assets | Where-Object { $_.name -eq "SHA256SUMS" -or $_.name -eq "sha256sums.txt" -or $_.name -like "*checksum*" }
    if ($checksumAsset) {
        $checksumUrl = $checksumAsset.browser_download_url
        $checksumPath = Join-Path $tempDir "SHA256SUMS"
        Download-File -Url $checksumUrl -OutputPath $checksumPath
        
        $checksumContent = Get-Content $checksumPath -Raw
        $expectedHash = ($checksumContent -split "`n" | Where-Object { $_ -like "*$($asset.name)*" } -split '\s+')[0]
        if ($expectedHash) {
            Verify-Checksum -FilePath $zipPath -ExpectedHash $expectedHash
        } else {
            Write-WarningColor "Checksum for $($asset.name) not found in checksums file, skipping verification"
        }
    } else {
        Write-WarningColor "No checksum file found in release, skipping verification"
    }
    
    # Create install directory
    if (-not (Test-Path $InstallDir)) {
        Write-Info "Creating install directory: $InstallDir"
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }
    
    # Extract
    Extract-Zip -ZipPath $zipPath -Destination $InstallDir
    
    # Verify installation
    $exePath = Join-Path $InstallDir "rtk.exe"
    if (Test-Path $exePath) {
        Write-Success "RTK installed to $exePath"
        
        # Test the binary
        try {
            $versionOutput = & $exePath --version
            Write-Success "RTK version: $versionOutput"
        } catch {
            Write-WarningColor "Could not verify RTK binary: $_"
        }
    } else {
        Write-ErrorColor "RTK binary not found after extraction!"
        throw "Installation failed"
    }
    
    # Add to PATH
    if (-not $NoPath) {
        Add-ToPath -PathToAdd $InstallDir
    } else {
        Write-WarningColor "Skipping PATH modification (--NoPath specified)"
        Write-Info "Add $InstallDir to your PATH manually to use 'rtk' from anywhere"
    }
    
    # Cleanup
    if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
    $checksumPath = Join-Path $tempDir "SHA256SUMS"
    if (Test-Path $checksumPath) { Remove-Item $checksumPath -Force }
    
    Write-Host ""
    Write-Color "==============================================================" $Cyan
    Write-Color "                    RTK Windows Installer                      " $Cyan
    Write-Color "             Rust Token Killer - Token Optimizer               " $Cyan
    Write-Color "==============================================================" $Cyan
    Write-Host ""
    Write-Info "Next steps:"
    Write-Info "  1. Restart your terminal/PowerShell"
    Write-Info "  2. Run 'rtk --help' to get started"
    Write-Info "  3. Run 'rtk init -g' to set up hooks for your AI coding agents"
    Write-Host ""
    Write-Info "Documentation: https://github.com/rtk-ai/rtk"
    Write-Info "Issues: https://github.com/rtk-ai/rtk/issues"
}

# Run installer
try {
    Install-RTK
} catch {
    Write-ErrorColor "Installation failed: $_"
    exit 1
}