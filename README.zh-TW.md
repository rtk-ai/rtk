<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>高效能 CLI 代理，為你閱讀的 bash 輸出削減多達 90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">官網</a> &bull;
  <a href="#安裝">安裝</a> &bull;
  <a href="https://www.rtk-ai.app/guide/troubleshooting">故障排除</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">架構</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README.zh-CN.md">簡體中文</a> &bull;
  <a href="README.zh-TW.md">繁體中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a> &bull;
  <a href="README_pt.md">Português</a>
</p>

---

rtk 在命令輸出到達你的 LLM 上下文之前對其進行過濾與壓縮。單一 Rust 二進位制檔案，支援 100+ 命令，開銷 <10ms。

## RTK 做什麼

RTK 攔截 shell 命令，並在你的智慧體讀取之前壓縮其輸出。

| 操作 | RTK 對輸出做了什麼 |
|-----------|-----------------------------|
| `ls` / `tree` | 帶檔案計數的樹形格式，而非每個條目一行 |
| `cat` / `read` | 智慧檔案讀取：保留簽名和結構，而非完整函式體 |
| `grep` / `rg` | 截斷長行，按檔案分組匹配 |
| `ast-grep` | 按檔案分組結構匹配，限制溢位 |
| `git status` | 緊湊的 stat 格式，按狀態分組 |
| `git diff` | 精簡上下文，去除頭部 |
| `git log` | 僅雜湊、作者和標題 |
| `git add/commit/push` | 一行確認，代替完整進度輸出 |
| `cargo test` / `npm test` | 僅失敗項，透過的測試摺疊為計數 |
| `ruff check` | 按規則和檔案分組 |
| `sqlfluff lint` | 按規則和檔案分組 |
| `pytest` | 僅失敗項，traceback 精簡 |
| `go test` | 解析 NDJSON，僅失敗項 |
| `docker ps` | 僅關鍵欄位 |

## 節省如何計算

RTK 為你閱讀的 bash 輸出削減**多達 90%**。這正是 RTK 所測量的指標，它與「將你的賬單削減 90%」不是一回事。

bash 輸出只是**輸入 token 的來源之一**，此外還有你的提示詞、系統提示詞和對話歷史。而輸入 token 本身也**只是賬單的一部分**，賬單還計入輸出 token。削減效果在每一步都會被稀釋。

RTK 報告的 token 數量按 `位元組數 / 4` 估算——RTK 不內建分詞器，因此**百分比是可靠的，但 token 絕對數值只是近似值**。

> 完整說明：[RTK 節省如何運作](docs/guide/resources/savings-explained.md)

## 安裝

### Homebrew（推薦）

```bash
brew install rtk
```

### winget (Windows)

Windows 上最簡便的安裝方式——一條命令，無需配置 PATH：

```powershell
winget install rtk-ai.rtk
```

### 快速安裝（Linux/macOS）

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

> 安裝到 `~/.local/bin`。如需加入 PATH：
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # or ~/.zshrc
> ```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### 預構建二進位制檔案

從 [releases](https://github.com/rtk-ai/rtk/releases) 下載：
- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-musl.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

> **Windows 使用者**：解壓 zip 並將 `rtk.exe` 放到 PATH 中的某個位置（例如 `C:\Users\<you>\.local\bin`）。從 **Command Prompt**、**PowerShell** 或 **Windows Terminal** 執行 RTK——不要雙擊 `.exe`（它會一閃而過並關閉）。完整的 hook 系統在 Windows 上原生可用（也可在 [WSL](https://learn.microsoft.com/en-us/windows/wsl/install) 中使用）。詳見下方 [Windows 設定](#windows)。

### 驗證安裝

```bash
rtk --version   # Should show "rtk 0.28.2"
rtk gain        # Should show the savings dashboard
```

> **名稱衝突警告**：crates.io 上存在另一個名為 "rtk"（Rust Type Kit）的專案。如果 `rtk gain` 失敗，說明你裝錯了包。請改用上面的 `cargo install --git`。

## 快速開始

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

基於 hook 的智慧體會在執行前重寫 Bash 命令（例如 `git status` -> `rtk git status`）。基於外掛的智慧體，包括 Hermes，透過各自的外掛 API 在執行前重寫命令。智慧體收到的是緊湊輸出，無需顯式呼叫 `rtk`。

**重要**：hook 僅在 Bash 工具呼叫上執行。Claude Code 的內建工具（如 `Read`、`Grep` 和 `Glob`）不會經過 Bash hook，因此不會被自動重寫。要在這類流程中獲得 RTK 的緊湊輸出，請使用 shell 命令（`cat`/`head`/`tail`、`rg`/`grep`、`find`）或直接呼叫 `rtk read`、`rtk grep`、`rtk find`。

## 工作原理

```
  Without rtk:                                    With rtk:

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |         full raw output           |            |  compact output      | filter   |
    +-----------------------------------+            +------- (filtered) ---+----------+
