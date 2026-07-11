# RTK Codex Windows Native Next Plan

> **Scope:** This is a standalone follow-up plan for the next Windows-native compatibility batch after the first Windows shell-native work (`ls` / `tree` / `wc` / `grep` fallback / `ps` / `df` / `du`). It now includes a Windows fallback transport layer for RTK-prefixed PowerShell shell-host calls before semantic cmdlet optimizations.
>
> **Primary goal:** reduce native Windows friction for Codex and other agent clients that naturally emit PowerShell-flavored commands, including quote/argv-safe fallback execution, without turning RTK into a full PowerShell interpreter.
>
> **Non-goal:** full PowerShell pipeline translation, object semantics emulation, scriptblock evaluation by RTK, broad alias takeover, or a Codex hook / `AGENTS.md` maintenance requirement.

---

## 0. Evidence And Priority Basis

### 0.1 Codex Local Evidence

Claude Code session scanners are not sufficient for this workspace because the active agent is Codex. A direct inspection of `~/.codex/logs_2.sqlite` found recent Codex `shell_command` tool calls.

Observed sample from the local Codex log database:

| Metric | Count |
|------|------:|
| Extracted `shell_command` calls | 421 |
| Commands starting with `rtk` | 415 |
| Direct `Get-Content` calls | 6 |

Top `rtk` subcommands in the extracted sample:

| Subcommand | Count | Interpretation |
|------|------:|------|
| `Get-Content` | 110 | Codex frequently asks RTK to run PowerShell file reads through fallback |
| `rg` | 105 | Already well-covered by RTK |
| `git` | 90 | Already well-covered by RTK |
| `proxy` | 59 | Mostly `rtk proxy powershell`, showing escape hatches are common |
| `dotnet` | 32 | Already covered by RTK filters |
| `Select-String` | 5 | Low sample count but important grep-equivalent pattern |
| `Get-ChildItem` | 4 | Low sample count but important ls/find-equivalent pattern |

This means the next batch should prioritize **PowerShell cmdlet compatibility used by Codex**, not only Unix command compatibility.

### 0.2 Priority Formula

Priority is based on:

1. Frequency in Codex traces.
2. Likelihood that native Windows lacks the Unix equivalent.
3. Existing RTK functionality that can be reused safely.
4. Risk of changing PowerShell semantics incorrectly.

### 0.3 Two Separate Windows Problems

This plan must keep two problem classes separate:

| Problem class | Example | Correct RTK responsibility |
|------|------|------|
| **Transport safety** | `rtk powershell -NoProfile -Command "Write-Output 'hello world'"` losing quote semantics through `args.join(" ")` | Preserve argv or script text and execute it safely; do not imply token compression |
| **Semantic optimization** | `Get-Content Cargo.toml` can become `rtk read Cargo.toml` | Rewrite only explicit, tested command shapes to RTK-native commands |

Every path should still enter through `rtk`, so tracking and explicit proxy behavior remain centralized. However, not every path should be compressed. Complex PowerShell must use an explicit shell-host or `rtk run` transport; unknown external commands use direct argv when resolved, while ambiguous bare PowerShell syntax fails closed instead of being guessed.

### 0.4 Non-Regression And Upstream-Reconciliation Invariants

The local worktree already contains Windows-native behavior that is not present in `origin/develop`. Upstream reconciliation is therefore a selective port, not a reset, wholesale checkout, or file replacement.

The following are release-blocking invariants for every task in this plan:

1. Preserve the existing Windows-native `ls`, `tree`, `wc`, Rust grep fallback, `ps`, `df`, and `du` implementations and their Clap surfaces.
2. Preserve the local `sysinfo` dependency and Windows-specific tests needed by `ps`, `df`, and `du`.
3. Never replace local `src/main.rs`, `src/cmds/system/ls.rs`, `tree.rs`, `wc_cmd.rs`, or `search.rs` wholesale with the upstream version.
4. In new or modified Windows fallback paths, `args.join(" ")` may be used for diagnostics, lookup keys, and telemetry only. It must never become input to `powershell -Command`, `cmd /c`, or another semantic parser. Existing non-Windows script surfaces are frozen by their own regression tests rather than refactored in this Windows plan.
5. Direct external execution must retain `OsString` argv boundaries through `Command::args`; generated PowerShell source must use UTF-16LE `-EncodedCommand` and a tested literal renderer.
6. Existing `rtk proxy` behavior remains a separate explicit escape hatch. C0.5 must not call `proxy`, reuse its Bash-style single-string `shell_split`, or route proxy traffic back through the fallback runner.
7. A remote patch is accepted only after its focused tests and the Windows-native preservation gate pass. A patch that restores upstream behavior while removing native functionality is rejected even if the upstream tests pass.
8. Unix/Linux behavior remains unchanged by Windows transport work. Platform-neutral upstream correctness fixes must pass both Windows and non-Windows CI before they are considered absorbed.

The single executable baseline gate is task B0 in section 2.9. Section 0.4 defines policy; B0 owns environment discovery, concrete test selection, smoke execution, and the recorded snapshot. Do not maintain a second independent baseline command list here.

Every post-B0 task runs the recorded selectors through the portable wrapper:

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test <recorded-selector> -- --exact
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 build
```

The wrapper must fail before Cargo when the MSVC or Windows SDK environment is incomplete. A selector that runs zero tests is a failed gate, not a successful baseline.

---

## 1. Target Layers

### 1.1 Layer C0: Codex History Support

Before future prioritization work, RTK should be able to inspect Codex logs directly.

Candidate work:

- Add a `CodexProvider` beside the current `ClaudeProvider` session provider.
- Read `~/.codex/logs_2.sqlite`, filter `logs` rows by `ts`, group logical sessions by `thread_id`, and extract `ToolCall: shell_command {...}` records in `(ts, ts_nanos, id)` order.
- Reuse the same command classification logic as `rtk discover`.
- Report provider name clearly so "no Claude Code sessions" is not mistaken for "no agent sessions".

This is not a native command, but it prevents future planning from using the wrong dataset.

### 1.2 Layer C0.5: Windows Fallback Transport Safety

Before adding more semantic rewrites, fix the Windows fallback path that currently reconstructs unknown RTK commands by joining argv into one string and invoking `powershell -Command`.

Candidate work:

- Detect shell hosts such as `powershell`, `powershell.exe`, `pwsh`, `pwsh.exe`, `cmd`, and `cmd.exe` in `Commands::Other`.
- Resolve and execute those shell hosts directly with their original argv, not through a second `powershell -Command` wrapper.
- Add a Windows-only safe PowerShell runner for unsupported cmdlets that need PowerShell execution but must preserve literal arguments.
- Use `-EncodedCommand` for generated PowerShell scripts so `$`, quotes, backticks, braces, and non-ASCII text do not pass through another shell quoting layer.
- Keep Unix/macOS fallback behavior unchanged unless a separate Unix-specific plan says otherwise.

This layer does **not** compress output by itself. It only ensures commands that cannot be safely optimized still execute with the intended PowerShell argv/script semantics.

### 1.3 Layer C1: PowerShell Cmdlet Compatibility

Add narrow, tested compatibility for common PowerShell command shapes that Codex emits on Windows:

- `Get-Content`
- `Select-String`
- `Get-ChildItem`
- `Get-Command -CommandType Application <name>` only; `Get-Command -Syntax`, bare `Get-Command`, and other introspection shapes remain transport-only and are not native rewrite candidates

These should be implemented as **RTK subcommand aliases or rewrite targets with strict shape validation**, not as a generic PowerShell parser.

### 1.4 Layer C2: Remaining Unix Small Tools

After C1, implement small Unix-style commands that agents often use cross-platform:

- `head`
- `tail`
- `pwd`
- `which`
- `touch`
- `mkdir -p`

These are still useful, but local Codex evidence puts them behind PowerShell cmdlet compatibility.

Implementation note:

Some C2 shapes are already rewritten to `rtk read --max-lines` / `--tail-lines`, but that is not a semantic foundation for native head/tail. `read --max-lines` intentionally inserts a smart-truncation marker, and `read_stdin` waits for EOF. C2 must replace head/tail rewrites with exact `rtk head` / `rtk tail` entrypoints backed by `core::line_window`. The same exact primitive can support C1 `Get-Content | Select-Object` windows without changing the existing smart `read --max-lines` contract.

---

## 2. Command Candidate Details

### 2.0 Windows Fallback Transport Runner

**Priority:** P0

**Why:** Codex already follows the global `rtk` prefix rule, but PowerShell shell-host calls can still lose quote and argument semantics when RTK joins unknown args into a string and invokes another `powershell -Command`. This is a transport problem, not a semantic optimization problem.

Initial supported forms:

| Input shape | Target behavior |
|------|------|
| `rtk powershell -NoProfile -Command "Write-Output 'hello world'"` | execute `powershell` directly with original argv; do not wrap in another `powershell -Command` |
| `rtk pwsh -NoProfile -Command "$PSVersionTable.PSVersion"` | execute `pwsh` directly with original argv |
| `rtk cmd /c "echo hello world"` | execute `cmd` directly with original argv |
| `rtk <known C1 cmdlet> <transport-schema-valid args>` | run through Windows PowerShell safe transport without RTK compression even when the shape is not semantically optimized |
| `rtk <other cmdlet or ambiguous args>` | return exit code `2` with explicit `powershell -Command` / `run -c` guidance; do not reconstruct guessed syntax |
| `rtk proxy powershell ...` | unchanged; explicit multi-argv proxy remains an escape hatch, but its one-combined-string form is not reused or advertised as PowerShell-safe |

Complex transport-only cases, supported only when the caller supplies an explicit `powershell` / `pwsh` host or uses `rtk run`:

- commands containing `$env:...`, `$_`, `$LASTEXITCODE`, or other PowerShell variables
- commands containing backticks, scriptblocks, `$(...)`, `{...}`, here-strings, or multi-line script text
- object pipelines such as `Where-Object`, `ForEach-Object`, `Measure-Object`, and `Compare-Object`
- long-running process orchestration such as `Start-Process -Wait`, `Start-Job`, and `Wait-Process`

Implementation note:

The fallback transport runner must execute these explicit-script cases correctly, but it must not claim RTK savings or rewrite them to RTK-native commands unless a separate shape whitelist proves equivalence. A bare form such as `rtk Where-Object { $_... }` is not equivalent: after crossing the external-process boundary RTK no longer has the original PowerShell AST, quote origin, or object values. Bare forms that require that lost context must fail closed and direct the caller to `rtk powershell -NoProfile -Command <script>` or `rtk run -c <script>`.

### 2.1 `Get-Content` -> `rtk read`

**Priority:** P0

**Why:** Highest observed unsupported Codex pattern. Codex often emits `rtk Get-Content ...`, which currently reaches the generic Windows fallback path instead of using RTK's native `read` behavior. That path can execute simple cmdlets, but it is transport-only and does not provide RTK's compact file-read contract.

Initial supported forms:

| Input shape | Target behavior |
|------|------|
| `Get-Content <file>` | `rtk read <file>` equivalent |
| `Get-Content -Encoding utf8 <file>` | ignore `-Encoding utf8` and use RTK UTF-8 read path |
| `Get-Content <file> -Encoding utf8` | same as above |
| `Get-Content <file> | Select-Object -First N` | map to bounded read output through the pre-pipe compound recognizer |
| `Get-Content <file> | Select-Object -Skip N -First M` | map to line window support |

Unsupported initially:

- `-Raw`
- `-Tail`
- multiple files
- wildcard paths
- dynamic paths such as `$env:TEMP`, `$(...)`, or script expressions
- arbitrary pipeline consumers other than the explicit `Select-Object` line-window shapes above

Implementation note:

Do not pass PowerShell-specific flags through to `rtk read`. Either translate a whitelisted shape or leave the command unchanged.

### 2.2 `Select-String` -> `rtk grep`

**Priority:** P1

**Why:** This is PowerShell's common grep equivalent. Even if the local sample count is small, it is a natural command for Windows agents and maps to existing RTK search functionality.

Initial supported forms:

| Input shape | Target behavior |
|------|------|
| `Select-String -Pattern <pat> -Path <file>` | `rtk grep <pat> <file>` |
| `Select-String <pat> <file>` | `rtk grep <pat> <file>` when unambiguous |
| `Select-String -SimpleMatch -Pattern <pat> -Path <file>` | map to fixed-string search only if RTK grep supports the needed mode |
| `Get-ChildItem ... | Select-String -Pattern <pat>` | leave for a later compound pipeline phase |

Unsupported initially:

- `-Context`
- `-AllMatches`
- `-List`
- `-NotMatch`
- object pipeline semantics
- scriptblock or calculated properties

Design caution:

PowerShell `Select-String` returns rich match objects, not plain grep output. RTK should only rewrite shapes where the intended user-facing output is textual search results.

### 2.3 Non-Recursive `Get-ChildItem` -> `rtk ls`

**Priority:** P1

**Why:** Natural Windows equivalent for listing and file discovery. Local sample count is lower than `Get-Content`, but this is a common Codex behavior when enumerating projects.

Initial supported forms:

| Input shape | Target behavior |
|------|------|
| `Get-ChildItem` | `rtk ls` |
| `Get-ChildItem <path>` | `rtk ls <path>` only when the positional path contains no wildcard characters; positional path is `-Path` semantics, not `-LiteralPath` |
| `Get-ChildItem -Path <path>` | `rtk ls <path>` only when `<path>` contains no wildcard characters |
| `Get-ChildItem -LiteralPath <path>` | `rtk ls <path>` with `<path>` treated as a literal string |

Unsupported initially:

- object projection pipelines
- `Where-Object`
- `ForEach-Object`
- `-Name`; it remains transport-only until `rtk ls` has an explicit tested name-only mode
- recursive enumeration, including `-Recurse -Filter`, because current `rtk find` changes ignore, hidden, file/directory, and cap semantics
- wildcard `-Path` or positional values, including `*`, `?`, `[`, and `]`, because PowerShell expands `-Path` patterns while `rtk ls` receives one literal path
- provider-specific paths
- registry paths
- dynamic paths and expressions

Design caution:

`Get-ChildItem` can enumerate non-filesystem providers. RTK should only rewrite safe filesystem-looking paths.
`-Path` and `-LiteralPath` are not interchangeable for semantic rewrites: positional paths follow `-Path` wildcard semantics, while `-LiteralPath` is already literal. A `-Path` or positional value containing PowerShell wildcard characters must not be converted into a literal `rtk ls` path. Transport may still pass either parameter through the C0.5 schema when a semantic rewrite is not allowed.

### 2.4 `Get-Command` / `where.exe` / `which`

**Priority:** P1

**Why:** Agents frequently check tool availability. Windows has `Get-Command` and `where.exe`; Unix agents often emit `which`. A native RTK command can provide stable, compact cross-platform output.

Initial supported forms:

| Input shape | Target behavior |
|------|------|
| `Get-Command -CommandType Application <name>` | map to executable-only `rtk which <name>` when exactly one static name is supplied |
| `which <name>` | same |

Unsupported initially:

- `Get-Command -Module`
- `Get-Command -Syntax <name>` and `Get-Command <name> -Syntax`; C0.5 may execute either transport-only if argv validates, but `-Syntax` forms are never native rewrite candidates because syntax output is not executable path discovery
- bare `Get-Command <name>`, because syntax alone cannot prove whether PowerShell resolves an application, alias, function, cmdlet, or script
- `where.exe <name>` until current-directory search behavior is pinned against `rtk which`
- wildcard command discovery
- aliases/functions/scripts beyond executable resolution, unless explicitly designed

Recommended RTK surface:

Add a formal `rtk which` command. Map raw `which <name>` and the explicit application-only PowerShell shape; leave bare `Get-Command` on C0.5 transport.
The transport-safe `Get-Command` parameter table in C0.5 is not a semantic rewrite allowlist. `-Syntax` belongs only to transport validation, and `Get-Command -Syntax <name>` must never rewrite to `rtk which <name>`.

### 2.5 `head` / `tail`

**Priority:** P2

**Why:** Very common across agents. The parser is small, but exact semantics require a new line-window primitive because existing `rtk read --max-lines` adds an omission marker and existing stdin reading waits for EOF.

Initial supported forms:

| Input shape | Target behavior |
|------|------|
| `head <file>` | first 10 lines |
| `head -n N <file>` | first N lines |
| `tail <file>` | last 10 lines |
| `tail -n N <file>` | last N lines |

Unsupported initially:

- `tail -f`
- byte mode
- multiple file headers unless explicitly designed

### 2.6 `pwd`

**Priority:** P2

**Why:** High-frequency agent utility with low implementation cost. Native output should be a single absolute path line.

Supported form:

- `rtk pwd`
- rewrite `pwd` to `rtk pwd` if the command is exactly `pwd`

### 2.7 `touch` / `mkdir -p`

**Priority:** P3

**Why:** Useful for agent file setup, but these mutate the filesystem and should be conservative.

Initial `touch` behavior:

- create missing file
- update modification time for existing file if safe and available
- reject directories unless explicitly supported

Initial `mkdir` behavior:

- support `mkdir -p <path>`
- no glob expansion
- no multiple path support until tests cover it

Safety note:

Mutation commands must have stricter tests and clearer error messages than read-only commands. First-batch support should prefer explicit `rtk touch` / `rtk mkdir -p` entrypoints and keep raw `touch` / `mkdir` ignored until a later phase proves that automatic rewrite is worth the side-effect risk.

### 2.8 Selective Upstream Absorption

At the 2026-07-10 review baseline, `HEAD` and `origin/develop` pointed to the same commit. B0 must fetch and record both hashes again; if they differ, regenerate this table before porting. "Absorb upstream" currently means reconciling local uncommitted divergence with behavior already present in the upstream tree. Do not cherry-pick or restore entire files over the Windows-native worktree.

| Upstream behavior | Placement | Conflict risk | Absorption decision and Plan B |
|------|------|------|------|
| `run_fallback` direct PATH execution with `resolved_command(...).args(...)` | C0.5 `DirectExternal` | Medium | Absorb the argv-preserving pattern and PATHEXT tests; do not absorb upstream `cmd /C`. If surrounding fallback code conflicts, implement direct execution against the new local decision enum. |
| `rtk proxy` implementation | C0.5 boundary test only | Low | No code import: local and upstream implementations are materially identical. Prevent its Bash-style one-string lexer from entering fallback. |
| TOML lossiness and never-worse fallback through `315a943` | U0 before C0.5 | High | Restore final behavior from tests. Plan B: implement the `Lossiness` contract locally rather than copying upstream blocks. |
| grep separator fidelity (`34a0f0e`, `cae9b71`) | U1 before `Select-String` | High | Port output invariants into local native grep. Plan B: transplant only tests and minimal separator logic; never replace `search.rs`. |
| Cargo JSON diagnostics and exit-code fixes | Independent U1 | High | Reconstruct final typed diagnostics in a separate task. Plan B: implement the tested contract locally rather than restoring the large historical diff. |
| custom-filter trust hardening through `2d487cb` | U0 security lane | High | Restore invariants one test at a time. Plan B: reimplement against local hooks; never restore whole hook files. |
| UTF-8 analytics and ccusage compatibility | Independent U1 | Medium | Port final char-boundary behavior and `period` aliases manually. |
| signal exit-code propagation (`a58de9d`) | Verification only | None | Already present locally through `exit_code_from_status`; no duplicate change. |
| Git checkout support (`bb01d6c`) | U2 after C1 | Medium | Port as a focused local handler with registry/guard tests; it must not block C0.5. |
| Claude absolute-hook handling (`48b883f`, `b0c0b20`) | Deferred redesign | High | Do not copy as-is: matcher does not cover Windows `rtk.exe` and its lexer consumes backslashes. Require a separate Windows path design. |
| Factory Droid and PHP command suites | Out of scope | High | Restore only through separate opt-in plans when those ecosystems are required. |

Absorption procedure for every accepted item:

1. Identify the final upstream behavior and all follow-up fixes, not only the first commit in a series.
2. Add or restore the focused regression test against the local implementation first and verify that it fails for the expected reason.
3. Port the smallest coherent code block while preserving local Windows `#[cfg]` branches, native modules, dependencies, and command variants.
4. Run the focused upstream test, the touched module tests, and the protected Windows-native baseline.
5. Inspect `git diff --stat` and `git diff -- <protected files>`; reject accidental deletion or replacement of local native code.
6. Commit each upstream behavior separately from C0.5 and C1 feature work so a regression can be reverted without removing Windows-native support.

