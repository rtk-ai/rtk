#Requires -Version 7.0

<#
.SYNOPSIS
Runs the Windows Claude/Codex hook release oracle without changing the installed RTK binary.

.DESCRIPTION
Validates direct hook contracts, rewrite coverage, raw-versus-RTK behavior, documentation,
binary shadowing, the full Cargo test suite, and savings against an isolated tracking database.
Generated artifacts default to target/ so they do not dirty the worktree.

.PARAMETER Corpus
Optional command-corpus CSV. When supplied, it must exist and should contain Source, Shell,
Category, and Command columns. Corpus commands are passed only to `rtk hook check`; they are
not executed.
#>
param(
    [string]$Rtk = "",
    [string]$Corpus = "",
    [string]$OutDir = "",
    [ValidateRange(1, 600)]
    [int]$TimeoutSeconds = 30
)

$ErrorActionPreference = "Stop"

function Resolve-RtkPath {
    param([string]$Value)
    if ($Value -and (Test-Path -LiteralPath $Value)) {
        return (Resolve-Path -LiteralPath $Value).Path
    }
    $release = Join-Path $PSScriptRoot "..\target\release\rtk.exe"
    if (Test-Path -LiteralPath $release) {
        return (Resolve-Path -LiteralPath $release).Path
    }
    $debug = Join-Path $PSScriptRoot "..\target\debug\rtk.exe"
    if (Test-Path -LiteralPath $debug) {
        return (Resolve-Path -LiteralPath $debug).Path
    }
    $cmd = Get-Command rtk -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    throw "rtk executable not found"
}

function New-OracleOutputDir {
    param([string]$Value)
    if ($Value) {
        $path = $Value
    } else {
        $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
        $path = Join-Path (Resolve-Path (Join-Path $PSScriptRoot "..")).Path "target\rtk-windows-oracle-$stamp"
    }
    New-Item -ItemType Directory -Force -Path $path | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $path "artifacts") | Out-Null
    return (Resolve-Path -LiteralPath $path).Path
}

function Invoke-Capture {
    param(
        [string]$FileName,
        [string[]]$Arguments,
        [string]$WorkingDirectory,
        [int]$TimeoutSeconds,
        [string]$StandardInputText = $null
    )
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $FileName
    foreach ($arg in $Arguments) { [void]$psi.ArgumentList.Add($arg) }
    $psi.WorkingDirectory = $WorkingDirectory
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.RedirectStandardInput = $null -ne $StandardInputText
    $psi.UseShellExecute = $false
    $psi.Environment["RTK_DISABLED"] = "1"

    $p = [System.Diagnostics.Process]::new()
    $p.StartInfo = $psi
    [void]$p.Start()
    if ($null -ne $StandardInputText) {
        $p.StandardInput.Write($StandardInputText)
        $p.StandardInput.Close()
    }
    $stdoutTask = $p.StandardOutput.ReadToEndAsync()
    $stderrTask = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit($TimeoutSeconds * 1000)) {
        try { $p.Kill($true) } catch {}
        return [pscustomobject]@{
            exitCode = 124
            timedOut = $true
            stdout = $stdoutTask.Result
            stderr = $stderrTask.Result
        }
    }
    [pscustomobject]@{
        exitCode = $p.ExitCode
        timedOut = $false
        stdout = $stdoutTask.Result
        stderr = $stderrTask.Result
    }
}

function Invoke-ShellCapture {
    param(
        [string]$Shell,
        [string]$Command,
        [string]$Cwd,
        [int]$TimeoutSeconds
    )
    if ($Command -match "(?i)\b(mysql|mysqldump|mariadb|psql|sqlcmd|sqlite3|mongosh|redis-cli)\b") {
        throw "Refusing database client command: $Command"
    }
    switch ($Shell) {
        "cmd" { Invoke-Capture -FileName "cmd.exe" -Arguments @("/d", "/s", "/c", $Command) -WorkingDirectory $Cwd -TimeoutSeconds $TimeoutSeconds }
        "bash" {
            $git = Get-Command git.exe -ErrorAction SilentlyContinue
            $gitRoot = if ($git) {
                Split-Path (Split-Path (Split-Path $git.Source -Parent) -Parent) -Parent
            }
            $gitBash = if ($gitRoot) { Join-Path $gitRoot "bin\bash.exe" }
            $bash = if ($gitBash -and (Test-Path -LiteralPath $gitBash)) { Get-Item -LiteralPath $gitBash } else { $null }
            if (-not $bash) { return [pscustomobject]@{ exitCode = 127; timedOut = $false; stdout = ""; stderr = "bash not found" } }
            Invoke-Capture -FileName $bash.FullName -Arguments @("-c", $Command) -WorkingDirectory $Cwd -TimeoutSeconds $TimeoutSeconds
        }
        default { Invoke-Capture -FileName "powershell.exe" -Arguments @("-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", $Command) -WorkingDirectory $Cwd -TimeoutSeconds $TimeoutSeconds }
    }
}

function Save-Text {
    param([string]$Path, [string]$Text)
    Set-Content -LiteralPath $Path -Value $Text -Encoding utf8
}

function Measure-Bytes {
    param([string]$Text)
    [System.Text.Encoding]::UTF8.GetByteCount($Text)
}

function Write-Json {
    param([string]$Path, [object]$Value)
    $Value | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath $Path -Encoding utf8
}

function Get-RewrittenFromHookStdout {
    param([string]$Stdout)
    if ([string]::IsNullOrWhiteSpace($Stdout)) { return $null }
    try {
        $json = $Stdout | ConvertFrom-Json
        return $json.hookSpecificOutput.updatedInput.command
    } catch {
        return $null
    }
}

function Get-PermissionDecisionFromHookStdout {
    param([string]$Stdout)
    if ([string]::IsNullOrWhiteSpace($Stdout)) { return $null }
    try {
        $json = $Stdout | ConvertFrom-Json
        return $json.hookSpecificOutput.permissionDecision
    } catch {
        return $null
    }
}

