# Smoke test for hooks/claude/rtk-rewrite.ps1.
# Drives the hook exactly as Claude Code does: spawns it as a separate process
# with the tool_input JSON on stdin and prints the emitted hookSpecificOutput.
# Requires rtk >= 0.23.0 in PATH. Run:  powershell -File test-rtk-rewrite.ps1
$hook = Join-Path $PSScriptRoot "rtk-rewrite.ps1"
$cases = @(
    'git diff; echo "a (b)"',                  # quotes+parens must survive
    'git status && grep -n "def foo(" .',      # multiple rewrites + quoted paren
    'git commit -m "fix(ci): pin (1M)"',       # parens inside a commit message
    'git log --oneline | grep "fix(x)"',       # quoted paren after a pipe
    'ls -la',                                   # plain rewrite, no quotes
    'echo "plain text, no rewrite target"'     # passthrough (no rtk equivalent)
)
foreach ($c in $cases) {
    $json = (@{ tool_input = @{ command = $c } } | ConvertTo-Json -Compress)
    $out = ($json | powershell.exe -ExecutionPolicy Bypass -NoProfile -File $hook) -join "`n"
    Write-Output ("IN : " + $c)
    Write-Output ("OUT: " + $out)
    Write-Output ""
}
