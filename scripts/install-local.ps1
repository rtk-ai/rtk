param(
    [string]$InstallDir = "$HOME\.cargo\bin"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$binaryPath = Join-Path $repoRoot "target\release\rtk.exe"
$installPath = Join-Path $InstallDir "rtk.exe"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "error: cargo not found"
    Write-Host "install Rust: https://rustup.rs"
    exit 1
}

Write-Host "installing to: $InstallDir"

$needsBuild = $true
if (Test-Path $binaryPath) {
    $binaryTime = (Get-Item $binaryPath).LastWriteTimeUtc
    $sources = @(
        (Join-Path $repoRoot "Cargo.toml"),
        (Join-Path $repoRoot "Cargo.lock")
    )

    $srcDir = Join-Path $repoRoot "src"
    if (Test-Path $srcDir) {
        $sources += Get-ChildItem $srcDir -Recurse -File | Select-Object -ExpandProperty FullName
    }

    $newerSource = $sources | Where-Object {
        (Test-Path $_) -and ((Get-Item $_).LastWriteTimeUtc -gt $binaryTime)
    } | Select-Object -First 1

    if (-not $newerSource) {
        $needsBuild = $false
    }
}

if ($needsBuild) {
    Write-Host "building rtk (release)..."
    Push-Location $repoRoot
    try {
        cargo build --release
    } finally {
        Pop-Location
    }
} else {
    Write-Host "binary is up to date"
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item $binaryPath $installPath -Force

Write-Host "installed: $installPath"
Write-Host "version: $(& $installPath --version)"

$pathEntries = ($env:Path -split ';') | Where-Object { $_ -ne "" }
if ($pathEntries -notcontains $InstallDir) {
    Write-Host ""
    Write-Host "warning: $InstallDir is not in your PATH"
    Write-Host "add that folder to your user PATH, then reopen PowerShell"
}