### 2.9 Executable Baseline And Upstream Reconciliation Tasks

#### B0: Baseline Gate

**Conflict risk:** none; this task records the pre-change state.

**Files:**

| File | Change |
|------|------|
| `scripts/windows-cargo.ps1` | New reusable MSVC environment discovery and Cargo command wrapper; no hard-coded VS or SDK version |
| `docs/windows-native-baseline.md` | Record commit, toolchain, exact non-zero test selectors, smoke outputs, and protected files |

`scripts/windows-cargo.ps1` interface is fixed for all later commands:

```powershell
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $CargoArgs
)

# Discover and enter the VS development environment, then validate headers/libs.
& cargo @CargoArgs
exit $LASTEXITCODE
```

The implementation fills in the discovery and validation block described below; callers pass only normal Cargo argv after the script path.

**Implementation and verification:**

1. Run `rtk git fetch origin --prune` and record `HEAD`, `origin/develop`, Rust, Cargo, PowerShell, and OS versions. If the two git hashes differ from the section 2.8 baseline, re-audit upstream before continuing.
2. Locate `vswhere.exe` under `${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer`. Query the latest installation requiring `Microsoft.VisualStudio.Component.VC.Tools.x86.x64`.
3. Locate that installation's `Common7\Tools\Launch-VsDevShell.ps1`, invoke it with `-Arch amd64 -HostArch amd64 -SkipAutomaticLocation`, and then run the supplied Cargo argv in the same PowerShell process.
4. Before Cargo, require `cl.exe`, `link.exe`, one `vcruntime.h` under the semicolon-separated `INCLUDE`, one `stdarg.h` under `INCLUDE`, and one `msvcrt.lib` under `LIB`. Print the missing artifact and exit non-zero instead of attempting a partial build.
5. Run `rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test -- --list` and record the concrete fully-qualified tests for `ls`, `tree`, `wc_cmd`, `search`, `ps`, `df`, `du`, and `test_every_subcommand_is_classified`. A selector that matches zero tests is a B0 failure.
6. Run the concrete selectors, `cargo build`, and native smoke commands for `rtk ls`, `tree`, `wc`, `ps`, `df`, and `du` through `rtk proxy .\target\debug\rtk.exe ...`. For native grep, launch the debug binary from an explicit PowerShell process whose temporary `PATH` contains the debug-binary directory but no `grep.exe`; search a fixture with one known match and assert the expected file/line/content output and exit code `0`.
7. Record protected files: `src/main.rs`, `Cargo.toml`, `Cargo.lock`, `src/cmds/system/{ls,tree,wc_cmd,search,ps,df,du}.rs`, and their relevant tests.
8. Commit only the environment wrapper and baseline document: `chore(windows): record native compatibility baseline`.

#### U0-TOML: Restore Reversible Lossiness Before C0.5

**Conflict risk:** High. `src/main.rs` and `src/core/toml_filter.rs` both diverge locally.

**Files:** `src/core/toml_filter.rs`, `src/main.rs`, `src/core/runner.rs`, TOML/guard integration tests.

**Contract to restore from final `origin/develop`:** `apply_filter_with_info`, `Lossiness::{None,Tail,Whole}`, success-path tee recovery, `toml_disabled`, and raw guarded fallback when a lossy result has no recovery hint. Include follow-up behavior through `315a943`; do not replay only the first historical commit.

**Steps:**

1. Restore focused tests named for: lossless filter, tail-loss payload/offset, whole-loss recovery, success-path tee hint, unrecoverable-loss raw fallback, and `RTK_NO_TOML` disable behavior. Run them and record the expected failures against the local implementation.
2. Port the final types and behavior into local `toml_filter.rs`; preserve local filter definitions and unrelated Windows changes.
3. Adapt local `run_fallback` to consume `Lossiness` before C0.5 refactors execution routing.
4. Run focused TOML tests, `guard_integration_test`, and the B0 native gate.
5. Inspect protected-file diffs; `main.rs` may gain only the TOML decision changes in this commit.
6. Commit separately: `fix(toml): restore reversible filter lossiness`.

**Plan B:** if source blocks conflict, implement the final `Lossiness` contract locally from tests instead of copying upstream functions.

#### U0-Trust: Restore Custom-Filter Trust Hardening

**Conflict risk:** High. Local hook, init, integrity, and trust files diverge substantially.

**Files:** `src/hooks/trust.rs`, `src/hooks/init.rs`, `src/hooks/integrity.rs`, focused hook/trust tests.

**Contract:** malformed trust state is visible; non-interactive approval fails closed; already-trusted files are skipped; project/global scope labels remain correct; re-trust after filter changes is possible.

**Steps:**

1. Add `malformed_trust_state_is_error`, `non_interactive_trust_fails_closed`, `already_trusted_file_is_not_rewritten`, `trust_scope_label_matches_source`, and `changed_filter_requires_retrust`; run each and record the expected local failure.
2. Port behavior through upstream `2d487cb` into local trust data types. Preserve local Codex hook behavior and omit Factory Droid branches.
3. For the no-rewrite test, compare file bytes and modified time before/after; sleeping is not an acceptable assertion strategy.
4. Run focused `hooks::trust`, `hooks::integrity`, and init trust tests, then B0.
5. Inspect all hook-file diffs and commit `fix(trust): restore custom filter hardening`.

**Plan B:** reimplement each invariant against the local hook model. Do not restore whole upstream hook files or Factory Droid code.

#### U1-Grep: Port Separator Fidelity Into Native Grep

**Conflict risk:** High. Local `search.rs` contains the Windows Rust fallback absent upstream.

**Files:** `src/cmds/system/search.rs`, `tests/grep_context_test.rs`, `tests/grep_faithful_format_test.rs`.

**Steps:**

1. Restore `no_separator_without_context_flag`, `group_separator_between_non_adjacent_matches`, and `context_group_separator_matches_grep_n`; verify they fail locally.
2. Port `has_context_flag`, raw `--` preservation, and non-adjacent group separator logic from final upstream behavior.
3. Apply the same output contract to external grep and the local Windows Rust fallback.
4. Run the three regressions with external grep when available and with a controlled PATH that forces native fallback.
5. Run B0 and commit `fix(grep): preserve context separator fidelity`.

**Plan B:** transplant only the output invariants; never replace local `search.rs`.

#### U1-Cargo: Restore Final JSON Diagnostic Behavior

**Conflict risk:** High. Local `cargo_cmd.rs` removed a large final-upstream diagnostic path.

**Files:** `src/cmds/rust/cargo_cmd.rs`, `src/cmds/rust/runner.rs`, focused inline Cargo tests and fixtures.

**Contract:** last `--message-format` wins; JSON build/check/clippy/install errors remain visible; process exit status is preserved; diagnostic caps retain recovery hints; success wording remains command-correct.

**Steps:**

1. Add `last_message_format_wins`, `json_build_failure_is_visible`, `json_check_failure_is_visible`, `json_clippy_failure_is_visible`, `json_install_failure_is_visible`, `json_exit_code_is_preserved`, and `capped_json_diagnostics_have_recovery_hint`; verify expected failures.
2. Reconstruct the final typed `CargoJsonLine`, rendered diagnostic extraction, count merge, and labeled success/failure summaries from `origin/develop`.
3. Keep process status as an explicit input to the filter; no formatter may infer success only from parsed text.
4. Run the named tests, all `cmds::rust::cargo_cmd` tests, and B0.
5. Inspect that no Windows system module changed and commit `fix(cargo): restore json diagnostic fidelity`.

**Plan B:** implement typed deserialization and count merging locally; do not restore the whole 691-line upstream diff.

#### U1-Analytics: Restore UTF-8 And ccusage Compatibility

**Conflict risk:** Medium.

**Files:** `src/analytics/gain.rs`, `src/analytics/session_cmd.rs`, `src/analytics/ccusage.rs`, `src/core/utils.rs`.

**Steps:**

1. Add failures for CJK, Cyrillic, and emoji display truncation in history/failure/session output.
2. Port final char-boundary behavior through `c9468ee`, not only initial `47b22e0`.
3. Restore `period` aliases for daily, weekly, and monthly ccusage records from `d823aaf`.
4. Name and run `history_truncation_is_unicode_safe`, `failure_truncation_is_unicode_safe`, `session_id_prefix_is_unicode_safe`, and the three `period` alias tests.
5. Run focused analytics tests and B0; commit `fix(analytics): restore utf8 and ccusage compatibility`.

#### U2-Checkout: Add Git Checkout As A Separate Feature

**Conflict risk:** Medium. This touches git dispatch and registry but not protected native system modules.

**Files:** `src/cmds/git/git.rs`, `src/discover/registry.rs`, `src/discover/rules.rs`, `src/main.rs`, `tests/guard_integration_test.rs`.

**Steps:**

1. Restore failing tests for checkout dispatch, passthrough flags, successful compact output, failure visibility, rewrite classification, and guard recoverability from `bb01d6c`.
2. Add `GitCommand::Checkout`, `run_checkout`, Clap dispatch, registry/rule support, and guard integration against local git helpers.
3. Preserve local sparse-checkout status detection; the new checkout command must not replace or rename that state logic.
4. Run checkout-specific git tests, registry tests, `guard_integration_test`, and B0.
5. Commit `feat(git): add checkout support`. Do not bundle this feature with C0.5 or C1.

### 2.10 Mandatory Execution Units

This document is the master roadmap. Implementation must be split into the following independently reviewable units; do not execute the whole document as one branch or one accumulated diff.

| Unit | Contents | Dependency | Merge gate |
|------|------|------|------|
| A | B0, U0-TOML, U0-Trust | none | recorded native baseline plus focused upstream safety tests |
| B | C0.5 Windows transport and `rtk run` migration | Unit A | argv probe, batch validation, no-join scan, Unix regression, B0 |
| C | PowerShell lexer/renderer and C1 cmdlet families | Unit B; U1-Grep before `Select-String` | parse-render-parse suite, semantic negative tests, B0 |
| D | CodexProvider logical sessions | B0 only; may proceed parallel to B/C in a separate branch | row-time/thread fixtures, existing Claude provider tests, B0 |
| E | U1-Cargo, U1-Analytics, U2-Checkout, and C2 commands | B0; each remains a separate commit/review | focused contract tests plus B0 |

A unit is complete only when its own diff can be reviewed and reverted without removing functionality from another unit.

---

## 3. Detailed Implementation Plans

The next batch should be implemented as small, separately testable changes. PowerShell cmdlet compatibility needs two surfaces:

