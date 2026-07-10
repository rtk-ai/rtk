# RTK - Rust Token Killer (Codex CLI)

RTK is the default command wrapper for this environment. It runs supported
commands through token-optimized native handlers and keeps command output small
enough for assistant workflows.

## Rule

Always prefix shell commands with `rtk`.

Prefer native RTK mode for commands that RTK supports:

```bash
rtk git status
rtk cargo test
rtk rg "pattern"
rtk npm run build
rtk pytest -q
```

Use `rtk proxy <cmd>` only when raw execution is required:

- installers and updaters, such as `winget`, `msiexec`, or setup programs
- authentication and device-login flows, such as `gh auth login`
- commands whose exact raw output is required for debugging
- commands not safely handled by an RTK native command yet
- complex shell invocations that should not be compressed or rewritten

`rtk proxy` still runs through RTK for tracking, but it does not apply output
filtering or command-specific compression.

## Supported Native Commands

Current RTK native/compact commands include:

```text
ls tree read head tail smart git gh glab aws psql pnpm err test json deps env
find which pwd touch mkdir diff log dotnet docker kubectl oc summary grep rg
wget wc ps df du gain cc-economics config jest vitest prisma tsc next lint
prettier format playwright cargo npm npx curl discover session telemetry learn
pipe trust untrust verify ruff pytest mypy rake rubocop rspec pip uv go gt
golangci-lint gradlew mvn hook-audit rewrite hook
```

For these commands, prefer `rtk <command> ...` unless there is a specific need
for raw, uncompressed output.

## Windows PowerShell

On Windows, avoid nested PowerShell `-Command "..."` when the command contains
PowerShell variables, `$env:` references, quotes, script blocks, or `$_`.
The outer PowerShell can expand or rewrite those tokens before RTK receives the
command.

Prefer script files for complex PowerShell:

```powershell
rtk proxy powershell -NoProfile -ExecutionPolicy Bypass -File scripts\example.ps1
```

For short and simple commands, native PowerShell is acceptable:

```powershell
rtk powershell -NoProfile -Command "Get-Location"
```

When PowerShell string interpolation is needed, write valid PowerShell
explicitly:

```powershell
"${Tag}:"
[Environment]::GetEnvironmentVariable("FOO", "Process")
$_.Exception.Message
```

Do not expect RTK to repair PowerShell script-language mistakes such as:

```powershell
"$Tag:"
"$env:FOO"          # risky inside an outer double-quoted PowerShell command
"$_.Exception"      # risky inside an outer double-quoted PowerShell command
```

RTK's responsibility is to avoid introducing Windows quoting or escaping errors
while dispatching commands. It does not rewrite, repair, or reinterpret the
semantic content of PowerShell scripts.

If RTK reports an ambiguous Windows command, follow the hint it prints. Use an
explicit native command, `rtk run -c ...`, `rtk powershell ...`, or an explicit
executable path depending on the situation.

## Unix and Linux

This instruction file does not change RTK behavior on Unix or Linux. It only
guides how the assistant should call RTK.

On Unix/Linux, continue using the normal RTK forms:

```bash
rtk git status
rtk cargo test
rtk rg "pattern"
rtk run -c 'echo "$HOME"'
```

The Windows PowerShell cautions above are Windows-specific. They should not be
treated as a reason to avoid native RTK commands on Unix/Linux.

## Meta Commands

```bash
rtk gain            # Token savings analytics
rtk gain --history  # Recent command savings history
rtk proxy <cmd>     # Raw execution through RTK tracking, without filtering
rtk run -c <cmd>    # Raw shell command execution
```

## Verification

```bash
rtk --version
rtk --help
rtk gain
rtk which rtk
```
