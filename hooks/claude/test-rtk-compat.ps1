# rtk Windows 兼容性 + 压缩率测试套件
# 目标: E:\Desktop\拼多多\客服网站\wwwroot\wendingjc_web
# 用法: powershell -File test-rtk-compat.ps1

param(
    [string]$Target = "E:\Desktop\拼多多\客服网站\wwwroot\wendingjc_web",
    [string]$RtkBin = "rtk"
)

$ErrorActionPreference = "Continue"
$script:Pass = 0
$script:Fail = 0
$script:Warn = 0
$script:TotalTests = 0
$script:TotalRawTokens = 0
$script:TotalRtkTokens = 0

function Write-H { param([string]$M, [string]$C="White") Write-Host $M -ForegroundColor $C }
function Write-OK { $script:Pass++; $script:TotalTests++; Write-H "  PASS" "Green" }
function Write-FAIL([string]$reason) { $script:Fail++; $script:TotalTests++; Write-H "  FAIL: $reason" "Red" }
function Write-WARN([string]$reason) { $script:Warn++; $script:TotalTests++; Write-H "  WARN: $reason" "Yellow" }

function Count-Tokens([string]$text) {
    # Rough token estimate: ~0.75 tokens per char for code, ~0.3 for spaces
    if ([string]::IsNullOrWhiteSpace($text)) { return 0 }
    $chars = $text.Length
    return [math]::Max(1, [math]::Round($chars * 0.75))
}

function Test-Command {
    param(
        [string]$Name,
        [string]$RawCommand,
        [string]$RtkCommand,
        [string]$WorkingDir = $Target,
        [bool]$ExpectOutput = $true,
        [bool]$ExpectRewrite = $true
    )

    Write-H ""
    Write-H "--- $Name ---" "Cyan"
    Write-H "  原始: $RawCommand" "DarkGray"

    # === 1. Hook interception test ===
    $hookInput = @{ tool_name = "Bash"; tool_input = @{ command = $RawCommand } } | ConvertTo-Json -Compress
    $hookOutput = $hookInput | & $RtkBin hook claude 2>$null | ConvertFrom-Json -EA 0

    if ($hookOutput -and $hookOutput.hookSpecificOutput.permissionDecision -eq "allow") {
        $rewritten = $hookOutput.hookSpecificOutput.updatedInput.command
        Write-H "  改写: $rewritten" "DarkGray"
        if ($ExpectRewrite -and $rewritten -ne $RawCommand) {
            Write-H "  Hook: OK (auto-rewrite)" "Green"
        } elseif (-not $ExpectRewrite) {
            Write-H "  Hook: OK (expected passthrough)" "Green"
        } else {
            Write-WARN "Hook not rewriting as expected"
        }
    } elseif ($ExpectRewrite) {
        Write-WARN "Hook not triggered or no permissionDecision"
    } else {
        Write-H "  Hook: OK (passthrough)" "Green"
    }

    # === 2. Raw command output ===
    Push-Location $WorkingDir -EA SilentlyContinue
    $rawOut = try { Invoke-Expression $RawCommand 2>&1 | Out-String } catch { $_.Exception.Message }
    Pop-Location -EA SilentlyContinue
    $rawTokens = Count-Tokens $rawOut
    $script:TotalRawTokens += $rawTokens

    # === 3. RTK command output ===
    Push-Location $WorkingDir -EA SilentlyContinue
    $rtkOut = try { Invoke-Expression $RtkCommand 2>&1 | Out-String } catch { $_.Exception.Message }
    Pop-Location -EA SilentlyContinue
    $rtkTokens = Count-Tokens $rtkOut
    $script:TotalRtkTokens += $rtkTokens

    # === 4. Compare ===
    $saved = $rawTokens - $rtkTokens
    $pct = if ($rawTokens -gt 0) { [math]::Round(($saved / $rawTokens) * 100, 1) } else { 0 }

    Write-H "  原始 tokens: ~$rawTokens | rtk tokens: ~$rtkTokens | 节省: $saved ($pct%)" "White"

    if ($ExpectOutput) {
        if ($rtkOut.Trim().Length -gt 0) {
            Write-OK
        } else {
            Write-FAIL "Expected output but got empty"
        }
    } else {
        Write-H "  Output check: skipped (expected no output)" "DarkGray"
    }

    # Show first 3 lines of RTK output
    $preview = ($rtkOut.Trim() -split "`n")[0..2] -join "`n"
    if ($preview) { Write-H "  输出预览: $preview" "DarkGray" }

    $saved
}

