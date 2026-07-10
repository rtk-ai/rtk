# RTK Local Repository Notes / RTK 本仓库说明

This repository is a Windows-native compatibility fork/worktree of RTK. For the baseline project description, installation, general usage, and original command coverage, read the main repository README first:

本仓库是 RTK 的 Windows native 兼容增强工作区/分支。基础项目介绍、安装方式、通用用法和原始命令覆盖范围，请先阅读主仓库 README：

- Main README / 主仓库 README: https://github.com/fuxkCH/rtk#readme
- Local acceptance test / 本地验收测试: [`tests/windows_native_acceptance.ps1`](tests/windows_native_acceptance.ps1)
- Windows-native fixtures / Windows native 测试夹具: [`tests/fixtures/windows-native/`](tests/fixtures/windows-native/)

## Feature Delta / 相对主仓库的功能变化

| Area | English | 中文 |
|---|---|---|
| Windows fallback transport | Adds a Windows fallback runner that preserves argv boundaries instead of rebuilding commands with unsafe string joins. Explicit shell hosts such as `powershell`, `pwsh`, and `cmd` run as direct argv calls. | 新增 Windows fallback runner，保留 argv 边界，避免用不安全的字符串拼接重构命令。`powershell`、`pwsh`、`cmd` 等显式 shell host 以直接 argv 方式执行。 |
| PowerShell cmdlet compatibility | Adds narrow, validated compatibility for common PowerShell shapes: `Get-Content`, `Select-String`, `Get-ChildItem`, and `Get-Command -CommandType Application`. Unsupported semantic shapes either use safe transport or fail closed instead of being guessed. | 新增常见 PowerShell 形态的窄口径兼容：`Get-Content`、`Select-String`、`Get-ChildItem`、`Get-Command -CommandType Application`。不安全或不等价的语义形态走安全 transport 或 fail-closed，不猜测执行。 |
| Windows native small commands | Adds or wires native RTK commands useful on Windows and for cross-platform agent habits: `which`, `pwd`, `head`, `tail`, `touch`, and `mkdir -p`. | 新增或接入 Windows 上常用、也符合跨平台 agent 习惯的小型 native 命令：`which`、`pwd`、`head`、`tail`、`touch`、`mkdir -p`。 |
| Existing Windows native baseline | Preserves and verifies the local Windows-native implementations for `ls`, `tree`, `wc`, Rust grep fallback, `ps`, `df`, and `du`. | 保留并验证本地已有 Windows-native 实现：`ls`、`tree`、`wc`、Rust grep fallback、`ps`、`df`、`du`。 |
| Grep fidelity | Ports separator-fidelity behavior into the Windows native grep fallback, including context group separators and no synthetic `--` when context is not requested. | 将 grep separator 保真逻辑应用到 Windows native grep fallback，包括 context 分组分隔符，以及非 context 模式不生成额外 `--`。 |
| Rewrite surface | Extends rewrite/classification support for Windows-friendly command forms while keeping unsafe PowerShell object/pipeline semantics out of semantic rewrites. | 扩展 rewrite/classification 对 Windows 常见命令形态的支持，同时避免把不安全的 PowerShell 对象/管道语义错误改写为 RTK 语义命令。 |
| Batch and script safety | Distinguishes `.ps1` transport from `.cmd`/`.bat` transport. `.ps1` arguments are preserved through `-File`; batch wrappers reject unsafe cmd metacharacters instead of pretending to provide exact native argv semantics. | 区分 `.ps1` transport 与 `.cmd`/`.bat` transport。`.ps1` 参数通过 `-File` 保真传递；batch wrapper 对不安全 cmd 元字符拒绝执行，不声称具备完全 native argv 语义。 |
| PowerShell encoded limits | Adds explicit rejection for oversized generated PowerShell transport source, with guidance to use `.ps1` / `-File` rather than truncating or partially executing. | 对过大的生成式 PowerShell transport source 显式拒绝，并提示使用 `.ps1` / `-File`，避免截断或部分执行。 |
| Codex analytics provider | Adds a Codex session provider for discover/session analytics, including SQLite/WAL-oriented diagnostics and safer row ordering. | 新增 Codex 会话 provider，用于 discover/session 分析，包含 SQLite/WAL 相关诊断和更安全的行排序。 |
| Upstream correctness reconciliation | Reconciles selected upstream correctness fixes without replacing local Windows-native modules wholesale: TOML lossiness fallback, custom-filter trust hardening, Cargo JSON diagnostics, UTF-8 analytics safety, ccusage `period` aliases, and Git checkout summaries. | 选择性吸收主仓库正确性修复，同时避免整文件覆盖本地 Windows-native 模块：TOML lossiness fallback、自定义 filter trust 加固、Cargo JSON diagnostics、UTF-8 analytics 安全、ccusage `period` 兼容、Git checkout 摘要等。 |
| Windows-native acceptance coverage | Adds a repository-level executable acceptance suite with fixed fixtures. The suite covers PowerShell cmdlets, native commands, rewrite behavior, fallback transport, `.ps1`/`.cmd` argv, UNC paths, `\\?\` extended-length paths, Unicode paths, and oversized implicit PowerShell transport rejection. | 新增仓库级 exe 验收套件与固定夹具。覆盖 PowerShell cmdlet、native 命令、rewrite 行为、fallback transport、`.ps1`/`.cmd` argv、UNC 路径、`\\?\` 长路径、Unicode 路径、以及过大的隐式 PowerShell transport 拒绝边界。 |

## Current Verification / 当前验证状态

The Windows-native compatibility target has been verified locally with the debug executable:

Windows native 兼容目标已使用本机 debug exe 验证：

```powershell
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 build --jobs 1
rtk proxy powershell -NoProfile -ExecutionPolicy Bypass -File tests\windows_native_acceptance.ps1
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 fmt --check
rtk proxy powershell -NoProfile -File scripts\windows-cargo.ps1 test --jobs 1
```

Latest local result / 最新本地结果：

| Check | Result |
|---|---|
| Debug build / Debug 编译 | PASS |
| Windows native acceptance / Windows native 验收 | 83 passed, 0 skipped, 0 failed |
| Format check / 格式检查 | PASS |
| Rust tests / Rust 测试 | 2437 passed, 0 failed, 8 ignored |
| Integration tests / 集成测试 | 11 passed, 0 failed |

Non-Windows runtime CI is intentionally out of scope for this local Windows verification pass. Non-Windows behavior should be reviewed by comparing the platform-neutral diffs against the main repository and by running CI in the appropriate environment before release.

非 Windows 运行时 CI 不属于本轮本机 Windows 验证范围。发布前应通过与主仓库的平台无关差异对比，以及在对应环境中运行 CI 来验证非 Windows 行为。
