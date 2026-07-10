$ErrorActionPreference = "Stop"

[string[]] $CargoArgs = $args

function Fail-BeforeCargo {
    param([string] $Message)

    Write-Error "windows-cargo: $Message"
    exit 1
}

function Find-FileInPathList {
    param(
        [string] $PathList,
        [string] $FileName
    )

    if ([string]::IsNullOrWhiteSpace($PathList)) {
        return $null
    }

    foreach ($entry in $PathList -split ";") {
        if ([string]::IsNullOrWhiteSpace($entry)) {
            continue
        }

        $candidate = Join-Path -Path $entry -ChildPath $FileName
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }

    return $null
}

if ($CargoArgs.Count -eq 0) {
    Fail-BeforeCargo "missing Cargo arguments"
}

$programFilesX86 = ${env:ProgramFiles(x86)}
if ([string]::IsNullOrWhiteSpace($programFilesX86)) {
    Fail-BeforeCargo "ProgramFiles(x86) is not set"
}

$vswhere = Join-Path -Path $programFilesX86 -ChildPath "Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
    Fail-BeforeCargo "vswhere.exe not found at $vswhere"
}

$installPath = & $vswhere `
    -latest `
    -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath

if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($installPath)) {
    Fail-BeforeCargo "Visual Studio installation with VC.Tools.x86.x64 not found"
}

$installPath = ($installPath | Select-Object -First 1).Trim()
$devShell = Join-Path -Path $installPath -ChildPath "Common7\Tools\Launch-VsDevShell.ps1"
if (-not (Test-Path -LiteralPath $devShell -PathType Leaf)) {
    Fail-BeforeCargo "Launch-VsDevShell.ps1 not found at $devShell"
}

& $devShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Null
if ($LASTEXITCODE -ne 0) {
    Fail-BeforeCargo "Launch-VsDevShell.ps1 failed with exit code $LASTEXITCODE"
}

$cl = Get-Command cl.exe -ErrorAction SilentlyContinue
if ($null -eq $cl) {
    Fail-BeforeCargo "cl.exe is not available after launching VS dev shell"
}

$link = Get-Command link.exe -ErrorAction SilentlyContinue
if ($null -eq $link) {
    Fail-BeforeCargo "link.exe is not available after launching VS dev shell"
}

$vcruntime = Find-FileInPathList -PathList $env:INCLUDE -FileName "vcruntime.h"
if ($null -eq $vcruntime) {
    Fail-BeforeCargo "vcruntime.h not found in INCLUDE"
}

$stdarg = Find-FileInPathList -PathList $env:INCLUDE -FileName "stdarg.h"
if ($null -eq $stdarg) {
    Fail-BeforeCargo "stdarg.h not found in INCLUDE"
}

$msvcrt = Find-FileInPathList -PathList $env:LIB -FileName "msvcrt.lib"
if ($null -eq $msvcrt) {
    Fail-BeforeCargo "msvcrt.lib not found in LIB"
}

& cargo @CargoArgs
exit $LASTEXITCODE
