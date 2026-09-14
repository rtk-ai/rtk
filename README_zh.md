<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>高性能 CLI 代理，为你的智能体削减多达 90% 的 bash 输出</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">官网</a> &bull;
  <a href="#安装">安装</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">故障排除</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">架构</a> &bull;
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

rtk 在命令输出到达 LLM 上下文之前进行过滤和压缩。单一 Rust 二进制文件，零依赖，<10ms 开销。

## RTK 做什么

RTK 拦截 shell 命令，在你的智能体读取之前压缩其输出。

| 操作 | RTK 对输出做了什么 |
|------|--------------------|
| `ls` / `tree` | 用带文件计数的树形格式代替每个条目一行 |
| `cat` / `read` | 智能文件读取：保留签名和结构，而非完整函数体 |
| `grep` / `rg` | 截断超长行，按文件分组匹配结果 |
| `git status` | 紧凑的 stat 格式，按状态分组 |
| `git diff` | 减少上下文，去掉头部信息 |
| `git log` | 仅保留哈希、作者和标题 |
| `git add/commit/push` | 用一行确认代替完整的进度输出 |
| `cargo test` / `npm test` | 仅显示失败，通过的测试折叠为计数 |
| `ruff check` | 按规则和文件分组 |
| `pytest` | 仅显示失败，精简 traceback |
| `go test` | 解析 NDJSON，仅显示失败 |
| `docker ps` | 仅保留关键字段 |

## 节省是如何计算的

RTK 为你的智能体削减**多达 90% 的 bash 输出**。这正是 RTK 所测量的指标，它与「账单降低 90%」不是一回事。

bash 输出只是**输入 token 的来源之一**，此外还有你的提示词、系统提示词和对话历史。而输入 token 本身也**只是账单的一部分**，账单还包含输出 token。削减效果在每一步都会被稀释。

RTK 报告的 token 数量按 `字节数 / 4` 估算：RTK 不内置分词器，因此**百分比是可靠的，但 token 绝对数值只是近似值**。

> 完整说明：[RTK 的节省是如何计算的](docs/guide/resources/savings-explained.md)

## 安装

### Homebrew（推荐）

```bash
brew install rtk
```

### 快速安装（Linux/macOS）

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### 验证

```bash
rtk --version   # 应显示 "rtk 0.27.x"
rtk gain        # 应显示 token 节省统计
```

## 快速开始

```bash
# 1. 为 Claude Code 安装 hook（推荐）
rtk init --global

# 2. 重启 Claude Code，然后测试
git status  # 自动重写为 rtk git status
```

## 工作原理

```
  没有 rtk：                                      使用 rtk：

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |        ~2,000 tokens（原始）       |            |   ~200 tokens        | 过滤     |
    +-----------------------------------+            +------- （已过滤）-----+----------+
```

四种策略：

1. **智能过滤** - 去除噪音（注释、空白、样板代码）
2. **分组** - 聚合相似项（按目录分文件，按类型分错误）
3. **截断** - 保留相关上下文，删除冗余
4. **去重** - 合并重复日志行并计数

## 命令

> 下列百分比是 **bash 输出字节数的削减比例**，由 RTK 的 `字节数 / 4` 估算器测得。参见[节省是如何计算的](#节省是如何计算的)。

### 文件
```bash
rtk ls .                        # 优化的目录树
rtk read file.rs                # 智能文件读取
rtk find "*.rs" .               # 紧凑的查找结果
rtk grep "pattern" .            # 按文件分组的搜索结果
```

### Git
```bash
rtk git status                  # 紧凑状态
rtk git log -n 10               # 单行提交
rtk git diff                    # 精简 diff
rtk git push                    # -> "ok main"
```

### 测试
```bash
rtk jest                        # Jest 紧凑输出
rtk vitest                      # Vitest 紧凑输出
rtk pytest                      # Python 测试（-90%）
rtk go test                     # Go 测试（-90%）
rtk test <cmd>                  # 仅显示失败（-90%）
```

### 构建 & 检查
```bash
rtk lint                        # ESLint 按规则分组
rtk tsc                         # TypeScript 错误分组
rtk cargo build                 # Cargo 构建（-80%）
rtk ruff check                  # Python lint（-80%）
```

### 容器
```bash
rtk docker ps                   # 紧凑容器列表
rtk docker logs <container>     # 去重日志
rtk kubectl pods                # 紧凑 Pod 列表
```

### 分析
```bash
rtk gain                        # 节省统计
rtk gain --graph                # ASCII 图表（30 天）
rtk discover                    # 发现遗漏的节省机会
```

## 文档

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - 解决常见问题
- **[INSTALL.md](INSTALL.md)** - 详细安装指南
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - 技术架构

## 贡献

欢迎贡献！请在 [GitHub](https://github.com/rtk-ai/rtk) 上提交 issue 或 PR。

加入 [Discord](https://discord.gg/RySmvNF5kF) 社区。

## 许可证

Apache 2.0 许可证 - 详见 [LICENSE](LICENSE)。

## 免责声明

详见 [DISCLAIMER.md](DISCLAIMER.md)。