```

按命令型別應用的四種策略：

1. **智慧過濾** - 去除噪音（註釋、空白、樣板程式碼）
2. **分組** - 聚合相似項（檔案按目錄、錯誤按型別）
3. **截斷** - 保留相關上下文，削減冗餘
4. **去重** - 以計數摺疊重複的日誌行

> **RTK 會破壞 Claude 的提示詞快取嗎？** 不會。RTK 對每個命令僅過濾一次輸出。結果存入歷史並在後續 API 呼叫中正常快取，因此快取照常工作。更小的輸出也意味著更便宜的快取寫入和讀取。詳見 [Troubleshooting](docs/guide/resources/troubleshooting.md#does-rtk-break-claudes-prompt-cache)。

## 命令

> 下方百分比是 **bash 輸出的削減比例**，而非賬單的削減比例。參見 [節省如何計算](#節省如何計算)。

### 檔案
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

### 測試執行器
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

### 構建與 Lint
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

### 執行時
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

### 基礎設施即程式碼
```bash
rtk pulumi preview              # Strip header/URL/duration noise
rtk pulumi up                   # Compact apply output
rtk pulumi destroy              # Compact destroy output
rtk pulumi refresh              # Drift summary
rtk pulumi stack                # Stack metadata (strips owner/timestamps)
```

### 資料與分析
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

### Token 節省分析
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

## 全域性標誌

```bash
-u, --ultra-compact    # ASCII icons, inline format (further output reduction)
-v, --verbose          # Increase verbosity (-v, -vv, -vvv)
```

## 示例

**目錄列表：**
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

**測試輸出：**
```
# cargo test (200+ lines on failure)     # rtk test cargo test (~20 lines)
running 15 tests                          FAILED: 2/15 tests
test utils::test_parse ... ok               test_edge_case: assertion failed
test utils::test_format ... ok              test_overflow: panic at utils.rs:18
...
```

## 自動重寫 Hook

使用 rtk 最有效的方式。hook 透明地攔截 Bash 命令，並在執行前將其重寫為 rtk 等價命令。

**結果**：所有對話與子智慧體中 100% 的 rtk 採用率，且無逐命令的上下文開銷。

**範圍說明：** 這僅適用於 Bash 工具呼叫。Claude Code 的內建工具（如 `Read`、`Grep` 和 `Glob`）會繞過 hook，因此若想在這些場景中獲得 RTK 過濾，請使用 shell 命令或顯式的 `rtk` 命令。

### 設定

```bash
rtk init -g                 # Install hook + RTK.md (recommended)
rtk init -g --opencode      # OpenCode plugin (instead of Claude Code)
rtk init -g --auto-patch    # Non-interactive (CI/CD)
rtk init -g --hook-only     # Hook only, no RTK.md
rtk init --show             # Verify installation
```

安裝後，**重啟 Claude Code**。

預設情況下 `RTK.md` 對 RTK 本身隻字未提。在 `config.toml` 中設定 `[awareness] level = "high"`，讓智慧體知曉 `rtk gain` / `rtk proxy`；或設為 `"full"` 適用於不支援 hook（或 RTK 尚不支援）的智慧體，使其自行新增 `rtk` 字首——參見 [Configuration](docs/guide/getting-started/configuration.md#awareness-level)。

## Windows

RTK 在原生 Windows 上完全可用。自 **v0.37.2** 起，自動重寫 hook 作為**原生二進位制命令**（`rtk hook claude`）執行——無需 Unix shell、bash 或 jq——因此命令會在 Command Prompt、PowerShell 和 Windows Terminal 上透明地重寫，就像在 Linux 和 macOS 上一樣。

### 原生 Windows（手動安裝）

如果可以，優先使用 [`winget`](#winget-windows)——它會為你處理 PATH。

```powershell
# 1. Download and extract rtk-x86_64-pc-windows-msvc.zip from releases
# 2. Add rtk.exe to your PATH (e.g. C:\Users\<you>\.local\bin)
# 3. Initialize — installs the native binary hook
rtk init -g
```

**從舊版本升級？** 如果你在 v0.37.2 之前配置過 RTK，可能仍保留著舊的 `rtk-rewrite.sh` shell hook（它需要 Unix shell）。重新執行 `rtk init -g` 即可遷移到原生二進位制 hook。

**前置要求**：部分過濾器會透過 shell 呼叫 [ripgrep](https://github.com/BurntSushi/ripgrep)（`rg`）。請安裝並保留在 PATH 中（例如 `winget install BurntSushi.ripgrep.MSVC`），以避免 `Binary 'rg' not found on PATH` 警告。

**重要**：不要雙擊 `rtk.exe`——它是一個 CLI 工具，會列印用法並立即退出。請始終從終端（Command Prompt、PowerShell 或 Windows Terminal）執行它。

### WSL

[WSL](https://learn.microsoft.com/en-us/windows/wsl/install) 同樣可用，行為與 Linux 完全一致：

```bash
# Inside WSL
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
rtk init -g
```

| 功能 | 原生 Windows | WSL |
|---------|----------------|-----|
| 過濾器（cargo、git 等） | 完整 | 完整 |
| 自動重寫 hook | 是（原生二進位制） | 是 |
| `rtk init -g` | hook 模式 | hook 模式 |
| `rtk gain` / 分析 | 完整 | 完整 |

## 支援的 AI 工具

RTK 支援 17 款 AI 程式設計工具。每個整合都會將 shell 命令重寫為 `rtk` 等價命令，在智慧體支援命令攔截的前提下，削減智慧體讀取的 bash 輸出。

| 工具 | 安裝 | 方式 |
|------|---------|--------|
| **Claude Code** | `rtk init -g` | PreToolUse hook（原生二進位制） |
| **GitHub Copilot (VS Code)** | `rtk init -g --copilot` | PreToolUse hook——透明重寫 |
| **GitHub Copilot CLI** | `rtk init -g --copilot` | PreToolUse deny-with-suggestion（CLI 限制） |
| **Cursor** | `rtk init -g --agent cursor` | preToolUse hook（hooks.json） |
| **Gemini CLI** | `rtk init -g --gemini` | BeforeTool hook |
| **Codex** | `rtk init -g --codex` | PreToolUse hook（`updatedInput`）+ AGENTS.md |
| **Windsurf** | `rtk init -g --agent windsurf` | .windsurfrules（專案級） |
| **Cline / Roo Code** | `rtk init --agent cline` | .clinerules（專案級） |
| **OpenCode** | `rtk init -g --opencode` | Plugin TS（tool.execute.before） |
| **OpenClaw** | `openclaw plugins install ./openclaw` | Plugin TS（before_tool_call） |
| **Pi** | `rtk init -g --agent pi`（全域性） | TypeScript 擴充套件（tool_call） |
| **Oh My Pi (OMP)** | `rtk init -g --agent omp`（全域性）/ `rtk init --agent omp`（專案級） | TypeScript 擴充套件（tool_call，與 Pi 共享） |
| **Hermes** | `rtk init --agent hermes` | Python 外掛介面卡（透過 `rtk rewrite` 進行終端命令變更） |
| **Mistral Vibe** | `rtk init -g --agent vibe` | `pre_tool` hook（hooks.toml） |
| **Kilo Code** | `rtk init --agent kilocode` | .kilocode/rules/rtk-rules.md（專案級） |
| **Google Antigravity** | `rtk init --agent antigravity` | .agents/rules/antigravity-rtk-rules.md（專案級） |
| **Kimi AI** | `rtk init --agent kimi` | AGENTS.md（專案級） |
| **Factory Droid** | `rtk init -g --agent droid`（或按專案） | PreToolUse hook（位於 `~/.factory/hooks.json`，matcher `Execute`） |

關於各智慧體的設定細節、覆蓋控制和優雅降級，請參閱 [Supported Agents guide](https://www.rtk-ai.app/guide/getting-started/supported-agents)。Hermes 外掛原始碼與測試位於 `hooks/hermes/`；已安裝的 Hermes 執行時檔案仍位於 `~/.hermes/plugins/rtk-rewrite/`。

## 配置

`~/.config/rtk/config.toml`（macOS：`~/Library/Application Support/rtk/config.toml`）：

```toml
[hooks]
exclude_commands = ["curl", "playwright"]  # skip rewrite for these (matches `npx playwright` too)

