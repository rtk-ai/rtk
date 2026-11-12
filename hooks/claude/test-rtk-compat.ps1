# rtk Windows 兼容性 + 压缩率测试套件 v2
# 改进: 动态探测目标文件、正确区分无匹配vs失败、检测负压缩
# 用法: pwsh -File test-rtk-compat.ps1

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
function Write-OK { param([string]$detail="") ; $script:Pass++; $script:TotalTests++; Write-H "  PASS $detail" "Green" }
function Write-FAIL([string]$reason) { $script:Fail++; $script:TotalTests++; Write-H "  FAIL: $reason" "Red" }
function Write-WARN([string]$reason) { $script:Warn++; $script:TotalTests++; Write-H "  WARN: $reason" "Yellow" }

function Count-Tokens([string]$text) {
    if ([string]::IsNullOrWhiteSpace($text)) { return 0 }
    return [math]::Max(1, [math]::Round($text.Length * 0.75))
}

# Run a command and capture stdout+stderr as string
function Invoke-Capture([string]$cmd, [string]$workDir) {
    Push-Location $workDir -EA SilentlyContinue
    # Wrap in script block to handle paths with spaces properly
    $out = try { & ([scriptblock]::Create($cmd)) 2>&1 | Out-String } catch { $_.Exception.Message }
    Pop-Location -EA SilentlyContinue
    return $out
}

