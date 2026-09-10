# Template. The release workflow replaces __URL64__, __SHA64__, __URLARM__ and
# __SHAARM__ with the values from the release's checksums.txt.
#
# The package ships this script rather than the binary so Chocolatey installs
# the same signed release archive everyone else downloads, verified by the
# checksum published alongside it.
$ErrorActionPreference = 'Stop'

$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition

$url = '__URL64__'
$checksum = '__SHA64__'

# Chocolatey has no first-class arm64 slot; on an ARM device prefer the native
# build and let the x64 build (under emulation) cover everything else.
if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64') {
  $url = '__URLARM__'
  $checksum = '__SHAARM__'
}

$packageArgs = @{
  PackageName    = 'rtk'
  UnzipLocation  = $toolsDir
  Url            = $url
  Checksum       = $checksum
  ChecksumType   = 'sha256'
  Url64bit       = $url
  Checksum64     = $checksum
  ChecksumType64 = 'sha256'
}

Install-ChocolateyZipPackage @packageArgs
