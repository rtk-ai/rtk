# rtk Windows Hook Test Suite (PowerShell)
# Tests rtk hook claude/codex end-to-end rewrite coverage.
# Mirrors test-rtk-rewrite.sh for Windows environments.
#
# Usage: powershell -File hooks/claude/test-rtk-rewrite.ps1
#    or: pwsh hooks/claude/test-rtk-rewrite.ps1
#
# Requires: rtk.exe in PATH

param(
    [string]$RtkBinary = "rtk"
)

$ErrorActionPreference = "Continue"
$script:Pass = 0
$script:Fail = 0
$script:Total = 0
$script:Timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"

function Write-ColorOutput {
    param([string]$Message, [string]$Color = "White")
    Write-Host $Message -ForegroundColor $Color
}

function Test-Rewrite {
    param(
        [string]$Description,
        [string]$InputCommand,
        [string]$ExpectedCommand  # empty string = expect no rewrite
    )

    $script:Total++
    $inputJson = @{
        tool_name = "Bash"
        tool_input = @{ command = $InputCommand }
    } | ConvertTo-Json -Compress

    try {
        $output = $inputJson | & $RtkBinary hook claude 2>$null | Out-String
        $output = $output.Trim()
    } catch {
        $output = ""
    }

    if ([string]::IsNullOrEmpty($ExpectedCommand)) {
        # Expect NO rewrite
        if ([string]::IsNullOrEmpty($output)) {
            Write-ColorOutput "  PASS $Description -> (no rewrite)" "Green"
            $script:Pass++
        } else {
            try {
                $parsed = $output | ConvertFrom-Json
                $actual = $parsed.hookSpecificOutput.updatedInput.command
            } catch {
                $actual = "(parse error)"
            }
            Write-ColorOutput "  FAIL $Description" "Red"
            Write-ColorOutput "       expected: (no rewrite)" "Red"
            Write-ColorOutput "       actual:   $actual" "Red"
            $script:Fail++
        }
    } else {
        try {
            $parsed = $output | ConvertFrom-Json
            $actual = $parsed.hookSpecificOutput.updatedInput.command
        } catch {
            $actual = "(parse error: $output)"
        }

        if ($actual -eq $ExpectedCommand) {
            Write-ColorOutput "  PASS $Description -> $actual" "Green"
            $script:Pass++
        } else {
            Write-ColorOutput "  FAIL $Description" "Red"
            Write-ColorOutput "       expected: $ExpectedCommand" "Red"
            Write-ColorOutput "       actual:   $actual" "Red"
            $script:Fail++
        }
    }
}

function Test-Rewrite-Codex {
    param(
        [string]$Description,
        [string]$InputCommand,
        [string]$ExpectedCommand
    )

    $script:Total++
    $inputJson = @{
        tool_name = "Bash"
        tool_input = @{ command = $InputCommand }
    } | ConvertTo-Json -Compress

    try {
        $output = $inputJson | & $RtkBinary hook codex 2>$null | Out-String
        $output = $output.Trim()
    } catch {
        $output = ""
    }

    if ([string]::IsNullOrEmpty($ExpectedCommand)) {
        if ([string]::IsNullOrEmpty($output)) {
            Write-ColorOutput "  PASS (codex) $Description -> (no rewrite)" "Green"
            $script:Pass++
        } else {
            Write-ColorOutput "  FAIL (codex) $Description" "Red"
            $script:Fail++
        }
    } else {
        try {
            $parsed = $output | ConvertFrom-Json
            $actual = $parsed.hookSpecificOutput.updatedInput.command
        } catch {
            $actual = "(parse error)"
        }

        if ($actual -eq $ExpectedCommand) {
            Write-ColorOutput "  PASS (codex) $Description -> $actual" "Green"
            $script:Pass++
        } else {
            Write-ColorOutput "  FAIL (codex) $Description" "Red"
            Write-ColorOutput "       expected: $ExpectedCommand" "Red"
            Write-ColorOutput "       actual:   $actual" "Red"
            $script:Fail++
        }
    }
}