function Test-Command {
    param(
        [string]$Name,
        [string]$RawCommand,
        [string]$RtkCommand,
        [string]$WorkingDir = $Target,
        [ValidateSet("HasOutput","NoOutput","Any")]
        [string]$ExpectOutput = "HasOutput",
        [bool]$ExpectRewrite = $true,
        [string]$MustContain = ""  # rtk output must contain this string
    )

    Write-H ""
    Write-H "--- $Name ---" "Cyan"
    Write-H "  原始: $RawCommand" "DarkGray"
    Write-H "  RTK:  $RtkCommand" "DarkGray"

    # === 1. Hook interception test ===
    $hookInput = @{ tool_name = "Bash"; tool_input = @{ command = $RawCommand } } | ConvertTo-Json -Compress
    $hookOutput = $null
    try {
        $hookOutput = $hookInput | & $RtkBin hook claude 2>$null | ConvertFrom-Json -EA 0
    } catch {
        Write-H "  Hook: JSON parse error (可能包含特殊字符)" "Yellow"
    }

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
    $rawOut = Invoke-Capture $RawCommand $WorkingDir
    $rawTokens = Count-Tokens $rawOut

    # === 3. RTK command output ===
    # Quote the RTK binary path to handle spaces in directory names
    $quotedCmd = $RtkCommand -replace "^$([regex]::Escape($RtkBin))", "`"$RtkBin`""
    $rtkOut = Invoke-Capture $quotedCmd $WorkingDir
    $rtkTokens = Count-Tokens $rtkOut

    # === 4. Token comparison ===
    $saved = $rawTokens - $rtkTokens
    $pct = if ($rawTokens -gt 0) { [math]::Round(($saved / $rawTokens) * 100, 1) } else { 0 }

    $tokenColor = if ($pct -ge 30) { "Green" } elseif ($pct -ge 0) { "Yellow" } else { "Red" }
    Write-H "  原始: ~${rawTokens}t | rtk: ~${rtkTokens}t | 节省: ${saved}t (${pct}%)" $tokenColor

    # Detect negative savings (RTK output LARGER than raw)
    if ($pct -lt -50 -and $rawTokens -gt 10) {
        Write-H "  ⚠️  负压缩! RTK 输出比原始大 $([math]::Abs($pct))% — 可能过滤器误识别" "Red"
    }

    # Track totals
    $script:TotalRawTokens += $rawTokens
    $script:TotalRtkTokens += $rtkTokens

    # === 5. Output validation ===
    $hasRtkOutput = $rtkOut.Trim().Length -gt 0

    switch ($ExpectOutput) {
        "HasOutput" {
            if ($hasRtkOutput) {
                if ($MustContain -and -not $rtkOut.Contains($MustContain)) {
                    Write-FAIL "输出缺少必需内容: '$MustContain'"
                } else {
                    Write-OK "(有输出)"
                }
            } else {
                Write-FAIL "期望有输出但得到空"
            }
        }
        "NoOutput" {
            # 无匹配是正常的 — 检查是否有有意义的"0 matches"提示
            if (-not $hasRtkOutput) {
                Write-OK "(空输出 — 可考虑返回 '0 matches' 提示)"
            } elseif ($rtkOut -match "0 match|no match|no result") {
                Write-OK "(有零匹配提示)"
            } else {
                Write-WARN "无匹配场景下有非预期输出"
            }
        }
        "Any" {
            if ($hasRtkOutput) {
                Write-OK "(有输出)"
            } else {
                Write-OK "(无输出)"
            }
        }
    }

    # Show preview
    $preview = ($rtkOut.Trim() -split "`n")[0..2] -join "`n"
    if ($preview) { Write-H "  预览: $preview" "DarkGray" }

    return @{ raw = $rawOut; rtk = $rtkOut; rawTokens = $rawTokens; rtkTokens = $rtkTokens }
}

# ============================================================
# Auto-detect test files in target directory
# ============================================================
function Find-TestAssets {
    param([string]$Dir)

    $assets = @{}

    # Find a small text file for cat tests
    $assets.TestFile = Get-ChildItem $Dir -Recurse -File -EA 0 |
        Where-Object { $_.Length -gt 100 -and $_.Length -lt 5000 -and $_.Extension -match '\.(js|ts|json|md|py)$' -and $_.FullName -notmatch 'node_modules' } |
        Select-Object -First 1

    # Check for git repo
    Push-Location $Dir -EA 0
    $assets.IsGit = try { git rev-parse --git-dir 2>$null; $true } catch { $false }
    Pop-Location -EA 0

    # Find common source code extensions (text-based only, exclude images/binary)
    $textExts = @('.js','.ts','.vue','.jsx','.tsx','.py','.rb','.go','.rs','.java','.cs','.php','.html','.css','.scss','.json','.xml','.yaml','.yml','.md','.sql','.sh','.bat','.ps1','.c','.cpp','.h','.hpp')
    $exts = Get-ChildItem $Dir -Recurse -File -EA 0 |
        Where-Object { $_.FullName -notmatch 'node_modules|\.git|dist|build' -and $_.Extension -in $textExts } |
        Group-Object Extension |
        Sort-Object Count -Descending |
        Select-Object -First 5 -ExpandProperty Name
    $assets.SourceExts = $exts

    # Find a source directory with actual code
    $assets.SourceDir = Get-ChildItem $Dir -Directory -EA 0 |
        Where-Object { $_.Name -notmatch 'node_modules|\.git|dist|build|\.claude|\.omc' } |
        Select-Object -First 1

    return $assets
}

# ============================================================
# Main
# ============================================================
Write-H ""
Write-H "================================================================" "Cyan"
Write-H "  RTK Windows 兼容性 + 压缩率测试 v2" "Cyan"
Write-H "  目标: $Target" "Cyan"
Write-H "  时间: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" "Cyan"
Write-H "================================================================" "Cyan"

# Verify target
if (-not (Test-Path $Target)) {
    Write-FAIL "目标目录不存在: $Target"
    exit 1
}

# Verify rtk
try {
    $ver = & $RtkBin --version 2>&1
    Write-H "  rtk: $ver" "Cyan"
} catch {
    Write-FAIL "rtk not found: $RtkBin"
    exit 1
}

# Discover test assets
$assets = Find-TestAssets $Target
Write-H "  测试文件: $($assets.TestFile.Name)" "DarkGray"
Write-H "  Git: $($assets.IsGit)" "DarkGray"
Write-H "  源码扩展名: $($assets.SourceExts -join ', ')" "DarkGray"

# Determine a valid source ext for grep tests
$grepExt = if ($assets.SourceExts) { $assets.SourceExts[0] } else { ".js" }
Write-H "  grep 测试扩展名: $grepExt" "DarkGray"

# ============================================================
# 1. ls / tree
# ============================================================
Write-H ""
Write-H "========== 1. 文件列表: ls / tree ==========" "Yellow"

Test-Command "ls (无参数)" "ls" "rtk ls"
# 注意: Windows PowerShell 的 ls 是 Get-ChildItem 别名，不支持 -la
# 用 rtk ls -la 单独测试，不与 PowerShell ls 对比
Test-Command "ls -la (rtk原生)" "rtk ls -la" "rtk ls -la" -ExpectRewrite $false

if ($assets.SourceDir) {
    Test-Command "ls 子目录" "ls $($assets.SourceDir.Name)" "rtk ls $($assets.SourceDir.Name)"
}

# ============================================================
# 2. cat / read
# ============================================================
Write-H ""
Write-H "========== 2. 文件读取: cat / read ==========" "Yellow"

if ($assets.TestFile) {
    $fpath = $assets.TestFile.FullName
    Test-Command "cat 文件" "cat `"$fpath`"" "rtk read `"$fpath`""
} else {
    Write-WARN "No suitable test file found"
}

