# Windows Native Baseline

Date: 2026-07-10
Workspace: `F:\AI\Rtk`
Branch: `develop`

## Git And Toolchain

| Item | Value |
|------|------|
| `HEAD` | `5d32d0736f686b69d1e8b9dc45c007d4eb77a0a2` |
| `origin/develop` after `git fetch origin --prune` | `5d32d0736f686b69d1e8b9dc45c007d4eb77a0a2` |
| Rust | `rustc 1.96.1 (31fca3adb 2026-06-26)` |
| Cargo | `cargo 1.96.1 (356927216 2026-06-26)` |
| PowerShell | `5.1.26100.8655` |
| OS | `Microsoft Windows NT 10.0.26200.0` |

`Cargo.toml` contains `sysinfo = "0.30"` and `Cargo.lock` contains the resolved `sysinfo` package.

## Wrapper

`scripts/windows-cargo.ps1` validates the MSVC environment before Cargo:

- finds `vswhere.exe` under `${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer`
- locates a Visual Studio installation requiring `Microsoft.VisualStudio.Component.VC.Tools.x86.x64`
- runs `Common7\Tools\Launch-VsDevShell.ps1 -Arch amd64 -HostArch amd64 -SkipAutomaticLocation`
- requires `cl.exe`, `link.exe`, `vcruntime.h`, `stdarg.h`, and `msvcrt.lib`
- forwards raw Cargo argv through `$args`

Validation command:

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 --version
```

Result: exit code `0`, output `cargo 1.96.1 (356927216 2026-06-26)`.

## Native Test Selectors

Each selector below was verified with `test <selector> -- --list`; every selector listed non-zero tests.

| Selector | Listed tests |
|------|------:|
| `cmds::system::ls` | 32 |
| `cmds::system::tree` | 9 |
| `cmds::system::wc_cmd` | 20 |
| `cmds::system::search` | 78 |
| `cmds::system::ps` | 1 |
| `cmds::system::df` | 2 |
| `cmds::system::du` | 6 |
| `test_every_subcommand_is_classified` | 1 |

Gate commands and results:

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::ls
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::tree
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::wc_cmd
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::search
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::ps
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::df
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::du
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test test_every_subcommand_is_classified
```

Results:

- `cmds::system::ls`: 32 passed, 0 failed
- `cmds::system::tree`: 9 passed, 0 failed
- `cmds::system::wc_cmd`: 20 passed, 0 failed
- `cmds::system::search`: 78 passed, 0 failed
- `cmds::system::ps`: 1 passed, 0 failed
- `cmds::system::df`: 2 passed, 0 failed
- `cmds::system::du`: 6 passed, 0 failed
- `test_every_subcommand_is_classified`: 1 passed, 0 failed

Build command:

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 build
```

Result: exit code `0`.

## Native Smoke

All smoke commands below were run against `.\target\debug\rtk.exe` after `cargo build`.

```powershell
rtk proxy .\target\debug\rtk.exe ls src\cmds\system
rtk proxy .\target\debug\rtk.exe tree src\cmds\system
rtk proxy .\target\debug\rtk.exe wc -l Cargo.toml
rtk proxy .\target\debug\rtk.exe ps
rtk proxy .\target\debug\rtk.exe df
rtk proxy .\target\debug\rtk.exe du -d 1 src\cmds\system
```

Results:

- `ls` listed `src\cmds\system` files including `ls.rs`, `tree.rs`, `wc_cmd.rs`, `search.rs`, `ps.rs`, `df.rs`, and `du.rs`.
- `tree` printed the `src\cmds\system` directory tree.
- `wc -l Cargo.toml` printed `73`.
- `ps` printed `PID NAME` plus process rows.
- `df` printed filesystem rows for local drives.
- `du -d 1 src\cmds\system` printed the root and direct child file sizes.

Native grep fallback smoke used a temporary `-File` script with:

```powershell
$env:PATH = "F:\AI\Rtk\target\debug;C:\Windows\System32"
& .\target\debug\rtk.exe grep package Cargo.toml
exit $LASTEXITCODE
```

Result: exit code `0`, output:

```text
1:[package]
55:[package.metadata.deb]
66:[package.metadata.generate-rpm]
```

## Known Non-Baseline Failures

An accidental full `cargo test` run through `scripts/windows-cargo.ps1 test` failed before this baseline was recorded. These failures are outside the protected Windows-native gate and are recorded so later work does not misattribute them to B0:

- 16 `core::stream` tests failed because Unix-style helper programs were not found on Windows.
- `discover::registry::tests::test_rewrite_uv_run` failed with `left: None`, `right: Some("rtk uv run python script.py")`.
- One run also showed `core::tracking::tests::test_timed_execution_passthrough` failing with `Passthrough record not found`.

## Protected Files

Later tasks must inspect diffs for these files before merging:

- `src/main.rs`
- `Cargo.toml`
- `Cargo.lock`
- `src/cmds/system/ls.rs`
- `src/cmds/system/tree.rs`
- `src/cmds/system/wc_cmd.rs`
- `src/cmds/system/search.rs`
- `src/cmds/system/ps.rs`
- `src/cmds/system/df.rs`
- `src/cmds/system/du.rs`