1. **Windows fallback transport:** Codex often emits `rtk powershell ...` or `rtk Get-Content ...`. Explicit shell-host and `rtk run` forms must execute complex scripts safely without quote loss. Bare cmdlet forms are limited to static argv shapes that the literal renderer can prove safe; ambiguous forms fail closed with guidance to use an explicit shell host instead of being reconstructed heuristically.
2. **Direct RTK invocation:** Codex often emits `rtk Get-Content ...`. Supported cmdlet shapes should be intercepted before raw fallback and routed to RTK-native commands.
3. **Hook/discover rewrite:** Agent hooks and `rtk discover` classify raw commands such as `Get-Content foo.txt`. Supported shapes should be recognized in `src/discover/registry.rs` and should stay passthrough when the shape is unsafe or unsupported.

Shared design:

- Create `src/core/windows_shell.rs` for Windows-only fallback transport decisions, shell-host direct argv execution, PowerShell script encoding, validated batch transport, and literal cmdlet invocation rendering.
- Create `src/discover/powershell_lexer.rs` for conservative parsing and rendering of raw PowerShell command strings used by hooks and `rtk rewrite`. It must not call `discover::lexer::shell_split` or treat backslash as an escape character.
- Create `src/discover/ps_classify.rs` for shape-aware classification and rewrite of PowerShell-like commands. Direct `rtk Get-*` handling consumes existing `OsString` argv; raw hook/rewrite handling consumes tokens from `powershell_lexer`.
- Modify `src/discover/mod.rs` to export the new module.
- Modify `src/discover/registry.rs` so `classify_command` asks `ps_classify::classify(cmd_clean)` before the generic regex rules, and `rewrite_segment_inner` consumes a tri-state `PowerShellRewriteDecision::{NotApplicable, Rewrite(String), Refuse}`. `Refuse` means the input started with a known PowerShell cmdlet but was unsafe or unsupported; return no rewrite and do not continue into generic prefix rules.
- Create `src/cmds/system/ps_cmdlet.rs` only for direct `rtk Get-*` / `rtk Select-String` fallback interception from `src/main.rs`.
- Add native Unix-small-tool modules under `src/cmds/system/` for commands that do not already have a direct RTK surface.
- Because `src/cmds/system/mod.rs` uses `automod::dir!`, adding a file under `src/cmds/system/` creates the Rust module automatically; `src/main.rs` still needs imports, Clap variants, dispatch arms, and command-classification test updates.

Cross-cutting conventions:

- Maintain two different tables with different responsibilities. `SemanticRewriteSpec` lists shapes that are equivalent to an RTK-native command. `CmdletTransportSpec` lists known PowerShell parameter tokens that can be rendered bare for transport-only execution. A parameter may be transport-safe without being semantically rewritable.
- Unsupported optimization shapes should return `Unsupported` to the caller so Windows transport can execute them only when the complete argv matches a `CmdletTransportSpec`. Ambiguous bare PowerShell syntax returns `RejectAmbiguous` and requires an explicit shell host; it must not silently fall through to string reconstruction. Unsupported does not mean "command failure" unless the user explicitly invoked an RTK-native subcommand with invalid arguments or safe transport cannot be proven.
- Direct compatibility handlers must normalize mixed return types into a single `Result<i32>` surface: `Result<()>` helpers map to exit code `0` on success, while helpers that already return `i32` preserve their code.
- Use stderr for unsupported-shape or invalid-argument diagnostics, stdout only for command results.
- Keep startup overhead small: shape parsing should be linear over argv/tokens and must not open the filesystem except for commands that already need filesystem access.
- Windows tests should include quoted paths, Unicode filenames, and at least one path with spaces for read/list/search transport paths.

Raw PowerShell rewrite contract:

1. Invoke the PowerShell parser only for full cmdlet names in the explicit C1 set; do not parse arbitrary shell commands as PowerShell.
2. Emit `StaticToken { value, quoting: Unquoted | Single | Double }` so semantic parsers can distinguish an unquoted parameter token from a quoted dash-prefixed literal. Do not discard quote origin before classification.
3. Backslash is always a literal path character. Backtick, interpolation, subexpressions, here-strings, splatting, and stop-parsing syntax make the command non-rewritable in the first batch.
4. Accept static unquoted words, single-quoted literals with doubled single quotes, and double-quoted literals only when they contain no `$` or backtick.
5. Preserve whether `|` is quoted and recognize a pipeline only when the token is an unquoted operator.
6. Render every generated value argument as a PowerShell single-quoted literal, doubling embedded single quotes. Render an empty value as `''`. Generated RTK option names may remain bare.
7. Require `parse(render(args)) == args` in tests before returning a rewrite. If round-trip validation fails, return `None`.

Required `powershell_lexer` tests:

| Test | Expected |
|------|------|
| `backslash_is_literal` | `C:\src\a.txt` remains one unchanged value |
| `single_quote_doubling_decodes` | `'can''t.txt'` becomes `can't.txt` |
| `quoted_dash_literal_retains_origin` | `'-Raw'` is a quoted value token, while unquoted `-Raw` is eligible for parameter classification |
| `static_double_quote_decodes` | `"hello world"` becomes one static value |
| `double_quote_interpolation_rejected` | `$`, `$(`, or backtick inside double quotes makes the command non-rewritable |
| `unquoted_pipe_is_operator` | only an unquoted `\|` splits the supported compound shape |
| `quoted_pipe_is_literal` | `'a|b'` remains one value |
| `renderer_handles_empty_and_apostrophe` | empty and apostrophe-containing argv round-trip exactly |
| `renderer_handles_unc_unicode_and_trailing_backslash` | Windows path code points round-trip exactly |
| `refused_powershell_shape_stops_generic_rewrite` | unsafe/unsupported known cmdlet input returns `Refuse` and generic rules are not evaluated |

### 3.0 C0.5 Windows Fallback Transport Runner

**Goal:** Make unknown RTK-prefixed commands preserve Windows argv and PowerShell script semantics before any semantic optimization is attempted.

**Files:**

| File | Change |
|------|------|
| `src/core/windows_shell.rs` | New Windows-only fallback runner, shell-host detection, PowerShell `-EncodedCommand` helper, and literal argv renderer |
| `src/core/mod.rs` | Export `windows_shell` behind `#[cfg(windows)]` |
| `src/main.rs` | Replace Windows branches in `Commands::Other` and `Commands::Run` with `windows_shell` helpers; keep non-Windows fallback behavior unchanged |
| `Cargo.toml` | Add direct dependency `base64 = "0.22"`; version `0.22.1` is already resolved in `Cargo.lock` |
| `src/cmds/system/ps_cmdlet.rs` | Add the initial cmdlet-name/parameter metadata used to distinguish safe bare transport from ambiguous argv; later C1 tasks add semantic handlers to the same tables |
| `tests/windows_fallback_transport_test.rs` | Windows integration tests for argv boundaries, explicit shell hosts, encoded scripts, and extension routing |
| `tests/fixtures/windows_argv_probe.rs` | Standalone std-only helper compiled by the integration test; emit each argv value as indexed UTF-16 code-unit hex |
| `tests/fixtures/windows_argv_probe.ps1` | PowerShell `-File` helper that emits `$args` as compressed JSON |
| `README.md` | Document Windows `rtk run -c <script>` versus positional literal argv and the migration from positional string joining |

**Decision model:**

```rust
enum WindowsFallbackDecision {
    DirectShellHost,      // powershell, pwsh, cmd: preserve argv exactly
    PowerShellTransport,  // execute via PowerShell without claiming compression
    DirectExternal,       // normal PATH executable
    BatchTransport,       // .cmd/.bat with a validated cmd-representable argv subset
    RejectAmbiguous,      // original PowerShell syntax cannot be reconstructed safely
}
```

RTK semantic optimization is decided before this enum by `ps_cmdlet::intercept`. `WindowsFallbackDecision` owns transport only: shell-host direct execution, PATH-resolved external execution, or implicit PowerShell transport.

**Single-entry call order:**

```text
Commands::Other / parse-error run_fallback
  -> ps_cmdlet::intercept(args)
       -> Handled(code): return
       -> Unsupported / Unknown: continue
  -> windows_shell::run_other(args)
       -> explicit shell host: direct argv
       -> PATH-resolved executable: direct argv
       -> PATH-resolved .cmd/.bat: validate batch argv, then execute resolved wrapper
       -> resolved .ps1: PowerShell -File with original argv
       -> unresolved known cmdlet with transport-schema-valid argv: PowerShell transport (encoded when within budget; temporary `.ps1` file otherwise)
       -> ambiguous bare PowerShell syntax: reject with explicit-host guidance

Commands::Run
  -> explicit `-c <script>`: windows_shell::run_script(script)
  -> positional argv: windows_shell::run_argv(args)
```

`ps_cmdlet::intercept` recognizes only the explicit C1 cmdlet whitelist. It must not intercept `powershell`, `pwsh`, or `cmd`; explicit shell hosts belong to `windows_shell`. `Commands::Run` is an explicit transport surface: `-c` means raw script and positional arguments mean literal invocation argv. Neither form receives semantic cmdlet rewrites.

**First-batch transport schema:**

Parameter matching is case-insensitive. For `-Name:value`, validate `-Name` against the table and keep the complete token bare. Common PowerShell parameters are accepted for all four cmdlets. Because process argv does not preserve whether a dash-prefixed token was quoted by the caller, direct RTK cmdlet compatibility defines an RTK-only `--` boundary: before `--`, listed dash tokens are parameters; after `--`, every token is a literal value and `--` itself is removed before invoking PowerShell. A dash-prefixed token not listed here and not protected by `--` is ambiguous and must not execute implicitly.

| Cmdlet | Transport-safe parameter tokens; this is not the semantic rewrite allowlist |
|------|------|
| `Get-Content` | `-Path`, `-LiteralPath`, `-Encoding`, `-Delimiter`, `-ReadCount`, `-TotalCount`, `-Tail`, `-Filter`, `-Include`, `-Exclude`, `-Stream`, `-Raw`, `-Wait`, `-Force`, `-AsByteStream` |
| `Select-String` | `-Pattern`, `-Path`, `-LiteralPath`, `-InputObject`, `-Encoding`, `-Context`, `-Include`, `-Exclude`, `-Culture`, `-SimpleMatch`, `-CaseSensitive`, `-Quiet`, `-List`, `-NotMatch`, `-AllMatches`, `-Raw`, `-NoEmphasis` |
| `Get-ChildItem` | `-Path`, `-LiteralPath`, `-Filter`, `-Include`, `-Exclude`, `-Depth`, `-Attributes`, `-Name`, `-Recurse`, `-Force`, `-File`, `-Directory`, `-Hidden`, `-ReadOnly`, `-System`, `-FollowSymlink` |
| `Get-Command` | `-Name`, `-Verb`, `-Noun`, `-Module`, `-CommandType`, `-ParameterName`, `-ParameterType`, `-ArgumentList`, `-TotalCount`, `-All`, `-ListImported`, `-Syntax`, `-ShowCommandInfo` |
| Common parameters | `-ErrorAction`, `-WarningAction`, `-InformationAction`, `-ProgressAction`, `-ErrorVariable`, `-WarningVariable`, `-InformationVariable`, `-OutVariable`, `-OutBuffer`, `-PipelineVariable`, `-Verbose`, `-Debug`, `-WhatIf`, `-Confirm` |

The schema guarantees only the explicit RTK compatibility contract above; it does not recover lost shell quote origin. PowerShell remains responsible for version-specific parameter availability and binding errors. A literal path such as `-Raw` must be written `rtk Get-Content -- -Raw`. Other cmdlet names and parameters require explicit `rtk powershell -NoProfile -Command <script>` or `rtk run -c <script>`.

**Implementation steps:**

1. Add `src/core/windows_shell.rs` with `pub fn run_other(args: &[OsString], verbose: u8) -> Result<i32>`, `pub fn run_script(script: &str, verbose: u8) -> Result<i32>`, and `pub fn run_argv(args: &[OsString], verbose: u8) -> Result<i32>`. Refactor the Windows execution side of parse-error `run_fallback` to collect `std::env::args_os()` and retain `OsString` through dispatch; derive lossy/display strings only for meta-command checks, lookup keys, diagnostics, and telemetry.
2. Implement `is_shell_host(name)` with case-insensitive matching for `powershell`, `powershell.exe`, `pwsh`, `pwsh.exe`, `cmd`, and `cmd.exe`.
3. For shell hosts, resolve the requested host with `resolve_binary`, then execute `Command::new(resolved_host).args(&args[1..])` directly and return `exit_code_from_status`.
4. Add `encode_powershell(script: &str) -> String` using UTF-16LE bytes and base64 for `powershell -EncodedCommand`.
5. Before spawning implicit or `run -c` PowerShell transport, use `-EncodedCommand` only when both the 8 KiB UTF-8 source budget and the 30,000 UTF-16-unit complete encoded command-line budget are satisfied. The budget is explicit: 8 KiB of UTF-8 source expands to about 16 KiB of UTF-16LE bytes and then about 21.4 KiB of base64 payload before host-argv overhead, so the 30,000-unit complete-command cap preserves headroom below the Windows 32,767-unit process limit. When either encoded budget is exceeded, write the complete script to an automatically cleaned up UTF-8-BOM temporary `.ps1` file and invoke it with `-File`; never truncate. Detect the execution policy only for that file fallback and add process-scoped `-ExecutionPolicy Bypass` only when it is required.
6. Add `quote_ps_literal(value: &OsStr) -> Result<String>`. Convert without `to_string_lossy`; if an `OsString` cannot be represented as valid PowerShell script text, return `RejectAmbiguous` and require direct external or explicit host execution. For valid text, single-quote the value and double embedded single quotes.
7. Add `render_powershell_invocation(args)` for transport-only bare-cmdlet execution. Render the command name and literal values with `quote_ps_literal`. Before an RTK-only `--`, keep a parameter token bare only when it appears in the exact first-batch `CmdletTransportSpec`; after `--`, quote every token literally and omit the boundary. Treat another dash-prefixed token, a scriptblock-like token, a here-string marker, or a token that depends on unavailable syntax as `RejectAmbiguous`. Negative numbers remain quoted literal values. The rejection message must show either the `--` literal boundary or explicit `rtk powershell -NoProfile -Command ...` alternative without attempting execution.
8. Implement `resolve_powershell_host()`. Preserve an explicitly requested host. For implicit transport, prefer `powershell.exe` to preserve current Windows behavior, then fall back to `pwsh.exe`; return a clear error if neither exists.
9. Classify non-shell-host commands with `resolve_binary(args[0])`: `.exe` and extensionless executables use `Command::new(resolved_path).args(original_args)`. A `.ps1` file uses the resolved PowerShell host with `-File` and original argv. An unresolved name uses `PowerShellTransport` only when it is one of the four known cmdlets and `render_powershell_invocation` accepts the complete argv; otherwise use `RejectAmbiguous`.
10. Classify `.cmd` / `.bat` as `BatchTransport`, not `DirectExternal`. Permit an argument only when it contains none of `"`, `%`, `!`, `^`, `&`, `|`, `<`, `>`, `(`, `)`, carriage return, line feed, or NUL. Empty arguments, spaces, apostrophes, Unicode, and backslashes remain allowed but require child-boundary tests. On rejection, return exit code `2` and require an explicit `rtk cmd /d /s /c <script>` invocation. Do not build a `/c` command string inside RTK.
11. For implicit PowerShell transport, run the resolved host with `-NoProfile -EncodedCommand <encoded>` while it fits the encoded budgets. For the automatic temporary-file fallback, run `-NoProfile [-ExecutionPolicy Bypass] -File <temporary .ps1>`; the optional process-scoped bypass is permitted only when the execution-policy check requires it. Preserve a user-supplied execution-policy option unchanged for an explicit `powershell` host invocation.
12. Define and document the Windows `Commands::Run` migration contract: keep `-c` as one Unicode script string and convert each existing positional `String` into one `OsString` without joining. `rtk run -c <script>` passes the exact script string to `run_script` and uses UTF-16LE `-EncodedCommand` when it fits the encoded budgets, otherwise the automatic UTF-8-BOM temporary `.ps1` / `-File` fallback. `rtk run <program> <args...>` calls `run_argv`, which reuses `run_other`. Resolved external programs and explicit shell hosts receive every remaining token as literal argv, including values containing `|`, `$`, braces, quotes, or empty strings; RTK must not interpret those values as shell syntax. Only unresolved commands entering implicit PowerShell transport use `CmdletTransportSpec` and `RejectAmbiguous`. This is an intentional Windows behavior change from positional string joining and requires release notes plus compatibility tests; non-Windows behavior remains unchanged in this plan.
13. Record `DirectShellHost`, `DirectExternal`, `BatchTransport`, and `PowerShellTransport` with `track_passthrough` (0% semantic savings), not the normal optimized tracking path.
14. Keep non-Windows `Commands::Other` and `Commands::Run` on their existing `sh -c` behavior, and keep parse-error `run_fallback` on its existing direct `resolved_command(...).args(...)` behavior. C0.5 must not unify these Unix paths under a new shell reconstruction.
15. Add a source-level regression assertion or narrowly scoped test proving no Windows fallback execution sink is fed by `args.join(" ")`; logging and telemetry joins remain allowed, and frozen non-Windows script surfaces are excluded.
16. Add unit tests for the decision model, literal quoting, UTF-16LE/base64 encoding and limits, host resolution, executable/script extension routing, batch validation, ambiguity rejection, `run -c` versus positional-run semantics, tracking mode, and shell-host detection.
17. In `windows_fallback_transport_test`, compile `tests/fixtures/windows_argv_probe.rs` with the active `rustc` into a temporary `.exe`. Compare its indexed UTF-16 output to the original `OsString` code units for direct external tests. Generate a temporary `.cmd` wrapper that forwards `%*` to the probe for the documented batch-safe subset, and use `windows_argv_probe.ps1` for `-File` tests. Skip none of these on Windows; gate the test module with `#[cfg(windows)]` elsewhere.
18. Before merging, rerun the protected Windows-native baseline from section 0.4 and inspect protected-file diffs for deletion of native branches.