[retriever]
mode = "sqlite"         # sqlite (default) | tee (legacy files) | disabled
```

當命令失敗時，RTK 會儲存完整的未過濾輸出，以便 LLM 無需重新執行即可回憶：

```
FAILED: 2/15 tests
[full output: rtk recall 3f9c2a81d4e7]
```

舊的 `[tee]` 配置段仍被沿用：它們對映到 `mode = "tee"`（失敗/截斷時基於檔案的恢復），若你曾設定 `enabled = false` 則對映到 `mode = "disabled"`。此前的 `mode = "always"` 保留其行為並對映到 `tee_on_success = true`，即同時歸檔成功執行。sqlite 儲存仍由失敗/截斷驅動。

完整的配置參考（所有配置段、環境變數、按專案過濾器），請參閱 [Configuration guide](https://www.rtk-ai.app/guide/getting-started/configuration)。

### 解除安裝

```bash
rtk init -g --uninstall     # Remove hook, RTK.md, settings.json entry
cargo uninstall rtk          # Remove binary
brew uninstall rtk           # If installed via Homebrew
```

## 文件

- **[rtk-ai.app/guide](https://www.rtk-ai.app/guide)** — 完整使用者指南（安裝、支援的智慧體、被最佳化的內容、分析、配置、故障排除）
- **[INSTALL.md](INSTALL.md)** — 詳細的安裝參考
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** — 系統設計與技術決策
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — 貢獻指南
- **[SECURITY.md](SECURITY.md)** — 安全策略

## 隱私與遙測

RTK 每天可收集一次**匿名、聚合的使用指標**。遙測**預設禁用**，並需要在 `rtk init` 期間或透過 `rtk telemetry enable` 獲得**明確的主動同意**（GDPR Art. 6, 7）。這些資料有助於我們打造更好的產品：識別哪些命令需要過濾器、哪些過濾器需要改進，以及 RTK 帶來了多少價值。完整的欄位列表、資料處理方式以及貢獻者指南，請參閱 **[docs/TELEMETRY.md](docs/TELEMETRY.md)**。

**收集內容與原因：**

| 類別 | 資料 | 原因 |
|----------|------|-----|
| 身份 | 加鹽裝置雜湊（SHA-256，不可逆） | 統計獨立安裝數，而不追蹤個人 |
| 環境 | RTK 版本、OS、架構、安裝方式 | 瞭解需要支援與測試的平臺 |
| 使用量 | 命令數（24h）、總命令數、預計節省 token（24h/30d/總計） | 衡量採用率與交付價值 |
| 質量 | Top 5 透傳命令（0% 削減）、解析失敗次數、削減 <30% 的命令 | 識別缺失與薄弱的過濾器以改進 |
| 生態系統 | 命令類別分佈（例如 git 45%、cargo 20%、js 15%） | 優先為熱門生態開發過濾器 |
| 留存 | 首次使用至今的天數、近 30 天活躍天數 | 瞭解參與度並檢測流失 |
| 採用 | AI 智慧體 hook 型別（claude/gemini/codex）、自定義 TOML 過濾器數量 | 追蹤整合覆蓋率與 DSL 採用率 |
| 配置 | 是否存在 config.toml、排除命令數、專案數 | 瞭解使用者成熟度與定製模式 |
| 功能 | 元命令（gain、discover、proxy、verify）的使用次數 | 瞭解哪些 RTK 功能被重視、哪些未被使用 |
| 經濟 | 預計 USD 價值，由預計節省 token 與固定內部常量推導 | 量化 RTK 為使用者提供的價值 |

所有資料均為**聚合計數或匿名化命令名**（前 3 個詞，不含引數）。Top 命令僅上報工具名（例如 "git"、"cargo"），絕不上報完整命令列。

**不收集的內容：** 原始碼、檔案路徑、命令引數、金鑰、環境變數、個人資料或倉庫內容。

**管理遙測：**
```bash
rtk telemetry status     # Check current consent state
rtk telemetry enable     # Give consent (interactive prompt)
rtk telemetry disable    # Withdraw consent — stops all collection immediately
rtk telemetry forget     # Withdraw consent + delete all local data + request server-side erasure
```

**透過環境變數覆蓋：**
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

## 核心團隊

- **Patrick Szymkowiak** — 創始人
  [GitHub](https://github.com/pszymkowiak) · [LinkedIn](https://www.linkedin.com/in/patrick-szymkowiak/)
- **Florian Bruniaux** — 核心貢獻者
  [GitHub](https://github.com/FlorianBruniaux) · [LinkedIn](https://www.linkedin.com/in/florian-bruniaux-43408b83/)
- **Adrien Eppling** — 核心貢獻者
  [GitHub](https://github.com/aeppling) · [LinkedIn](https://www.linkedin.com/in/adrien-eppling/)
- **Nicolas Le Cam** — 核心貢獻者
  [Github](https://github.com/kush) · [LinkedIn](https://www.linkedin.com/in/nicolas-le-cam-386387160/)
- **Takayuki Maeda** — 核心貢獻者
  [GitHub](https://github.com/TaKO8Ki) · [LinkedIn](https://www.linkedin.com/in/tako8ki/)

## 貢獻

歡迎貢獻！請在 [GitHub](https://github.com/rtk-ai/rtk) 上提交 issue 或 PR。

加入 [Discord](https://discord.gg/RySmvNF5kF) 社群。

## 許可證

Apache License 2.0 - 詳見 [LICENSE](LICENSE)。

## 免責宣告

詳見 [DISCLAIMER.md](DISCLAIMER.md)。