function Invoke-HookCase {
    param(
        [string]$Name,
        [string]$Agent,
        [object]$Payload,
        [AllowNull()][object]$Expected,
        [string]$RtkPath,
        [string]$Out
    )
    $payloadPath = Join-Path $Out "artifacts\$Name.payload.json"
    Write-Json -Path $payloadPath -Value $Payload
    $payloadText = Get-Content -LiteralPath $payloadPath -Raw
    $result = Invoke-Capture -FileName $RtkPath -Arguments @("hook", $Agent) -WorkingDirectory $Out -TimeoutSeconds 15 -StandardInputText $payloadText
    Save-Text (Join-Path $Out "artifacts\$Name.stdout.txt") $result.stdout
    Save-Text (Join-Path $Out "artifacts\$Name.stderr.txt") $result.stderr
    $rewritten = Get-RewrittenFromHookStdout $result.stdout
    $permissionDecision = Get-PermissionDecisionFromHookStdout $result.stdout
    $stdoutBytes = Measure-Bytes $result.stdout
    $stderrBytes = Measure-Bytes $result.stderr
    $expectsNoRewrite = $null -eq $Expected
    $pass = if ($expectsNoRewrite) {
        (-not $result.timedOut) -and
        $result.exitCode -eq 0 -and
        $stdoutBytes -eq 0 -and
        $null -eq $rewritten -and
        $null -eq $permissionDecision
    } else {
        (-not $result.timedOut) -and
        $result.exitCode -eq 0 -and
        $rewritten -eq $Expected -and
        $null -eq $permissionDecision
    }
    [pscustomobject]@{
        name = $Name
        type = "hook"
        agent = $Agent
        exitCode = $result.exitCode
        stdoutBytes = $stdoutBytes
        stderrBytes = $stderrBytes
        rewritten = $rewritten
        expected = $Expected
        expectsNoRewrite = $expectsNoRewrite
        permissionDecision = $permissionDecision
        defaultDoesNotAutoAllow = $null -eq $permissionDecision
        pass = $pass
        payload = $payloadPath
    }
}