**Unit tests:**

| Test | Expected |
|------|------|
| `detects_powershell_shell_host_case_insensitive` | `powershell`, `PowerShell.EXE`, `pwsh.exe` return `true` |
| `detects_cmd_shell_host_case_insensitive` | `cmd`, `CMD.EXE` return `true` |
| `quotes_powershell_literal_with_spaces` | `hello world` becomes `'hello world'` |
| `quotes_powershell_literal_with_single_quote` | `can't` becomes `'can''t'` |
| `powershell_literal_rejects_non_unicode_osstring` | invalid script text is rejected without lossy replacement; direct external argv remains unaffected |
| `encoded_command_uses_utf16le` | encoding `Write-Output 'x'` decodes to UTF-16LE script text |
| `oversized_implicit_transport_uses_file_with_bypass` | source larger than 8 KiB UTF-8 uses a complete UTF-8-BOM temporary `.ps1` through `-File`, with process-scoped bypass when required |
| `encoded_transport_estimate_uses_actual_host_path` | a complete encoded host command line larger than 30,000 UTF-16 code units selects the temporary-file transport rather than truncating or rejecting the script |
| `file_transport_preserves_complete_script` | inputs over either encoded budget execute their complete script through the file fallback; no prefix is encoded, written, or executed |
| `renders_parameter_names_bare` | `["Get-Content", "-LiteralPath", "a b.txt"]` renders `-LiteralPath 'a b.txt'` |
| `resolved_exe_uses_direct_external` | a PATH-resolved `.exe` is executed without PowerShell transport |
| `resolved_ps1_uses_powershell_file` | a PATH-resolved `.ps1` is routed through the selected PowerShell host |
| `batch_transport_accepts_safe_subset` | empty args, spaces, apostrophes, Unicode, and backslashes pass validation and use the resolved wrapper path without RTK string reconstruction |
| `batch_transport_rejects_cmd_metacharacters` | quotes, expansion markers, command operators, parentheses, and newlines return exit code 2 with explicit `cmd` guidance |
| `known_cmdlet_uses_powershell_transport` | one of the four known cmdlets with schema-valid argv uses encoded PowerShell transport |
| `unknown_cmdlet_requires_explicit_host` | an unresolved cmdlet outside the first-batch schema does not execute implicitly |
| `unsupported_semantic_flag_can_still_transport` | `Get-Content -Raw file` and `Select-String -Context 2 ...` are not optimized but pass the transport schema |
| `ambiguous_dash_literal_is_rejected` | a bare unresolved invocation with an unclassifiable dash-prefixed value does not execute and recommends an explicit shell host |
| `dash_literal_after_boundary_is_quoted` | `rtk Get-Content -- -Raw` renders `Get-Content '-Raw'`, not the `-Raw` switch |
| `scriptblock_like_bare_args_are_rejected` | `{ $_.Name }`, here-string markers, and multi-line bare argv do not enter the literal renderer |
| `run_c_encodes_exact_script` | `rtk run -c <script>` encodes the exact script text once and does not join or re-tokenize it |
| `run_positional_preserves_literal_argv` | `rtk run <program> <args...>` renders ordered literal argv, including empty values and embedded quotes |
| `run_positional_external_treats_operators_as_literals` | a resolved external probe receives `\|`, `$`, braces, quotes, and empty strings as unchanged argv |
| `run_positional_unresolved_shell_text_is_rejected` | an unresolved program plus script-like argv does not become an implicitly reconstructed script and recommends `run -c` |
| `short_implicit_transport_uses_encoded_command_without_policy_override` | in-budget encoded host args do not contain `-ExecutionPolicy Bypass` |
| `execution_policy_classification_requires_bypass_only_for_restricted_or_all_signed` | file transport adds process-scoped `-ExecutionPolicy Bypass` only for `Restricted` or `AllSigned` policy |
| `implicit_host_falls_back_to_pwsh` | `pwsh` is used when Windows PowerShell is unavailable |
| `transport_paths_track_passthrough` | transport-only and direct-external paths record 0% semantic savings |
| `non_windows_other_and_run_remain_sh_c` | existing non-Windows `Commands::Other` / `Commands::Run` tests still expect `sh -c` |
| `non_windows_parse_fallback_remains_direct_argv` | parse-error `run_fallback` still uses `resolved_command(...).args(...)` without shell reconstruction |
| `windows_execution_sinks_do_not_receive_joined_argv` | no Windows fallback execution path feeds `args.join(" ")` to a shell parser |

**Verification commands:**

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test core::windows_shell
rtk proxy .\target\debug\rtk.exe powershell -NoProfile -Command "Write-Output 'hello world'"
rtk proxy .\target\debug\rtk.exe powershell -NoProfile -Command "$env:TEMP"
rtk proxy .\target\debug\rtk.exe powershell -NoProfile -Command "Get-ChildItem | Where-Object { $_.Name -match 'src' }"
rtk proxy .\target\debug\rtk.exe Get-Content -Raw Cargo.toml
rtk proxy .\target\debug\rtk.exe cmd /c "echo hello world"
rtk proxy .\target\debug\rtk.exe run -c "Write-Output 'run script'"
rtk proxy .\target\debug\rtk.exe run powershell -NoProfile -Command "Write-Output 'literal argv'"
```

**Unix/macOS regression verification:**

```bash
rtk cargo test
rtk proxy sh -c 'printf "%s\n" "unix fallback smoke"'
```

**Unsupported in this batch:**

- PowerShell AST parsing.
- Translating object pipelines into RTK text commands.
- Claiming token savings for transport-only execution.
- Changing the existing Unix/macOS split between script surfaces (`sh -c`) and parse-error direct argv execution.

### 3.1 C0: CodexProvider For `discover` And `session`

**Goal:** Make RTK's analytics read Codex session logs directly, so future prioritization is not based on Claude-only evidence.

**Files:**

| File | Change |
|------|------|
| `src/discover/provider.rs` | Replace file-only session references with `SessionRef` / `SessionSource`; add `CodexProvider` using logical `thread_id` sessions |
| `src/discover/mod.rs` | Select provider from a new CLI option and include provider name in verbose output |
| `src/analytics/session_cmd.rs` | Reuse provider selection or scan both Claude and Codex sessions |
| `src/main.rs` | Add provider flags to `discover` and `session`, for example `--provider claude|codex|all`; add `--codex-path <path>` for explicit database override |
| `Cargo.toml` | No new SQLite dependency is needed because `rusqlite` already exists |

**Parser and data source:**

- Try Codex database candidates in this order: explicit `--codex-path`, then `%USERPROFILE%\.codex\logs_2.sqlite` / `$HOME/.codex/logs_2.sqlite`. Do not automatically scan other `*.sqlite` files; diagnostics may list them as unselected candidates, but reading them requires an explicit override.
- Add an internal helper `CodexProvider::candidate_db_paths(override_path: Option<PathBuf>) -> Vec<PathBuf>` that uses `dirs::home_dir()` if the crate is already available; otherwise use `std::env::var_os("USERPROFILE")` on Windows and `HOME` elsewhere.
- Use `rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)` so analytics never mutates Codex state.
- Set `busy_timeout` to 2 seconds on the read-only connection. Do not change journal mode or create indexes in the Codex database. If SQLite still returns `SQLITE_BUSY` or `SQLITE_LOCKED`, return a non-zero diagnostic naming the selected database; never convert lock contention into a valid zero-session result.
- Probe for the observed `logs` table and required columns `id`, `ts`, `ts_nanos`, `thread_id`, and `feedback_log_body`. Unknown schemas return a diagnostic and no successful zero-session result. Do not scan arbitrary text columns from unrelated tables.
- Represent sessions explicitly:

```rust
pub struct SessionRef {
    pub provider: ProviderKind,
    pub id: String,
    pub source: SessionSource,
}

pub enum SessionSource {
    ClaudeFile(PathBuf),
    CodexThread { db_path: PathBuf, thread_id: String },
}
```

- Change `SessionProvider` to `discover_sessions(...) -> Result<Vec<SessionRef>>` and `extract_commands(&SessionRef, since_days)`. Claude wraps each JSONL path in `ClaudeFile`; Codex returns one `CodexThread` per distinct `thread_id` in the selected time window.
- Compute the cutoff as an epoch-second value and apply it in SQL with `WHERE ts >= ?`. Never use SQLite file mtime as the command-history filter.
- Discover threads with `SELECT DISTINCT thread_id FROM logs WHERE thread_id IS NOT NULL AND ts >= ? ORDER BY thread_id`.
- Extract rows with `SELECT id, ts, ts_nanos, feedback_log_body FROM logs WHERE thread_id = ? AND ts >= ? AND feedback_log_body LIKE '%ToolCall: shell_command%' ORDER BY ts, ts_nanos, id`.
- Set `ExtractedCommand.session_id` to `thread_id` and `sequence_index` from the ordered rows. Add `occurred_at_unix: Option<i64>` if downstream analytics needs stable cross-provider timestamps.
- Extract the `shell_command` payload's `command` string through structured JSON parsing after locating the payload boundary. Do not parse it with regex-only quote matching.
- Add a diagnostic mode, for example `rtk discover --provider codex --check-provider`, that prints detected candidate paths, which database opened, the observed `logs` columns, and any required columns that are missing. This prevents silent "0 sessions" results when the Codex schema or path changes.
- If a database opens but no compatible shell-command payload is found, return a provider diagnostic stating which tables/columns were scanned. Do not silently report this case as a valid zero-command session.
- In the first batch, `project_filter` is unsupported for Codex unless a tested field can be extracted from every selected row. `--provider codex --project ...` returns exit code `2`. `--provider all --project ...` applies the filter to Claude, skips Codex, and prints an explicit stderr diagnostic; it must not include unfiltered Codex rows.
- Do not claim support for a fixed Codex version range. Compatibility is schema-probed against explicit adapters; fixtures should represent each observed schema variant and diagnostics must make unknown variants actionable.

**Implementation steps:**

1. Introduce `SessionRef` and `SessionSource`, migrate `ClaudeProvider` without changing its current file discovery or extraction behavior, and run all existing Claude provider tests.
2. Add `CodexProvider` fixtures that create the exact observed `logs` columns in a temporary SQLite database.
3. Implement read-only schema validation and return an actionable diagnostic when any required table or column is missing.
4. Implement row-level cutoff filtering and distinct-thread discovery with the SQL above.
5. Implement per-thread command extraction ordered by `(ts, ts_nanos, id)`, structured payload parsing, and malformed-row skipping.
6. Execute row iteration with `query_map` or `query_and_then`; do not collect raw SQLite rows or full feedback bodies into an intermediate `Vec` before extracting commands.
7. Add an explicit adapter fixture for each newly observed future schema; do not fall back to arbitrary text-column scanning.
8. Update `discover::run` so provider selection decides between `ClaudeProvider`, `CodexProvider`, or both.
9. Update `session_cmd::run` to accept the same provider selector and group Codex output by `thread_id`.
10. Make `--project` with Codex fail clearly until a complete, tested workdir/project extraction strategy exists.

**Unit tests:**

| Test | Expected |
|------|------|
| `codex_missing_db_returns_empty_sessions` | No error, zero sessions |
| `codex_extracts_shell_command_payload` | One `ExtractedCommand.command` equals the stored command |
| `codex_skips_malformed_payload` | Bad rows do not stop the scan |
| `codex_since_filter_excludes_old_rows_in_recent_db` | rows older than the SQL cutoff are excluded even when the database file mtime is recent |
| `codex_groups_rows_by_thread_id` | multiple rows in one thread share one session ID; separate threads remain separate sessions |
| `codex_orders_rows_by_timestamp_nanos_and_id` | sequence order is stable when second-level timestamps collide |
| `discover_provider_all_combines_results` | Claude and Codex providers are both represented |
| `codex_path_override_is_used` | `--codex-path` candidate is tried first |
| `codex_check_provider_reports_schema` | diagnostic output includes selected path and present/missing required `logs` columns |
| `codex_unknown_schema_reports_diagnostic` | an openable but unsupported schema does not silently return a valid zero-command result |
| `codex_schema_variants_extract_commands` | fixtures for each observed schema variant produce the same `ExtractedCommand` shape |
| `codex_does_not_scan_unselected_sqlite_files` | unrelated SQLite files under Codex home are not opened automatically |
| `codex_project_filter_fails_explicitly` | Codex-only project filtering exits 2; provider-all skips Codex with a diagnostic instead of mixing unfiltered rows |
| `codex_wal_writer_and_reader_coexist` | a second connection appends rows in WAL mode while the provider reads a consistent snapshot without corruption |
| `codex_locked_database_is_not_zero_sessions` | lock timeout returns a non-zero diagnostic containing the database path |

**Verification commands:**

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::provider
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test analytics::session_cmd
rtk proxy .\target\debug\rtk.exe discover --provider codex --all --since 30
rtk proxy .\target\debug\rtk.exe session --provider codex
```

**Unsupported in this batch:**

- Full Codex database schema migration logic.
- Reading archived or remote Codex threads.
- Extracting non-shell tools such as file reads or patch edits.

### 3.2 `Get-Content` Compatibility

**Goal:** Route safe `Get-Content` forms to `rtk read` for both `rtk Get-Content ...` and hook rewrite of raw `Get-Content ...`.

**Files:**

