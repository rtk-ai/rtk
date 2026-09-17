<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>高性能 CLI 代理，为你阅读的 bash 输出削减多达 90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">官网</a> &bull;
  <a href="#安装">安装</a> &bull;
  <a href="https://www.rtk-ai.app/guide/troubleshooting">故障排除</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">架构</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README.zh-CN.md">简体中文</a> &bull;
  <a href="README.zh-TW.md">繁體中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a> &bull;
  <a href="README_pt.md">Português</a>
</p>

---

rtk 在命令输出到达你的 LLM 上下文之前对其进行过滤与压缩。单一 Rust 二进制文件，支持 100+ 命令，开销 <10ms。

## RTK 做什么

RTK 拦截 shell 命令，并在你的智能体读取之前压缩其输出。

| 操作 | RTK 对输出做了什么 |
|-----------|-----------------------------|
| `ls` / `tree` | 带文件计数的树形格式，而非每个条目一行 |
| `cat` / `read` | 智能文件读取：保留签名和结构，而非完整函数体 |
| `grep` / `rg` | 截断长行，按文件分组匹配 |
| `ast-grep` | 按文件分组结构匹配，限制溢出 |
| `git status` | 紧凑的 stat 格式，按状态分组 |
| `git diff` | 精简上下文，去除头部 |
| `git log` | 仅哈希、作者和标题 |
| `git add/commit/push` | 一行确认，代替完整进度输出 |
| `cargo test` / `npm test` | 仅失败项，通过的测试折叠为计数 |
| `ruff check` | 按规则和文件分组 |
| `sqlfluff lint` | 按规则和文件分组 |
| `pytest` | 仅失败项，traceback 精简 |
| `go test` | 解析 NDJSON，仅失败项 |
| `docker ps` | 仅关键字段 |

## 节省如何计算

RTK 为你阅读的 bash 输出削减**多达 90%**。这正是 RTK 所测量的指标，它与「将你的账单削减 90%」不是一回事。

bash 输出只是**输入 token 的来源之一**，此外还有你的提示词、系统提示词和对话历史。而输入 token 本身也**只是账单的一部分**，账单还计入输出 token。削减效果在每一步都会被稀释。

RTK 报告的 token 数量按 `字节数 / 4` 估算——RTK 不内置分词器，因此**百分比是可靠的，但 token 绝对数值只是近似值**。

> 完整说明：[RTK 节省如何运作](docs/guide/resources/savings-explained.md)

## 安装

### Homebrew（推荐）

```bash
brew install rtk
```

### winget (Windows)

Windows 上最简便的安装方式——一条命令，无需配置 PATH：

```powershell
winget install rtk-ai.rtk
```

### 快速安装（Linux/macOS）

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

> 安装到 `~/.local/bin`。如需加入 PATH：
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # or ~/.zshrc
> ```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### 预构建二进制文件

从 [releases](https://github.com/rtk-ai/rtk/releases) 下载：
- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-musl.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

> **Windows 用户**：解压 zip 并将 `rtk.exe` 放到 PATH 中的某个位置（例如 `C:\Users\<you>\.local\bin`）。从 **Command Prompt**、**PowerShell** 或 **Windows Terminal** 运行 RTK——不要双击 `.exe`（它会一闪而过并关闭）。完整的 hook 系统在 Windows 上原生可用（也可在 [WSL](https://learn.microsoft.com/en-us/windows/wsl/install) 中使用）。详见下方 [Windows 设置](#windows)。

### 验证安装

```bash
rtk --version   # Should show "rtk 0.28.2"
rtk gain        # Should show the savings dashboard
```

> **名称冲突警告**：crates.io 上存在另一个名为 "rtk"（Rust Type Kit）的项目。如果 `rtk gain` 失败，说明你装错了包。请改用上面的 `cargo install --git`。

## 快速开始

```bash
# 1. Install for your AI tool
rtk init -g                     # Claude Code / Copilot (default)
rtk init -g --gemini            # Gemini CLI
rtk init -g --codex             # Codex (OpenAI)
rtk init -g --agent cursor      # Cursor
rtk init -g --agent windsurf    # Windsurf
rtk init --agent cline          # Cline / Roo Code
rtk init --agent kilocode       # Kilo Code
rtk init --agent antigravity    # Google Antigravity
rtk init --agent kimi           # Kimi AI
rtk init -g --agent pi          # Pi
rtk init --agent omp            # Oh My Pi (OMP)
rtk init --agent hermes         # Hermes
rtk init -g --agent droid       # Factory Droid

