$ErrorActionPreference = "Stop"

function Section($text) {
    Write-Host ""
    Write-Host $text
}

function HasCommand($name) {
    return [bool](Get-Command $name -ErrorAction SilentlyContinue)
}

Write-Host "==========================================================="
Write-Host "           RTK Installation Verification"
Write-Host "==========================================================="

$rtkCmd = Get-Command rtk -ErrorAction SilentlyContinue
if (-not $rtkCmd) {
    Write-Host ""
    Write-Host "1. RTK is not installed"
    Write-Host ""
    Write-Host "If you are in this repository, build it with:"
    Write-Host "  cargo build --release"
    Write-Host "  .\target\release\rtk.exe --version"
    exit 1
}

Section "1. RTK is installed"
Write-Host "Location: $($rtkCmd.Source)"

Section "2. RTK version"
$versionOutput = & rtk --version 2>$null
if (-not $versionOutput) {
    $versionOutput = "unknown"
}
Write-Host "Version: $versionOutput"

Section "3. Verify this is Rust Token Killer"
$gainWorked = $false
& rtk gain *> $null
if ($LASTEXITCODE -eq 0) {
    $gainWorked = $true
} else {
    & rtk gain --help *> $null
    if ($LASTEXITCODE -eq 0) {
        $gainWorked = $true
    }
}

if ($gainWorked) {
    Write-Host "OK: gain command is available"
} else {
    Write-Host "ERROR: this does not look like the RTK project that provides 'gain'"
    Write-Host "You may have installed a different crate named 'rtk'."
    exit 1
}

Section "4. Check important commands"
$helpOutput = & rtk --help 2>$null
$features = @(
    @{ Command = "gain"; Name = "Token savings analytics" },
    @{ Command = "git"; Name = "Git operations" },
    @{ Command = "gh"; Name = "GitHub CLI" },
    @{ Command = "pnpm"; Name = "pnpm support" },
    @{ Command = "vitest"; Name = "Vitest test runner" },
    @{ Command = "lint"; Name = "Lint support" },
    @{ Command = "tsc"; Name = "TypeScript compiler" },
    @{ Command = "next"; Name = "Next.js" },
    @{ Command = "prettier"; Name = "Prettier" },
    @{ Command = "playwright"; Name = "Playwright" },
    @{ Command = "prisma"; Name = "Prisma" },
    @{ Command = "discover"; Name = "Discover missed savings" }
)

foreach ($feature in $features) {
    if ($helpOutput -match "(?m)^\s+$($feature.Command)\b") {
        Write-Host "OK  $($feature.Name)"
    } else {
        Write-Host "WARN $($feature.Name) missing from --help"
    }
}

Section "5. Check project-local setup"
$localClaude = Join-Path (Get-Location) "CLAUDE.md"
if ((Test-Path $localClaude) -and (Select-String -Path $localClaude -Pattern "rtk" -Quiet)) {
    Write-Host "OK  Local CLAUDE.md mentions rtk"
} else {
    Write-Host "WARN Local CLAUDE.md does not mention rtk in this directory"
}

Section "6. Check global Claude files"
$claudeDir = Join-Path $HOME ".claude"
$globalClaude = Join-Path $claudeDir "CLAUDE.md"
$hookPath = Join-Path $claudeDir "hooks\rtk-rewrite.sh"
$settingsPath = Join-Path $claudeDir "settings.json"

if ((Test-Path $globalClaude) -and (Select-String -Path $globalClaude -Pattern "rtk|@RTK.md" -Quiet)) {
    Write-Host "OK  Global CLAUDE.md looks configured"
} else {
    Write-Host "WARN Global CLAUDE.md does not look configured"
}

if (Test-Path $hookPath) {
    Write-Host "WARN Hook file exists: $hookPath"
    if ((Test-Path $settingsPath) -and (Select-String -Path $settingsPath -Pattern "rtk-rewrite.sh" -Quiet)) {
        Write-Host "WARN settings.json references the Unix hook"
    } else {
        Write-Host "WARN Unix hook file exists but settings.json does not reference it"
    }
    Write-Host "INFO Hook-based setup is still Unix-first; see WINDOWS.md"
} else {
    Write-Host "INFO No hook file detected"
}

Section "Summary"
Write-Host "RTK is installed and the gain command is available."
Write-Host "For native Windows usage, prefer the binary itself over the Bash hook workflow."