| File | Change |
|------|------|
| `src/discover/ps_classify.rs` | Add `parse_get_content`, `rewrite_get_content`, and classification metadata |
| `src/discover/powershell_lexer.rs` | Parse static PowerShell tokens and render generated RTK argv with round-trip validation |
| `src/cmds/system/ps_cmdlet.rs` | Add direct fallback handler that calls `read::run` for one static path; do not map PowerShell `-` to stdin |
| `src/main.rs` | Call direct compatibility handler early in the top-level fallback surfaces (`Commands::Other` and parse-error `run_fallback` where applicable); import the new system module |
| `src/discover/registry.rs` | Call PowerShell compatibility rewrite before generic rules |
| `src/core/line_window.rs` | Add exact skip/take primitives shared by C1 windows and later native head/tail |
| `src/cmds/system/read.rs` | Add explicit exact `--skip-lines` / `--take-lines` options; keep existing smart `--max-lines` semantics unchanged |

**Supported shapes:**

| Input | Direct behavior | Rewrite behavior |
|------|------|------|
| `rtk Get-Content foo.txt` | call `read::run(foo.txt, FilterLevel::None, None, None, false, verbose)` | `Get-Content foo.txt` -> `rtk read foo.txt` |
| `rtk Get-Content -Encoding utf8 foo.txt` | same as above | `Get-Content -Encoding utf8 foo.txt` -> `rtk read foo.txt` |
| `rtk Get-Content foo.txt -Encoding utf8` | same as above | `Get-Content foo.txt -Encoding utf8` -> `rtk read foo.txt` |
| `Get-Content foo.txt \| Select-Object -First 20` | hook-only compound support | add exact `rtk read foo.txt --take-lines 20` support before enabling; do not use smart `--max-lines` |
| `Get-Content foo.txt \| Select-Object -Skip 10 -First 20` | hook-only compound support | add exact `rtk read foo.txt --skip-lines 10 --take-lines 20` support before enabling |

**Parser rules:**

- Direct invocation parses the existing `OsString` argv and performs no shell tokenization.
- Raw hook/rewrite input uses only `src/discover/powershell_lexer.rs`. Do not call `src/discover/lexer.rs::shell_split` or reuse its Bash escape rules.
- Accept command names case-insensitively for the full cmdlet name: `Get-Content` / `get-content`.
- Defer aliases such as `gc`, `cat`, and `type` until a separate alias policy is written. `type` in particular has shell-specific meaning and should remain passthrough in the first batch.
- Accept exactly one static file path. PowerShell `Get-Content -` means a path named `-`; it is not stdin and must not map to `read::run_stdin`.
- For direct `rtk Get-Content`, accept the RTK-only `--` boundary so dash-prefixed filenames are unambiguous; remove it before calling `read::run`. Raw PowerShell rewrite does not invent this boundary because its parser still has the original quoted source.
- Accept `-Encoding` only when the value is `utf8`, `utf8BOM`, or `utf8NoBOM`, case-insensitively.
- `utf8BOM` is accepted for file-inspection compatibility, but RTK output does not preserve or synthesize a BOM. Scripts that inspect BOM bytes must remain transport-only.
- Reject arguments containing `$`, backticks, `$(`, `{`, `}`, or `;`.
- Reject provider paths such as `HKLM:\...`; allow normal Windows drive paths such as `C:\src\a.txt`.
- For rewrite, render the generated argv through `powershell_lexer::render_static_argv`, then parse it again and require an exact argv round trip before appending already-stripped redirect suffixes exactly as `registry.rs` does for other rewrites.
- Document the semantic trade-off: default PowerShell `Get-Content` streams line objects, while `rtk read` reads file text and emits a compact text view. The rewrite is only acceptable for Codex file-inspection shapes, not for scripts that depend on PowerShell line-object pipeline semantics.

**Implementation steps:**

1. Add a PowerShell short-circuit parser in `src/discover/ps_classify.rs` that returns the existing `Classification` data and rewrite string shape; do not create a second long-lived classification pipeline parallel to `Classification` / `RULES`.
2. Implement separate entrypoints `parse_get_content_argv(args: &[OsString])` and `parse_get_content_raw(raw: &str)`. The raw entrypoint delegates only to `powershell_lexer`; both produce `GetContentSpec { file, max_lines, skip, tail }`.
3. Implement direct `run_get_content(args, verbose) -> CompatDirectResult` in `src/cmds/system/ps_cmdlet.rs`.
4. Normalize direct handler return types: `read::run` returns `Result<()>`, while `search::run` / `ls::run` return `Result<i32>`. `CompatDirectResult::Handled(code)` should return `0` for successful `Result<()>` calls and preserve explicit `i32` exit codes from commands that have them.
5. In the top-level fallback path, if `args[0]` is a known PowerShell command, call the direct handler; on unsupported shape, continue to Windows transport validation. Execute only `PowerShellTransport`; return the `RejectAmbiguous` diagnostic otherwise. Cover both `Commands::Other` and parse-error `run_fallback` surfaces if both can see the shape.
6. Add compound rewrite detection for the exact `Get-Content ... | Select-Object ...` shapes using the PowerShell lexer before generic compound handling. Recognize only an unquoted `|`; if any token uses interpolation, backtick escaping, a scriptblock, or cannot round-trip, return no rewrite before the generic logic can rewrite only the left side.
7. Add exact `read --skip-lines N --take-lines M` support through `core::line_window`; `--take-lines` emits only selected lines and no omission marker. Do not overload or rename existing smart `--max-lines`. Enable either compound rewrite only after exact-window tests pass.

**Unit tests:**

| Test | Expected |
|------|------|
| `rewrite_get_content_basic` | `Some("rtk read foo.txt")` |
| `rewrite_get_content_encoding_prefix` | `Some("rtk read foo.txt")` |
| `rewrite_get_content_encoding_suffix` | `Some("rtk read foo.txt")` |
| `rewrite_get_content_raw_passthrough` | `None` |
| `rewrite_get_content_tail_passthrough` | `None` |
| `rewrite_get_content_dynamic_path_passthrough` | `None` |
| `rewrite_get_content_dash_is_not_stdin` | `Get-Content -` does not map to `rtk read -` |
| `direct_get_content_dash_prefixed_path_uses_boundary` | `rtk Get-Content -- -Raw` reads a file named `-Raw`; without `--`, `-Raw` remains the cmdlet switch |
| `rewrite_get_content_quoted_path_round_trips` | spaces, apostrophes, Unicode, UNC paths, and trailing backslashes survive parse-render-parse |
| `rewrite_get_content_interpolated_path_passthrough` | double-quoted `$env:` and backtick-containing paths return `None` |
| `rewrite_get_content_first_uses_exact_take` | compound `-First N` targets `--take-lines N`, not smart `--max-lines` |
| `read_exact_take_has_no_omission_marker` | exact take emits only selected lines |
| `read_exact_skip_take_window` | skip N then emit at most M lines with correct zero-based skipped count |
| `direct_get_content_reads_file` | stdout contains file content |
| `direct_get_content_unsupported_falls_back` | handler returns `Unsupported`, not an error |
| `direct_get_content_raw_uses_transport_schema` | `rtk Get-Content -Raw file` bypasses semantic optimization and executes through validated PowerShell transport |
| `direct_and_rewrite_get_content_match` | direct `rtk Get-Content foo.txt` output matches executing `rtk read foo.txt` |

**Verification commands:**

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::ps_classify
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::registry::tests::rewrite_get_content
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::ps_cmdlet
rtk proxy .\target\debug\rtk.exe rewrite Get-Content Cargo.toml
rtk proxy .\target\debug\rtk.exe Get-Content Cargo.toml
rtk proxy .\target\debug\rtk.exe Get-Content -Encoding utf8 Cargo.toml
```

**Unsupported in this batch:**

- `-Raw`, `-Tail`, wildcard input, multiple files, `-ReadCount`, `-TotalCount`, dynamic paths, and arbitrary pipelines.

### 3.3 `Select-String` Compatibility

**Goal:** Route safe PowerShell grep-equivalent commands to `rtk grep` while preserving the important default case-insensitive behavior.

**Files:**

| File | Change |
|------|------|
| `src/discover/ps_classify.rs` | Add `parse_select_string` and rewrite generation |
| `src/discover/powershell_lexer.rs` | Reuse the static PowerShell token and argv renderer contract from 3.2 |
| `src/cmds/system/ps_cmdlet.rs` | Add direct handler that calls `search::run(Engine::Grep, ...)` |
| `src/main.rs` | Reuse the direct compatibility interception from 3.2 |
| `src/cmds/system/search.rs` | Verify the Windows Rust grep fallback is active before enabling this mapping; fixed-string mode should use escaped regex because fallback rejects `-F` |

**Supported shapes:**

| Input | Mapping |
|------|------|
| `Select-String -Pattern NEEDLE -Path src\a.rs` | `rtk grep -i NEEDLE src\a.rs` |
| `Select-String NEEDLE src\a.rs` | `rtk grep -i NEEDLE src\a.rs` |
| `Select-String -CaseSensitive -Pattern NEEDLE -Path src\a.rs` | `rtk grep NEEDLE src\a.rs` |
| `Select-String -SimpleMatch -Pattern a.b -Path src\a.rs` | `rtk grep -i a\.b src\a.rs` using `regex::escape` |

**Parser rules:**

- PowerShell `Select-String` is case-insensitive by default; add `-i` unless `-CaseSensitive` is present.
- Accept `-Pattern <value>` and `-Path <value>`.
- Accept two positional values as pattern then path when neither starts with `-`.
- Accept one path only; multiple paths remain raw fallback.
- For `-SimpleMatch`, escape the pattern with `regex::escape` instead of relying on grep `-F`, because Windows native grep fallback currently rejects `-F`.
- Reject `-Context`, `-AllMatches`, `-List`, `-NotMatch`, `-Quiet`, and pipeline input.

**Implementation steps:**

1. Add `SelectStringSpec { pattern, path, ignore_case, simple_match }`.
2. Implement direct `parse_select_string_argv` and raw `parse_select_string_raw`; the raw form accepts only tokens that satisfy the PowerShell static-token contract.
3. Generate argv in a stable order, `rtk grep [-i] <pattern> <path>`, then render with `powershell_lexer::render_static_argv` and require an exact round trip. Never interpolate pattern or path directly into a format string.
4. In the direct handler, call `search::run(Engine::Grep, default_max_len, default_max_results, false, &args, verbose)`. Use the same limit values as the `Commands::Grep` dispatch arm in `src/main.rs`; do not invent new defaults.
5. Add classification metadata with category `Files` and savings around the existing grep rule.
6. Add an implementation gate that runs `rtk grep -i <pattern> <file>` on native Windows without `grep.exe` available. The current codebase has a Rust fallback for this path; the task still needs a smoke test so future regressions do not silently break `Select-String`.

**Unit tests:**

| Test | Expected |
|------|------|
| `rewrite_select_string_named` | `rtk grep -i NEEDLE src/a.rs` |
| `rewrite_select_string_positional` | `rtk grep -i NEEDLE src/a.rs` |
| `rewrite_select_string_case_sensitive` | `rtk grep NEEDLE src/a.rs` |
| `rewrite_select_string_simple_match_escapes_regex` | pattern `a.b` becomes `a\.b` |
| `rewrite_select_string_quoted_values_round_trip` | spaces, apostrophes, empty patterns, Unicode, and trailing backslashes are rendered as the same argv |
| `rewrite_select_string_interpolation_passthrough` | interpolated or backtick-containing raw patterns return `None` |
| `rewrite_select_string_context_passthrough` | `None` |
| `direct_select_string_matches_file` | exit code 0 and compact grep output |
| `direct_select_string_no_match` | exit code 1 |
| `direct_select_string_context_uses_transport` | `-Context` is not optimized but passes the cmdlet transport schema |
| `select_string_windows_uses_native_grep_fallback` | succeeds on Windows when `grep.exe` is absent |

**Verification commands:**

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::ps_classify
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::ps_cmdlet
rtk proxy .\target\debug\rtk.exe rewrite "Select-String -Pattern NEEDLE -Path docs\rtk-codex-windows-native-next-plan.md"
rtk proxy .\target\debug\rtk.exe Select-String -Pattern RTK -Path docs\rtk-codex-windows-native-next-plan.md
rtk proxy .\target\debug\rtk.exe Select-String -CaseSensitive -Pattern RTK -Path docs\rtk-codex-windows-native-next-plan.md
```

**Unsupported in this batch:**

- Object pipeline input, context objects, `-List`, `-Quiet`, `-AllMatches`, `-NotMatch`, and multiple paths.

### 3.4 `Get-ChildItem` Compatibility

**Goal:** Route only safe non-recursive filesystem listing shapes to the existing local Windows-native `rtk ls`.

**Files:**

| File | Change |
|------|------|
| `src/discover/ps_classify.rs` | Add `parse_get_child_item` and rewrite generation |
| `src/discover/powershell_lexer.rs` | Reuse the static PowerShell token and round-trip renderer contract |
| `src/cmds/system/ps_cmdlet.rs` | Add direct handler that delegates only to `ls::run` for semantically supported shapes |
| `src/cmds/system/ls.rs` | No change for basic listing |
| `src/main.rs` | Reuse direct compatibility interception |

**Supported shapes:**

| Input | Mapping |
|------|------|
| `Get-ChildItem` | `rtk ls` |
| `Get-ChildItem src` | `rtk ls src` |
| `Get-ChildItem -Path src` | `rtk ls src` when `src` has no wildcard characters |
| `Get-ChildItem -LiteralPath src` | `rtk ls src` with literal-path semantics |
| `Get-ChildItem -Force src` | `rtk ls -a src` |

**Parser rules:**

- Accept command names case-insensitively for the full cmdlet name: `Get-ChildItem`.
- Defer aliases such as `gci`, `ls`, and `dir`. `ls` already has Unix-style RTK semantics, and `dir` has broad `cmd.exe` / PowerShell behavior. Alias takeover needs a separate explicit policy.
- Accept zero or one path, either positional, `-Path`, or `-LiteralPath`.
- Treat `-LiteralPath` as literal. Treat both positional paths and `-Path` values as rewrite-safe only when the value contains no PowerShell wildcard characters such as `*`, `?`, `[`, or `]`; wildcard positional or `-Path` values must remain transport-only.
- Accept `-Force` as `-a`.
- Reject all `-Recurse` and `-Filter` semantic rewrites in this batch. They may use the C0.5 transport schema, but must not delegate to `rtk find`.
- Reject `-Name` unless `rtk ls` gains a tested `--name-only` mode.
- Reject `Where-Object`, `ForEach-Object`, `-Include`, `-Exclude`, `-Directory`, `-File`, and provider paths.

**Implementation steps:**

1. Add `GetChildItemSpec { path, force }` for semantic rewrites. Keep transport-only parameters in `CmdletTransportSpec`, not this type.
2. Map non-recursive shapes to `rtk ls` args: `[]`, `[path]`, `["-a", path]`.
3. Render rewrite argv with `powershell_lexer::render_static_argv` and require an exact round trip.
4. Return `Unsupported` for wildcard `-Path`, `-Recurse`, `-Filter`, and other non-optimized parameters so C0.5 can transport schema-valid forms without semantic rewriting.

**Unit tests:**