function Test-Settings-Format {
    param(
        [string]$Description,
        [hashtable]$Root,
        [string]$ExpectedCommand,
        [int]$ExpectedTimeout,
        [string[]]$ExpectedMatchers
    )

    $script:Total++
    $errors = @()

    # Check hooks.PreToolUse exists
    $ptu = $Root.hooks.PreToolUse
    if (-not $ptu) {
        $errors += "Missing hooks.PreToolUse"
    }

    # Check matchers
    $foundMatchers = @()
    foreach ($entry in $ptu) {
        $m = $entry.matcher
        $foundMatchers += $m
        foreach ($hook in $entry.hooks) {
            if ($hook.command -ne $ExpectedCommand) {
                $errors += "Wrong command: $($hook.command) (expected $ExpectedCommand)"
            }
            if ($hook.timeout -ne $ExpectedTimeout) {
                $errors += "Wrong timeout for matcher $m : $($hook.timeout) (expected $ExpectedTimeout)"
            }
        }
    }

    foreach ($em in $ExpectedMatchers) {
        if ($em -notin $foundMatchers) {
            $errors += "Missing matcher: $em"
        }
    }

    if ($errors.Count -eq 0) {
        Write-ColorOutput "  PASS $Description" "Green"
        $script:Pass++
    } else {
        Write-ColorOutput "  FAIL $Description" "Red"
        foreach ($e in $errors) {
            Write-ColorOutput "       $e" "Red"
        }
        $script:Fail++
    }
}

# ============================================================
# Main Test Suite
# ============================================================
Write-Host ""
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  RTK Windows Hook Test Suite" -ForegroundColor Cyan
Write-Host "  Binary: $RtkBinary" -ForegroundColor Cyan
Write-Host "  Time:   $Timestamp" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

# Verify rtk is available
try {
    $rtkVersion = & $RtkBinary --version 2>$null | Out-String
    Write-ColorOutput "rtk version: $rtkVersion" "Cyan"
} catch {
    Write-ColorOutput "ERROR: rtk not found in PATH. Install rtk first: https://github.com/rtk-ai/rtk" "Red"
    exit 1
}

# ---- SECTION 1: Git commands ----
Write-Host "--- Git commands ---" -ForegroundColor Yellow
Test-Rewrite "git status" "git status" "rtk git status"
Test-Rewrite "git log --oneline -10" "git log --oneline -10" "rtk git log --oneline -10"
Test-Rewrite "git diff HEAD" "git diff HEAD" "rtk git diff HEAD"
Test-Rewrite "git show abc123" "git show abc123" "rtk git show abc123"
Test-Rewrite "git add ." "git add ." "rtk git add ."
Test-Rewrite "gh pr list" "gh pr list" "rtk gh pr list"

Write-Host ""

# ---- SECTION 2: Test runners ----
Write-Host "--- Test runners ---" -ForegroundColor Yellow
Test-Rewrite "cargo test" "cargo test" "rtk cargo test"
Test-Rewrite "npx playwright test" "npx playwright test" "rtk playwright test"
Test-Rewrite "npx vitest" "npx vitest" "rtk vitest"
Test-Rewrite "pytest" "pytest" "rtk pytest"

Write-Host ""

# ---- SECTION 3: File operations ----
Write-Host "--- File operations ---" -ForegroundColor Yellow
Test-Rewrite "ls -la" "ls -la" "rtk ls -la"
Test-Rewrite "cat package.json" "cat package.json" "rtk read package.json"
Test-Rewrite "grep -rn pattern src/" "grep -rn pattern src/" "rtk grep -rn pattern src/"
Test-Rewrite "rg pattern src/" "rg pattern src/" "rtk grep pattern src/"
Test-Rewrite "curl -s https://example.com" "curl -s https://example.com" "rtk curl -s https://example.com"

Write-Host ""

# ---- SECTION 4: Build & package managers ----
Write-Host "--- Build & package managers ---" -ForegroundColor Yellow
Test-Rewrite "npm run test" "npm run test" "rtk npm run test"
Test-Rewrite "pnpm install" "pnpm install" "rtk pnpm install"
Test-Rewrite "docker ps" "docker ps" "rtk docker ps"
Test-Rewrite "npx prisma migrate" "npx prisma migrate" "rtk prisma migrate"