# ASCII config file
if (Test-Path "$Target\package.json") {
    Test-Command "cat package.json (ASCII路径)" "cat package.json" "rtk read package.json"
}

# ============================================================
# 3. grep / rg — 使用实际存在的文件类型
# ============================================================
Write-H ""
Write-H "========== 3. 搜索: grep / rg ==========" "Yellow"

# 3a. grep 搜索 — 使用 --include (兼容 grep 和 rg)
$grepPattern = "export"
Test-Command "grep 搜索 (--include)" "grep -rn '$grepPattern' . --include='*$grepExt'" "rtk grep '$grepPattern' . --include='*$grepExt'"

# 3b. grep 无匹配 — 搜索不可能存在的字符串
Test-Command "grep 无匹配 (应返回0matches)" "grep -rn 'ZZZNONEXISTENT_STRING_12345' . --include='*$grepExt'" "rtk grep 'ZZZNONEXISTENT_STRING_12345' . --include='*$grepExt'" -ExpectOutput "NoOutput"

# 3c. grep --files-with-matches 路径列表模式 (用长标志避免与 rtk grep -l/--max-len 冲突)
Test-Command "grep --files-with-matches 路径列表" "grep -rln '$grepPattern' . --include='*$grepExt'" "rtk grep '$grepPattern' . --include='*$grepExt' --files-with-matches"

# 3d. grep 搜索 子目录
if ($assets.SourceDir) {
    $sd = $assets.SourceDir.Name
    Test-Command "grep 子目录搜索" "grep -rn 'function' $sd --include='*$grepExt'" "rtk grep 'function' $sd --include='*$grepExt'"
}

# 3e. grep 无匹配 子目录
Test-Command "grep 子目录无匹配" "grep -rn 'ZZZNONEXISTENT_12345' . --include='*$grepExt'" "rtk grep 'ZZZNONEXISTENT_12345' . --include='*$grepExt'" -ExpectOutput "NoOutput"

# ============================================================
# 4. git
# ============================================================
Write-H ""
Write-H "========== 4. Git ==========" "Yellow"

$gitDir = if ($assets.IsGit) { $Target } else { "D:\AI\RTK fuben\rtk-repo" }

Test-Command "git status" "git status" "rtk git status" -WorkingDir $gitDir
Test-Command "git log" "git log --oneline -5" "rtk git log --oneline -5" -WorkingDir $gitDir
Test-Command "git diff" "git diff --stat HEAD~1" "rtk git diff --stat HEAD~1" -WorkingDir $gitDir