| Test | Expected |
|------|------|
| `rewrite_get_child_item_empty` | `rtk ls` |
| `rewrite_get_child_item_path` | `rtk ls src` |
| `rewrite_get_child_item_named_path` | `Get-ChildItem -Path src` -> `rtk ls src` |
| `rewrite_get_child_item_literal_path` | `Get-ChildItem -LiteralPath src` -> `rtk ls src` |
| `rewrite_get_child_item_wildcard_path_passthrough` | `Get-ChildItem -Path *.rs` -> `None`; wildcard matching is PowerShell semantics |
| `rewrite_get_child_item_positional_wildcard_passthrough` | `Get-ChildItem *.rs` -> `None`; positional path follows `-Path`, not `-LiteralPath` |
| `rewrite_get_child_item_force` | `rtk ls -a src` |
| `rewrite_get_child_item_recurse_filter_passthrough` | `None`; do not map to the current ignore-aware, file-only `rtk find` |
| `rewrite_get_child_item_name_passthrough` | `None` |
| `rewrite_get_child_item_provider_path_passthrough` | `None` |
| `direct_get_child_item_lists_directory` | exit code 0 |
| `rewrite_get_child_item_quoted_path_round_trip` | spaces, apostrophes, Unicode, UNC paths, and trailing backslashes preserve argv |
| `direct_get_child_item_recurse_uses_transport` | recursive/filter argv is not delegated to `rtk find` and executes through validated PowerShell transport |

**Verification commands:**

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::ps_classify
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::ps_cmdlet
rtk proxy .\target\debug\rtk.exe rewrite Get-ChildItem
rtk proxy .\target\debug\rtk.exe rewrite "Get-ChildItem -Recurse -Filter *.rs src"  # expected: no semantic rewrite
rtk proxy .\target\debug\rtk.exe Get-ChildItem src
rtk proxy .\target\debug\rtk.exe Get-ChildItem -Force src
```

**Unsupported in this batch:**

- wildcard `-Path`, `-Name`, all recursive/filter listings, provider paths, object projections, and multiple paths.

### 3.5 `Get-Command`, `where.exe`, And `which`

**Goal:** Provide one stable native executable-resolution command and map only discovery shapes that explicitly request PATH applications.

**Files:**

| File | Change |
|------|------|
| `src/cmds/system/which.rs` | New native implementation |
| `src/main.rs` | Add `Which` subcommand, import module, dispatch arm, add `which` to `RTK_META_COMMANDS`, and update command-classification tests |
| `src/discover/rules.rs` | Remove `which ` from ignored prefixes and add a `which` rule if generic rule matching is sufficient |
| `src/discover/ps_classify.rs` | Add `Get-Command` shape-aware rewrite; defer `where.exe` until current-directory semantics are pinned |
| `src/cmds/system/ps_cmdlet.rs` | Direct fallback for `rtk Get-Command ...` |

**Supported shapes:**

| Input | Mapping |
|------|------|
| `rtk which cargo` | print first resolved path, exit 0 |
| `which cargo` | `rtk which cargo` |
| `Get-Command -CommandType Application cargo` | `rtk which cargo` |
| `Get-Command -CommandType Application -Name cargo` | `rtk which cargo` |

**Implementation details:**

- Use `crate::core::utils::resolve_binary(name)` because it already honors PATHEXT on Windows through the `which` crate.
- Print the resolved path with one trailing newline.
- On missing command, print `rtk which: <name> not found` to stderr and return exit code `1`.
- Reject names containing path separators for the first batch; command availability checks should resolve names from PATH, not arbitrary files.
- Reject wildcard names, multiple names, mixed `-CommandType` values, `Get-Command -Module`, `Get-Command -Syntax`, and every bare `Get-Command` semantic rewrite.
- `-Syntax` remains allowed only by the transport schema because it changes the PowerShell output contract to syntax information rather than executable path discovery.
- Document the behavior difference: `rtk which` resolves executables from PATH via the `which` crate. It does not implement PowerShell alias/function lookup or `where.exe` modes such as current-directory search and `/R`.
- Keep `where.exe` passthrough in the first batch. Add its rewrite only after tests pin current-directory and all-match behavior.
- Add `which` to `RTK_META_COMMANDS`. `test_every_subcommand_is_classified` requires every new `Commands` variant to be registered in `RTK_META_COMMANDS` or the test-local `PASSTHROUGH` list; `which` is an RTK-native command and should fail closed on invalid syntax.

**Implementation steps:**

1. Create `src/cmds/system/which.rs` with `pub fn run(name: &str) -> Result<i32>`.
2. Add `Commands::Which { name: String }` in `src/main.rs`.
3. Add dispatch `Commands::Which { name } => which::run(&name)?`.
4. Add `which` to `RTK_META_COMMANDS` and keep `test_every_subcommand_is_classified` passing.
5. Add rewrite support for raw `which <name>` and exact `Get-Command -CommandType Application [ -Name ] <name>` forms. Render generated argv through the PowerShell renderer and require a round trip.
6. Remove `which ` from `IGNORED_PREFIXES`.

**Unit tests:**

| Test | Expected |
|------|------|
| `which_resolves_rtk_or_cargo` | found command exits 0 and prints a path |
| `which_missing_returns_1` | missing command exits 1 |
| `rewrite_which` | `which cargo` -> `rtk which cargo` |
| `rewrite_where_exe_passthrough` | `where.exe cargo` -> `None` in the first batch |
| `rewrite_get_command_application` | `Get-Command -CommandType Application cargo` -> `rtk which cargo` |
| `rewrite_get_command_bare_passthrough` | `Get-Command cargo` -> `None` |
| `rewrite_get_command_alias_candidate_passthrough` | `Get-Command ls` -> `None` |
| `rewrite_get_command_module_passthrough` | `None` |
| `rewrite_get_command_syntax_passthrough` | `Get-Command -Syntax cargo` -> `None`; validated transport only |
| `direct_get_command_application_uses_which` | explicit `-CommandType Application` calls the native resolver |
| `direct_get_command_bare_uses_transport` | bare `rtk Get-Command cargo` is not intercepted semantically and reaches validated PowerShell transport |

**Verification commands:**

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::which
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::ps_classify
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::registry
rtk proxy .\target\debug\rtk.exe which cargo
rtk proxy .\target\debug\rtk.exe rewrite "Get-Command -CommandType Application cargo"
rtk proxy .\target\debug\rtk.exe rewrite "Get-Command cargo"  # expected: no semantic rewrite
rtk proxy .\target\debug\rtk.exe rewrite "where.exe cargo"  # expected: no rewrite in first batch
```

**Unsupported in this batch:**

- Bare `Get-Command`, all matches, aliases, functions, modules, scripts outside PATH resolution, and wildcard discovery.

### 3.6 `head`

**Goal:** Add an exact, streaming `rtk head` entrypoint. Do not reuse `rtk read --max-lines`, because that path intentionally inserts a smart-truncation marker.

**Files:**

| File | Change |
|------|------|
| `src/cmds/system/head_tail.rs` | New shared parser and runners for `head` and `tail` |
| `src/core/line_window.rs` | Reuse the exact skip/take primitive introduced by C1 and add bounded head streaming plus bounded-memory tail buffering |
| `src/core/mod.rs` | Export `line_window` |
| `src/main.rs` | Add `Head` subcommand, import module, dispatch arm, add `head` to `RTK_META_COMMANDS`, and update classification tests |
| `src/discover/registry.rs` | Extend `rewrite_line_range` to support default `head <file>` if not already covered |
| `src/discover/rules.rs` | Existing `head` rule can stay; update ignored/rule tests if behavior changes |
| `src/cmds/system/read.rs` | No semantic change; keep its smart-truncation behavior separate from exact head/tail behavior |

**Supported shapes:**

| Input | Behavior |
|------|------|
| `rtk head foo.txt` | first 10 lines |
| `rtk head -n 20 foo.txt` | first 20 lines |
| `rtk head -20 foo.txt` | first 20 lines |
| `rtk head --lines=20 foo.txt` | first 20 lines |
| `rtk head --lines 20 foo.txt` | first 20 lines |
| `head foo.txt` | rewrite only to `rtk head foo.txt` |

**Parser rules:**

- Accept exactly one file or `-` for stdin.
- Reject multiple files to avoid missing Unix banner semantics.
- Reject byte mode `-c`, quiet/verbose headers, and negative counts.
- Decode text as UTF-8 consistently with `rtk read`; invalid UTF-8 returns a clear non-zero error in this batch.
- Preserve each line terminator exactly as returned by `BufRead::read_line`.
- Do not emit omission markers, summaries, or tee hints on successful head output.

**Implementation steps:**

1. Create `HeadTailMode::Head` and `LineWindowSpec { lines, file }` in `src/cmds/system/head_tail.rs`.
2. Parse default count as `10`.
3. Add `line_window::write_head<R: BufRead, W: Write>(reader, writer, lines)`. Read and write at most N lines, then return without waiting for EOF.
4. File mode opens `BufReader<File>`; stdin mode locks stdin and calls the same primitive. A `0` count writes nothing and returns immediately.
5. Rewrite `head` shapes only to `rtk head`, never to `rtk read --max-lines`.
6. Keep `head file1 file2` passthrough.
7. Track head as the requested command output, not as semantic token compression; do not claim savings from the unread file tail.
8. Add `head` to `RTK_META_COMMANDS`; the new native `Commands::Head` variant must satisfy `test_every_subcommand_is_classified` and fail closed on invalid syntax.
9. Add a consistency test: direct `rtk head -n N file` must match executing the rewrite target for `head -n N file`.

**Unit tests:**

| Test | Expected |
|------|------|
| `parse_head_default` | lines `10`, file `foo.txt` |
| `parse_head_dash_n` | lines `20` |
| `parse_head_compact_dash_number` | lines `20` |
| `parse_head_multiple_files_rejected` | error |
| `rewrite_head_default` | bounded RTK rewrite |
| `rewrite_head_multiple_files_passthrough` | `None` |
| `head_output_has_no_omission_marker` | output contains exactly the first N input lines |
| `head_empty_file_is_empty` | an empty file succeeds with zero stdout bytes and no omission marker |
| `head_stdin_stops_after_n_lines` | a controlled reader that has more data is read only until N line terminators, without requiring EOF |
| `head_zero_does_not_read_stdin` | count 0 returns immediately with empty output |

**Verification commands:**

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::head_tail
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::registry::tests::test_rewrite_head
rtk proxy .\target\debug\rtk.exe head Cargo.toml
rtk proxy .\target\debug\rtk.exe head -n 5 Cargo.toml
rtk proxy .\target\debug\rtk.exe rewrite "head -5 Cargo.toml"
```

**Unsupported in this batch:**

- Byte counts, multiple files, and header formatting.

### 3.7 `tail`

**Goal:** Add an exact `rtk tail` entrypoint using a bounded-memory line ring. Tail reads to EOF by definition but must not use smart truncation or load the entire input into one string.

**Files:**

| File | Change |
|------|------|
| `src/cmds/system/head_tail.rs` | Add tail mode to the shared parser |
| `src/core/line_window.rs` | Add exact bounded-memory tail buffering to the shared primitive module |
| `src/main.rs` | Add `Tail` subcommand, import module, dispatch arm, add `tail` to `RTK_META_COMMANDS`, and update classification tests |
| `src/discover/registry.rs` | Extend or keep existing tail line-window rewrite |
| `src/cmds/system/read.rs` | No semantic change; exact tail remains separate from `read` filtering/tracking |

**Supported shapes:**

| Input | Behavior |
|------|------|
| `rtk tail foo.txt` | last 10 lines |
| `rtk tail -n 20 foo.txt` | last 20 lines |
| `rtk tail -20 foo.txt` | last 20 lines |
| `rtk tail --lines=20 foo.txt` | last 20 lines |
| `rtk tail --lines 20 foo.txt` | last 20 lines |
| `tail foo.txt` | rewrite to bounded RTK tail/read form |

**Parser rules:**

- Accept exactly one file or `-` for stdin.
- Reject `-f` / `--follow` with a clear message: `rtk tail: follow mode is unsupported`.
- Reject byte mode and multiple files.
- Use `line_window::write_tail` and preserve exact line text without omission markers.

**Implementation steps:**

1. Reuse the parser from task 3.6 with `HeadTailMode::Tail`.
2. Add explicit `-f` detection before numeric parsing.
3. Add `line_window::write_tail<R: BufRead, W: Write>(reader, writer, lines)` backed by `VecDeque<String>` capped at N entries. A `0` count drains no content and writes nothing; for stdin, document that tail still waits for EOF when N is greater than zero.
4. Add direct dispatch in `src/main.rs`.
5. Rewrite supported tail shapes only to `rtk tail`, not `rtk read`.
6. Keep existing shape tests for `tail -n`, `tail --lines`, and `tail -20` but update expected targets to `rtk tail`.
7. Add default `tail <file>` rewrite only if the test suite confirms no conflict with pipe behavior.
8. Add `tail` to `RTK_META_COMMANDS`; the new native `Commands::Tail` variant must satisfy `test_every_subcommand_is_classified` and fail closed on invalid syntax.
9. Add a consistency test: direct `rtk tail -n N file` must match executing the rewrite target for `tail -n N file`.

**Unit tests:**

| Test | Expected |
|------|------|
| `parse_tail_default` | lines `10`, file `foo.txt` |
| `parse_tail_dash_n` | lines `20` |
| `parse_tail_follow_rejected` | error message mentions follow mode |
| `parse_tail_multiple_files_rejected` | error |
| `rewrite_tail_default` | bounded RTK rewrite |
| `rewrite_tail_follow_passthrough` | `None` |
| `tail_output_has_no_omission_marker` | output contains exactly the last N lines |
| `tail_empty_file_is_empty` | an empty file succeeds with zero stdout bytes and no omission marker |
| `tail_memory_is_bounded_by_line_count` | the ring never retains more than N line entries |
| `tail_zero_writes_nothing` | count 0 returns empty output |

**Verification commands:**

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::head_tail
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::registry::tests::test_rewrite_tail
rtk proxy .\target\debug\rtk.exe tail Cargo.toml
rtk proxy .\target\debug\rtk.exe tail -n 5 Cargo.toml
rtk proxy .\target\debug\rtk.exe rewrite "tail -n 5 Cargo.toml"
```

**Unsupported in this batch:**

- Follow mode, byte counts, multiple files, and header formatting.

### 3.8 `pwd`

**Goal:** Provide a stable native current-directory command.

**Files:**

| File | Change |
|------|------|
| `src/cmds/system/pwd.rs` | New native implementation |
| `src/main.rs` | Add `Pwd` subcommand, import module, dispatch arm, add `pwd` to `RTK_META_COMMANDS`, and update classification tests |
| `src/discover/rules.rs` | Remove `pwd` from ignored exact/prefix lists and add exact rewrite rule |
| `src/discover/registry.rs` | No special parser needed if the exact rule is sufficient |

**Supported shapes:**

| Input | Behavior |
|------|------|
| `rtk pwd` | print current directory absolute path |
| `pwd` | rewrite to `rtk pwd` |

**Implementation details:**

- Use `std::env::current_dir()`.
- Print one path line with `println!("{}", cwd.display())`.
- Reject all arguments; `rtk pwd -P` and `rtk pwd -L` are unsupported in this batch.
- Add `pwd` to `RTK_META_COMMANDS`. The new RTK-native variant must satisfy `test_every_subcommand_is_classified` and should not fall back to an external `pwd` after an invalid argument.

**Unit tests:**

| Test | Expected |
|------|------|
| `pwd_prints_current_dir` | output equals `std::env::current_dir()` |
| `rewrite_pwd_exact` | `Some("rtk pwd")` |
| `rewrite_pwd_with_args_passthrough` | `None` |
| `classify_pwd_supported` | category `System`, equivalent `rtk pwd` |

