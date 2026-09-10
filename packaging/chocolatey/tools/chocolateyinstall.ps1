$ErrorActionPreference = 'Stop'

$version = $env:ChocolateyPackageVersion
$url = "https://github.com/rtk-ai/rtk/releases/download/v$version/rtk-x86_64-pc-windows-msvc.zip"

# SHA256 of rtk-x86_64-pc-windows-msvc.zip for this release.
# Replaced automatically by .github/workflows/release.yml before `choco pack`.
$checksum = 'REPLACE_AT_RELEASE'

if ($checksum -eq 'REPLACE_AT_RELEASE') {
    throw 'Package checksum not set. Use a release-built .nupkg or run the Chocolatey release CI job.'
}

$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition

$packageArgs = @{
    packageName   = $env:ChocolateyPackageName
    unzipLocation = $toolsDir
    fileType      = 'ZIP'
    url           = $url
    checksum      = $checksum
    checksumType  = 'sha256'
}

Install-ChocolateyZipPackage @packageArgs
