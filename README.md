# RTK Local Repository Notes / RTK 本仓库说明

This repository is a Windows-native compatibility fork/worktree of RTK. For the baseline project description, general usage, and original command coverage, read the original repository README first:

本仓库是 RTK 的 Windows native 兼容增强工作区/分支。基础项目介绍、通用用法和原始命令覆盖范围，请先阅读原始仓库 README：

- Original README / 原始仓库 README: https://github.com/rtk-ai/rtk#readme
- Local acceptance test / 本地验收测试: [`tests/windows_native_acceptance.ps1`](tests/windows_native_acceptance.ps1)
- Windows-native fixtures / Windows native 测试夹具: [`tests/fixtures/windows-native/`](tests/fixtures/windows-native/)

## Install This Windows Build / 安装本仓库 Windows 版本

Download the Windows executable from this fork's release assets, place it in a
stable directory, and add that directory to `PATH`:

从本 fork 的 release assets 下载 Windows 可执行文件，放到一个固定目录，并把该目录加入 `PATH`：

```powershell
$InstallDir = "$env:LOCALAPPDATA\Programs\rtk"
New-Item -ItemType Directory -Force -Path $InstallDir
Invoke-WebRequest -Uri "https://github.com/fuxkCH/rtk/releases/download/v0.43.2/rtk.exe" -OutFile "$InstallDir\rtk.exe"
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($UserPath -split ';') -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
}
```

Do not double-click `rtk.exe`. Open a new terminal after updating `PATH`, then
verify the installed binary:

不要双击 `rtk.exe`。更新 `PATH` 后打开一个新的终端，然后验证安装结果：

```powershell
rtk --version
rtk which rtk
rtk gain
```

Expected version for this release:

当前 release 的预期版本：

```text
rtk 0.43.2
```

Initialize Codex integration after verifying the binary:

验证二进制可用后，初始化 Codex 集成：

```powershell
rtk init -g --codex
```

If an older `rtk.exe` already appears earlier on `PATH`, replace that existing
file or move this install directory before it in `PATH`.

如果 `PATH` 中更靠前的位置已经有旧版 `rtk.exe`，请替换那个旧文件，或把本安装目录移动到
`PATH` 中更靠前的位置。

## Feature Delta / 能力域对比表

下表按**已支持的命令**重新梳理 upstream `rtk-ai/rtk` 与本仓库修复后的能力面。命令以 `rtk --help` 和各子命令帮助为准；同一能力域中的命令以斜杠分隔。