# 2. Restart your AI tool, then test
git status  # Automatically rewritten to rtk git status
```

基于 hook 的智能体会在执行前重写 Bash 命令（例如 `git status` -> `rtk git status`）。基于插件的智能体，包括 Hermes，通过各自的插件 API 在执行前重写命令。智能体收到的是紧凑输出，无需显式调用 `rtk`。

**重要**：hook 仅在 Bash 工具调用上运行。Claude Code 的内置工具（如 `Read`、`Grep` 和 `Glob`）不会经过 Bash hook，因此不会被自动重写。要在这类流程中获得 RTK 的紧凑输出，请使用 shell 命令（`cat`/`head`/`tail`、`rg`/`grep`、`find`）或直接调用 `rtk read`、`rtk grep`、`rtk find`。

## 工作原理

```
  Without rtk:                                    With rtk:

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |         full raw output           |            |  compact output      | filter   |
    +-----------------------------------+            +------- (filtered) ---+----------+
```

按命令类型应用的四种策略：

1. **智能过滤** - 去除噪音（注释、空白、样板代码）
2. **分组** - 聚合相似项（文件按目录、错误按类型）
3. **截断** - 保留相关上下文，削减冗余
4. **去重** - 以计数折叠重复的日志行

> **RTK 会破坏 Claude 的提示词缓存吗？** 不会。RTK 对每个命令仅过滤一次输出。结果存入历史并在后续 API 调用中正常缓存，因此缓存照常工作。更小的输出也意味着更便宜的缓存写入和读取。详见 [Troubleshooting](docs/guide/resources/troubleshooting.md#does-rtk-break-claudes-prompt-cache)。

## 命令

> 下方百分比是 **bash 输出的削减比例**，而非账单的削减比例。参见 [节省如何计算](#节省如何计算)。

### 文件
```bash
rtk ls .                        # Compact directory tree
rtk read file.rs                # Smart file reading
rtk read file.rs -l aggressive  # Signatures only (strips bodies)
rtk smart file.rs               # 2-line heuristic code summary
rtk find "*.rs" .               # Compact find results
rtk grep "pattern" .            # Grouped search results
rtk diff file1 file2            # Condensed diff (exit 1 if files differ)
```

### Git
```bash
rtk git status                  # Compact status
rtk git log -n 10               # One-line commits
rtk git diff                    # Condensed diff
rtk git add                     # -> "ok"
rtk git commit -m "msg"         # -> "ok abc1234"
rtk git push                    # -> "ok main"
rtk git pull                    # -> "ok 3 files +10 -2"
```

### GitHub CLI
```bash
rtk gh pr list                  # Compact PR listing
rtk gh pr view 42               # PR details + checks
rtk gh issue list               # Compact issue listing
rtk gh run list                 # Workflow run status
```

### 测试运行器
```bash
rtk jest                        # Jest compact (failures only)
rtk vitest                      # Vitest compact (failures only)
rtk playwright test             # E2E results (failures only)
rtk pytest                      # Python tests (-90%)
rtk phpt                        # PHP .phpt tests (run-tests.php, -99%)
rtk go test                     # Go tests (NDJSON, -90%)
rtk cargo test                  # Cargo tests (-90%)
rtk rake test                   # Ruby minitest (-90%)
rtk rspec                       # RSpec tests (JSON, -60%+)
rtk err <cmd>                   # Filter errors only from any command
rtk test <cmd>                  # Generic test wrapper - failures only (-90%)
```

### 构建与 Lint
```bash
rtk lint                        # ESLint grouped by rule/file
rtk lint biome                  # Supports other linters
rtk sqlfluff lint               # SQL linting (JSON, -75%)
rtk sqlfluff lint models/       # Lint a specific directory (pass path after `lint`)
rtk tsc                         # TypeScript errors grouped by file
rtk next build                  # Next.js build compact
rtk prettier --check .          # Files needing formatting
rtk cargo build                 # Cargo build (-80%)
rtk cargo clippy                # Cargo clippy (-80%)
rtk ruff check                  # Python linting (JSON, -80%)
rtk golangci-lint run           # Go linting (JSON, -85%)
rtk rubocop                     # Ruby linting (JSON, -60%+)
rtk mvnd verify                 # Maven Daemon (same filters as rtk mvn)
rtk sbt test                    # ScalaTest output (-90%)
rtk sbt compile                 # Compilation errors only (-75%)
rtk sbt run                     # Strip SBT preamble noise
```

### 包管理器
```bash
rtk pnpm list                   # Compact dependency tree
rtk uv run pytest               # Preserve uv env, keep program output
rtk pip list                    # Python packages (auto-detect uv)
rtk pip outdated                # Outdated packages
rtk bundle install              # Ruby gems (strip Using lines)
rtk prisma generate             # Schema generation (no ASCII art)
```

### 运行时
```bash
rtk bun install                  # Strip progress and version lines
rtk bun test                     # Failures only (-90%)
rtk bun build                    # Errors only when writing to disk, else passthrough
rtk bunx tsc                     # Smart routing to tsc filter
rtk deno test                    # Failures only (-90%)
rtk deno lint                    # Strip download lines + tee recovery
rtk deno check                   # Strip download lines + tee recovery
```

### AWS
```bash
rtk aws sts get-caller-identity # One-line identity
rtk aws ec2 describe-instances  # Compact instance list
rtk aws lambda list-functions   # Name/runtime/memory (strips secrets)
rtk aws logs get-log-events     # Timestamped messages only
rtk aws cloudformation describe-stack-events  # Failures first
rtk aws dynamodb scan           # Unwraps type annotations
rtk aws iam list-roles          # Strips policy documents
rtk aws s3 ls                   # Truncated with recall recovery
```

### 容器
```bash
rtk docker ps                   # Compact container list
rtk docker images               # Compact image list
rtk docker logs <container>     # Deduplicated logs
rtk docker compose ps           # Compose services
rtk kubectl pods                # Compact pod list
rtk kubectl logs <pod>          # Deduplicated logs
rtk kubectl services            # Compact service list
rtk oc get pods                 # OpenShift pod summary
rtk oc get services             # OpenShift service list
rtk oc logs <pod>               # Deduplicated logs
```

### 基础设施即代码
```bash
rtk pulumi preview              # Strip header/URL/duration noise
rtk pulumi up                   # Compact apply output
rtk pulumi destroy              # Compact destroy output
rtk pulumi refresh              # Drift summary
rtk pulumi stack                # Stack metadata (strips owner/timestamps)
```

### 数据与分析
```bash
rtk json config.json            # Structure without values
rtk deps                        # Dependencies summary
rtk env -f AWS                  # Filtered env vars
rtk log app.log                 # Deduplicated logs
rtk curl <url>                  # Truncate + save full output
rtk wget <url>                  # Download, strip progress bars
rtk summary <long command>      # Heuristic summary
rtk proxy <command>             # Raw passthrough + tracking
```

### Token 节省分析
```bash
rtk gain                        # Summary stats
rtk gain --graph                # ASCII graph (last 30 days)
rtk gain --history              # Recent command history
rtk gain --daily                # Day-by-day breakdown
rtk gain --all --format json    # JSON export for dashboards

