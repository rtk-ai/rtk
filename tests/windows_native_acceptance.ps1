$ErrorActionPreference = "Stop"

$Repo = (Resolve-Path ".").Path
$RtkExe = Join-Path $Repo "target\debug\rtk.exe"
$FixtureRoot = Join-Path $Repo "tests\fixtures\windows-native"
$Scratch = Join-Path $Repo "target\windows-native-acceptance"
$Results = New-Object System.Collections.Generic.List[object]

if (-not (Test-Path -LiteralPath $RtkExe)) {
    throw "Debug exe not found: $RtkExe"
}
if (-not (Test-Path -LiteralPath $FixtureRoot)) {
    throw "Fixture root not found: $FixtureRoot"
}

if (Test-Path -LiteralPath $Scratch) {
    Remove-Item -LiteralPath $Scratch -Recurse -Force
}
New-Item -ItemType Directory -Path $Scratch | Out-Null

$QuoteFile = Join-Path $FixtureRoot "quote-and-wildcard.txt"
$ContextFile = Join-Path $FixtureRoot "grep-context.txt"
$HeadTailFile = Join-Path $FixtureRoot "head-tail.txt"
$EmptyFile = Join-Path $Scratch "zero-byte.txt"
$UnicodeArg = "unicode-" + [char]0x6D4B + [char]0x8BD5
$UnicodeFile = (Get-ChildItem -LiteralPath $FixtureRoot -Filter "unicode-*.txt" | Select-Object -First 1).FullName
$Ps1Probe = Join-Path $FixtureRoot "scripts\argv-probe.ps1"
$CmdProbe = Join-Path $FixtureRoot "scripts\argv-probe.cmd"
$TouchFile = Join-Path $Scratch "touch created file.txt"
$TouchExisting = Join-Path $Scratch "touch existing file.txt"
$MkdirTarget = Join-Path $Scratch "mkdir target\a b\c"
$ExistingDir = Join-Path $Scratch "already-there"
$ExistingFile = Join-Path $Scratch "file-blocks-mkdir.txt"

Set-Content -LiteralPath $TouchExisting -Encoding UTF8 -Value "keep me"
New-Item -ItemType Directory -Path $ExistingDir | Out-Null
Set-Content -LiteralPath $ExistingFile -Encoding UTF8 -Value "not a directory"
New-Item -ItemType File -Path $EmptyFile | Out-Null