**Verification commands:**

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::pwd
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::registry::tests::rewrite_pwd
rtk proxy .\target\debug\rtk.exe pwd
rtk proxy .\target\debug\rtk.exe rewrite pwd
```

**Unsupported in this batch:**

- Logical vs physical path flags and shell-specific path aliases.

### 3.9 `touch`

**Goal:** Provide a conservative native file timestamp/create command for agent setup steps.

**Files:**

| File | Change |
|------|------|
| `src/cmds/system/touch.rs` | New native implementation |
| `src/main.rs` | Add `Touch` subcommand, import module, dispatch arm, add `touch` to `RTK_META_COMMANDS`, and update classification tests |
| `src/discover/rules.rs` | Keep raw `touch ` ignored in the first batch; remove it only if a later rewrite phase proves value |
| `Cargo.toml` | No new dependency required for basic mtime updates; use `std::fs::File::set_modified(SystemTime::now())` |

**Supported shapes:**

| Input | Behavior |
|------|------|
| `rtk touch foo.txt` | create missing file or update mtime |
| `touch foo.txt` | deferred raw rewrite; direct `rtk touch foo.txt` is the first-batch portability surface |

**Parser rules:**

- Accept exactly one path.
- Reject directories with `rtk touch: <path> is a directory`.
- Reject flags in this batch, including `-a`, `-m`, `-t`, `-r`, and `--date`.
- Reject wildcard and dynamic-looking paths in hook rewrite.
- Do not impose workspace containment or reject `..` / absolute paths for the explicit `rtk touch` command; RTK is a general CLI and should preserve normal filesystem path semantics. Safety comes from keeping raw automatic rewrite disabled and requiring an explicit RTK mutation command.
- Document that filesystem symlink/reparse-point behavior follows the operating system and `File::set_modified`; do not claim a stronger sandbox boundary.

**Implementation steps:**

1. Create the file with `OpenOptions::new().create(true).append(true).open(path)`.
2. If the file already exists, update mtime with `File::set_modified(SystemTime::now())`.
3. Do not write bytes to existing files.
4. Return exit code `0` on create or timestamp update; return `1` on filesystem errors.
5. Do not enable raw `touch` rewrite in the first batch. This command has little output to compress and mutates the filesystem; direct `rtk touch` is the explicit portability surface.
6. Add `touch` to `RTK_META_COMMANDS`; invalid direct syntax must fail closed instead of attempting an external `touch` fallback.

**Unit tests:**

| Test | Expected |
|------|------|
| `touch_creates_missing_file` | file exists after run |
| `touch_preserves_existing_content` | content unchanged |
| `touch_updates_mtime` | set the fixture mtime to a fixed old `SystemTime`, run touch, and assert it becomes newer; do not rely on sleeps or filesystem timestamp granularity |
| `touch_rejects_directory` | exit code 1 |
| `touch_readonly_failure_is_clear` | read-only/permission failure returns non-zero with the path on stderr |
| `rewrite_touch_static_path` | `None` in the first batch |
| `rewrite_touch_flag_passthrough` | `None` |

**Verification commands:**

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::touch
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::registry::tests::rewrite_touch
rtk proxy .\target\debug\rtk.exe touch target\rtk-touch-smoke.txt
rtk proxy .\target\debug\rtk.exe rewrite "touch target\rtk-touch-smoke.txt"  # expected: no rewrite in first batch
```

**Unsupported in this batch:**

- Multiple files, timestamp parsing, reference-file timestamps, access-time-only updates, and directory timestamp updates.

### 3.10 `mkdir -p`

**Goal:** Provide the one mutating directory creation shape agents most commonly use.

**Files:**

| File | Change |
|------|------|
| `src/cmds/system/mkdir.rs` | New native implementation |
| `src/main.rs` | Add `Mkdir` subcommand, import module, dispatch arm, add `mkdir` to `RTK_META_COMMANDS`, and update classification tests |
| `src/discover/rules.rs` | Keep raw `mkdir ` ignored in the first batch; remove it only if a later rewrite phase proves value |
| `src/discover/registry.rs` | Add shape guard only if raw `mkdir -p` rewrite is enabled later |

**Supported shapes:**

| Input | Behavior |
|------|------|
| `rtk mkdir -p a\b\c` | create nested directory; existing directory succeeds |
| `rtk mkdir --parents a\b\c` | same |
| `mkdir -p a\b\c` | deferred raw rewrite; keep passthrough/ignored in the first batch |

**Parser rules:**

- Require `-p` or `--parents`.
- Accept exactly one path.
- Reject no-flag `mkdir <path>` because normal `mkdir` errors when parent directories are missing and has different shell behavior.
- Reject glob or dynamic-looking paths in hook rewrite.
- If the target exists as a file, print a clear error and return exit code `1`.
- Do not impose workspace containment or reject `..` / absolute paths for explicit `rtk mkdir -p`; preserve `create_dir_all` path semantics. Keep raw automatic rewrite disabled so mutation always requires an explicit RTK command.

**Implementation steps:**

1. Create `MkdirSpec { path }`.
2. Parse only `-p <path>`, `--parents <path>`, and `<path> -p`.
3. Call `std::fs::create_dir_all(&path)`.
4. After creation, verify `path.is_dir()`; if not, return a clear error.
5. Do not enable raw `mkdir -p` rewrite in the first batch. This command has little output to compress and mutates the filesystem; direct `rtk mkdir -p` is the portability surface.
6. Add `mkdir` to `RTK_META_COMMANDS`; invalid direct syntax must fail closed instead of attempting an external `mkdir` fallback.

**Unit tests:**

| Test | Expected |
|------|------|
| `mkdir_p_creates_nested_path` | nested directory exists |
| `mkdir_p_existing_directory_succeeds` | exit code 0 |
| `mkdir_without_p_rejected` | exit code 2 or handler unsupported |
| `mkdir_p_existing_file_fails` | exit code 1 |
| `rewrite_mkdir_p_static_path` | `None` in the first batch |
| `rewrite_mkdir_without_p_passthrough` | `None` |

**Verification commands:**

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::mkdir
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::registry::tests::rewrite_mkdir
rtk proxy .\target\debug\rtk.exe mkdir -p target\rtk-mkdir-smoke\a\b
rtk proxy .\target\debug\rtk.exe rewrite "mkdir -p target\rtk-mkdir-smoke\a\b"  # expected: no rewrite in first batch
```

**Unsupported in this batch:**

- Multiple paths, mode flags, verbose flags, and no-parent `mkdir` emulation.

### 3.11 Cross-Cutting Verification For The Batch

Run focused tests as each task lands, then run full Windows verification before replacing any installed binary.

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::powershell_lexer
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::ps_classify
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::registry
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test discover::provider
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test core::windows_shell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test core::line_window
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system::ps_cmdlet
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test cmds::system
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test test_every_subcommand_is_classified
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test analytics::session_cmd
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 build
rtk proxy .\target\debug\rtk.exe discover --provider codex --check-provider
```

Manual smoke checks:

```powershell
rtk proxy .\target\debug\rtk.exe powershell -NoProfile -Command "Write-Output 'hello world'"
rtk proxy .\target\debug\rtk.exe powershell -NoProfile -Command "$env:TEMP"
rtk proxy .\target\debug\rtk.exe cmd /c "echo hello world"
rtk proxy .\target\debug\rtk.exe grep -i package Cargo.toml
rtk proxy .\target\debug\rtk.exe Get-Content Cargo.toml
rtk proxy .\target\debug\rtk.exe Select-String -Pattern package -Path Cargo.toml
rtk proxy .\target\debug\rtk.exe Get-ChildItem src
rtk proxy .\target\debug\rtk.exe Get-Command cargo  # validated transport, not semantic rewrite
rtk proxy .\target\debug\rtk.exe Get-Command -CommandType Application cargo
rtk proxy .\target\debug\rtk.exe where.exe cargo  # C0.5 DirectExternal smoke; not a C1 rewrite
rtk proxy .\target\debug\rtk.exe which cargo
rtk proxy .\target\debug\rtk.exe head -n 5 Cargo.toml
rtk proxy .\target\debug\rtk.exe tail -n 5 Cargo.toml
rtk proxy .\target\debug\rtk.exe pwd
rtk proxy .\target\debug\rtk.exe touch target\rtk-touch-smoke.txt
rtk proxy .\target\debug\rtk.exe mkdir -p target\rtk-mkdir-smoke\a\b
```

Completion criteria for this detailed batch:

1. Every supported direct `rtk <command>` shape works without relying on PowerShell fallback.
2. Every supported raw shape is discoverable and rewritable.
3. Unsupported PowerShell shapes use C0.5 safe transport only when their static argv is provably representable; ambiguous bare syntax fails closed, while explicit shell-host scripts remain passthrough from semantic optimization.
4. Mutating commands only operate on exactly one static path.
5. Existing first-batch Windows-native commands still pass their tests.
6. A rejected known PowerShell shape cannot fall through to generic rewrite rules.
7. Smoke checks execute `target\debug\rtk.exe` through `rtk proxy`; they do not accidentally test the previously installed RTK binary.

---

## 4. Recommended Implementation Order

| Phase | Work | Reason |
|------|------|------|
| B0 | Execute section 2.9 B0 environment validation, exact native tests, smoke checks, and protected-file snapshot | Establishes the one canonical gate proving later ports did not remove `ls/tree/wc/grep/ps/df/du` or dependencies |
| U0-P0 | Restore TOML reversible-lossiness and custom-filter trust hardening as two separate commits | These are high-conflict correctness/security prerequisites and a hard gate for C0.5; each has a behavior-level Plan B in section 2.9 |
| C0 | Add Codex log provider for analysis | Prevents future planning from relying only on Claude Code data; can run in parallel after B0 |
| C0.5-P0 | Windows fallback transport runner using direct upstream argv execution as its external-program branch | Fixes quote/argv/PowerShell script transport without importing upstream `cmd /C` or proxy lexer behavior |
| C1-P0 | `Get-Content` compatibility | Highest observed Codex unsupported pattern |
| U1-P0 | Port upstream grep separator-fidelity fixes into local native grep | Must land before `Select-String` delegates to grep, or the new cmdlet surface would inherit known formatting regressions |
| C1-P1 | `Select-String` compatibility | Windows-native grep equivalent |
| C1-P1 | Non-recursive `Get-ChildItem` compatibility | Delegate only semantically safe filesystem listing shapes to the existing native `rtk ls`; recursive find mapping is removed |
| C1-P1 | `rtk which` + application-only `Get-Command` mapping | Stable PATH executable discovery; bare `Get-Command` and `where.exe` rewrites are deferred |
| U1-P1 | Reconcile Cargo JSON diagnostics, UTF-8 analytics, and ccusage `period` fixes in separate commits | Valuable upstream correctness work, but independent of Windows transport and forbidden from replacing native system modules |
| U2-P2 | Add upstream Git checkout support as a focused feature | Useful but does not block Windows native compatibility; land after the read-only C1 surfaces stabilize |
| C2-P2 | `head` / `tail` | Common Unix small tools with low risk |
| C2-P2 | `pwd` | Simple and stable |
| C2-P3 | `touch` / `mkdir -p` | Useful but mutating, so lower priority |

Every row is a separate commit or review unit. After B0, each U/C phase reruns its focused tests plus the protected Windows-native baseline; phases must not accumulate into one large reconciliation commit.

Hard dependency: C0.5-P0 must not start until both U0 commits land separately and each one reruns the B0 native gate successfully. If either U0 port uses its local Plan B, that substitute behavior and its focused evidence still must land before any Windows fallback transport refactor begins.

### 4.1 Effort And Performance Budget

Effort sizing for sprint planning:

| Phase | Relative effort | Main cost |
|------|------|------|
| B0 | Medium | portable VS/SDK discovery wrapper, exact test enumeration, smoke behavior, protected files, and recorded baseline |
| U0 | Medium to large | two high-conflict manual reconciliations in fallback/TOML and hook/trust code |
| C0.5 | Large | new Windows execution router, host/script routing, encoding, cross-platform regression tests |
| C0 | Medium to large | provider abstraction migration plus row-level SQLite session/time queries and diagnostics |
| C1-P0/P1 | Medium per cmdlet family | shape parsers, dual-surface handlers, negative tests |
| U1/U2 | Small to medium per isolated port | restore final upstream behavior and tests without whole-file replacement |
| C2 | Small to medium per command | head/tail require exact streaming primitives; mutation commands require extra safety tests |

Performance expectations:

- Pure argv classification, literal rendering, and UTF-16LE/base64 encoding must add less than 5 ms in release builds for source command text up to 8 KiB, excluding child-process startup and PATH lookup. Add a focused benchmark or elapsed-time diagnostic; do not infer this from unit-test duration.
- First-batch generated PowerShell source has an RTK-specific 8 KiB UTF-8 transport budget, not an official PowerShell limit, and the complete encoded host command line (including the actual resolved host path) is capped at 30,000 UTF-16 code units. The sizing math is intentional: 8 KiB source becomes about 16 KiB of UTF-16LE bytes and about 21.4 KiB of base64 text before `powershell -NoProfile -EncodedCommand` argv overhead, leaving headroom below CreateProcess's 32,767-unit process command-line limit. Exceeding either RTK budget creates a UTF-8-BOM temporary `.ps1` file and runs it with `-File`; never truncate script text. RTK checks execution policy only for this fallback and may use process-scoped `-ExecutionPolicy Bypass` when required. This neither elevates the process nor changes persisted execution-policy settings.
- Encoding and rendering memory is `O(n)` in source length and may allocate at most the UTF-16 buffer plus one base64 result and final argv strings. Do not retain duplicate joined command strings for execution.
- Direct shell-host execution must not introduce a second shell process.
- `PowerShellTransport` should start exactly one PowerShell host.
- CodexProvider must stream database rows rather than loading the entire database into memory. Aside from returned `ExtractedCommand` values, peak scan memory must be bounded by the largest selected row plus parser scratch space, not total database size.
- Codex diagnostic output reports elapsed time, selected row count, extracted command count, and database size. A scan exceeding 5 seconds prints a performance warning but remains correct; do not fail solely on wall-clock time because storage speed varies.

---

## 5. Rewrite Safety Model

### 5.1 Allowed Rewrite Principles

Only rewrite when all of the following are true:

1. The input command shape is explicitly listed.
2. The path arguments are static strings or plain paths.
3. No script expressions, variables, scriptblocks, or command substitution are present.
4. No unsupported switches are present.
5. The target RTK command has matching tests for the translated behavior.

When these conditions are not met, automatic transport is allowed only for one of the four known cmdlets whose complete argv passes `CmdletTransportSpec`. Other bare commands return `RejectAmbiguous`. Explicit `powershell` / `pwsh` / `cmd` and `rtk run -c` remain available and never report semantic optimization.

### 5.2 Must Stay Passthrough

These should not be rewritten to RTK-native commands in the first batch:

- `Where-Object`
- `ForEach-Object`
- `Measure-Object`
- `Compare-Object`
- arbitrary `Select-Object`
- any command containing `$`, backticks, `$(...)`, `{...}`, `;`, `&&`, or `||`
- PowerShell provider paths such as `HKLM:\...`

When users invoke these through an explicit shell host, for example `rtk powershell -Command "..."`, the Windows fallback transport runner should execute the script safely. They remain passthrough from the optimization perspective.

### 5.3 Compound Pipeline Rule

Only two compound shapes should be considered initially, and only when `powershell_lexer` identifies one unquoted pipe and every token satisfies the static parse-render-parse contract:

- `Get-Content <file> | Select-Object -First N`
- `Get-Content <file> | Select-Object -Skip N -First M`

All other pipelines stay passthrough until a separate design defines object-pipeline behavior. Generic Bash-oriented compound logic must not rewrite the left side after the PowerShell parser has rejected the full command.

### 5.4 Alias Policy

The first batch supports full command or cmdlet names only:

- `Get-Content`
- `Select-String`
- `Get-ChildItem`
- `Get-Command`, but only the explicit `-CommandType Application` semantic shape; other forms are transport-only