rtk discover                    # Find missed savings opportunities
rtk discover --all --since 7    # All projects, last 7 days

rtk session                     # Show RTK adoption across recent sessions
```

## 全局标志

```bash
-u, --ultra-compact    # ASCII icons, inline format (further output reduction)
-v, --verbose          # Increase verbosity (-v, -vv, -vvv)
```

## 示例

**目录列表：**
```
# ls -la (45 lines)                     # rtk ls (12 lines)
drwxr-xr-x  15 user staff 480 ...       my-project/
-rw-r--r--   1 user staff 1234 ...       +-- src/ (8 files)
...                                      |   +-- main.rs
                                         +-- Cargo.toml
```

**Git 操作：**
```
# git push (15 lines)                    # rtk git push (1 line)
Enumerating objects: 5, done.             ok main
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...
```

**测试输出：**
```
# cargo test (200+ lines on failure)     # rtk test cargo test (~20 lines)
running 15 tests                          FAILED: 2/15 tests
test utils::test_parse ... ok               test_edge_case: assertion failed
test utils::test_format ... ok              test_overflow: panic at utils.rs:18
...
```

## 自动重写 Hook

使用 rtk 最有效的方式。hook 透明地拦截 Bash 命令，并在执行前将其重写为 rtk 等价命令。

**结果**：所有对话与子智能体中 100% 的 rtk 采用率，且无逐命令的上下文开销。

**范围说明：** 这仅适用于 Bash 工具调用。Claude Code 的内置工具（如 `Read`、`Grep` 和 `Glob`）会绕过 hook，因此若想在这些场景中获得 RTK 过滤，请使用 shell 命令或显式的 `rtk` 命令。

### 设置

```bash
rtk init -g                 # Install hook + RTK.md (recommended)
rtk init -g --opencode      # OpenCode plugin (instead of Claude Code)
rtk init -g --auto-patch    # Non-interactive (CI/CD)
rtk init -g --hook-only     # Hook only, no RTK.md
rtk init --show             # Verify installation
```

安装后，**重启 Claude Code**。

默认情况下 `RTK.md` 对 RTK 本身只字未提。在 `config.toml` 中设置 `[awareness] level = "high"`，让智能体知晓 `rtk gain` / `rtk proxy`；或设为 `"full"` 适用于不支持 hook（或 RTK 尚不支持）的智能体，使其自行添加 `rtk` 前缀——参见 [Configuration](docs/guide/getting-started/configuration.md#awareness-level)。

## Windows

RTK 在原生 Windows 上完全可用。自 **v0.37.2** 起，自动重写 hook 作为**原生二进制命令**（`rtk hook claude`）运行——无需 Unix shell、bash 或 jq——因此命令会在 Command Prompt、PowerShell 和 Windows Terminal 上透明地重写，就像在 Linux 和 macOS 上一样。

### 原生 Windows（手动安装）

如果可以，优先使用 [`winget`](#winget-windows)——它会为你处理 PATH。

```powershell
# 1. Download and extract rtk-x86_64-pc-windows-msvc.zip from releases
# 2. Add rtk.exe to your PATH (e.g. C:\Users\<you>\.local\bin)
# 3. Initialize — installs the native binary hook
rtk init -g
```

**从旧版本升级？** 如果你在 v0.37.2 之前配置过 RTK，可能仍保留着旧的 `rtk-rewrite.sh` shell hook（它需要 Unix shell）。重新运行 `rtk init -g` 即可迁移到原生二进制 hook。

**前置要求**：部分过滤器会通过 shell 调用 [ripgrep](https://github.com/BurntSushi/ripgrep)（`rg`）。请安装并保留在 PATH 中（例如 `winget install BurntSushi.ripgrep.MSVC`），以避免 `Binary 'rg' not found on PATH` 警告。

**重要**：不要双击 `rtk.exe`——它是一个 CLI 工具，会打印用法并立即退出。请始终从终端（Command Prompt、PowerShell 或 Windows Terminal）运行它。

### WSL

[WSL](https://learn.microsoft.com/en-us/windows/wsl/install) 同样可用，行为与 Linux 完全一致：

```bash
# Inside WSL
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
rtk init -g
```

| 功能 | 原生 Windows | WSL |
|---------|----------------|-----|
| 过滤器（cargo、git 等） | 完整 | 完整 |
| 自动重写 hook | 是（原生二进制） | 是 |
| `rtk init -g` | hook 模式 | hook 模式 |
| `rtk gain` / 分析 | 完整 | 完整 |

## 支持的 AI 工具

RTK 支持 17 款 AI 编程工具。每个集成都会将 shell 命令重写为 `rtk` 等价命令，在智能体支持命令拦截的前提下，削减智能体读取的 bash 输出。

| 工具 | 安装 | 方式 |
|------|---------|--------|
| **Claude Code** | `rtk init -g` | PreToolUse hook（原生二进制） |
| **GitHub Copilot (VS Code)** | `rtk init -g --copilot` | PreToolUse hook——透明重写 |
| **GitHub Copilot CLI** | `rtk init -g --copilot` | PreToolUse deny-with-suggestion（CLI 限制） |
| **Cursor** | `rtk init -g --agent cursor` | preToolUse hook（hooks.json） |
| **Gemini CLI** | `rtk init -g --gemini` | BeforeTool hook |
| **Codex** | `rtk init -g --codex` | PreToolUse hook（`updatedInput`）+ AGENTS.md |
| **Windsurf** | `rtk init -g --agent windsurf` | .windsurfrules（项目级） |
| **Cline / Roo Code** | `rtk init --agent cline` | .clinerules（项目级） |
| **OpenCode** | `rtk init -g --opencode` | Plugin TS（tool.execute.before） |
| **OpenClaw** | `openclaw plugins install ./openclaw` | Plugin TS（before_tool_call） |
| **Pi** | `rtk init -g --agent pi`（全局） | TypeScript 扩展（tool_call） |
| **Oh My Pi (OMP)** | `rtk init -g --agent omp`（全局）/ `rtk init --agent omp`（项目级） | TypeScript 扩展（tool_call，与 Pi 共享） |
| **Hermes** | `rtk init --agent hermes` | Python 插件适配器（通过 `rtk rewrite` 进行终端命令变更） |
| **Mistral Vibe** | `rtk init -g --agent vibe` | `pre_tool` hook（hooks.toml） |
| **Kilo Code** | `rtk init --agent kilocode` | .kilocode/rules/rtk-rules.md（项目级） |
| **Google Antigravity** | `rtk init --agent antigravity` | .agents/rules/antigravity-rtk-rules.md（项目级） |
| **Kimi AI** | `rtk init --agent kimi` | AGENTS.md（项目级） |
| **Factory Droid** | `rtk init -g --agent droid`（或按项目） | PreToolUse hook（位于 `~/.factory/hooks.json`，matcher `Execute`） |

关于各智能体的设置细节、覆盖控制和优雅降级，请参阅 [Supported Agents guide](https://www.rtk-ai.app/guide/getting-started/supported-agents)。Hermes 插件源码与测试位于 `hooks/hermes/`；已安装的 Hermes 运行时文件仍位于 `~/.hermes/plugins/rtk-rewrite/`。

## 配置

`~/.config/rtk/config.toml`（macOS：`~/Library/Application Support/rtk/config.toml`）：

```toml
[hooks]
exclude_commands = ["curl", "playwright"]  # skip rewrite for these (matches `npx playwright` too)