Write-H ""
Write-H "================================================================" "Cyan"
Write-H "  RTK Windows 兼容性 + 压缩率测试" "Cyan"
Write-H "  目标: $Target" "Cyan"
Write-H "  时间: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" "Cyan"
Write-H "================================================================" "Cyan"

# Verify target exists
if (-not (Test-Path $Target)) {
    Write-FAIL "目标目录不存在: $Target"
    exit 1
}

# Verify rtk available
try {
    $ver = & $RtkBin --version 2>&1
    Write-H "  rtk: $ver" "Cyan"
} catch {
    Write-FAIL "rtk not found: $RtkBin"
    exit 1
}

# ============================================================
# SECTION 1: ls / tree
# ============================================================
Write-H ""
Write-H "========== 1. 文件列表: ls / tree ==========" "Yellow"

Test-Command "ls (no args)" "ls" "rtk ls"
Test-Command "ls -la" "ls -la" "rtk ls -la"
Test-Command "ls 子目录" "ls server" "rtk ls server"
Test-Command "tree (dir listing)" "cmd /c dir /b" "rtk ls"

# ============================================================
# SECTION 2: cat / read
# ============================================================
Write-H ""
Write-H "========== 2. 文件读取: cat / read ==========" "Yellow"

# Find a readable file
$testFile = Get-ChildItem $Target -Recurse -File -EA 0 | Where-Object { $_.Length -gt 100 -and $_.Length -lt 10000 -and $_.Extension -match '\.(js|ts|vue|json|md|py)$' } | Select-Object -First 1
if ($testFile) {
    $relPath = $testFile.FullName.Substring($Target.Length).TrimStart('\')
    Test-Command "cat 小文件" "cat `"$($testFile.FullName)`"" "rtk read `"$($testFile.FullName)`"" -WorkingDir $Target
    Test-Command "head 文件" "head `"$($testFile.FullName)`"" "rtk read `"$($testFile.FullName)`"" -WorkingDir $Target
} else {
    Write-WARN "No suitable test file found in $Target"
}

# Test with a config file
$configFile = Get-ChildItem "$Target\package.json" -EA 0
if ($configFile) {
    Test-Command "cat package.json" "cat package.json" "rtk read package.json"
}

# ============================================================
# SECTION 3: grep / rg
# ============================================================
Write-H ""
Write-H "========== 3. 搜索: grep / rg ==========" "Yellow"

Test-Command "rg 搜索" "rg 'export default' --glob '*.vue'" "rtk grep 'export default' --glob '*.vue'"
Test-Command "rg --count" "rg 'import' --glob '*.vue' --count" "rtk grep 'import' --glob '*.vue' --count"
Test-Command "grep 搜索" "grep -rn 'function' server --include='*.js'" "rtk grep -rn 'function' server --include='*.js'"
Test-Command "rg 特定文件" "rg 'const' server --glob '*.js' -l" "rtk grep 'const' server --glob '*.js' -l"

# ============================================================
# SECTION 4: git diff
# ============================================================
Write-H ""
Write-H "========== 4. Git: diff ==========" "Yellow"

Push-Location $Target
$isGit = try { git rev-parse --git-dir 2>$null; $true } catch { $false }
Pop-Location

if ($isGit) {
    Test-Command "git diff --stat" "git diff --stat HEAD~1" "rtk git diff --stat HEAD~1"
    Test-Command "git diff" "git diff HEAD~1" "rtk git diff HEAD~1"
    Test-Command "git status" "git status" "rtk git status"
    Test-Command "git log" "git log --oneline -5" "rtk git log --oneline -5"
} else {
    Write-WARN "Not a git repository, skipping git tests"
    # Test general git behavior
    Push-Location "D:\AI\RTK fuben\rtk-repo"
    Test-Command "git diff (rtk-repo)" "git diff --stat HEAD~1" "rtk git diff --stat HEAD~1" -WorkingDir "D:\AI\RTK fuben\rtk-repo"
    Test-Command "git status (rtk-repo)" "git status" "rtk git status" -WorkingDir "D:\AI\RTK fuben\rtk-repo"
    Pop-Location
}

# ============================================================
# SECTION 5: npm / test
# ============================================================
Write-H ""
Write-H "========== 5. 包管理 & 测试: npm / cargo ==========" "Yellow"

Test-Command "npm run build" "npm run build" "rtk npm run build" -ExpectOutput $true
Test-Command "npm list" "npm ls --depth=0" "rtk npm ls --depth=0"

# Test pytest if python files exist
$hasPython = Get-ChildItem $Target -Recurse -File -EA 0 | Where-Object { $_.Extension -eq '.py' } | Select-Object -First 1
if ($hasPython -and (Get-Command python -EA 0)) {
    Test-Command "pytest" "pytest --collect-only" "rtk pytest --collect-only"
}

# ============================================================
# SECTION 6: Edge cases
# ============================================================
Write-H ""
Write-H "========== 6. 边缘情况 ==========" "Yellow"

# Empty command
$hookInput = '{"tool_name":"Bash","tool_input":{"command":""}}'
$hookOutput = $hookInput | & $RtkBin hook claude 2>$null
if ([string]::IsNullOrEmpty($hookOutput.Trim())) { Write-OK } else { Write-FAIL "Empty command should pass through" }

# Unrecognized command
$hookInput = '{"tool_name":"Bash","tool_input":{"command":"mycustomtool --flag"}}'
$hookOutput = $hookInput | & $RtkBin hook claude 2>$null
if ([string]::IsNullOrEmpty($hookOutput.Trim())) { Write-OK } else { Write-FAIL "Unrecognized command should pass through" }

# PowerShell tool call
$hookInput = '{"tool_name":"PowerShell","tool_input":{"command":"git status"}}'
$hookOutput = $hookInput | & $RtkBin hook claude 2>$null
if ($hookOutput -match "permissionDecision") { Write-OK } else { Write-FAIL "PowerShell tool should trigger hook" }

# ============================================================
# SUMMARY
# ============================================================
Write-H ""
Write-H "================================================================" "Cyan"
Write-H "  测试结果汇总" "Cyan"
Write-H "================================================================" "Cyan"

$total = $script:Pass + $script:Fail + $script:Warn
$coverage = if ($total -gt 0) { [math]::Round(($script:Pass / $total) * 100, 1) } else { 0 }
$savePct = if ($script:TotalRawTokens -gt 0) { [math]::Round((($script:TotalRawTokens - $script:TotalRtkTokens) / $script:TotalRawTokens) * 100, 1) } else { 0 }

Write-H "  测试数:   $total  ($($script:Pass) PASS / $($script:Fail) FAIL / $($script:Warn) WARN)" "White"
Write-H "  通过率:   ${coverage}%" $(if ($coverage -ge 90) { "Green" } else { "Yellow" })
Write-H "  原始 tokens: ~$($script:TotalRawTokens)" "DarkGray"
Write-H "  RTK tokens:  ~$($script:TotalRtkTokens)" "DarkGray"
Write-H "  压缩率:      ${savePct}%" $(if ($savePct -ge 50) { "Green" } elseif ($savePct -ge 30) { "Yellow" } else { "Red" })
Write-H ""

exit $(if ($script:Fail -eq 0) { 0 } else { 1 })
