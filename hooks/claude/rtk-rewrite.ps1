# RTK PreToolUse hook for Claude Code on Windows (native PowerShell).
# Counterpart of hooks/claude/rtk-rewrite.sh for users who run Claude Code on
# native Windows (no WSL / Unix shell). Requires: rtk >= 0.23.0 in PATH.
#
# Why a dedicated implementation instead of a thin `& rtk rewrite $cmd` wrapper:
# Windows PowerShell 5.1 strips embedded double-quotes when a string is passed as
# a native-command argument, so `& rtk rewrite $cmd` corrupts the command before
# rtk sees it. For example the input
#     git diff; echo "a (b)"
# reaches rtk as
#     git diff; echo a (b)
# and the rewritten result `... echo a (b)` is invalid shell (unbalanced "("),
# which breaks the very command the hook was meant to optimize. (rtk's own output
# is correctly quoted; the corruption happens purely in PowerShell argument
# passing.) This script therefore builds the child process command line itself
# using MSVCRT / CommandLineToArgvW quoting, so the exact command reaches
# `rtk rewrite` untouched on both Windows PowerShell 5.1 and PowerShell 7+.
#
# Install (PreToolUse, matcher "Bash"):
#   powershell.exe -ExecutionPolicy Bypass -NoProfile -File <path>\rtk-rewrite.ps1
#
# Exit code protocol from `rtk rewrite` (identical to rtk-rewrite.sh):
#   0 + stdout = auto-allow the rewrite
#   1          = no equivalent, passthrough unchanged
#   2          = deny rule matched, passthrough (Claude Code native deny handles it)
#   3 + stdout = ask rule matched, rewrite but do NOT auto-allow

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

$rtk = Get-Command rtk -ErrorAction SilentlyContinue
if (-not $rtk) {
    [Console]::Error.WriteLine("[rtk] WARNING: rtk not found in PATH. Hook is a no-op.")
    exit 0
}

$json = [Console]::In.ReadToEnd()
if (-not $json) { exit 0 }

try {
    $data = $json | ConvertFrom-Json -ErrorAction Stop
} catch {
    exit 0
}

$cmd = $data.tool_input.command
if (-not $cmd) { exit 0 }

function ConvertTo-NativeArg([string]$s) {
    # Quote a single argument per the MSVCRT / CommandLineToArgvW rules so it
    # reaches the child process intact (PowerShell would otherwise mangle quotes).
    if ($s.Length -eq 0) { return '""' }
    if ($s -notmatch '[ \t"]') { return $s }
    $sb = [System.Text.StringBuilder]::new()
    [void]$sb.Append('"')
    $bs = 0
    foreach ($ch in $s.ToCharArray()) {
        if ($ch -eq '\') {
            $bs++
        }
        elseif ($ch -eq '"') {
            [void]$sb.Append('\' * ($bs * 2 + 1))
            [void]$sb.Append('"')
            $bs = 0
        }
        else {
            if ($bs -gt 0) { [void]$sb.Append('\' * $bs); $bs = 0 }
            [void]$sb.Append($ch)
        }
    }
    [void]$sb.Append('\' * ($bs * 2))
    [void]$sb.Append('"')
    return $sb.ToString()
}

# Invoke `rtk rewrite <cmd>` with a hand-quoted command line. Any failure falls
# through to passthrough (exit 0) so the hook can never break command execution.
try {
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $rtk.Source
    $psi.Arguments = "rewrite " + (ConvertTo-NativeArg $cmd)
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $proc = [System.Diagnostics.Process]::Start($psi)
    $rewritten = $proc.StandardOutput.ReadToEnd()
    [void]$proc.StandardError.ReadToEnd()
    $proc.WaitForExit()
    $exitCode = $proc.ExitCode
    $rewritten = $rewritten.Replace("`r`n", "`n").TrimEnd("`n")
} catch {
    exit 0
}

function Emit-Update {
    param([string]$NewCmd, [bool]$AutoAllow)
    $hookOut = @{
        hookEventName = "PreToolUse"
        updatedInput  = @{ command = $NewCmd }
    }
    if ($AutoAllow) {
        $hookOut.permissionDecision       = "allow"
        $hookOut.permissionDecisionReason = "RTK auto-rewrite"
    }
    @{ hookSpecificOutput = $hookOut } | ConvertTo-Json -Compress -Depth 10
}

switch ($exitCode) {
    0 {
        if ($rewritten -and $rewritten -ne $cmd) {
            Emit-Update -NewCmd $rewritten -AutoAllow $true
        }
    }
    3 {
        if ($rewritten -and $rewritten -ne $cmd) {
            Emit-Update -NewCmd $rewritten -AutoAllow $false
        }
    }
    default {
        # 1 / 2 / unknown — passthrough
    }
}

exit 0
