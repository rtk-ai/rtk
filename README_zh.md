<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>高性能 CLI 代理，将 LLM token 消耗降低 60-90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">官网</a> &bull;
  <a href="#安装">安装</a> &bull;
  <a href="https://www.rtk-ai.app/guide/troubleshooting">故障排除</a> &bull;
  <a href="ARCHITECTURE.md">架构</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a>
</p>

---

rtk 在命令输出抵达 LLM 上下文之前对其进行过滤和压缩。单一 Rust 二进制文件，支持 100+ 命令，开销 <10ms。

## Token 节省（30 分钟 Claude Code 会话）

| 操作 | 频率 | 标准 | rtk | 节省 |
|------|------|------|-----|------|
| `ls` / `tree` | 10x | 2,000 | 400 | -80% |
| `cat` / `read` | 20x | 40,000 | 12,000 | -70% |
| `grep` / `rg` | 8x | 16,000 | 3,200 | -80% |
| `git status` | 10x | 3,000 | 600 | -80% |
| `git diff` | 5x | 10,000 | 2,500 | -75% |
| `git log` | 5x | 2,500 | 500 | -80% |
| `git add/commit/push` | 8x | 1,600 | 120 | -92% |
| `cargo test` / `npm test` | 5x | 25,000 | 2,500 | -90% |
| `ruff check` | 3x | 3,000 | 600 | -80% |
| `pytest` | 4x | 8,000 | 800 | -90% |
| `go test` | 3x | 6,000 | 600 | -90% |
| `docker ps` | 3x | 900 | 180 | -80% |
| **总计** | | **~118,000** | **~23,900** | **-80%** |

> 估算基于中等规模的 TypeScript / Rust 项目，实际节省随项目规模而异。

## 安装

### Homebrew（推荐）

```bash
brew install rtk
```

### 快速安装（Linux / macOS）

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

> 默认安装到 `~/.local/bin`，必要时将其加入 PATH：
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # 或 ~/.zshrc
> ```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### 预编译二进制

可以从 [releases](https://github.com/rtk-ai/rtk/releases) 直接下载：
- macOS：`rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux：`rtk-x86_64-unknown-linux-musl.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows：`rtk-x86_64-pc-windows-msvc.zip`

> **Windows 用户注意**：解压 zip 后将 `rtk.exe` 放到 PATH 上的某个目录（例如 `C:\Users\<你>\.local\bin`）。请在 **命令提示符**、**PowerShell** 或 **Windows Terminal** 中运行 RTK，**不要双击 `.exe`**（双击会一闪而过）。如需最佳体验，建议使用 [WSL](https://learn.microsoft.com/en-us/windows/wsl/install)，那里 hook 系统可以原生工作。详见下方 [Windows](#windows) 章节。

### 验证安装

```bash
rtk --version   # 应显示 "rtk 0.28.2"
rtk gain        # 应显示 token 节省统计
```

> **同名包警告**：crates.io 上还有另一个名为 "rtk" 的项目（Rust Type Kit）。如果 `rtk gain` 报错，说明你装错了包，请改用上面的 `cargo install --git` 命令。

## 快速开始

```bash
# 1. 为你的 AI 工具安装
rtk init -g                     # Claude Code / Copilot（默认）
rtk init -g --gemini            # Gemini CLI
rtk init -g --codex             # Codex（OpenAI）
rtk init -g --agent cursor      # Cursor
rtk init --agent windsurf       # Windsurf
rtk init --agent cline          # Cline / Roo Code
rtk init --agent kilocode       # Kilo Code
rtk init --agent antigravity    # Google Antigravity