Do not add PowerShell or shell aliases such as `gc`, `gci`, `cat`, `type`, `ls`, or `dir` in C1. Future alias support requires all of the following:

1. Codex telemetry shows meaningful usage.
2. The alias is unambiguous in the active shell context.
3. It does not collide with an existing RTK/Unix command surface.
4. Its supported argument shapes have the same whitelist and negative tests as the full cmdlet name.

`type`, `ls`, and `dir` are specifically deferred because their meaning differs across PowerShell, `cmd.exe`, and Unix-like shells.

---

## 6. Test Matrix

### 6.0 Windows Fallback Transport Tests

| Scenario | Expected |
|------|------|
| `rtk powershell -NoProfile -Command "Write-Output 'hello world'"` | output is one line `hello world`; no nested `powershell -Command` quote loss |
| `rtk powershell -NoProfile -Command "$env:TEMP"` | `$env:TEMP` is evaluated by PowerShell, not stripped by RTK |
| `rtk powershell -NoProfile -Command "Get-ChildItem \| Where-Object { $_.Name -match 'src' }"` | object pipeline runs under PowerShell safe transport, not RTK rewrite |
| `rtk pwsh -NoProfile -Command "$PSVersionTable.PSVersion"` | direct argv execution of `pwsh` |
| `rtk cmd /c "echo hello world"` | direct argv execution of `cmd` |
| `rtk powershell -NoProfile -Command "Get-Content 'target\hello world.txt'"` | path with spaces survives transport |
| `rtk powershell -NoProfile -Command "Get-Content 'target\你好.txt'"` | Unicode path survives transport |
| PowerShell literal renderer receives `\\server\share\file.txt` | UNC path is preserved without requiring a live network share |
| PowerShell literal renderer receives `\\?\C:\...` | extended-length path prefix is preserved; live long-path smoke is conditional on host policy |
| explicit shell-host argv contains an empty string | the child receives an empty argument; it is not dropped by joining or splitting |
| explicit shell-host argv contains an embedded double quote, single quote, backtick, dollar sign, braces, semicolon, or ampersand | each value reaches the requested host in its original argv position; RTK performs no second parse |
| direct external argv contains spaces, an empty value, an embedded quote, and a trailing backslash | the child observes the same ordered argv values |
| resolved `.cmd` / `.bat` receives only batch-safe args | empty args, spaces, apostrophes, Unicode, and backslashes pass the child-boundary test without a hand-built `/c` string |
| resolved `.cmd` / `.bat` receives quote, expansion, operator, parenthesis, or newline metacharacters | `BatchTransport` rejects with exit code 2 and explicit `cmd` guidance |
| resolved `.ps1` receives quoted and Unicode arguments through `-File` | each argument reaches the script unchanged |
| bare unresolved command contains scriptblock-like or ambiguous dash-prefixed argv | RTK does not execute a guessed command and prints explicit-shell-host guidance |
| generated implicit PowerShell host argv within encoded budgets | uses `-NoProfile -EncodedCommand` and never contains an RTK-added `-ExecutionPolicy Bypass` |
| generated implicit PowerShell host argv over an encoded budget | uses a UTF-8-BOM temporary `.ps1` with `-File`; an RTK-added process-scoped `-ExecutionPolicy Bypass` is present only when the policy check requires it |
| transport-only execution | no RTK semantic savings claim |
| `rtk proxy` with multiple argv items | behavior remains unchanged and does not enter C0.5 |
| `rtk proxy "<combined command>"` on Windows | remains the existing explicit proxy behavior; C0.5 does not copy its Bash-style splitter or claim this form is PowerShell-safe |
| source scan of Windows fallback sinks | no `args.join(" ")` result is passed to `-Command`, `/c`, or an equivalent Windows shell parser; frozen non-Windows script surfaces are not changed by this plan |
| generated source is larger than 8 KiB UTF-8 | the complete source is written to an automatically cleaned up UTF-8-BOM temporary `.ps1` and executed with `-File`; no text is truncated |
| complete encoded host command line is larger than 30,000 UTF-16 code units | the complete script uses the same temporary-file transport; no prefix or partial command executes |
| non-Windows `Commands::Other` / `Commands::Run` | still use existing `sh -c` behavior |
| non-Windows parse-error `run_fallback` | still executes the resolved program with original argv |

### 6.1 Codex Provider Tests

| Scenario | Expected |
|------|------|
| Codex sqlite missing | no sessions, clear message |
| Codex sqlite has `ToolCall: shell_command {...}` rows | commands extracted |
| malformed JSON in log body | row skipped, scan continues |
| recent SQLite file contains old and new rows | `since_days` filters by row `ts`, not file mtime |
| two `thread_id` values share one database | they appear as two logical sessions and preserve per-thread ordering |
| unrelated SQLite file exists under Codex home | it is not opened without `--codex-path` |
| provider selected explicitly | report says `Codex`, not `Claude Code` |
| both Claude and Codex exist | report can distinguish providers |
| Codex database uses WAL while another connection appends rows | provider reads a consistent snapshot without changing journal mode or corrupting the writer |
| Codex database remains busy or locked after the fixed 2-second timeout | non-zero result names the selected database; it is never reported as a valid zero-session scan |

### 6.2 Rewrite Tests

| Input | Expected |
|------|------|
| `Get-Content foo.txt` | rewrite to `rtk read foo.txt` |
| `Get-Content -Encoding utf8 foo.txt` | rewrite to `rtk read foo.txt` |
| `Get-Content foo.txt -Tail 10` | passthrough |
| `Get-Content -` | no stdin rewrite; transport as a literal path or return its native path error |
| `Get-Content $env:TEMP\a.txt` | passthrough |
| `Select-String -Pattern NEEDLE -Path src/a.rs` | rewrite to `rtk grep NEEDLE src/a.rs` |
| `Select-String -Context 2 NEEDLE src/a.rs` | passthrough |
| `Get-ChildItem src` | rewrite to `rtk ls src` |
| `Get-ChildItem -Path src` | rewrite to `rtk ls src` |
| `Get-ChildItem -LiteralPath src` | rewrite to `rtk ls src` |
| `Get-ChildItem *.rs` | no semantic rewrite; positional path follows `-Path` wildcard semantics |
| `Get-ChildItem -Path *.rs` | no semantic rewrite; wildcard matching belongs to PowerShell transport |
| `Get-ChildItem -Recurse -Filter *.rs src` | no semantic rewrite; current `rtk find` is not equivalent |
| `Get-Command cargo` | no semantic rewrite; bare PowerShell resolution is not application-only |
| `Get-Command -CommandType Application cargo` | rewrite to `rtk which cargo` |
| `Get-Command -Syntax cargo` | no semantic rewrite; syntax output is transport-only |
| `which cargo` | rewrite to `rtk which cargo` |
| `head -n 20 Cargo.toml` | rewrite only to exact `rtk head -n 20 Cargo.toml`, never to smart-truncating `rtk read` |
| `tail -f app.log` | passthrough |

### 6.3 Native Command Tests

| Command | Required scenarios |
|------|------|
| `rtk which` | found executable, missing executable, `.exe` / `.cmd` / `.bat` resolution on Windows |
| `rtk head` | default 10 lines, `-n`, short and empty files, stdin stops after N lines without EOF, exact zero-byte output for an empty file, no omission marker |
| `rtk tail` | default 10 lines, `-n`, empty file, bounded-memory stdin/file buffering, exact zero-byte output for an empty file, no omission marker, reject `-f` |
| `rtk pwd` | prints current directory as one absolute path |
| `rtk touch` | create missing file, preserve existing content, update mtime, reject directories/read-only failures clearly |
| `rtk mkdir` | `-p` creates nested path, existing path succeeds, invalid path fails clearly |
| command classification | `which`, `head`, `tail`, `pwd`, `touch`, and `mkdir` are registered in `RTK_META_COMMANDS`; `test_every_subcommand_is_classified` passes |

### 6.4 Upstream Absorption And Native Preservation Tests

| Gate | Required evidence |
|------|------|
| protected native modules | focused tests for local `ls`, `tree`, `wc`, grep fallback, `ps`, `df`, and `du` pass before and after every absorbed patch |
| dependency preservation | `Cargo.toml` and `Cargo.lock` still contain dependencies required by local native modules, including `sysinfo` |
| TOML safety | lossy successful filters either emit a valid recovery hint or fall back to guarded raw output; no unrecoverable truncation marker |
| grep fidelity | plain grep emits no synthetic `--`; context mode preserves separators between non-adjacent groups; native Windows grep follows the same contract |
| trust hardening | malformed trust data fails visibly, non-interactive trust fails closed, and already-trusted files are not needlessly rewritten |
| analytics UTF-8 | CJK, Cyrillic, and emoji command/session values never use byte-boundary slicing and do not panic |
| Cargo JSON | JSON build/check/clippy/install failures preserve non-zero exit status and visible diagnostics |
| protected-file diff | review shows no wholesale replacement or deletion of local native branches in `main.rs` and protected system modules |
| cross-platform CI | Windows native gate and Unix/Linux full tests both pass for platform-neutral upstream ports |

---

## 7. Risks

| Risk | Impact | Mitigation |
|------|------|------|
| Treating transport-only execution as token optimization | High | Track a distinct transport-only path and do not claim semantic savings |
| Direct shell-host execution changes Unix fallback behavior | High | Gate the new runner behind `#[cfg(windows)]`; preserve both non-Windows `sh -c` script surfaces and direct-argv parse fallback tests |
| PowerShell literal rendering changes argument meaning | High | Prefer direct argv for shell hosts; use encoded scripts only for generated transport invocations with literal-rendering tests |
| Raw PowerShell rewrite uses Bash quoting rules | High | Use the dedicated conservative PowerShell parser/renderer; prohibit `discover::lexer::shell_split` in C1 and require argv round trips |
| Bare cmdlet argv cannot recover original PowerShell AST or quote origin | High | Accept only provably static literal/parameter shapes; reject ambiguous forms and require explicit `powershell` / `pwsh` or `rtk run` |
| `.cmd` / `.bat` cannot provide native `.exe` argv guarantees | High | Use distinct `BatchTransport`, validate the documented safe subset, reject cmd metacharacters, and prohibit hand-built `/c` strings |
| RTK silently overrides PowerShell execution policy | High | Use process-scoped `-ExecutionPolicy Bypass` only for RTK-generated temporary `.ps1` file transport after an execution-policy check requires it; never add it to encoded transport or alter persisted policy settings |
| Base64 expansion exceeds the Windows process command-line limit or encourages silent truncation | High | Enforce the 8 KiB source and 30,000 UTF-16-unit complete-command-line budgets for encoded transport; switch oversized scripts to an automatically cleaned up UTF-8-BOM temporary `.ps1` / `-File` transport and test that no partial command executes |
| Upstream reconciliation deletes local Windows-native implementations | High | Treat native files as protected, port minimal blocks, inspect diffs, and run the section 0.4 gate after every patch |
| Accidentally implementing a partial PowerShell interpreter | High | Only support explicit static command shapes |
| Rewriting object pipeline commands into text commands incorrectly | High | Keep object pipelines passthrough except listed `Get-Content | Select-Object` windows |
| Dynamic paths execute differently after rewrite | High | Reject `$`, backticks, scriptblocks, command substitution, and provider paths |
| Codex time/session analytics use database file metadata | High | Filter rows by `ts`, group by `thread_id`, and order by `(ts, ts_nanos, id)` through logical `SessionRef` values |
| `head` waits for EOF or emits smart-truncation annotations | High | Use exact streaming `write_head`; stop after N lines and keep `rtk read` smart truncation separate |
| Executable-only checks rewrite a PowerShell alias/cmdlet lookup | High | Rewrite only explicit `Get-Command -CommandType Application`; keep bare forms on transport |
| Mutating commands create unintended files/directories | Medium | Put `touch` / `mkdir -p` after read-only compatibility work |
| Future priorities skewed by Claude-only analytics | Medium | Add Codex provider before relying on session analytics |

---

## 8. Definition Of Done

A candidate in this document is complete only when:

1. Its supported input shapes are documented.
2. Unsupported shapes fail safe by staying passthrough or returning a clear unsupported error.
3. Unit tests cover positive, negative, and Windows-specific cases.
4. Debug or release exe validation confirms behavior on native Windows.
5. Existing Unix/macOS behavior is unchanged unless explicitly documented.
6. Transport-only paths are distinguishable from RTK semantic optimization paths.
7. Shell-host fallback tests prove no second `powershell -Command` wrapper is introduced on Windows.
8. No new or modified Windows fallback execution sink receives a reconstructed `args.join(" ")` command line.
9. Existing local Windows-native `ls`, `tree`, `wc`, grep fallback, `ps`, `df`, and `du` tests and smoke checks still pass.
10. Any absorbed upstream behavior passes its restored regression tests without whole-file replacement of protected modules.
11. Explicit `rtk proxy` behavior remains separate and no C0.5 path uses its one-string Bash lexer.
12. Raw PowerShell rewrites use the dedicated parser/renderer and pass parse-render-parse tests; no C1 path uses the Bash lexer.
13. Codex `since_days` and session grouping are proven with mixed-age, multi-thread SQLite fixtures.
14. Batch wrappers reject the documented unsafe metacharacters and are never reported as exact native argv transport.
15. `rtk head` output is exact and stdin tests prove it stops after N lines without EOF.
16. Bare `Get-Command` and recursive `Get-ChildItem` produce no semantic rewrite.
17. In-budget generated implicit PowerShell host argv never adds `-ExecutionPolicy Bypass`; temporary-file fallback adds a process-scoped bypass only when the execution-policy check requires it.
18. Generated PowerShell source and complete encoded host command lines enforce the documented 8 KiB and 30,000 UTF-16-unit budgets before encoded process creation; over-limit tests prove automatic complete-script temporary-file fallback without truncation or partial execution.
19. Codex provider tests prove WAL writer coexistence and prove that `SQLITE_BUSY` / `SQLITE_LOCKED` after the fixed 2-second timeout produces a non-zero diagnostic rather than a zero-session success.
20. Empty-file `rtk head` and `rtk tail` tests succeed with exactly zero stdout bytes and no omission marker.

---

## 9. Current Recommendation

The next practical implementation batch should be:

1. Lock and record the existing Windows-native baseline and protected-file diff gate.
2. Selectively restore upstream TOML reversible-lossiness and custom-filter trust hardening before modifying shared fallback code.
3. Implement the Windows fallback transport runner, using direct resolved-program argv execution for external programs and encoded PowerShell for in-budget generated scripts with an automatic UTF-8-BOM temporary `.ps1` / `-File` fallback for oversized scripts.
4. Add `Get-Content` compatibility mapped to `rtk read`.
5. Add `CodexProvider` for `discover` / session analysis in an independent commit.
6. Port upstream grep separator-fidelity fixes into the local native grep implementation.
7. Add `Select-String` compatibility mapped to `rtk grep`, gated by both upstream fidelity tests and a Windows native-grep fallback smoke test.
8. Add only non-recursive `Get-ChildItem` compatibility mapped to the existing local native `rtk ls`; keep recursive/filter forms on validated transport.
9. Add `rtk which`, raw `which`, and explicit `Get-Command -CommandType Application` mapping; keep bare `Get-Command` and `where.exe` without semantic rewrite.

Cargo JSON, analytics UTF-8/ccusage, and Git checkout upstream ports should remain separate review units. They may proceed after the baseline gate, but they must not be bundled into C0.5 or used as a reason to replace local native files.

Only after that should RTK prioritize C2. `head` / `tail` use exact `core::line_window` primitives, `pwd` is read-only, and `touch` / `mkdir -p` remain explicit direct commands with no raw automatic rewrite.