| 能力域 | rtk-ai/rtk | 本仓库(修复后) | 差异概述 |
|---|---|---|---|
| 文件、目录与检索：`ls` / `tree` / `read` / `head` / `tail` / `wc` / `grep` / `rg` / `find` / `which` / `pwd` / `touch` / `mkdir` / `df` / `du` / `ps` | 提供文件读取、搜索、目录与系统信息的压缩输出。 | 同类命令全部可用；Windows 原生实现覆盖 `ls`、`tree`、`wc`、grep fallback、`ps`、`df`、`du` 以及跨平台小命令。 | 补齐 Windows 语义：多文件 `head`/`tail`、`mkdir`、不 append 的 `touch`、path-like `which` 诊断、`df` 零总量与 `du` 访问错误处理。 |
| 版本控制与代码托管：`git` / `diff` / `log` / `gh` / `glab` / `gt` | 提供 Git、GitHub、GitLab 与 Graphite 的压缩输出。 | 同类命令全部可用；保留 checkout、stash、日志与平台 CLI 的专用摘要。 | 本仓库额外修复 Git checkout 摘要、空 stash 输出和 Windows 参数传递；不改变原生 Git 行为。 |
| 构建、测试与质量：`cargo` / `test` / `err` / `format` / `lint` / `prettier` / `jest` / `vitest` / `playwright` / `tsc` / `next` / `prisma` / `npm` / `npx` / `pnpm` | 提供常见构建、测试、格式化与 JS/TS 工具的压缩输出。 | 同类命令全部可用；`cargo` 保留 JSON diagnostics、失败原始输出和测试摘要。 | 吸收 Cargo 诊断正确性修复；本仓库以严格 Clippy 和 Windows 验收作为额外门槛。 |
| 语言与生态工具：`python` 系列 `pytest` / `ruff` / `mypy` / `pip` / `uv`；Ruby `rake` / `rubocop` / `rspec`；Go `go` / `golangci-lint`；JVM `gradlew` / `mvn`；`.NET` `dotnet` | 覆盖上述语言生态的常见测试、格式化、包管理与构建命令。 | 同类命令全部可用，并保持各专用过滤器的原始工具兼容入口。 | 本轮不重写这些上游命令模块；本仓库仅在共享错误反馈和 Windows 传输层中提供增量保障。 |
| PHP 工具链：`php` / `phpunit` / `phpstan` / `pest` / `paratest` / `ecs` / `pint` | 上游提供 PHP 命令模块与测试/静态分析/格式化过滤器。 | 顶层 PHP 命令与 `pipe --filter phpunit|pest|paratest|php-test|ecs|phpstan|pint` 全部可用。 | 与上游 PHP parity；本地只调整注册、帮助、管道和非零退出时保留 stderr 的错误反馈。 |
| 云、容器、网络与数据库：`aws` / `docker` / `kubectl` / `oc` / `curl` / `wget` / `psql` | 提供云 CLI、容器编排、HTTP、下载与 PostgreSQL 的结构化/压缩输出。 | 同类命令全部可用。 | 保持上游过滤能力；本仓库没有把这些命令替换成 Windows 专用语义。 |
| 命令执行与管道：`run` / `proxy` / `summary` / `smart` / `pipe` / `json` / `env` / `deps` | 提供原样代理、摘要、stdin 过滤、JSON/环境/依赖查看。 | 同类命令全部可用；`pipe` 新增完整 PHP filter 集。 | Windows 上显式 shell 保留 argv 边界；超长隐式 PowerShell 自动使用受控临时 `.ps1 -File`，不会截断脚本。 |
| 初始化、配置与安全：`init` / `uninstall` / `config` / `trust` / `untrust` / `verify` | 管理 RTK 配置、项目过滤器、信任与完整性。 | 同类命令全部可用；新增一等 `uninstall -g --agent opencode`，保留旧 `init --uninstall`。 | 本仓库补 OpenCode 全局插件安装/卸载、干跑与状态；项目本地 TOML filter 继续采用显式信任。 |
| Agent hooks 与改写：`hook` / `rewrite` / `hook-audit` | 为 agent 工具调用生成安全的 RTK 改写，并提供审计能力。 | 支持 Claude、Cursor、Gemini、Copilot、Cline、Windsurf、Kilo Code、Antigravity、Pi、Hermes、OpenCode 和 Droid。 | 补 Droid first-class hook；OpenCode 保持 TypeScript `tool.execute.before` 插件模型，不新增权限模型。Droid 安装/卸载额外拒绝异常 JSON、链接逃逸与非 RTK 条目误删。 |
| 发现、会话与遥测：`discover` / `session` / `gain` / `cc-economics` / `telemetry` / `learn` | 分析历史命令、会话采用率、节省量、成本与学习规则。 | 同类命令全部可用；保留 Codex provider、SQLite/WAL 诊断和稳定排序。 | 不把 `db_path` 或 `thread_id` 当作项目路径；没有可靠 cwd 元数据时明确 unsupported。 |
| Windows PowerShell 与脚本兼容：`powershell` / `pwsh` / `cmd` 的代理路径，以及相关 `rewrite` | 以通用 dispatch 为主，不以本地 Windows-native 兼容为核心。 | 支持受控的 `Get-Content`、`Select-String`、`Get-ChildItem`、`Get-Command` 形态；`.ps1` 通过 `-File` 传参，unsafe `.cmd`/`.bat` 元字符 fail-closed。 | 仅 RTK 自动生成的临时 `.ps1` 在执行策略检测需要时加进程级 Bypass；不提权、不修改用户/机器策略，Group Policy 阻止时返回明确错误。 |
| 诊断与输出保真：全部 native/代理命令 | 以压缩输出和退出码为核心。 | native/fallback 失败保留底层 stderr 或明确 `rtk <cmd>:` 诊断；grep context 分隔符、空匹配和 Windows 歧义命令均有确定行为。 | 重点解决“非零退出但没有可见错误”的 agent 体验；Unix/Linux command-not-found 语义保持不变。 |
| 验证面：`cargo` / `hook` / `pipe` / Windows acceptance | 以上游 CI/release checks 为基线。 | 当前源码要求格式检查、完整 Rust 测试、严格 Clippy、all-target check 与 Windows native acceptance。 | Windows 侧额外执行 87 项验收；非 Windows 运行时仍应由相应 CI 环境验证。 |

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
| Windows native acceptance / Windows native 验收 | 87 passed, 0 skipped, 0 failed |
| Format check / 格式检查 | PASS |
| Rust tests / Rust 测试 | 2475 unit tests run: 2467 passed, 0 failed, 8 ignored; 11 existing integration tests and 7 native error-feedback integration tests passed |

Non-Windows runtime CI is intentionally out of scope for this local Windows verification pass. Non-Windows behavior should be reviewed by comparing the platform-neutral diffs against the main repository and by running CI in the appropriate environment before release.

非 Windows 运行时 CI 不属于本轮本机 Windows 验证范围。发布前应通过与主仓库的平台无关差异对比，以及在对应环境中运行 CI 来验证非 Windows 行为。