function ConvertTo-WindowsCommandLineArg {
    param([string]$Value)

    if ($Value.Length -gt 0 -and $Value -notmatch '[\s"]') {
        return $Value
    }

    $builder = New-Object System.Text.StringBuilder
    [void]$builder.Append('"')
    $slashes = 0
    foreach ($ch in $Value.ToCharArray()) {
        if ($ch -eq '\') {
            $slashes += 1
            continue
        }
        if ($ch -eq '"') {
            [void]$builder.Append('\' * (($slashes * 2) + 1))
            [void]$builder.Append('"')
            $slashes = 0
            continue
        }
        if ($slashes -gt 0) {
            [void]$builder.Append('\' * $slashes)
            $slashes = 0
        }
        [void]$builder.Append($ch)
    }
    if ($slashes -gt 0) {
        [void]$builder.Append('\' * ($slashes * 2))
    }
    [void]$builder.Append('"')
    $builder.ToString()
}

function Invoke-RtkExe {
    param(
        [Parameter(Mandatory = $true)][string[]] $Argv,
        [string] $Stdin = $null,
        [hashtable] $Env = @{}
    )

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $RtkExe
    $psi.WorkingDirectory = $Repo
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.RedirectStandardInput = $null -ne $Stdin
    $psi.Arguments = (($Argv | ForEach-Object { ConvertTo-WindowsCommandLineArg $_ }) -join " ")
    foreach ($key in $Env.Keys) {
        $psi.EnvironmentVariables[$key] = [string]$Env[$key]
    }

    $process = [System.Diagnostics.Process]::Start($psi)
    if ($null -ne $Stdin) {
        $process.StandardInput.Write($Stdin)
        $process.StandardInput.Close()
    }
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()

    [pscustomobject]@{
        Code = $process.ExitCode
        Stdout = $stdout
        Stderr = $stderr
        Output = ($stdout + $stderr)
    }
}

function Add-Result {
    param([string]$Name, [string]$Status, [string]$Detail = "")
    $Results.Add([pscustomobject]@{ Name = $Name; Status = $Status; Detail = $Detail }) | Out-Null
}

function Check {
    param(
        [string] $Name,
        [string[]] $Argv,
        [string[]] $Needles = @(),
        [string[]] $Absent = @(),
        [int] $ExpectedCode = 0,
        [string] $Stdin = $null,
        [hashtable] $Env = @{}
    )

    $result = Invoke-RtkExe -Argv $Argv -Stdin $Stdin -Env $Env
    if ($result.Code -ne $ExpectedCode) {
        Add-Result $Name "FAIL" "argv=[$($Argv -join ' | ')]; exit=$($result.Code), expected=$ExpectedCode; output=$($result.Output.Trim())"
        return
    }
    foreach ($needle in $Needles) {
        if (-not $result.Output.Contains($needle)) {
            Add-Result $Name "FAIL" "missing '$needle'; output=$($result.Output.Trim())"
            return
        }
    }
    foreach ($needle in $Absent) {
        if ($result.Output.Contains($needle)) {
            Add-Result $Name "FAIL" "unexpected '$needle'; output=$($result.Output.Trim())"
            return
        }
    }
    Add-Result $Name "PASS"
}

function CheckStdoutExact {
    param(
        [string] $Name,
        [string[]] $Argv,
        [string] $ExpectedStdout,
        [int] $ExpectedCode = 0,
        [string] $Stdin = $null
    )

    $result = Invoke-RtkExe -Argv $Argv -Stdin $Stdin
    if ($result.Code -ne $ExpectedCode) {
        Add-Result $Name "FAIL" "exit=$($result.Code), expected=$ExpectedCode; output=$($result.Output.Trim())"
        return
    }
    if ($result.Stdout -ne $ExpectedStdout) {
        Add-Result $Name "FAIL" "stdout length=$($result.Stdout.Length), expected length=$($ExpectedStdout.Length); stdout=$($result.Stdout.Replace("`r", "\r").Replace("`n", "\n"))"
        return
    }
    Add-Result $Name "PASS"
}

function CheckRewrite {
    param(
        [string] $Name,
        [string] $Raw,
        [string] $Expected = $null,
        [switch] $NoRewrite
    )

    $expectedCode = if ($NoRewrite) { 1 } else { 3 }
    $result = Invoke-RtkExe -Argv @("rewrite", $Raw)
    if ($result.Code -ne $expectedCode) {
        Add-Result $Name "FAIL" "rewrite exit=$($result.Code), expected=$expectedCode; output=$($result.Output.Trim())"
        return
    }
    if (-not $NoRewrite -and -not $result.Stdout.Trim().Contains($Expected)) {
        Add-Result $Name "FAIL" "rewrite missing '$Expected'; output=$($result.Output.Trim())"
        return
    }
    if ($NoRewrite -and $result.Output.Trim().Length -ne 0) {
        Add-Result $Name "FAIL" "expected empty no-rewrite output; output=$($result.Output.Trim())"
        return
    }
    Add-Result $Name "PASS"
}

function CheckSkipUnlessCommand {
    param([string] $CommandName, [scriptblock] $Block)
    $found = Get-Command $CommandName -ErrorAction SilentlyContinue
    if ($null -eq $found) {
        Add-Result "$CommandName transport" "SKIP" "$CommandName not available on PATH"
        return
    }
    & $Block
}

function ConvertTo-PowerShellSingleQuotedLiteral {
    param([string] $Value)
    "'" + $Value.Replace("'", "''") + "'"
}

Check -Name "Get-Content path spaces/brackets/quotes" -Argv @("Get-Content", $QuoteFile) -Needles @('"quoted value"', 'literal [brackets]', "can't stop argv")
Check -Name "Get-Content Encoding utf8 prefix" -Argv @("Get-Content", "-Encoding", "utf8", $QuoteFile) -Needles @("alpha one")
Check -Name "Get-Content Encoding utf8 suffix" -Argv @("Get-Content", $QuoteFile, "-Encoding", "utf8") -Needles @("epsilon apostrophe")
Check -Name "Get-Content unsupported Raw transports" -Argv @("Get-Content", "-Raw", $QuoteFile) -Needles @('"quoted value"', "can't stop argv")
Check -Name "Get-Content dash is not stdin" -Argv @("Get-Content", "-") -ExpectedCode 2 -Needles @("ambiguous")
Check -Name "Get-Content unicode path" -Argv @("Get-Content", $UnicodeFile) -Needles @("unicode payload")

Check -Name "Select-String quoted pattern" -Argv @("Select-String", "-Pattern", '"quoted value"', "-Path", $QuoteFile) -Needles @('"quoted value"')
Check -Name "Select-String positional pattern path" -Argv @("Select-String", "alpha", $QuoteFile) -Needles @("alpha one")
Check -Name "Select-String CaseSensitive no match exits 1" -Argv @("Select-String", "-CaseSensitive", "-Pattern", "ALPHA", "-Path", $QuoteFile) -ExpectedCode 1
Check -Name "Select-String SimpleMatch wildcard chars" -Argv @("Select-String", "-SimpleMatch", "-Pattern", "star * question ?", "-Path", $QuoteFile) -Needles @("star * question ?")
Check -Name "Select-String Context transports" -Argv @("Select-String", "-Context", "1", "beta", $QuoteFile) -Needles @('"quoted value"', "alpha one")

Check -Name "Get-ChildItem no args" -Argv @("Get-ChildItem") -Needles @("Cargo.toml")
Check -Name "Get-ChildItem positional path" -Argv @("Get-ChildItem", $FixtureRoot) -Needles @("quote-and-wildcard.txt")
Check -Name "Get-ChildItem Path" -Argv @("Get-ChildItem", "-Path", $FixtureRoot) -Needles @("grep-context.txt")
Check -Name "Get-ChildItem LiteralPath" -Argv @("Get-ChildItem", "-LiteralPath", $FixtureRoot) -Needles @("unicode-")
Check -Name "Get-ChildItem Force" -Argv @("Get-ChildItem", "-Force", $FixtureRoot) -Needles @("head-tail.txt")
Check -Name "Get-ChildItem wildcard path transports" -Argv @("Get-ChildItem", "*.rs")
Check -Name "Get-ChildItem recurse filter transports" -Argv @("Get-ChildItem", "-Recurse", "-Filter", "*.txt", $FixtureRoot) -Needles @("quote-and-wildcard.txt")

Check -Name "Get-Command application cargo" -Argv @("Get-Command", "-CommandType", "Application", "cargo") -Needles @("cargo")
Check -Name "Get-Command application name cargo" -Argv @("Get-Command", "-CommandType", "Application", "-Name", "cargo") -Needles @("cargo")
Check -Name "Get-Command bare transports" -Argv @("Get-Command", "cargo") -Needles @("cargo")
Check -Name "Get-Command syntax transports" -Argv @("Get-Command", "-Syntax", "cargo") -Needles @("cargo")
Check -Name "which cargo" -Argv @("which", "cargo") -Needles @("cargo")
Check -Name "which missing returns 1" -Argv @("which", "rtk-definitely-missing-command-for-acceptance") -ExpectedCode 1 -Needles @("not found")
Check -Name "which path-like name is not found" -Argv @("which", ".\cargo") -ExpectedCode 1 -Needles @("not found")

Check -Name "head default 10 lines" -Argv @("head", $HeadTailFile) -Needles @("line 01", "line 10") -Absent @("line 11", "omitted")
Check -Name "head -n 2" -Argv @("head", "-n", "2", $HeadTailFile) -Needles @("line 01", "line 02") -Absent @("line 03", "omitted")
Check -Name "head compact -3" -Argv @("head", "-3", $HeadTailFile) -Needles @("line 03") -Absent @("line 04", "omitted")
CheckStdoutExact -Name "head empty file exact zero stdout" -Argv @("head", $EmptyFile) -ExpectedStdout ""
Check -Name "head rejects multiple files" -Argv @("head", $HeadTailFile, $QuoteFile) -ExpectedCode 1 -Needles @("multiple files")
Check -Name "head stdin stops after N lines" -Argv @("head", "-n", "2", "-") -Stdin "stdin 1`nstdin 2`nstdin 3`n" -Needles @("stdin 1", "stdin 2") -Absent @("stdin 3", "omitted")

Check -Name "tail default 10 lines" -Argv @("tail", $HeadTailFile) -Needles @("line 03", "line 12") -Absent @("line 02", "omitted")
Check -Name "tail -n 2" -Argv @("tail", "-n", "2", $HeadTailFile) -Needles @("line 11", "line 12") -Absent @("line 10", "omitted")
CheckStdoutExact -Name "tail empty file exact zero stdout" -Argv @("tail", $EmptyFile) -ExpectedStdout ""
Check -Name "tail -f rejected" -Argv @("tail", "-f", $HeadTailFile) -ExpectedCode 1 -Needles @("unsupported")
Check -Name "tail rejects multiple files" -Argv @("tail", $HeadTailFile, $QuoteFile) -ExpectedCode 1 -Needles @("multiple files")
Check -Name "tail stdin bounded output" -Argv @("tail", "-n", "2", "-") -Stdin "stdin 1`nstdin 2`nstdin 3`n" -Needles @("stdin 2", "stdin 3") -Absent @("stdin 1", "omitted")

Check -Name "pwd" -Argv @("pwd") -Needles @($Repo)
Check -Name "touch creates file with spaces" -Argv @("touch", $TouchFile)
if (Test-Path -LiteralPath $TouchFile) { Add-Result "touch created file exists" "PASS" } else { Add-Result "touch created file exists" "FAIL" "missing $TouchFile" }
$beforeTouch = Get-Content -LiteralPath $TouchExisting -Raw
Start-Sleep -Milliseconds 20
Check -Name "touch preserves existing content" -Argv @("touch", $TouchExisting)
$afterTouch = Get-Content -LiteralPath $TouchExisting -Raw
if ($beforeTouch -eq $afterTouch) { Add-Result "touch existing content preserved" "PASS" } else { Add-Result "touch existing content preserved" "FAIL" "content changed" }
Check -Name "touch rejects directory" -Argv @("touch", $ExistingDir) -ExpectedCode 1 -Needles @("directory")
Check -Name "mkdir -p nested spaces" -Argv @("mkdir", "-p", $MkdirTarget)
if (Test-Path -LiteralPath $MkdirTarget) { Add-Result "mkdir -p target exists" "PASS" } else { Add-Result "mkdir -p target exists" "FAIL" "missing $MkdirTarget" }
Check -Name "mkdir -p existing directory succeeds" -Argv @("mkdir", "-p", $ExistingDir)
Check -Name "mkdir without -p rejected" -Argv @("mkdir", (Join-Path $Scratch "no-p")) -ExpectedCode 2 -Needles @("-p")
Check -Name "mkdir -p existing file fails" -Argv @("mkdir", "-p", $ExistingFile) -ExpectedCode 1

Check -Name "ls fixture root" -Argv @("ls", $FixtureRoot) -Needles @("quote-and-wildcard.txt")
Check -Name "tree fixture root" -Argv @("tree", $FixtureRoot) -Needles @("argv-probe.ps1")
Check -Name "wc -l quote file" -Argv @("wc", "-l", $QuoteFile) -Needles @("5")
Check -Name "ps" -Argv @("ps") -Needles @("PID")
Check -Name "df" -Argv @("df") -Needles @("Filesystem")
Check -Name "du -s fixture root" -Argv @("du", "-s", $FixtureRoot) -Needles @("windows-native")

$fallbackPath = (Resolve-Path (Join-Path $Repo "target\debug")).Path
Check -Name "native grep fallback context/separator" -Argv @("grep", "-A1", "match", $ContextFile) -Needles @("match first", "after first", "--", "match second") -Env @{ PATH = $fallbackPath }
Check -Name "native grep fallback no separator without context" -Argv @("grep", "match", $ContextFile) -Needles @("match first", "match second") -Absent @("--") -Env @{ PATH = $fallbackPath }

Check -Name "powershell direct argv quoted literal" -Argv @("powershell", "-NoProfile", "-Command", "Write-Output 'hello world'") -Needles @("hello world")
Check -Name "powershell env literal" -Argv @("powershell", "-NoProfile", "-Command", 'Write-Output $env:TEMP') -Needles @($env:TEMP)
Check -Name "powershell object pipeline explicit host" -Argv @("powershell", "-NoProfile", "-Command", 'Get-ChildItem .\src | Where-Object { $_.Name -match "core" } | Select-Object -ExpandProperty Name') -Needles @("core")
$normalQuotePath = (Resolve-Path $QuoteFile).Path
$extendedQuotePath = "\\?\" + $normalQuotePath
Check -Name "powershell extended-length path transport" -Argv @("powershell", "-NoProfile", "-Command", "Get-Content -LiteralPath $(ConvertTo-PowerShellSingleQuotedLiteral $extendedQuotePath)") -Needles @('"quoted value"', "literal [brackets]")
$drive = $normalQuotePath.Substring(0, 1)
$uncQuotePath = "\\localhost\$drive$" + $normalQuotePath.Substring(2)
if (Test-Path -LiteralPath $uncQuotePath) {
    Check -Name "powershell UNC path transport" -Argv @("powershell", "-NoProfile", "-Command", "Get-Content -LiteralPath $(ConvertTo-PowerShellSingleQuotedLiteral $uncQuotePath)") -Needles @('"quoted value"', "literal [brackets]")
} else {
    Add-Result "powershell UNC path transport" "SKIP" "localhost admin share unavailable"
}
$longPattern = "a" * 9000
Check -Name "implicit PowerShell transport rejects oversized source" -Argv @("Select-String", "-Context", "1", "-Pattern", $longPattern, "-Path", $QuoteFile) -ExpectedCode 2 -Needles @("too large", ".ps1", "-File")
Check -Name "cmd fallback quoted literal" -Argv @("cmd", "/c", "echo hello world") -Needles @("hello world")
CheckSkipUnlessCommand "pwsh" {
    Check -Name "pwsh transport version" -Argv @("pwsh", "-NoProfile", "-Command", '$PSVersionTable.PSVersion.Major') -ExpectedCode 0
}

Check -Name "ps1 argv probe spaces unicode quotes" -Argv @($Ps1Probe, "hello world", $UnicodeArg, 'quote "inside"') -Needles @("arg0=hello world", "arg1=$UnicodeArg", 'arg2=quote "inside"')
Check -Name "cmd argv probe safe spaces" -Argv @($CmdProbe, "hello world", "plain") -Needles @("arg0=hello world", "arg1=plain")
Check -Name "cmd argv probe rejects metachar" -Argv @($CmdProbe, "bad&arg") -ExpectedCode 2 -Needles @("cmd")
Check -Name "unknown scriptblock-like fails closed" -Argv @("Where-Object", "{ `$_.Name -match 'src' }") -ExpectedCode 2 -Needles @("ambiguous Windows command")

CheckRewrite -Name "rewrite Get-Content basic" -Raw "Get-Content tests/fixtures/windows-native/quote-and-wildcard.txt" -Expected "rtk read"
CheckRewrite -Name "rewrite Get-Content encoding suffix" -Raw "Get-Content tests/fixtures/windows-native/quote-and-wildcard.txt -Encoding utf8" -Expected "rtk read"
CheckRewrite -Name "rewrite Get-Content Raw none" -Raw "Get-Content -Raw tests/fixtures/windows-native/quote-and-wildcard.txt" -NoRewrite
CheckRewrite -Name "rewrite Select-String named" -Raw "Select-String -Pattern alpha -Path tests/fixtures/windows-native/quote-and-wildcard.txt" -Expected "rtk grep"
CheckRewrite -Name "rewrite Select-String context none" -Raw "Select-String -Context 2 alpha tests/fixtures/windows-native/quote-and-wildcard.txt" -NoRewrite
CheckRewrite -Name "rewrite Get-ChildItem path" -Raw "Get-ChildItem tests/fixtures/windows-native" -Expected "rtk ls"
CheckRewrite -Name "rewrite Get-ChildItem wildcard none" -Raw "Get-ChildItem *.rs" -NoRewrite
CheckRewrite -Name "rewrite Get-ChildItem recurse none" -Raw "Get-ChildItem -Recurse -Filter *.rs tests/fixtures/windows-native" -NoRewrite
CheckRewrite -Name "rewrite Get-Command app" -Raw "Get-Command -CommandType Application cargo" -Expected "rtk which cargo"
CheckRewrite -Name "rewrite Get-Command bare none" -Raw "Get-Command cargo" -NoRewrite
CheckRewrite -Name "rewrite Get-Command syntax none" -Raw "Get-Command -Syntax cargo" -NoRewrite
CheckRewrite -Name "rewrite where.exe none" -Raw "where.exe cargo" -NoRewrite
CheckRewrite -Name "rewrite which" -Raw "which cargo" -Expected "rtk which cargo"
CheckRewrite -Name "rewrite head exact" -Raw "head -n 2 tests/fixtures/windows-native/head-tail.txt" -Expected "rtk head"
CheckRewrite -Name "rewrite tail -f none" -Raw "tail -f tests/fixtures/windows-native/head-tail.txt" -NoRewrite

$Results | Format-Table -AutoSize | Out-String | Write-Output

$failed = @($Results | Where-Object { $_.Status -eq "FAIL" })
$skipped = @($Results | Where-Object { $_.Status -eq "SKIP" })
if ($failed.Count -gt 0) {
    Write-Output "FAILED: $($failed.Count) acceptance checks; SKIPPED: $($skipped.Count)"
    exit 1
}

Write-Output "PASSED: $($Results.Count - $skipped.Count) acceptance checks; SKIPPED: $($skipped.Count)"