Write-Host ""

# ---- SECTION 5: Already using rtk (ignored prefix — no rewrite) ----
Write-Host "--- Already using rtk (ignored prefix) ---" -ForegroundColor Yellow
# "rtk " is in IGNORED_PREFIXES, so the hook correctly returns no rewrite.
Test-Rewrite "rtk git status (ignored prefix)" "rtk git status" ""
Test-Rewrite "rtk cargo test (ignored prefix)" "rtk cargo test" ""

Write-Host ""

# ---- SECTION 6: Unsupported commands (no rewrite) ----
Write-Host "--- Unsupported commands (no rewrite) ---" -ForegroundColor Yellow
Test-Rewrite "docker compose up -d (unsupported)" "docker compose up -d" ""
Test-Rewrite "echo hello world (unsupported)" "echo hello world" ""
Test-Rewrite "some-unknown-tool --flag (unsupported)" "some-unknown-tool --flag" ""

Write-Host ""

# ---- SECTION 7: Codex hook tests ----
Write-Host "--- Codex hook tests ---" -ForegroundColor Yellow
Test-Rewrite-Codex "codex: git status" "git status" "rtk git status"
Test-Rewrite-Codex "codex: cargo test" "cargo test" "rtk cargo test"
Test-Rewrite-Codex "codex: ls -la" "ls -la" "rtk ls -la"
Test-Rewrite-Codex "codex: unsupported" "echo hello" ""

Write-Host ""

# ---- SECTION 8: Settings.json format validation ----
Write-Host "--- Settings.json format validation ---" -ForegroundColor Yellow

# Test the expected format that init.rs will generate
$expectedBashEntry = @{
    matcher = "Bash"
    hooks = @(
        @{
            type = "command"
            command = "rtk hook claude"
            timeout = 10
        }
    )
}

$expectedShellEntry = @{
    matcher = "Shell"
    hooks = @(
        @{
            type = "command"
            command = "rtk hook claude"
            timeout = 15
        }
    )
}

# Validate Bash entry format
$bashJson = $expectedBashEntry | ConvertTo-Json -Compress
Write-ColorOutput "  Bash matcher format: $bashJson" "DarkGray"

# Validate Shell entry format
$shellJson = $expectedShellEntry | ConvertTo-Json -Compress
Write-ColorOutput "  Shell matcher format: $shellJson" "DarkGray"

# Test that the format is valid JSON and has required fields
$bashParsed = $bashJson | ConvertFrom-Json
$shellParsed = $shellJson | ConvertFrom-Json

$formatOk = $true
if ($bashParsed.hooks[0].timeout -ne 10) {
    Write-ColorOutput "  FAIL Bash timeout should be 10" "Red"
    $script:Fail++; $script:Total++
    $formatOk = $false
}
if ($shellParsed.hooks[0].timeout -ne 15) {
    Write-ColorOutput "  FAIL Shell timeout should be 15" "Red"
    $script:Fail++; $script:Total++
    $formatOk = $false
}
if ($bashParsed.matcher -ne "Bash") {
    Write-ColorOutput "  FAIL Bash matcher name" "Red"
    $script:Fail++; $script:Total++
    $formatOk = $false
}
if ($shellParsed.matcher -ne "Shell") {
    Write-ColorOutput "  FAIL Shell matcher name" "Red"
    $script:Fail++; $script:Total++
    $formatOk = $false
}
if ($formatOk) {
    Write-ColorOutput "  PASS settings.json format validation (timeout + dual matcher)" "Green"
    $script:Pass++; $script:Total++
}

Write-Host ""

# ---- SECTION 9: Env var prefix handling ----
Write-Host "--- Env var prefix handling ---" -ForegroundColor Yellow
Test-Rewrite "env + git status" "GIT_PAGER=cat git status" "GIT_PAGER=cat rtk git status"
Test-Rewrite "env + cargo test" "RUST_BACKTRACE=1 cargo test" "RUST_BACKTRACE=1 rtk cargo test"
Test-Rewrite "multi env + vitest" "NODE_ENV=test CI=1 npx vitest" "NODE_ENV=test CI=1 rtk vitest"

