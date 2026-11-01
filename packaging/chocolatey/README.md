# Chocolatey package (`rtk`)

Windows install via [Chocolatey Community](https://community.chocolatey.org/):

```powershell
choco install rtk
```

Adds `rtk.exe` to PATH (Chocolatey shim). Binary is downloaded from [GitHub Releases](https://github.com/rtk-ai/rtk/releases) (`rtk-x86_64-pc-windows-msvc.zip`).

## Layout

```
packaging/chocolatey/
├── rtk.nuspec
├── tools/
│   ├── chocolateyinstall.ps1
│   └── chocolateyUninstall.ps1
└── README.md
```

## Manual pack (maintainers)

From a machine with [Chocolatey CLI](https://chocolatey.org/install):

```powershell
# 1. Pick release tag (e.g. v0.42.4)
$version = '0.42.4'
$tag = "v$version"

# 2. SHA256 from release checksums.txt (line for rtk-x86_64-pc-windows-msvc.zip)
$checksum = '<sha256-from-checksums.txt>'

# 3. Patch install script
(Get-Content packaging/chocolatey/tools/chocolateyinstall.ps1 -Raw) `
  -replace 'REPLACE_AT_RELEASE', $checksum |
  Set-Content packaging/chocolatey/tools/chocolateyinstall.ps1 -NoNewline

# 4. Pack
choco pack packaging/chocolatey/rtk.nuspec --version=$version

# 5. Push (first time: create account + moderation on community.chocolatey.org)
choco push rtk.$version.nupkg --source https://push.chocolatey.org/ --api-key CHOCOLATEY_API_KEY

# 6. Restore placeholder for CI (optional)
git checkout packaging/chocolatey/tools/chocolateyinstall.ps1
```

## CI

Stable releases (`release.yml`, `prerelease: false`) pack and push when `CHOCOLATEY_API_KEY` is set in repo secrets.

Chocolatey package id: **`rtk`** (`choco install rtk`).
