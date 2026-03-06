# Windows Guide

This guide explains how to use RTK on native Windows without relying on WSL.

## What works on Windows

RTK itself can run on native Windows as a normal CLI binary.

That includes:

- building RTK from source with Cargo
- running the compiled `rtk.exe`
- using core commands such as:
  - `rtk --version`
  - `rtk gain`
  - `rtk ls .`
  - `rtk read Cargo.toml`
  - `rtk rewrite "git status"`

The project also publishes a Windows release artifact:

- `rtk-x86_64-pc-windows-msvc.zip`

## What does not have a full native-Windows story yet

Some parts of this repository are still Unix-first.

Treat these as limited or unsupported on plain PowerShell/cmd unless they are explicitly ported:

- `install.sh`
- the Bash hook file: `hooks/rtk-rewrite.sh`
- hook-first global Claude setup via `rtk init -g`
- Bash helper scripts in `scripts/`

In practice, the safe rule is:

- the RTK binary itself works on Windows
- the surrounding Bash-based tooling should not be assumed to work natively

## Make a sane setup decision first

You do not need to install every Unix-like tool just to use RTK on Windows.

A good rule of thumb is:

- If you only want to run `rtk.exe` itself:
  - PowerShell is enough
- If you want to follow Bash-heavy repo docs and run Bash scripts from `scripts/`:
  - install Git for Windows so you have Git Bash
- If you want common Unix-style search tools that many developer docs assume:
  - install `ripgrep`
  - optionally install real `curl`

## When Git Bash is worth installing

Install Git Bash if you expect to do any of the following on Windows:

- run files in `scripts/` that end in `.sh`
- execute commands copied from docs that use:
  - `bash`
  - `sh`
  - `cp`
  - `chmod`
  - `grep`
  - `source`
- work with the Unix hook file:
  - `hooks/rtk-rewrite.sh`

Git Bash is the simplest way to make many of those instructions behave the way the repo expects.

### Install Git Bash

Check whether you already have a Windows package manager:

```powershell
winget --version
choco --version
```

Install Git for Windows with one of these:

```powershell
winget install Git.Git
```

or:

```powershell
choco install git
```

After installing:

1. Close PowerShell.
2. Open a new terminal.
3. Verify:

```powershell
bash --version
where bash
git --version
```

Typical Git Bash paths:

- `C:\Program Files\Git\bin\bash.exe`
- `C:\Program Files\Git\usr\bin\bash.exe`

Important on Windows:

- the plain command `bash` may resolve to the WSL launcher instead of Git Bash
- if you specifically want Git for Windows behavior, prefer the Git Bash executable from the Git install directory

## Useful Windows replacements for common Unix command names

If you are staying in PowerShell, these replacements are usually the least confusing:

| Unix-style command | PowerShell-native choice | Notes |
|--------------------|--------------------------|-------|
| `which rtk` | `Get-Command rtk` | Best replacement for command lookup |
| `cp a b` | `Copy-Item a b` | Use `-Force` if needed |
| `cat file` | `Get-Content file` | Reads file content |
| `grep text file` | `Select-String text file` | Good built-in text search |
| `head -n 20 file` | `Get-Content file -TotalCount 20` | Quick first lines |
| `ls` | `Get-ChildItem` | `ls` is also an alias in PowerShell |
| `pwd` | `Get-Location` | Shows current directory |
| `rm file` | `Remove-Item file` | Use carefully |

Examples:

```powershell
Get-Command rtk
Copy-Item $HOME\.claude\settings.json.bak $HOME\.claude\settings.json -Force
Select-String "rtk" .\CLAUDE.md
Get-Content .\Cargo.toml -TotalCount 20
```

## Tools that are often worth installing on Windows

### ripgrep

This repo and many developer docs prefer `rg`.

Install with:

```powershell
winget install BurntSushi.ripgrep.MSVC
```

or:

```powershell
choco install ripgrep
```

Verify:

```powershell
rg --version
```

### curl

Some docs use `curl`, but PowerShell users should be careful because shell behavior varies by command name and environment.

Good options:

- use `Invoke-WebRequest` if you want a PowerShell-native command
- use `curl.exe` if you want the real curl executable

Examples:

```powershell
Invoke-WebRequest https://example.com
curl.exe --version
```

### wget

If a script or doc expects real `wget`, install it explicitly or use a PowerShell-native download command instead.

PowerShell-native example:

```powershell
Invoke-WebRequest https://example.com/file -OutFile .\file
```

## Which docs should you follow in PowerShell vs Git Bash

Use PowerShell-native instructions for:

- building RTK
- running `rtk.exe`
- adding `rtk.exe` to `PATH`
- running:
  - `scripts\check-installation.ps1`
  - `scripts\install-local.ps1`