# 2. 重启你的 AI 工具，然后测试
git status  # 自动重写为 rtk git status
```

Hook 会在 Bash 命令执行前透明地将其改写为对应的 rtk 命令（例如把 `git status` 改写为 `rtk git status`）。Claude 看不到这次改写，只会拿到压缩后的输出。

**注意：** Hook 仅作用于 Bash 类工具调用。Claude Code 内置的 `Read`、`Grep`、`Glob` 等工具不会经过 Bash hook，因此不会被自动改写。如果想让这些工作流也享受 RTK 的精简输出，请改用 shell 命令（`cat` / `head` / `tail`、`rg` / `grep`、`find`），或直接调用 `rtk read`、`rtk grep`、`rtk find`。

## 工作原理

```
  没有 rtk：                                       使用 rtk：

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |        ~2,000 tokens（原始）       |            |   ~200 tokens        |  过滤    |
    +-----------------------------------+            +------- （已过滤）----+----------+
```

针对不同命令应用四种策略：

1. **智能过滤** - 去除噪音（注释、空白、样板内容）
2. **分组聚合** - 合并相似项（按目录归并文件，按类型归并错误）
3. **截断** - 保留关键上下文，剔除冗余
4. **去重** - 折叠重复日志行并附带计数

## 命令

### 文件
```bash
rtk ls .                        # token 优化的目录树
rtk read file.rs                # 智能文件读取
rtk read file.rs -l aggressive  # 仅签名（去除函数体）
rtk smart file.rs               # 两行启发式代码摘要
rtk find "*.rs" .               # 紧凑的查找结果
rtk grep "pattern" .            # 按文件分组的搜索结果
rtk diff file1 file2            # 精简 diff
```

### Git
```bash
rtk git status                  # 紧凑状态
rtk git log -n 10               # 单行提交
rtk git diff                    # 精简 diff
rtk git add                     # -> "ok"
rtk git commit -m "msg"         # -> "ok abc1234"
rtk git push                    # -> "ok main"
rtk git pull                    # -> "ok 3 files +10 -2"
```

### GitHub CLI
```bash
rtk gh pr list                  # 紧凑的 PR 列表
rtk gh pr view 42               # PR 详情 + checks
rtk gh issue list               # 紧凑的 issue 列表
rtk gh run list                 # workflow 运行状态
```

### 测试运行器
```bash
rtk jest                        # Jest 紧凑输出（仅失败用例）
rtk vitest                      # Vitest 紧凑输出（仅失败用例）
rtk playwright test             # E2E 结果（仅失败用例）
rtk pytest                      # Python 测试（-90%）
rtk go test                     # Go 测试（NDJSON，-90%）
rtk cargo test                  # Cargo 测试（-90%）
rtk rake test                   # Ruby minitest（-90%）
rtk rspec                       # RSpec 测试（JSON，-60%+）
rtk err <cmd>                   # 从任意命令中只过滤出错误
rtk test <cmd>                  # 通用测试封装 — 仅显示失败（-90%）
```

### 构建 & 检查
```bash
rtk lint                        # ESLint 按规则 / 文件分组
rtk lint biome                  # 也支持其他 linter
rtk tsc                         # TypeScript 错误按文件分组
rtk next build                  # Next.js 构建紧凑输出
rtk prettier --check .          # 列出待格式化的文件
rtk cargo build                 # Cargo 构建（-80%）
rtk cargo clippy                # Cargo clippy（-80%）
rtk ruff check                  # Python lint（JSON，-80%）
rtk golangci-lint run           # Go lint（JSON，-85%）
rtk rubocop                     # Ruby lint（JSON，-60%+）
```

### 包管理器
```bash
rtk pnpm list                   # 紧凑的依赖树
rtk pip list                    # Python 包（自动识别 uv）
rtk pip outdated                # 过时的包
rtk bundle install              # Ruby gems（去掉 Using 行）
rtk prisma generate             # Schema 生成（去掉 ASCII art）
```

### AWS
```bash
rtk aws sts get-caller-identity # 单行身份信息
rtk aws ec2 describe-instances  # 紧凑的实例列表
rtk aws lambda list-functions   # 名称 / runtime / 内存（去除敏感字段）
rtk aws logs get-log-events     # 仅保留时间戳和消息
rtk aws cloudformation describe-stack-events  # 失败优先
rtk aws dynamodb scan           # 拆开类型注解
rtk aws iam list-roles          # 去掉策略文档
rtk aws s3 ls                   # 截断并自动 tee 备份
```

### 容器
```bash
rtk docker ps                   # 紧凑的容器列表
rtk docker images               # 紧凑的镜像列表
rtk docker logs <container>     # 去重日志
rtk docker compose ps           # Compose 服务
rtk kubectl pods                # 紧凑的 Pod 列表
rtk kubectl logs <pod>          # 去重日志
rtk kubectl services            # 紧凑的 Service 列表
```

### 数据 & 分析
```bash
rtk json config.json            # 仅保留结构（去掉值）
rtk deps                        # 依赖摘要
rtk env -f AWS                  # 过滤环境变量
rtk log app.log                 # 去重日志
rtk curl <url>                  # 截断 + 保存完整输出
rtk wget <url>                  # 下载并去掉进度条
rtk summary <long command>      # 启发式摘要
rtk proxy <command>             # 原样透传 + 统计
```

### Token 节省分析
```bash
rtk gain                        # 节省统计概要
rtk gain --graph                # ASCII 图表（最近 30 天）
rtk gain --history              # 最近的命令历史
rtk gain --daily                # 按天细分
rtk gain --all --format json    # 用于仪表盘的 JSON 导出