# ============================================================
# 5. npm / package manager
# ============================================================
Write-H ""
Write-H "========== 5. 包管理 ==========" "Yellow"

if (Test-Path "$Target\package.json") {
    # npm ls is not in rewrite rules (only npm run|exec|run-script matched)
    Test-Command "npm ls" "npm ls --depth=0" "npm ls --depth=0" -ExpectOutput "Any" -ExpectRewrite $false
}

# ============================================================
# 6. 边缘情况
# ============================================================
Write-H ""
Write-H "========== 6. 边缘情况 ==========" "Yellow"

# Empty command
$hookInput = '{"tool_name":"Bash","tool_input":{"command":""}}'
$hookOutput = $hookInput | & $RtkBin hook claude 2>$null
$script:TotalTests++
if ($null -eq $hookOutput -or [string]::IsNullOrEmpty("$hookOutput".Trim())) { Write-OK "(空命令不改写)"; $script:Pass++ } else { Write-FAIL "空命令不应改写"; $script:Fail++ }

# Non-Bash tool
$hookInput = '{"tool_name":"Glob","tool_input":{"pattern":"*.rs"}}'
$hookOutput = $hookInput | & $RtkBin hook claude 2>$null
$script:TotalTests++
if ($null -eq $hookOutput -or [string]::IsNullOrEmpty("$hookOutput".Trim())) { Write-OK "(非Bash工具不改写)"; $script:Pass++ } else { Write-FAIL "非Bash工具不应改写"; $script:Fail++ }

# PowerShell tool
$hookInput = '{"tool_name":"PowerShell","tool_input":{"command":"git status"}}'
$hookOutput = $hookInput | & $RtkBin hook claude 2>$null
$script:TotalTests++
if ($hookOutput -match "permissionDecision") { Write-OK "(PowerShell工具触发hook)"; $script:Pass++ } else { Write-FAIL "PowerShell工具应触发hook"; $script:Fail++ }

# Unicode path hook test
Write-H ""
Write-H "--- Unicode 路径 hook 测试 ---" "Cyan"
$unicodeInput = @{ tool_name = "Bash"; tool_input = @{ command = "cat `"E:\桌面\测试\文件.txt`"" } } | ConvertTo-Json -Compress
$script:TotalTests++
try {
    $unicodeOutput = $unicodeInput | & $RtkBin hook claude 2>$null | ConvertFrom-Json -EA Stop
    if ($unicodeOutput.hookSpecificOutput.updatedInput.command) {
        Write-OK "(Unicode路径hook正常)"; $script:Pass++
    } else {
        Write-WARN "Unicode路径hook返回但无command"
    }
} catch {
    Write-FAIL "Unicode路径导致hook JSON解析失败: $($_.Exception.Message)"
}

# ============================================================
# 7. 压缩质量分析
# ============================================================
Write-H ""
Write-H "========== 7. 压缩质量分析 ==========" "Yellow"

if ($script:TotalRawTokens -gt 0) {
    $overallPct = [math]::Round((($script:TotalRawTokens - $script:TotalRtkTokens) / $script:TotalRawTokens) * 100, 1)
    $color = if ($overallPct -ge 30) { "Green" } elseif ($overallPct -ge 0) { "Yellow" } else { "Red" }
    Write-H "  整体压缩率: ${overallPct}%" $color
    if ($overallPct -lt 0) {
        Write-H "  ⚠️ 整体负压缩! RTK 输出比原始大 $([math]::Abs($overallPct))%" "Red"
        Write-H "  原因可能是: grep -l 路径被误识别为 find 输出" "Red"
    }
}

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
Write-H "  压缩率:      ${savePct}%" $(if ($savePct -ge 30) { "Green" } elseif ($savePct -ge 0) { "Yellow" } else { "Red" })
Write-H ""

exit $(if ($script:Fail -eq 0) { 0 } else { 1 })