Use Git Bash-oriented instructions for:

- `.sh` scripts in `scripts/`
- the current hook file `hooks/rtk-rewrite.sh`
- docs that rely on Unix shell syntax or commands

If `bash --version` shows a WSL-style environment instead of `x86_64-pc-msys`, call Git Bash explicitly:

- `C:\Program Files\Git\bin\bash.exe`
- `C:\Program Files\Git\usr\bin\bash.exe`

## Option 1: Build from source

Use this if you already have Rust installed.

### Prerequisites

- Rust stable
- the `x86_64-pc-windows-msvc` toolchain

Check your toolchain:

```powershell
rustc --version
cargo --version
rustup show active-toolchain
```

### Build steps

1. Open PowerShell.
2. Go to the RTK repository root.
3. Build the release binary:

```powershell
cargo build --release
```

1. After the build finishes, the binary will be here:

```text
.\target\release\rtk.exe
```

### Verify the build

Run:

```powershell
.\target\release\rtk.exe --version
.\target\release\rtk.exe gain
.\target\release\rtk.exe ls .
.\target\release\rtk.exe read Cargo.toml
```

If these commands work, RTK itself is usable on native Windows.

## Option 2: Use the Windows release zip

Use this if you do not want to build from source.

### Steps

1. Download:
   - `rtk-x86_64-pc-windows-msvc.zip`
2. Extract `rtk.exe` to a permanent folder, for example:
   - `C:\Tools\rtk\`
3. Add that folder to your `PATH`.
4. Close PowerShell.
5. Open a new PowerShell window.
6. Verify:

```powershell
rtk --version
rtk gain
rtk ls .
```

## Making `rtk` available everywhere

If you built from source and want to run `rtk` from any folder:

1. Copy the binary from:
   - `.\target\release\rtk.exe`
2. Put it in a permanent folder, for example:
   - `C:\Tools\rtk\rtk.exe`
3. Add that folder to your user `PATH`.
4. Open a new PowerShell window.
5. Verify:

```powershell
rtk --version
rtk gain
```

## Project-local setup on Windows

If you want RTK instructions for one project only, you can use:

```powershell
rtk init
```

Or, if you are running the built binary directly:

```powershell
.\target\release\rtk.exe init
```

This updates the project-level `CLAUDE.md`.

This is not the same as the Unix hook-first global setup.

## Global hook setup on Windows

Do not assume the Unix hook instructions are directly runnable in plain PowerShell or `cmd.exe`.

Today, this is the important distinction:

- local project instructions: reasonable on Windows
- Bash hook-first global setup: not a complete native-Windows workflow yet

If you need hook-based global rewriting on Windows, treat that as a separate portability task.

## Scripts in `scripts/`

The files in `scripts/` are Bash scripts.

Examples:

- `scripts/test-all.sh`
- `scripts/test-tracking.sh`
- `scripts/check-installation.sh`
- `scripts/benchmark.sh`

These scripts may work in Git Bash, but they should not be treated as native PowerShell/cmd tools.

Windows-specific helper scripts are available for the simplest local workflows:

- `scripts\check-installation.ps1`
- `scripts\install-local.ps1`

Some also depend on additional tools or environment state such as:

- `gh`
- `docker`
- `wget`
- `pnpm`
- `pytest`
- `ruff`
- `go`
- `golangci-lint`
- network access

## Recommended verification on Windows

If you want a quick confidence check after setup, run:

```powershell
rtk --version
rtk gain
rtk ls .
rtk read Cargo.toml
```

If you are working from source in this repository, you can also run:

```powershell
cargo test --test cli_tools_smoke
```

If Git Bash is installed and you want to verify the Bash integration layer too, run:

```powershell
cargo test --test git_bash_integration
```

## Troubleshooting

### `rtk` is not found

Check whether the folder containing `rtk.exe` is in `PATH`:

```powershell
Get-Command rtk
```

If PowerShell cannot find it:

1. Add the folder containing `rtk.exe` to your user `PATH`
2. Close PowerShell
3. Open a new PowerShell window
4. Try again

### You built RTK but only `.\target\release\rtk.exe` works

That means the binary exists, but it is not installed anywhere on your `PATH`.

Either:

- keep using the full path, or
- copy `rtk.exe` into a permanent folder and add that folder to `PATH`

### Hook instructions look Unix-only

That is expected right now.

If you see references to:

- `~/.claude/...`
- `cp`
- `sh`
- `.sh` hook files

those are Unix-oriented instructions, not native PowerShell steps.

## Related docs

- [INSTALL.md](INSTALL.md)
- [README.md](README.md)
- [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)