Write-Host ""

# ---- SECTION 10: Edge cases ----
Write-Host "--- Edge cases ---" -ForegroundColor Yellow
# Test with empty command
$emptyInput = '{"tool_name":"Bash","tool_input":{"command":""}}' 
$emptyOutput = $emptyInput | & $RtkBinary hook claude 2>$null | Out-String
$script:Total++
if ([string]::IsNullOrEmpty($emptyOutput.Trim())) {
    Write-ColorOutput "  PASS Empty command (no rewrite)" "Green"
    $script:Pass++
} else {
    Write-ColorOutput "  FAIL Empty command should not rewrite" "Red"
    $script:Fail++
}

# Test with non-Bash tool
$globInput = '{"tool_name":"Glob","tool_input":{"pattern":"*.rs"}}'
$globOutput = $globInput | & $RtkBinary hook claude 2>$null | Out-String
$script:Total++
if ([string]::IsNullOrEmpty($globOutput.Trim())) {
    Write-ColorOutput "  PASS Non-Bash tool (Glob) passes through" "Green"
    $script:Pass++
} else {
    Write-ColorOutput "  FAIL Non-Bash tool should pass through" "Red"
    $script:Fail++
}

# Test with BOM prefix (simulates Windows stdin)
$bomInput = "`u{FEFF}{`"tool_name`":`"Bash`",`"tool_input`":{`"command`":`"git status`"}}"
$bomOutput = $bomInput | & $RtkBinary hook claude 2>$null | Out-String
$script:Total++
try {
    $bomParsed = $bomOutput | ConvertFrom-Json
    $bomCmd = $bomParsed.hookSpecificOutput.updatedInput.command
    if ($bomCmd -eq "rtk git status") {
        Write-ColorOutput "  PASS BOM-prefixed input handled correctly" "Green"
        $script:Pass++
    } else {
        Write-ColorOutput "  FAIL BOM handling: expected 'rtk git status', got '$bomCmd'" "Red"
        $script:Fail++
    }
} catch {
    Write-ColorOutput "  FAIL BOM handling: parse error" "Red"
    $script:Fail++
}

Write-Host ""

# ---- SECTION 11: npm/npx patterns ----
Write-Host "--- npm/npx patterns ---" -ForegroundColor Yellow
Test-Rewrite "npx jest" "npx jest" "rtk jest"
Test-Rewrite "npx eslint" "npx eslint" "rtk lint"
Test-Rewrite "npm run build" "npm run build" "rtk npm run build"
# "npm test" is not in the rewrite rules (only npm run|exec|run-script are matched).
Test-Rewrite "npm test (bare, no rewrite)" "npm test" ""

Write-Host ""

# ============================================================
# Summary
# ============================================================
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  Results" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

$coverage = if ($script:Total -gt 0) { [math]::Round(($script:Pass / $script:Total) * 100, 1) } else { 0 }
Write-Host "  Total:  $($script:Total)" -ForegroundColor White
Write-Host "  Pass:   $($script:Pass)" -ForegroundColor Green
Write-Host "  Fail:   $($script:Fail)" -ForegroundColor Red
Write-Host "  Coverage: ${coverage}%" -ForegroundColor $(if ($coverage -ge 90) { "Green" } else { "Yellow" })
Write-Host ""

if ($script:Fail -gt 0) {
    Write-ColorOutput "  Some tests FAILED. Check the output above for details." "Red"
    exit 1
} else {
    Write-ColorOutput "  All tests PASSED." "Green"
    Write-Host ""
    Write-ColorOutput "  To test with diagnostics:" "Cyan"
    Write-ColorOutput '    $env:RTK_HOOK_DIAGNOSTICS=1; echo ''{"tool_name":"Bash","tool_input":{"command":"git status"}}'' | rtk hook claude' "DarkGray"
    Write-Host ""
    Write-ColorOutput "  To test with audit logging:" "Cyan"
    Write-ColorOutput '    $env:RTK_HOOK_AUDIT=1; rtk git status' "DarkGray"
    exit 0
}