function Invoke-RawRtkCase {
    param(
        [string]$Name,
        [string]$RawShell,
        [string]$RawCommand,
        [string[]]$RtkArgs,
        [string]$Cwd,
        [int]$ExpectedExit,
        [int]$ExpectedRawExit = -999999,
        [int]$ExpectedRtkExit = -999999,
        [string[]]$MustContain,
        [double]$MinSavings,
        [string]$RtkPath,
        [string]$DbPath,
        [string]$Out
    )
    if ($ExpectedRawExit -eq -999999) { $ExpectedRawExit = $ExpectedExit }
    if ($ExpectedRtkExit -eq -999999) { $ExpectedRtkExit = $ExpectedExit }
    Remove-Item -LiteralPath $DbPath -Force -ErrorAction SilentlyContinue
    $raw = Invoke-ShellCapture -Shell $RawShell -Command $RawCommand -Cwd $Cwd -TimeoutSeconds $TimeoutSeconds
    $oldDb = $env:RTK_DB_PATH
    $env:RTK_DB_PATH = $DbPath
    try {
        $rtk = Invoke-Capture -FileName $RtkPath -Arguments $RtkArgs -WorkingDirectory $Cwd -TimeoutSeconds $TimeoutSeconds
    } finally {
        if ($null -eq $oldDb) { Remove-Item Env:\RTK_DB_PATH -ErrorAction SilentlyContinue } else { $env:RTK_DB_PATH = $oldDb }
    }
    Save-Text (Join-Path $Out "artifacts\$Name.raw.stdout.txt") $raw.stdout
    Save-Text (Join-Path $Out "artifacts\$Name.raw.stderr.txt") $raw.stderr
    Save-Text (Join-Path $Out "artifacts\$Name.rtk.stdout.txt") $rtk.stdout
    Save-Text (Join-Path $Out "artifacts\$Name.rtk.stderr.txt") $rtk.stderr

    $rawText = "$($raw.stdout)$($raw.stderr)"
    $rtkText = "$($rtk.stdout)$($rtk.stderr)"
    $rawBytes = Measure-Bytes $rawText
    $rtkBytes = Measure-Bytes $rtkText
    $savings = if ($rawBytes -gt 0) { 1.0 - ($rtkBytes / [double]$rawBytes) } else { 0.0 }
    $containsOk = $true
    foreach ($needle in $MustContain) {
        if ($rtkText.IndexOf($needle, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
            $containsOk = $false
        }
    }
    $dbRows = @()
    $parseRows = @()
    if (Test-Path -LiteralPath $DbPath) {
        $py = @"
import sqlite3, json, sys
con = sqlite3.connect(sys.argv[1])
con.row_factory = sqlite3.Row
print(json.dumps({
  "commands": [dict(r) for r in con.execute("select original_cmd,rtk_cmd,input_tokens,output_tokens,saved_tokens,savings_pct,exec_time_ms from commands order by id")],
  "parse_failures": [dict(r) for r in con.execute("select raw_command,error_message,fallback_succeeded from parse_failures order by id")]
}, ensure_ascii=False))
"@
        $tmp = Join-Path $Out "artifacts\$Name.dbquery.py"
        Save-Text $tmp $py
        $dbJson = & python $tmp $DbPath
        $parsed = $dbJson | ConvertFrom-Json
        $dbRows = @($parsed.commands)
        $parseRows = @($parsed.parse_failures)
    }
    [pscustomobject]@{
        name = $Name
        type = "raw-vs-rtk"
        rawShell = $RawShell
        rawCommand = $RawCommand
        rtkArgs = $RtkArgs
        rawExitCode = $raw.exitCode
        rtkExitCode = $rtk.exitCode
        expectedExit = $ExpectedExit
        expectedRawExit = $ExpectedRawExit
        expectedRtkExit = $ExpectedRtkExit
        rawBytes = $rawBytes
        rtkBytes = $rtkBytes
        savingsPct = [math]::Round($savings * 100.0, 2)
        minSavingsPct = [math]::Round($MinSavings * 100.0, 2)
        commands = $dbRows
        parseFailures = $parseRows
        pass = (-not $raw.timedOut) -and (-not $rtk.timedOut) -and $raw.exitCode -eq $ExpectedRawExit -and $rtk.exitCode -eq $ExpectedRtkExit -and $containsOk -and ($rawBytes -lt 200 -or $savings -ge $MinSavings)
    }
}

function Invoke-HookCheckCase {
    param(
        [string]$Name,
        [string]$Command,
        [string]$Expected,
        [string]$RtkPath,
        [string]$Out,
        [switch]$AllowNoRewrite
    )
    $result = Invoke-Capture -FileName $RtkPath -Arguments @("hook", "check", $Command) -WorkingDirectory $Out -TimeoutSeconds 10
    Save-Text (Join-Path $Out "artifacts\$Name.hookcheck.stdout.txt") $result.stdout
    Save-Text (Join-Path $Out "artifacts\$Name.hookcheck.stderr.txt") $result.stderr
    $stdout = $result.stdout.Trim()
    $pass = if ($AllowNoRewrite) {
        (-not $result.timedOut) -and $result.exitCode -ne 124 -and (($stdout -match "^No rewrite for:") -or ($result.stderr -match "No rewrite for:"))
    } else {
        (-not $result.timedOut) -and $result.exitCode -eq 0 -and $stdout -eq $Expected
    }
    [pscustomobject]@{
        name = $Name
        type = "hook-check"
        command = $Command
        expected = $Expected
        exitCode = $result.exitCode
        stdout = $stdout
        pass = $pass
    }
}

function Select-CorpusSample {
    param([object[]]$Rows, [int]$Limit = 2200)
    $targets = [ordered]@{ bash_like = 1400; powershell = 600; cmd = 200 }
    $selected = New-Object System.Collections.Generic.List[object]
    $seen = @{}

    foreach ($shell in $targets.Keys) {
        $shellRows = @($Rows | Where-Object { $_.Shell -eq $shell -and $_.Command -and $_.Command.Length -lt 800 } | Sort-Object Command -Unique)
        foreach ($row in ($shellRows | Select-Object -First $targets[$shell])) {
            if (-not $seen.ContainsKey($row.Command)) {
                $seen[$row.Command] = $true
                $selected.Add($row)
            }
        }
    }

    if ($selected.Count -lt $Limit) {
        foreach ($row in @($Rows | Where-Object { $_.Command -and $_.Command.Length -lt 800 } | Sort-Object Command -Unique)) {
            if ($selected.Count -ge $Limit) { break }
            if (-not $seen.ContainsKey($row.Command)) {
                $seen[$row.Command] = $true
                $selected.Add($row)
            }
        }
    }

    @($selected | Select-Object -First $Limit)
}

function Invoke-CorpusHookCheckGate {
    param(
        [object[]]$Rows,
        [string]$RtkPath,
        [string]$Out,
        [int]$Limit = 2200
    )
    $sample = Select-CorpusSample -Rows $Rows -Limit $Limit
    $samplePath = Join-Path $Out "corpus-hookcheck-sample.csv"
    $sample | Export-Csv -LiteralPath $samplePath -NoTypeInformation -Encoding utf8

    $rewritten = 0
    $noRewrite = 0
    $timeouts = 0
    $errorCount = 0
    $failureSamples = New-Object System.Collections.Generic.List[object]

    foreach ($row in $sample) {
        $result = Invoke-Capture -FileName $RtkPath -Arguments @("hook", "check", [string]$row.Command) -WorkingDirectory $Out -TimeoutSeconds 5
        if ($result.timedOut) {
            $timeouts += 1
            if ($failureSamples.Count -lt 20) { $failureSamples.Add([pscustomobject]@{ command=$row.Command; shell=$row.Shell; kind="timeout" }) }
        } elseif ($result.exitCode -eq 0) {
            $rewritten += 1
        } elseif ($result.exitCode -eq 1) {
            $noRewrite += 1
        } else {
            $errorCount += 1
            if ($failureSamples.Count -lt 20) { $failureSamples.Add([pscustomobject]@{ command=$row.Command; shell=$row.Shell; kind="exit_$($result.exitCode)"; stderr=$result.stderr }) }
        }
    }

    $sampleCount = @($sample).Count
    $powershellCount = @($sample | Where-Object { $_.Shell -eq "powershell" }).Count
    $cmdCount = @($sample | Where-Object { $_.Shell -eq "cmd" }).Count
    $bashLikeCount = @($sample | Where-Object { $_.Shell -eq "bash_like" }).Count
    $byShell = @($sample | Group-Object Shell | ForEach-Object { [pscustomobject]@{ name=$_.Name; count=$_.Count } })
    $failureItems = @()
    foreach ($item in $failureSamples) { $failureItems += $item }
    $pass = ($sampleCount -ge 2000) -and ($powershellCount -gt 0) -and ($cmdCount -gt 0) -and ($bashLikeCount -gt 0) -and ($timeouts -eq 0) -and ($errorCount -eq 0) -and ($rewritten -gt 0)

    [pscustomobject]@{
        name = "corpus_hook_check_2200"
        type = "corpus-hook-check"
        samplePath = $samplePath
        total = $sampleCount
        byShell = $byShell
        rewritten = $rewritten
        noRewrite = $noRewrite
        timeouts = $timeouts
        errors = $errorCount
        failureSamples = $failureItems
        pass = $pass
    }
}

function Test-HistoricalMatrixCoverage {
    param([object[]]$Rows)
    $patterns = [ordered]@{
        git_status = "(?i)^\s*git\s+status\b"
        git_diff = "(?i)^\s*git\s+diff\b"
        git_log = "(?i)^\s*git\s+log\b"
        git_show = "(?i)^\s*git\s+show\b"
        git_add = "(?i)^\s*git\s+add\b"
        git_commit = "(?i)^\s*git\s+commit\b"
        git_push = "(?i)^\s*git\s+push\b"
        git_pull = "(?i)^\s*git\s+pull\b"
        cat = "(?i)^\s*cat\b"
        head = "(?i)^\s*head\b"
        tail = "(?i)^\s*tail\b"
        ls = "(?i)^\s*ls\b"
        grep = "(?i)^\s*grep\b"
        rg = "(?i)^\s*rg\b"
        cargo_test = "(?i)^\s*cargo\s+test\b"
        pytest = "(?i)^\s*(python\s+-m\s+)?pytest\b"
        npm_run = "(?i)^\s*npm\s+run\b"
        pnpm_install = "(?i)^\s*pnpm\s+install\b"
        ps_get_content = "(?i)^\s*(Get-Content|gc)\b"
        ps_get_childitem = "(?i)^\s*(Get-ChildItem|gci)\b"
        ps_select_string = "(?i)^\s*(Select-String|sls)\b"
        cmd_dir = "(?i)^\s*dir\b"
        cmd_type = "(?i)^\s*type\b"
    }
    $items = @()
    foreach ($key in $patterns.Keys) {
        $count = @($Rows | Where-Object { $_.Command -match $patterns[$key] }).Count
        $items += [pscustomobject]@{ name=$key; count=$count; present=($count -gt 0) }
    }
    [pscustomobject]@{
        name = "historical_matrix_coverage"
        type = "matrix-coverage"
        items = $items
        missing = @($items | Where-Object { -not $_.present })
        pass = @($items | Where-Object { -not $_.present }).Count -eq 0
    }
}

function Get-ZeroSavedClassifications {
    param([object[]]$Cases)
    $items = @()
    foreach ($case in $Cases) {
        foreach ($row in @($case.commands)) {
            if ($null -eq $row.saved_tokens -or [int64]$row.saved_tokens -ne 0) { continue }
            $kind = if ([string]$row.rtk_cmd -match "\(passthrough\)") {
                "expected_passthrough"
            } elseif ([int64]$row.input_tokens -lt 100) {
                "expected_small_output_zero_gain"
            } elseif ([string]$row.rtk_cmd -match "fallback") {
                "degraded_parser"
            } else {
                "unexpected_zero_saved"
            }
            $items += [pscustomobject]@{
                case = $case.name
                kind = $kind
                original_cmd = $row.original_cmd
                rtk_cmd = $row.rtk_cmd
                input_tokens = $row.input_tokens
                output_tokens = $row.output_tokens
            }
        }
    }
    [pscustomobject]@{
        name = "zero_saved_classification"
        type = "zero-saved"
        items = $items
        unexpected = @($items | Where-Object { $_.kind -eq "unexpected_zero_saved" -or $_.kind -eq "degraded_parser" })
        pass = @($items | Where-Object { $_.kind -eq "unexpected_zero_saved" -or $_.kind -eq "degraded_parser" }).Count -eq 0
    }
}

function Test-DatabaseGuard {
    param([string]$RtkPath, [string]$Out)
    $commands = @("mysql -e 'select 1'", "mysqldump db", "mariadb -e 'select 1'", "psql -c 'select 1'", "sqlcmd -Q 'select 1'", "sqlite3 test.db '.tables'", "mongosh --eval 'db.stats()'", "redis-cli ping")
    $items = @()
    foreach ($cmd in $commands) {
        $result = Invoke-HookCheckCase -Name ("db_guard_" + (($cmd -replace "[^A-Za-z0-9]+", "_").Trim("_"))) -Command $cmd -Expected "" -RtkPath $RtkPath -Out $Out -AllowNoRewrite
        $items += [pscustomobject]@{
            command = $cmd
            noRewrite = $result.pass
            exitCode = $result.exitCode
            stdout = $result.stdout
        }
    }
    [pscustomobject]@{
        name = "database_hook_no_rewrite_guard"
        type = "database-guard"
        items = $items
        pass = @($items | Where-Object { -not $_.noRewrite }).Count -eq 0
    }
}

function Test-DocsCodexHookConsistency {
    param([string]$Repo)
    $checks = @(
        @{ path = 'README.md'; forbidden = 'Native Windows.*limited support|Auto-rewrite hook[^\r\n|]*\|\s*No\b|CLAUDE\.md fallback|falls back to CLAUDE\.md|Codex.*AGENTS\.md \+ RTK\.md instructions'; required = 'Codex.*rtk hook codex' },
        @{ path = 'hooks/README.md'; forbidden = 'Rules file.*Codex'; required = 'Full hook.*Codex' },
        @{ path = 'hooks/README.md'; forbidden = 'Codex CLI.*Prompt-level|Codex CLI.*N/A'; required = 'Codex CLI.*rtk hook codex.*updatedInput' },
        @{ path = 'docs/contributing/TECHNICAL.md'; forbidden = 'Codex CLI.*Awareness doc|Codex CLI.*N/A \(prompt\)'; required = 'Codex CLI.*rtk hook codex.*updatedInput' },
        @{ path = 'docs/guide/getting-started/installation.md'; forbidden = 'For full hook support, use|Native Windows.*limited support|auto-rewrite hook.*Unix shell|WSL.*required for hook support'; required = 'Native Windows hooks are supported.*Claude Code.*Codex CLI' },
        @{ path = 'docs/guide/getting-started/supported-agents.md'; forbidden = 'Codex CLI.*AGENTS\.md instructions|Rules file integrations \([^)]*Codex|Auto-rewrite does not work|falls back to \*\*CLAUDE\.md injection mode\*\*|For full hook support on Windows, use|Full hook integrations.*guaranteed|Codex CLI.*guaranteed'; required = 'Codex CLI.*rtk hook codex.*updatedInput' },
        @{ path = 'docs/guide/resources/troubleshooting.md'; forbidden = 'auto-rewrite hook.*Unix shell|Native Windows does not have one|Native Windows doesn''t have one|falls back to CLAUDE\.md injection|won''t auto-rewrite commands'; required = 'rtk init -g --codex' },
        @{ path = 'hooks/codex/README.md'; forbidden = 'no programmatic hook|prompt-level only|Installed to .* by `rtk init --codex`'; required = 'rtk init -g --codex.*hooks\.json' },
        @{ path = 'hooks/codex/README.md'; forbidden = 'no programmatic hook|prompt-level only|project-local Codex configs install hooks'; required = 'rtk init --codex.*project-scoped guidance only' }
    )
    $items = @()
    foreach ($check in $checks) {
        $fullPath = Join-Path $Repo $check.path
        $text = Get-Content -LiteralPath $fullPath -Raw
        $forbiddenHit = [regex]::Match($text, $check.forbidden, [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)
        $requiredHit = [regex]::Match($text, $check.required, [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)
        $items += [pscustomobject]@{
            path = $check.path
            forbidden = $check.forbidden
            forbiddenMatched = $forbiddenHit.Success
            required = $check.required
            requiredMatched = $requiredHit.Success
        }
    }
    [pscustomobject]@{
        name = "docs_codex_hook_consistency"
        type = "docs-check"
        items = $items
        pass = @($items | Where-Object { $_.forbiddenMatched -or -not $_.requiredMatched }).Count -eq 0
    }
}

function Test-MarkdownLocalLinks {
    param([string]$Repo)
    $files = @("docs/contributing/TECHNICAL.md")
    $items = @()
    foreach ($rel in $files) {
        $file = Join-Path $Repo $rel
        $base = Split-Path -Parent $file
        $lines = Get-Content -LiteralPath $file
        for ($i = 0; $i -lt $lines.Count; $i++) {
            foreach ($match in [regex]::Matches($lines[$i], '\[[^\]]+\]\(([^)]+\.md(?:#[^)]+)?)\)')) {
                $target = $match.Groups[1].Value
                if ($target -match '^(https?:|mailto:|#)') { continue }
                $targetNoFragment = ($target -split '#', 2)[0]
                $targetPath = Join-Path $base $targetNoFragment
                $items += [pscustomobject]@{
                    file = $rel
                    line = $i + 1
                    target = $target
                    exists = Test-Path -LiteralPath $targetPath
                }
            }
        }
    }
    [pscustomobject]@{
        name = "markdown_local_links"
        type = "docs-link-check"
        items = $items
        pass = @($items | Where-Object { -not $_.exists }).Count -eq 0
    }
}

function Test-InitShowUsage {
    param(
        [string]$RtkPath,
        [string]$Repo,
        [string]$Out
    )
    $result = Invoke-Capture -FileName $RtkPath -Arguments @("init", "-g", "--codex", "--show") -WorkingDirectory $Repo -TimeoutSeconds 15
    $artifact = Join-Path $Out "artifacts\init-codex-show.txt"
    Save-Text $artifact ($result.stdout + $result.stderr)
    $hasStatus = $result.stdout.Contains("Global hooks.json")
    $hasUsage = $result.stdout.Contains('Configure $CODEX_HOME/hooks.json + AGENTS.md + RTK.md')
    $hasExecutableResolution = $result.stdout.Contains("command resolves to")
    [pscustomobject]@{
        name = "init_codex_show_usage_hooks_json"
        type = "init-show"
        artifact = $artifact
        exitCode = $result.exitCode
        hasGlobalHooksStatus = $hasStatus
        hasHooksJsonUsage = $hasUsage
        hasExecutableResolution = $hasExecutableResolution
        pass = $result.exitCode -eq 0 -and $hasStatus -and $hasUsage -and $hasExecutableResolution
    }
}

function Test-ParentBinaryShadow {
    param(
        [string]$RtkPath,
        [string]$Repo,
        [string]$Out
    )
    $parent = Split-Path -Parent $Repo
    $parentExe = Join-Path $parent "rtk.exe"
    $expectedHash = (Get-FileHash -LiteralPath $RtkPath -Algorithm SHA256).Hash
    $expectedVersion = (& $RtkPath --version 2>$null)
    $parentExists = Test-Path -LiteralPath $parentExe
    $parentHash = if ($parentExists) { (Get-FileHash -LiteralPath $parentExe -Algorithm SHA256).Hash } else { $null }
    $cmd = Invoke-Capture -FileName "cmd.exe" -Arguments @("/c", "where rtk && rtk --version") -WorkingDirectory $parent -TimeoutSeconds 15
    $artifact = Join-Path $Out "artifacts\parent-shadow-where-rtk.txt"
    Save-Text $artifact ($cmd.stdout + $cmd.stderr)
    $lines = @($cmd.stdout -split "`r?`n" | Where-Object { $_ })
    $first = if ($lines.Count -gt 0) { $lines[0] } else { "" }
    [pscustomobject]@{
        name = "parent_binary_shadow_guard"
        type = "binary-shadow"
        parent = $parent
        parentExe = $parentExe
        artifact = $artifact
        expectedHash = $expectedHash
        parentExists = $parentExists
        parentHash = $parentHash
        firstWhereRtk = $first
        expectedVersion = $expectedVersion
        cmdExitCode = $cmd.exitCode
        cmdOutput = $cmd.stdout
        pass = ((-not $parentExists) -or ($parentHash -eq $expectedHash)) -and $cmd.exitCode -eq 0 -and $cmd.stdout.Contains($expectedVersion)
    }
}

function Invoke-FullCargoTestGate {
    param(
        [string]$Repo,
        [string]$Out
    )
    $oldDisabled = $env:RTK_DISABLED
    $oldJobs = $env:CARGO_BUILD_JOBS
    $env:RTK_DISABLED = "1"
    $env:CARGO_BUILD_JOBS = "4"
    try {
        $result = Invoke-Capture -FileName "cargo" -Arguments @("test", "--", "--test-threads=1") -WorkingDirectory $Repo -TimeoutSeconds 180
    } finally {
        if ($null -eq $oldDisabled) { Remove-Item Env:\RTK_DISABLED -ErrorAction SilentlyContinue } else { $env:RTK_DISABLED = $oldDisabled }
        if ($null -eq $oldJobs) { Remove-Item Env:\CARGO_BUILD_JOBS -ErrorAction SilentlyContinue } else { $env:CARGO_BUILD_JOBS = $oldJobs }
    }
    $stdoutPath = Join-Path $Out "artifacts\cargo-test.stdout.txt"
    $stderrPath = Join-Path $Out "artifacts\cargo-test.stderr.txt"
    Save-Text $stdoutPath $result.stdout
    Save-Text $stderrPath $result.stderr
    $combined = $result.stdout + "`n" + $result.stderr
    $matches = [regex]::Matches($combined, 'test result:\s+ok\.\s+(\d+)\s+passed;\s+(\d+)\s+failed;\s+(\d+)\s+ignored')
    $passed = if ($matches.Count -gt 0) { [int](@($matches | ForEach-Object { [int]$_.Groups[1].Value } | Measure-Object -Sum).Sum) } else { 0 }
    $failed = if ($matches.Count -gt 0) { [int](@($matches | ForEach-Object { [int]$_.Groups[2].Value } | Measure-Object -Sum).Sum) } else { -1 }
    $ignored = if ($matches.Count -gt 0) { [int](@($matches | ForEach-Object { [int]$_.Groups[3].Value } | Measure-Object -Sum).Sum) } else { 0 }
    [pscustomobject]@{
        name = "full_cargo_test_gate"
        type = "cargo-test"
        stdoutPath = $stdoutPath
        stderrPath = $stderrPath
        exitCode = $result.exitCode
        timedOut = $result.timedOut
        passedCount = $passed
        failedCount = $failed
        ignoredCount = $ignored
        pass = (-not $result.timedOut) -and $result.exitCode -eq 0 -and $passed -ge 2196 -and $failed -eq 0 -and $ignored -ge 8
    }
}

function Invoke-GainGate {
    param(
        [string]$RtkPath,
        [string]$Repo,
        [string]$Out
    )
    $gainDb = Join-Path $Out "gain-gate.db"
    Remove-Item -LiteralPath $gainDb -Force -ErrorAction SilentlyContinue
    $oldDb = $env:RTK_DB_PATH
    $oldJobs = $env:CARGO_BUILD_JOBS
    $env:RTK_DB_PATH = $gainDb
    $env:CARGO_BUILD_JOBS = "4"
    try {
        $commands = @(
            # Use two representative recursive rg searches so the gain sample stays
            # portable on Windows and no single command dominates the benchmark.
            ,@("rg", "fn ", "src")
            ,@("rg", "test_", "src")
            ,@("cargo", "test", "hooks::hook_cmd", "--", "--test-threads=1")
            ,@("git", "log", "-n", "80", "--stat")
            ,@("git", "status")
        )
        foreach ($args in $commands) {
            [void](Invoke-Capture -FileName $RtkPath -Arguments $args -WorkingDirectory $Repo -TimeoutSeconds 60)
        }
        $gainText = Invoke-Capture -FileName $RtkPath -Arguments @("gain") -WorkingDirectory $Repo -TimeoutSeconds 15
        $gainJson = Invoke-Capture -FileName $RtkPath -Arguments @("gain", "--format", "json") -WorkingDirectory $Repo -TimeoutSeconds 15
    } finally {
        if ($null -eq $oldDb) { Remove-Item Env:\RTK_DB_PATH -ErrorAction SilentlyContinue } else { $env:RTK_DB_PATH = $oldDb }
        if ($null -eq $oldJobs) { Remove-Item Env:\CARGO_BUILD_JOBS -ErrorAction SilentlyContinue } else { $env:CARGO_BUILD_JOBS = $oldJobs }
    }

    Save-Text (Join-Path $Out "gain-gate.txt") $gainText.stdout
    Save-Text (Join-Path $Out "gain-gate.json") $gainJson.stdout

    $parsed = $gainJson.stdout | ConvertFrom-Json
    $dbRows = @()
    if (Test-Path -LiteralPath $gainDb) {
        $py = @"
import sqlite3, json, sys
con = sqlite3.connect(sys.argv[1])
con.row_factory = sqlite3.Row
print(json.dumps([dict(r) for r in con.execute("select original_cmd,rtk_cmd,input_tokens,output_tokens,saved_tokens,savings_pct,exec_time_ms from commands order by id")], ensure_ascii=False))
"@
        $tmp = Join-Path $Out "artifacts\gain-gate-dbquery.py"
        Save-Text $tmp $py
        $dbRows = @((& python $tmp $gainDb) | ConvertFrom-Json)
    }
    $readRows = @($dbRows | Where-Object { $_.rtk_cmd -match "^rtk read" })
    $nonReadRows = @($dbRows | Where-Object { $_.rtk_cmd -notmatch "^rtk read" })
    $inputTotal = [double](@($dbRows | Measure-Object -Property input_tokens -Sum).Sum)
    $readInput = [double](@($readRows | Measure-Object -Property input_tokens -Sum).Sum)
    $nonReadInput = [double](@($nonReadRows | Measure-Object -Property input_tokens -Sum).Sum)
    $nonReadOutput = [double](@($nonReadRows | Measure-Object -Property output_tokens -Sum).Sum)
    $nonReadSaved = [double](@($nonReadRows | Measure-Object -Property saved_tokens -Sum).Sum)
    $nonReadPct = if ($nonReadInput -gt 0) { ($nonReadSaved / $nonReadInput) * 100.0 } else { 0.0 }
    $readShare = if ($inputTotal -gt 0) { ($readInput / $inputTotal) * 100.0 } else { 0.0 }
    $largestShare = if ($inputTotal -gt 0 -and $dbRows.Count -gt 0) {
        ([double](@($dbRows | Sort-Object input_tokens -Descending | Select-Object -First 1).input_tokens) / $inputTotal) * 100.0
    } else { 0.0 }

    [pscustomobject]@{
        name = "gain_gate_non_read"
        type = "gain-gate"
        db = $gainDb
        textPath = (Join-Path $Out "gain-gate.txt")
        jsonPath = (Join-Path $Out "gain-gate.json")
        totalCommands = $parsed.summary.total_commands
        avgSavingsPct = [math]::Round([double]$parsed.summary.avg_savings_pct, 2)
        rows = $dbRows
        nonReadInputTokens = [int64]$nonReadInput
        nonReadOutputTokens = [int64]$nonReadOutput
        nonReadSavedTokens = [int64]$nonReadSaved
        nonReadSavingsPct = [math]::Round($nonReadPct, 2)
        readInputSharePct = [math]::Round($readShare, 2)
        largestCommandInputSharePct = [math]::Round($largestShare, 2)
        pass = $parsed.summary.total_commands -ge 5 -and [double]$parsed.summary.avg_savings_pct -ge 70.0 -and $nonReadPct -ge 70.0 -and $readShare -lt 10.0 -and $largestShare -lt 90.0
    }
}

$rtkPath = Resolve-RtkPath $Rtk
$Corpus = if ($Corpus) { (Resolve-Path -LiteralPath $Corpus -ErrorAction Stop).Path } else { "" }
$out = New-OracleOutputDir $OutDir
$dbPath = Join-Path $out "oracle-history.db"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$fixtureDir = Join-Path $out "fixtures"
New-Item -ItemType Directory -Force -Path $fixtureDir | Out-Null
Set-Content -LiteralPath (Join-Path $fixtureDir "dash-pattern.log") -Encoding utf8 -Value @"
FAST_QUOTE_RETRY_REASON=pending
--reason|FAST_QUOTE_RETRY_REASON|pending
"@

$corpusSummary = $null
$corpusHookGate = $null
$historicalMatrix = $null
if (Test-Path -LiteralPath $Corpus) {
    $rows = Import-Csv -LiteralPath $Corpus
    $corpusSummary = [pscustomobject]@{
        path = $Corpus
        rows = @($rows).Count
        uniqueCommands = @($rows | Sort-Object Command -Unique).Count
        byShell = @($rows | Group-Object Shell | ForEach-Object { [pscustomobject]@{ name=$_.Name; count=$_.Count } })
        byCategory = @($rows | Group-Object Category | ForEach-Object { [pscustomobject]@{ name=$_.Name; count=$_.Count } })
        codexRows = @($rows | Where-Object { $_.Source -match "\\.codex" }).Count
        claudeRows = @($rows | Where-Object { $_.Source -match "\\.claude" }).Count
        credible = @($rows).Count -ge 2000 -and @($rows | Where-Object { $_.Shell -eq "powershell" }).Count -gt 0 -and @($rows | Where-Object { $_.Shell -eq "cmd" }).Count -gt 0 -and @($rows | Where-Object { $_.Shell -eq "bash_like" }).Count -gt 0
    }
    $corpusHookGate = Invoke-CorpusHookCheckGate -Rows $rows -RtkPath $rtkPath -Out $out -Limit 2200
    $historicalMatrix = Test-HistoricalMatrixCoverage -Rows $rows
}

$hookCases = @(
    Invoke-HookCase -Name "claude_bash_git_status" -Agent "claude" -Payload ([pscustomobject]@{ tool_name="Bash"; tool_input=[pscustomobject]@{ command="git status" } }) -Expected "rtk git status" -RtkPath $rtkPath -Out $out
    Invoke-HookCase -Name "claude_shell_git_status" -Agent "claude" -Payload ([pscustomobject]@{ tool_name="Shell"; tool_input=[pscustomobject]@{ command="git status" } }) -Expected "rtk git status" -RtkPath $rtkPath -Out $out
    Invoke-HookCase -Name "claude_powershell_get_content" -Agent "claude" -Payload ([pscustomobject]@{ tool_name="PowerShell"; tool_input=[pscustomobject]@{ command="Get-Content -LiteralPath Cargo.toml -TotalCount 3" } }) -Expected $null -RtkPath $rtkPath -Out $out
    Invoke-HookCase -Name "claude_powershell_instruction_passthrough" -Agent "claude" -Payload ([pscustomobject]@{ tool_name="PowerShell"; tool_input=[pscustomobject]@{ command="Get-Content -LiteralPath C:\validation\SKILL.md" } }) -Expected $null -RtkPath $rtkPath -Out $out
    Invoke-HookCase -Name "codex_bash_git_status" -Agent "codex" -Payload ([pscustomobject]@{ tool_name="Bash"; tool_input=[pscustomobject]@{ command="git status" } }) -Expected $null -RtkPath $rtkPath -Out $out
    Invoke-HookCase -Name "codex_shell_git_status" -Agent "codex" -Payload ([pscustomobject]@{ tool_name="Shell"; tool_input=[pscustomobject]@{ command="git status" } }) -Expected $null -RtkPath $rtkPath -Out $out
    Invoke-HookCase -Name "codex_powershell_get_content" -Agent "codex" -Payload ([pscustomobject]@{ tool_name="PowerShell"; tool_input=[pscustomobject]@{ command="Get-Content -LiteralPath Cargo.toml -TotalCount 3" } }) -Expected $null -RtkPath $rtkPath -Out $out
    Invoke-HookCase -Name "codex_powershell_instruction_passthrough" -Agent "codex" -Payload ([pscustomobject]@{ tool_name="PowerShell"; tool_input=[pscustomobject]@{ command="Get-Content -LiteralPath C:\validation\SKILL.md" } }) -Expected $null -RtkPath $rtkPath -Out $out
)

$badPayload = Invoke-HookCase -Name "codex_bad_payload_must_not_rewrite" -Agent "codex" -Payload ([pscustomobject]@{ tool="shell"; input=[pscustomobject]@{ command="git status" } }) -Expected $null -RtkPath $rtkPath -Out $out
$hookCases += $badPayload

$matrixCases = @(
    Invoke-HookCheckCase -Name "matrix_git_status" -Command "git status" -Expected "rtk git status" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_git_diff" -Command "git diff" -Expected "rtk git diff" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_git_log" -Command "git log -n 5" -Expected "rtk git log -n 5" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_git_show" -Command "git show" -Expected "rtk git show" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_git_add" -Command "git add ." -Expected "rtk git add ." -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_git_commit" -Command "git commit -m test" -Expected "rtk git commit -m test" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_git_push" -Command "git push" -Expected "rtk git push" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_git_pull" -Command "git pull" -Expected "rtk git pull" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_cat" -Command "cat package.json" -Expected "rtk read package.json" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_head" -Command "head -n 20 Cargo.toml" -Expected "rtk read Cargo.toml --max-lines 20" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_tail" -Command "tail -n 20 Cargo.toml" -Expected "rtk read Cargo.toml --tail-lines 20" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_ls" -Command "ls" -Expected "rtk ls" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_grep" -Command "grep -rn fn src" -Expected "rtk grep -rn fn src" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_rg" -Command "rg fn src" -Expected "rtk rg fn src" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_cargo_test" -Command "cargo test" -Expected "rtk cargo test" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_pytest" -Command "pytest" -Expected "rtk pytest" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_npm_test" -Command "npm test" -Expected "rtk npm test" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_npm_run" -Command "npm run build" -Expected "rtk npm run build" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_pnpm_install" -Command "pnpm install" -Expected "rtk pnpm install" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_get_content" -Command "Get-Content file.txt" -Expected "rtk read file.txt" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_gc" -Command "gc file.txt" -Expected "rtk read file.txt" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_get_child_item" -Command "Get-ChildItem" -Expected "rtk ls" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_gci" -Command "gci" -Expected "rtk ls" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_select_string" -Command "Select-String RTK Cargo.toml" -Expected "rtk grep RTK Cargo.toml" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_sls" -Command "sls RTK Cargo.toml" -Expected "rtk grep RTK Cargo.toml" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_cmd_dir" -Command "dir /b src" -Expected "rtk ls src" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_cmd_type" -Command "type Cargo.toml" -Expected "rtk read Cargo.toml" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_cmd_findstr" -Command "findstr RTK Cargo.toml" -Expected "rtk grep RTK Cargo.toml" -RtkPath $rtkPath -Out $out
    Invoke-HookCheckCase -Name "matrix_agent_instruction_passthrough" -Command "cat AGENTS.md" -Expected "" -RtkPath $rtkPath -Out $out -AllowNoRewrite
    Invoke-HookCheckCase -Name "matrix_get_content_instruction_passthrough" -Command "Get-Content C:\validation\SKILL.md" -Expected "" -RtkPath $rtkPath -Out $out -AllowNoRewrite
    Invoke-HookCheckCase -Name "matrix_gc_instruction_passthrough" -Command "gc RTK.md" -Expected "" -RtkPath $rtkPath -Out $out -AllowNoRewrite
    Invoke-HookCheckCase -Name "matrix_cmd_type_instruction_passthrough" -Command "type AGENTS.md" -Expected "" -RtkPath $rtkPath -Out $out -AllowNoRewrite
)

$instructionFiles = @("AGENTS.md", "SKILL.md", "CLAUDE.md", "RTK.md", "instructions.md")
foreach ($instructionFile in $instructionFiles) {
    $label = $instructionFile.ToLowerInvariant().Replace(".", "_")
    $matrixCases += @(
        Invoke-HookCheckCase -Name "matrix_instruction_${label}_cat_passthrough" -Command "cat $instructionFile" -Expected "" -RtkPath $rtkPath -Out $out -AllowNoRewrite
        Invoke-HookCheckCase -Name "matrix_instruction_${label}_get_content_passthrough" -Command "Get-Content $instructionFile" -Expected "" -RtkPath $rtkPath -Out $out -AllowNoRewrite
        Invoke-HookCheckCase -Name "matrix_instruction_${label}_gc_passthrough" -Command "gc $instructionFile" -Expected "" -RtkPath $rtkPath -Out $out -AllowNoRewrite
        Invoke-HookCheckCase -Name "matrix_instruction_${label}_type_passthrough" -Command "type $instructionFile" -Expected "" -RtkPath $rtkPath -Out $out -AllowNoRewrite
    )
}

$cases = @(
    Invoke-RawRtkCase -Name "grep_smart_case" -RawShell "powershell" -RawCommand "rg -S RTK_DISABLED src" -RtkArgs @("rg", "-S", "RTK_DISABLED", "src") -Cwd $repo -ExpectedExit 0 -MustContain @("RTK_DISABLED") -MinSavings 0.10 -RtkPath $rtkPath -DbPath $dbPath -Out $out
    Invoke-RawRtkCase -Name "grep_dash_pattern" -RawShell "powershell" -RawCommand "rg -n -- '--reason|FAST_QUOTE_RETRY_REASON|pending' '$fixtureDir'" -RtkArgs @("rg", "-n", "--", "--reason|FAST_QUOTE_RETRY_REASON|pending", $fixtureDir) -Cwd $repo -ExpectedExit 0 -MustContain @("FAST_QUOTE_RETRY_REASON") -MinSavings -0.20 -RtkPath $rtkPath -DbPath $dbPath -Out $out
    Invoke-RawRtkCase -Name "git_diff_check_clean" -RawShell "powershell" -RawCommand "git diff --check" -RtkArgs @("git", "diff", "--check") -Cwd $repo -ExpectedExit 0 -MustContain @() -MinSavings 0.00 -RtkPath $rtkPath -DbPath $dbPath -Out $out
    Invoke-RawRtkCase -Name "rtk_disabled_powershell_bypass" -RawShell "powershell" -RawCommand "git status --short" -RtkArgs @("hook", "check", "`$env:RTK_DISABLED='1'; git status --short") -Cwd $repo -ExpectedExit 0 -ExpectedRawExit 0 -ExpectedRtkExit 1 -MustContain @("RTK_DISABLED=1 detected") -MinSavings 0.00 -RtkPath $rtkPath -DbPath $dbPath -Out $out
    Invoke-RawRtkCase -Name "cmd_grep_case" -RawShell "cmd" -RawCommand "rg -n fn src" -RtkArgs @("rg", "fn", "src") -Cwd $repo -ExpectedExit 0 -MustContain @("matches") -MinSavings 0.70 -RtkPath $rtkPath -DbPath $dbPath -Out $out
    Invoke-RawRtkCase -Name "bash_grep_case" -RawShell "bash" -RawCommand "rg -n 'fn ' src" -RtkArgs @("rg", "fn ", "src") -Cwd $repo -ExpectedExit 0 -MustContain @("matches") -MinSavings 0.70 -RtkPath $rtkPath -DbPath $dbPath -Out $out
    Invoke-RawRtkCase -Name "grep_dashdash_multiple_paths" -RawShell "powershell" -RawCommand "rg -n -- 'Codex|Prompt-level' hooks docs/contributing/TECHNICAL.md" -RtkArgs @("rg", "-n", "--", "Codex|Prompt-level", "hooks", "docs/contributing/TECHNICAL.md") -Cwd $repo -ExpectedExit 0 -MustContain @("Codex CLI") -MinSavings 0.00 -RtkPath $rtkPath -DbPath $dbPath -Out $out
)

$zeroSaved = Get-ZeroSavedClassifications -Cases $cases
$databaseGuard = Test-DatabaseGuard -RtkPath $rtkPath -Out $out
$docsCheck = Test-DocsCodexHookConsistency -Repo $repo
$docsLinks = Test-MarkdownLocalLinks -Repo $repo
$initShow = Test-InitShowUsage -RtkPath $rtkPath -Repo $repo -Out $out
$binaryShadow = Test-ParentBinaryShadow -RtkPath $rtkPath -Repo $repo -Out $out
$cargoTest = Invoke-FullCargoTestGate -Repo $repo -Out $out
$gainGate = Invoke-GainGate -RtkPath $rtkPath -Repo $repo -Out $out

$all = @($hookCases + $matrixCases + $cases + $zeroSaved + $databaseGuard + $docsCheck + $docsLinks + $initShow + $binaryShadow + $cargoTest + $gainGate)
if ($null -ne $corpusHookGate) { $all += $corpusHookGate }
if ($null -ne $historicalMatrix) { $all += $historicalMatrix }
$summary = [pscustomobject]@{
    generatedAt = (Get-Date).ToString("o")
    rtk = $rtkPath
    binaryLastWrite = (Get-Item -LiteralPath $rtkPath).LastWriteTime.ToString("o")
    corpus = $corpusSummary
    corpusHookGate = $corpusHookGate
    historicalMatrix = $historicalMatrix
    zeroSaved = $zeroSaved
    databaseGuard = $databaseGuard
    docsCheck = $docsCheck
    docsLinks = $docsLinks
    initShow = $initShow
    binaryShadow = $binaryShadow
    cargoTest = $cargoTest
    gainGate = $gainGate
    totalCases = $all.Count
    passed = @($all | Where-Object pass).Count
    failed = @($all | Where-Object { -not $_.pass }).Count
    allPassed = @($all | Where-Object { -not $_.pass }).Count -eq 0
    cases = $all
    outDir = $out
}

Write-Json -Path (Join-Path $out "summary.json") -Value $summary
Write-Json -Path (Join-Path $out "cases.json") -Value $all
$summary | ConvertTo-Json -Depth 30
if (-not $summary.allPassed) { exit 1 }