[retriever]
mode = "sqlite"         # sqlite (default) | tee (legacy files) | disabled
```

当命令失败时，RTK 会保存完整的未过滤输出，以便 LLM 无需重新执行即可回忆：

```
FAILED: 2/15 tests
[full output: rtk recall 3f9c2a81d4e7]
```

旧的 `[tee]` 配置段仍被沿用：它们映射到 `mode = "tee"`（失败/截断时基于文件的恢复），若你曾设置 `enabled = false` 则映射到 `mode = "disabled"`。此前的 `mode = "always"` 保留其行为并映射到 `tee_on_success = true`，即同时归档成功运行。sqlite 存储仍由失败/截断驱动。

完整的配置参考（所有配置段、环境变量、按项目过滤器），请参阅 [Configuration guide](https://www.rtk-ai.app/guide/getting-started/configuration)。

### 卸载

```bash
rtk init -g --uninstall     # Remove hook, RTK.md, settings.json entry
cargo uninstall rtk          # Remove binary
brew uninstall rtk           # If installed via Homebrew
```

## 文档

- **[rtk-ai.app/guide](https://www.rtk-ai.app/guide)** — 完整用户指南（安装、支持的智能体、被优化的内容、分析、配置、故障排除）
- **[INSTALL.md](INSTALL.md)** — 详细的安装参考
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** — 系统设计与技术决策
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — 贡献指南
- **[SECURITY.md](SECURITY.md)** — 安全策略

## 隐私与遥测

RTK 每天可收集一次**匿名、聚合的使用指标**。遥测**默认禁用**，并需要在 `rtk init` 期间或通过 `rtk telemetry enable` 获得**明确的主动同意**（GDPR Art. 6, 7）。这些数据有助于我们打造更好的产品：识别哪些命令需要过滤器、哪些过滤器需要改进，以及 RTK 带来了多少价值。完整的字段列表、数据处理方式以及贡献者指南，请参阅 **[docs/TELEMETRY.md](docs/TELEMETRY.md)**。

**收集内容与原因：**

| 类别 | 数据 | 原因 |
|----------|------|-----|
| 身份 | 加盐设备哈希（SHA-256，不可逆） | 统计独立安装数，而不追踪个人 |
| 环境 | RTK 版本、OS、架构、安装方式 | 了解需要支持与测试的平台 |
| 使用量 | 命令数（24h）、总命令数、预计节省 token（24h/30d/总计） | 衡量采用率与交付价值 |
| 质量 | Top 5 透传命令（0% 削减）、解析失败次数、削减 <30% 的命令 | 识别缺失与薄弱的过滤器以改进 |
| 生态系统 | 命令类别分布（例如 git 45%、cargo 20%、js 15%） | 优先为热门生态开发过滤器 |
| 留存 | 首次使用至今的天数、近 30 天活跃天数 | 了解参与度并检测流失 |
| 采用 | AI 智能体 hook 类型（claude/gemini/codex）、自定义 TOML 过滤器数量 | 追踪集成覆盖率与 DSL 采用率 |
| 配置 | 是否存在 config.toml、排除命令数、项目数 | 了解用户成熟度与定制模式 |
| 功能 | 元命令（gain、discover、proxy、verify）的使用次数 | 了解哪些 RTK 功能被重视、哪些未被使用 |
| 经济 | 预计 USD 价值，由预计节省 token 与固定内部常量推导 | 量化 RTK 为用户提供的价值 |

所有数据均为**聚合计数或匿名化命令名**（前 3 个词，不含参数）。Top 命令仅上报工具名（例如 "git"、"cargo"），绝不上报完整命令行。

**不收集的内容：** 源代码、文件路径、命令参数、密钥、环境变量、个人数据或仓库内容。

**管理遥测：**
```bash
rtk telemetry status     # Check current consent state
rtk telemetry enable     # Give consent (interactive prompt)
rtk telemetry disable    # Withdraw consent — stops all collection immediately
rtk telemetry forget     # Withdraw consent + delete all local data + request server-side erasure
```

**通过环境变量覆盖：**
```bash
export RTK_TELEMETRY_DISABLED=1   # Blocks telemetry regardless of consent
```

## Star History

<a href="https://www.star-history.com/?repos=rtk-ai%2Frtk&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=rtk-ai/rtk&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=rtk-ai/rtk&type=date&theme=light&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=rtk-ai/rtk&type=date&legend=top-left" />
 </picture>
</a>

## StarMapper

<a href="https://starmapper.bruniaux.com/rtk-ai/rtk">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://starmapper.bruniaux.com/api/map-image/rtk-ai/rtk?theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://starmapper.bruniaux.com/api/map-image/rtk-ai/rtk?theme=light" />
    <img alt="StarMapper" src="https://starmapper.bruniaux.com/api/map-image/rtk-ai/rtk" />
  </picture>
</a>

## 核心团队

- **Patrick Szymkowiak** — 创始人
  [GitHub](https://github.com/pszymkowiak) · [LinkedIn](https://www.linkedin.com/in/patrick-szymkowiak/)
- **Florian Bruniaux** — 核心贡献者
  [GitHub](https://github.com/FlorianBruniaux) · [LinkedIn](https://www.linkedin.com/in/florian-bruniaux-43408b83/)
- **Adrien Eppling** — 核心贡献者
  [GitHub](https://github.com/aeppling) · [LinkedIn](https://www.linkedin.com/in/adrien-eppling/)
- **Nicolas Le Cam** — 核心贡献者
  [Github](https://github.com/kush) · [LinkedIn](https://www.linkedin.com/in/nicolas-le-cam-386387160/)
- **Takayuki Maeda** — 核心贡献者
  [GitHub](https://github.com/TaKO8Ki) · [LinkedIn](https://www.linkedin.com/in/tako8ki/)

## 贡献

欢迎贡献！请在 [GitHub](https://github.com/rtk-ai/rtk) 上提交 issue 或 PR。

加入 [Discord](https://discord.gg/RySmvNF5kF) 社区。

## 许可证

Apache License 2.0 - 详见 [LICENSE](LICENSE)。

## 免责声明

详见 [DISCLAIMER.md](DISCLAIMER.md)。