rtk discover                    # 发现遗漏的节省机会
rtk discover --all --since 7    # 所有项目，最近 7 天

rtk session                     # 查看最近会话中 RTK 的覆盖情况
```

## 全局参数

```bash
-u, --ultra-compact    # ASCII 图标 + 内联格式（额外节省 token）
-v, --verbose          # 增加日志级别（-v、-vv、-vvv）
```

## 示例

**目录列表：**
```
# ls -la（45 行，~800 tokens）           # rtk ls（12 行，~150 tokens）
drwxr-xr-x  15 user staff 480 ...       my-project/
-rw-r--r--   1 user staff 1234 ...       +-- src/ (8 files)
...                                      |   +-- main.rs
                                         +-- Cargo.toml
```

**Git 操作：**
```
# git push（15 行，~200 tokens）          # rtk git push（1 行，~10 tokens）
Enumerating objects: 5, done.             ok main
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...
```

**测试输出：**
```
# cargo test（失败时 200+ 行）            # rtk test cargo test（~20 行）
running 15 tests                          FAILED: 2/15 tests
test utils::test_parse ... ok               test_edge_case: assertion failed
test utils::test_format ... ok              test_overflow: panic at utils.rs:18
...
```

## 自动改写 Hook

这是使用 rtk 最有效的方式。Hook 在执行前透明地拦截 Bash 命令，并将其改写为等价的 rtk 命令。

**效果**：所有会话和子代理 100% 走 rtk，且无任何 token 开销。

**作用范围说明：** 仅作用于 Bash 类工具调用。Claude Code 的 `Read`、`Grep`、`Glob` 等内置工具会绕过 hook，所以如果想让这些场景也享受 RTK 过滤，请使用 shell 命令或显式调用 `rtk` 命令。

### 安装

```bash
rtk init -g                 # 安装 hook + RTK.md（推荐）
rtk init -g --opencode      # 使用 OpenCode 插件（替代 Claude Code）
rtk init -g --auto-patch    # 非交互模式（CI / CD）
rtk init -g --hook-only     # 只装 hook，不写 RTK.md
rtk init --show             # 验证安装
```

安装后请**重启 Claude Code**。

## Windows

RTK 在 Windows 上可用，但有一些限制。自动改写 hook（`rtk-rewrite.sh`）需要 Unix shell，因此在原生 Windows 上 RTK 会回退到 **CLAUDE.md 注入模式** —— AI 助手仍然会收到 RTK 的指引，但命令不会被自动改写。

### 推荐：WSL（完整支持）

如需最佳体验，请使用 [WSL](https://learn.microsoft.com/en-us/windows/wsl/install)（适用于 Linux 的 Windows 子系统）。在 WSL 内部，RTK 的行为与 Linux 完全一致 —— 完整的 hook 支持、自动改写，全部到位：

```bash
# 在 WSL 中
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
rtk init -g
```

### 原生 Windows（受限支持）

在原生 Windows（cmd.exe / PowerShell）上，RTK 的过滤器照常工作，但 hook 不会自动改写命令：

```powershell
# 1. 从 releases 下载并解压 rtk-x86_64-pc-windows-msvc.zip
# 2. 把 rtk.exe 加入 PATH
# 3. 初始化（会回退到 CLAUDE.md 注入模式）
rtk init -g
# 4. 显式调用 rtk
rtk cargo test
rtk git status
```

**重要**：不要双击 `rtk.exe` —— 它是一个命令行工具，会立刻打印用法并退出。请始终在终端中运行（命令提示符、PowerShell 或 Windows Terminal）。

| 特性 | WSL | 原生 Windows |
|------|-----|--------------|
| 过滤器（cargo、git 等） | 完整支持 | 完整支持 |
| 自动改写 hook | 支持 | 不支持（回退到 CLAUDE.md） |
| `rtk init -g` | Hook 模式 | CLAUDE.md 模式 |
| `rtk gain` / 分析 | 完整支持 | 完整支持 |

## 支持的 AI 工具

RTK 支持 12 款 AI 编码工具，每种集成都会透明地把 shell 命令改写为对应的 `rtk` 命令，从而带来 60-90% 的 token 节省。

| 工具 | 安装方式 | 接入方式 |
|------|----------|----------|
| **Claude Code** | `rtk init -g` | PreToolUse hook（bash） |
| **GitHub Copilot（VS Code）** | `rtk init -g --copilot` | PreToolUse hook —— 透明改写 |
| **GitHub Copilot CLI** | `rtk init -g --copilot` | PreToolUse 拒绝并提示（受 CLI 限制） |
| **Cursor** | `rtk init -g --agent cursor` | preToolUse hook（hooks.json） |
| **Gemini CLI** | `rtk init -g --gemini` | BeforeTool hook |
| **Codex** | `rtk init -g --codex` | AGENTS.md + RTK.md 指令 |
| **Windsurf** | `rtk init --agent windsurf` | .windsurfrules（项目级） |
| **Cline / Roo Code** | `rtk init --agent cline` | .clinerules（项目级） |
| **OpenCode** | `rtk init -g --opencode` | TS 插件（tool.execute.before） |
| **OpenClaw** | `openclaw plugins install ./openclaw` | TS 插件（before_tool_call） |
| **Mistral Vibe** | 计划中（[#800](https://github.com/rtk-ai/rtk/issues/800)） | 等待上游支持 |
| **Kilo Code** | `rtk init --agent kilocode` | .kilocode/rules/rtk-rules.md（项目级） |
| **Google Antigravity** | `rtk init --agent antigravity` | .agents/rules/antigravity-rtk-rules.md（项目级） |

各工具的详细配置、覆盖控制以及优雅降级方式，请参阅 [Supported Agents 指南](https://www.rtk-ai.app/guide/getting-started/supported-agents)。

## 配置

`~/.config/rtk/config.toml`（macOS：`~/Library/Application Support/rtk/config.toml`）：

```toml
[hooks]
exclude_commands = ["curl", "playwright"]  # 跳过这些命令的改写

[tee]
enabled = true          # 命令失败时保存原始输出（默认：true）
mode = "failures"       # "failures"、"always" 或 "never"
```

当命令失败时，RTK 会保存完整的未过滤输出，方便 LLM 直接读取，无需重新执行命令：

```
FAILED: 2/15 tests
[full output: ~/.local/share/rtk/tee/1707753600_cargo_test.log]
```

完整的配置参考（所有字段、环境变量、按项目过滤器等），请参阅 [Configuration 指南](https://www.rtk-ai.app/guide/getting-started/configuration)。

### 卸载

```bash
rtk init -g --uninstall     # 删除 hook、RTK.md、settings.json 中的相关项
cargo uninstall rtk          # 删除二进制
brew uninstall rtk           # 如果通过 Homebrew 安装
```

## 文档

- **[rtk-ai.app/guide](https://www.rtk-ai.app/guide)** — 完整的用户指南（安装、支持的 agent、覆盖范围、分析、配置、故障排除）
- **[INSTALL.md](INSTALL.md)** — 详细安装参考
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — 系统设计和技术决策
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — 贡献指南
- **[SECURITY.md](SECURITY.md)** — 安全策略

## 隐私 & 遥测

RTK 可以选择性地收集**匿名、聚合的使用指标**，每天最多一次。遥测**默认关闭**，需要在 `rtk init` 期间或通过 `rtk telemetry enable` 显式同意才会启用（符合 GDPR 第 6、7 条）。这些数据帮助我们改进产品：识别哪些命令需要新过滤器、哪些过滤器需要改进，以及 RTK 实际带来了多少价值。完整字段、数据处理方式以及贡献者指引，请参阅 **[docs/TELEMETRY.md](docs/TELEMETRY.md)**。

**收集了什么、为什么收集：**

| 类别 | 数据 | 用途 |
|------|------|------|
| 身份 | 加盐设备哈希（SHA-256，不可逆） | 统计独立安装数，但不追踪个人 |
| 环境 | RTK 版本、操作系统、架构、安装方式 | 确定需要支持和测试的平台 |
| 使用量 | 24 小时内命令数、总命令数、节省的 token（24 小时 / 30 天 / 总计） | 衡量采用度和实际价值 |
| 质量 | 节省 0% 的前 5 个直通命令、解析失败次数、节省 <30% 的命令 | 找出缺失的过滤器和有待改进的过滤器 |
| 生态 | 命令分类分布（如 git 45%、cargo 20%、js 15%） | 优先开发主流生态的过滤器 |
| 留存 | 自首次使用以来的天数、最近 30 天的活跃天数 | 了解参与度并发现流失 |
| 接入 | AI agent 的 hook 类型（claude / gemini / codex）、自定义 TOML 过滤器数量 | 跟踪集成覆盖率和 DSL 采用情况 |
| 配置 | 是否存在 config.toml、被排除的命令数、项目数 | 了解用户成熟度和定制模式 |
| 功能 | 元命令的使用次数（gain、discover、proxy、verify） | 了解哪些功能被重视、哪些被冷落 |
| 经济价值 | 估算的 USD 节省（基于 API token 价格） | 量化 RTK 给用户带来的价值 |

所有数据都是**聚合计数或匿名化的命令名**（前 3 个单词、不含参数）。Top 命令只上报工具名（如 "git"、"cargo"），不会上报完整命令行。

**不会收集的内容：** 源代码、文件路径、命令参数、密钥、环境变量、个人数据、仓库内容。

**遥测管理：**
```bash
rtk telemetry status     # 查看当前授权状态
rtk telemetry enable     # 授权（交互式提示）
rtk telemetry disable    # 撤回授权 —— 立即停止所有数据收集
rtk telemetry forget     # 撤回授权 + 删除本地数据 + 请求服务端删除
```

**通过环境变量覆盖：**
```bash
export RTK_TELEMETRY_DISABLED=1   # 无视授权状态，强制屏蔽遥测
```

## Star 历史

<a href="https://www.star-history.com/?repos=rtk-ai%2Frtk&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=rtk-ai/rtk&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=rtk-ai/rtk&type=date&legend=top-left" />
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

## 贡献

欢迎贡献！请在 [GitHub](https://github.com/rtk-ai/rtk) 上提交 issue 或 PR。

加入 [Discord](https://discord.gg/RySmvNF5kF) 社区。

## 许可证

MIT 许可证 — 详见 [LICENSE](LICENSE)。

## 免责声明

详见 [DISCLAIMER.md](DISCLAIMER.md)。
