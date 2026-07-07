<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>降低 LLM Token 消耗達 60-90% 的高性能 CLI 代理</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">官方網站</a> &bull;
  <a href="#安裝說明">安裝說明</a> &bull;
  <a href="https://www.rtk-ai.app/guide/troubleshooting">故障排除</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">架構設計</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh.md">简体中文</a> &bull;
  <a href="README_zh-TW.md">繁體中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a> &bull;
  <a href="README_pt.md">Português</a>
</p>

---

rtk 在命令輸出傳送到您的 LLM Context 之前進行過濾與壓縮。採用單一 Rust 二進位檔，支援 100+ 種指令，額外開銷小於 10ms。

## Token 節省量（30 分鐘 Claude Code 工作階段）

| 操作 | 執行次數 | 標準 | rtk | 節省比例 |
|-----------|-----------|----------|-----|---------|
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
| **總計** | | **~118,000** | **~23,900** | **-80%** |

> 此估算基於中型 TypeScript/Rust 專案，實際節省量會因專案大小而異。

## 安裝說明

### Homebrew（推薦）

```bash
brew install rtk
```

### 快速安裝（Linux/macOS）

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

> 將安裝至 `~/.local/bin`。若有需要，請將其新增至 PATH：
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # 或 ~/.zshrc
> ```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### 預編譯二進位檔

請從 [releases](https://github.com/rtk-ai/rtk/releases) 下載：
- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-musl.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

> **Windows 使用者**：解壓縮 Zip 檔並將 `rtk.exe` 放置於您的 PATH 路徑下（例如 `C:\Users\<您的使用者名稱>\.local\bin`）。請從 **命令提示字元 (Command Prompt)**、**PowerShell** 或 **Windows Terminal** 執行 rtk — 請勿直接按兩下執行 `.exe` 檔（視窗會閃退）。完整的 hook 系統在 Windows（以及 [WSL](https://learn.microsoft.com/en-us/windows/wsl/install)）上皆可原生運作。詳情請參閱下方的 [Windows 設定](#windows)。

### 驗證安裝

```bash
rtk --version   # 應顯示 "rtk 0.28.2"
rtk gain        # 應顯示 Token 節省統計
```

> **名稱衝突警告**：crates.io 上存在另一個名為 "rtk" (Rust Type Kit) 的專案。若 `rtk gain` 執行失敗，代表您安裝了錯誤的套件。請改用上述的 `cargo install --git` 指令進行安裝。

## 快速開始

```bash
# 1. 為您的 AI 工具進行安裝
rtk init -g                     # Claude Code / Copilot (預設)
rtk init -g --gemini            # Gemini CLI
rtk init -g --codex             # Codex (OpenAI)
rtk init -g --agent cursor      # Cursor
rtk init -g --agent windsurf    # Windsurf
rtk init --agent cline          # Cline / Roo Code
rtk init --agent kilocode       # Kilo Code
rtk init --agent antigravity    # Google Antigravity
rtk init -g --agent pi          # Pi
rtk init --agent hermes         # Hermes

# 2. 重啟您的 AI 工具，然後進行測試
git status  # 會自動被重寫為 rtk git status
```

基於 hook 的 Agent 會在執行前自動重寫 Bash 指令（例如 `git status` -> `rtk git status`）。而基於外掛程式（plugin）的 Agent（包括 Hermes），則會在執行前透過其外掛程式 API 重寫指令。如此一來，Agent 便能直接接收壓縮後的精簡輸出，無需手動呼叫 `rtk`。

**重要說明**：hook 僅適用於 Bash 工具呼叫。Claude Code 的內建工具（如 `Read`、`Grep` 和 `Glob`）不會經過 Bash hook，因此無法自動重寫。若要在這些工作流程中獲得 rtk 的精簡輸出，請直接使用 shell 指令（如 `cat`/`head`/`tail`、`rg`/`grep`、`find`）或直接呼叫 `rtk read`、`rtk grep` 或 `rtk find`。

## 工作原理

```
  沒有 rtk：                                      使用 rtk：

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  rtk  -->  git
    ^                                   |            ^                      |          |
    |        ~2,000 Token (原始)         |            |   ~200 Token         | 過濾     |
    +-----------------------------------+            +------- (已過濾) ------+----------+
```

針對不同指令類型，rtk 套用以下四種核心優化策略：

1. **智慧過濾 (Smart Filtering)**：移除雜訊（如註解、空白字元、樣板程式碼）。
2. **分組彙整 (Grouping)**：聚合相似的項目（例如按目錄歸類檔案、按類型歸類錯誤）。
3. **精簡截斷 (Truncation)**：僅保留關鍵的 Context，刪除冗餘內容。
4. **重複去重 (Deduplication)**：將重複的日誌行合併並顯示累計次數。

## 指令介紹

### 檔案操作
```bash
rtk ls .                        # 經過 Token 優化的目錄樹
rtk read file.rs                # 智慧檔案讀取
rtk read file.rs -l aggressive  # 僅顯示函式特徵標記（省略實作內容）
rtk smart file.rs               # 兩行的啟發式程式碼摘要
rtk find "*.rs" .               # 緊湊的尋找結果
rtk grep "pattern" .            # 分組後的搜尋結果
rtk diff file1 file2            # 精簡的 diff（若檔案不同則結束代碼為 1）
```

### Git
```bash
rtk git status                  # 緊湊的狀態資訊
rtk git log -n 10               # 單行 commit 紀錄
rtk git diff                    # 精簡的 diff
rtk git add                     # -> "ok"
rtk git commit -m "msg"         # -> "ok abc1234"
rtk git push                    # -> "ok main"
rtk git pull                    # -> "ok 3 files +10 -2"
```

### GitHub CLI
```bash
rtk gh pr list                  # 緊湊的 PR 列表
rtk gh pr view 42               # PR 詳細資訊與狀態檢查
rtk gh issue list               # 緊湊的 Issue 列表
rtk gh run list                 # Workflow 執行狀態
```

### 測試執行工具
```bash
rtk jest                        # 緊湊的 Jest 輸出（僅限失敗項）
rtk vitest                      # 緊湊的 Vitest 輸出（僅限失敗項）
rtk playwright test             # E2E 測試結果（僅限失敗項）
rtk pytest                      # Python 測試（節省 90%）
rtk go test                     # Go 測試 (NDJSON，節省 90%)
rtk cargo test                  # Cargo 測試（節省 90%）
rtk rake test                   # Ruby minitest（節省 90%）
rtk rspec                       # RSpec 測試 (JSON，節省 60%+)
rtk err <cmd>                   # 僅過濾並顯示任何指令的錯誤資訊
rtk test <cmd>                  # 通用測試包裝器 — 僅限失敗項（節省 90%）
```

### 建置與 Linter
```bash
rtk lint                        # ESLint 結果（按規則/檔案分組）
rtk lint biome                  # 亦支援其他 Linter
rtk tsc                         # TypeScript 錯誤（按檔案分組）
rtk next build                  # Next.js 緊湊的建置輸出
rtk prettier --check .          # 需進行格式化的檔案
rtk cargo build                 # Cargo 建置（節省 80%）
rtk cargo clippy                # Cargo clippy（節省 80%）
rtk ruff check                  # Python 程式碼檢查 (JSON，節省 80%)
rtk golangci-lint run           # Go 程式碼檢查 (JSON，節省 85%)
rtk rubocop                     # Ruby 程式碼檢查 (JSON，節省 60%+)
```

### 套件管理器
```bash
rtk pnpm list                   # 緊湊的依賴關係樹
rtk uv run pytest               # 保留 uv 環境，僅顯示錯誤
rtk pip list                    # Python 套件（自動偵測 uv）
rtk pip outdated                # 已過期的套件
rtk bundle install              # Ruby gems（去除 Using 行）
rtk prisma generate             # 產生 Schema（去除 ASCII 藝術圖）
```

### AWS
```bash
rtk aws sts get-caller-identity # 單行身分資訊
rtk aws ec2 describe-instances  # 緊湊的執行個體列表
rtk aws lambda list-functions   # 名稱/Runtime/記憶體（移除機密資訊）
rtk aws logs get-log-events     # 僅顯示帶有時間戳記的訊息
rtk aws cloudformation describe-stack-events  # 失敗項優先顯示
rtk aws dynamodb scan           # 解開型別註解
rtk aws iam list-roles          # 移除政策文件
rtk aws s3 ls                   # 截斷並保留 tee 復原功能
```

### 容器
```bash
rtk docker ps                   # 緊湊的容器列表
rtk docker images               # 緊湊的映像檔列表
rtk docker logs <container>     # 已去重的日誌
rtk docker compose ps           # Compose 服務列表
rtk kubectl pods                # 緊湊的 Pod 列表
rtk kubectl logs <pod>          # 已去重的日誌
rtk kubectl services            # 緊湊的服務列表
rtk oc get pods                 # OpenShift Pod 摘要
rtk oc get services             # OpenShift 服務列表
rtk oc logs <pod>               # 已去重的日誌
```

### 基礎設施即程式碼 (IaC)
```bash
rtk pulumi preview              # 去除標頭、URL 與時間長度雜訊
rtk pulumi up                   # 緊湊的套用 (apply) 輸出
rtk pulumi destroy              # 緊湊的銷毀 (destroy) 輸出
rtk pulumi refresh              # 漂移 (drift) 摘要
rtk pulumi stack                # Stack 中繼資料（移除擁有者與時間戳記）
```

### 資料與分析
```bash
rtk json config.json            # 僅顯示結構（不包含值）
rtk deps                        # 依賴關係摘要
rtk env -f AWS                  # 已過濾的環境變數
rtk log app.log                 # 已去重的日誌
rtk curl <url>                  # 截斷並儲存完整輸出
rtk wget <url>                  # 下載並去除進度條
rtk summary <long command>      # 啟發式摘要
rtk proxy <command>             # 原始傳遞與追蹤
```

### Token 節省量分析
```bash
rtk gain                        # 摘要統計
rtk gain --graph                # ASCII 圖表（最近 30 天）
rtk gain --history              # 最近的指令歷史紀錄
rtk gain --daily                # 按日分析明細
rtk gain --all --format json    # 匯出為 JSON 格式供儀表板使用

rtk discover                    # 尋找未被節省的潛在機會
rtk discover --all --since 7    # 所有專案（最近 7 天）

rtk session                     # 顯示最近工作階段中 rtk 的採用狀況
```

## 全域 Flag

```bash
-u, --ultra-compact    # ASCII 圖示、行內格式（節省更多 Token）
-v, --verbose          # 增加詳細度 (-v, -vv, -vvv)
```

## 範例說明

**目錄清單：**
```
# ls -la (45 行，約 800 Token)          # rtk ls (12 行，約 150 Token)
drwxr-xr-x  15 user staff 480 ...       my-project/
-rw-r--r--   1 user staff 1234 ...       +-- src/ (8 files)
...                                      |   +-- main.rs
                                         +-- Cargo.toml
```

**Git 操作：**
```
# git push (15 行，約 200 Token)         # rtk git push (1 行，約 10 Token)
Enumerating objects: 5, done.             ok main
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...
```

**測試輸出：**
```
# cargo test（失敗時達 200+ 行）          # rtk test cargo test（約 20 行）
running 15 tests                          FAILED: 2/15 tests
test utils::test_parse ... ok               test_edge_case: assertion failed
test utils::test_format ... ok              test_overflow: panic at utils.rs:18
...
```

## 自動重寫 Hook

這是使用 rtk 最有效率的方式。此 hook 會透明地攔截 Bash 指令，並在執行前將其重寫為對應的 rtk 指令。

**效果**：在所有對話與子 Agent (subagent) 中達到 100% 的 rtk 採用率，且完全沒有 Token 的額外開銷。

**適用範圍說明**：此設定僅適用於 Bash 工具的呼叫。Claude Code 的內建工具（例如 `Read`、`Grep` 與 `Glob`）會繞過 hook，因此如果您希望在這些功能中也套用 rtk 過濾，請改用 shell 指令或明確呼叫 `rtk` 指令。

### 設定步驟

```bash
rtk init -g                 # 安裝 hook + RTK.md (推薦)
rtk init -g --opencode      # OpenCode 外掛程式（替代 Claude Code）
rtk init -g --auto-patch    # 非互動模式 (CI/CD)
rtk init -g --hook-only     # 僅安裝 hook，不建立 RTK.md
rtk init --show             # 驗證安裝結果
```

安裝完成後，**請重啟 Claude Code**。

## Windows

rtk 在原生 Windows 上可完全正常運作。自 **v0.37.2** 起，自動重寫 hook 會作為**原生二進位指令** (`rtk hook claude`) 執行，無需 Unix shell、Bash 或 jq。因此，無論是在命令提示字元 (Command Prompt)、PowerShell 還是 Windows Terminal 中，指令都會被透明重寫，運作方式就和在 Linux 與 macOS 一模一樣。

### 原生 Windows

```powershell
# 1. 自 releases 下載並解壓縮 rtk-x86_64-pc-windows-msvc.zip
# 2. 將 rtk.exe 新增至您的 PATH（例如 C:\Users\<您的使用者名稱>\.local\bin）
# 3. 初始化 — 安裝原生二進位 hook
rtk init -g
```

**要從舊版本升級嗎？** 如果您是在 v0.37.2 之前設定 rtk，則可能仍在使用舊版的 `rtk-rewrite.sh` shell hook（該版本需要 Unix shell 才能運作）。請重新執行 `rtk init -g` 以遷移至原生二進位 hook。

**系統要求**：部分過濾器需要呼叫 [ripgrep](https://github.com/BurntSushi/ripgrep) (`rg`)。請安裝它並將其保持在您的 PATH 中（例如執行 `winget install BurntSushi.ripgrep.MSVC`），以避免出現 `Binary 'rg' not found on PATH` 的警告資訊。

**重要說明**：請勿直接按兩下執行 `rtk.exe` — 這是一個 CLI 工具，執行後只會顯示用法並立即結束。請務必在終端機（如命令提示字元、PowerShell 或 Windows Terminal）中執行它。

### WSL

[WSL](https://learn.microsoft.com/en-us/windows/wsl/install) 也同樣支援，且運作方式與 Linux 完全相同：

```bash
# Inside WSL
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
rtk init -g
```

| 功能 | 原生 Windows | WSL |
|---------|----------------|-----|
| 過濾器 (cargo, git 等) | 完整支援 | 完整支援 |
| 自動重寫 hook | 是（原生二進位） | 是 |
| `rtk init -g` | Hook 模式 | Hook 模式 |
| `rtk gain` / 節省統計分析 | 完整支援 | 完整支援 |

## 支援的 AI 工具

rtk 支援 14 種 AI 程式碼編寫工具。在支援指令攔截的 Agent 中，各項整合功能會自動將 shell 指令重寫為對應的 rtk 指令，從而節省 60-90% 的 Token 消耗。

| 工具 | 安裝方式 | 實作方式 |
|------|---------|--------|
| **Claude Code** | `rtk init -g` | PreToolUse hook（原生二進位） |
| **GitHub Copilot (VS Code)** | `rtk init -g --copilot` | PreToolUse hook — 透明重寫 |
| **GitHub Copilot CLI** | `rtk init -g --copilot` | PreToolUse deny-with-suggestion（受限於 CLI 限制） |
| **Cursor** | `rtk init -g --agent cursor` | preToolUse hook (hooks.json) |
| **Gemini CLI** | `rtk init -g --gemini` | BeforeTool hook |
| **Codex** | `rtk init -g --codex` | AGENTS.md + RTK.md 指示 |
| **Windsurf** | `rtk init -g --agent windsurf` | .windsurfrules（專案級別） |
| **Cline / Roo Code** | `rtk init --agent cline` | .clinerules（專案級別） |
| **OpenCode** | `rtk init -g --opencode` | TypeScript 外掛（tool.execute.before） |
| **OpenClaw** | `openclaw plugins install ./openclaw` | TypeScript 外掛（before_tool_call） |
| **Pi** | `rtk init -g --agent pi` (global) | TypeScript 擴充功能 (tool_call) |
| **Hermes** | `rtk init --agent hermes` | Python 外掛轉接器（透過 rtk rewrite 進行終端機指令修改） |
| **Mistral Vibe** | 已規劃（[#800](https://github.com/rtk-ai/rtk/issues/800)） | 受限於上游支援 |
| **Kilo Code** | `rtk init --agent kilocode` | `.kilocode/rules/rtk-rules.md（專案級別）` |
| **Google Antigravity** | `rtk init --agent antigravity` | `.agents/rules/antigravity-rtk-rules.md（專案級別）` |

如需各個 Agent 的設定詳情、覆寫控制項及漸進式降級說明，請參閱 [Supported Agents 指南](https://www.rtk-ai.app/guide/getting-started/supported-agents)。Hermes 外掛程式的原始碼和測試位於 `hooks/hermes/` 目錄中；已安裝的 Hermes Runtime 檔案則存放於 `~/.hermes/plugins/rtk-rewrite/` 目錄下。

## 設定

`~/.config/rtk/config.toml` (macOS: `~/Library/Application Support/rtk/config.toml`):

```toml
[hooks]
exclude_commands = ["curl", "playwright"]  # 略過這些指令的重寫

[tee]
enabled = true          # 失敗時儲存原始輸出（預設：true）
mode = "failures"       # "failures"、"always" 或 "never"
```

當指令執行失敗時，rtk 會儲存完整且未經過濾的原始輸出，讓 LLM 可以直接讀取而無需重新執行指令：

```
FAILED: 2/15 tests
[完整輸出: ~/.local/share/rtk/tee/1707753600_cargo_test.log]
```

如需完整的設定參考（包括所有區段、環境變數及單一專案過濾器），請參閱 [Configuration 指南](https://www.rtk-ai.app/guide/getting-started/configuration)。

### 解除安裝

```bash
rtk init -g --uninstall     # 移除 hook、RTK.md 以及 settings.json 中的項目
cargo uninstall rtk          # 移除二進位檔
brew uninstall rtk           # 若是透過 Homebrew 安裝
```

## 文件連結

- **[rtk-ai.app/guide](https://www.rtk-ai.app/guide)** — 完整使用者指南（包含安裝、支援的 Agent、優化細節、分析、設定及故障排除）
- **[INSTALL.md](INSTALL.md)** — 詳細的安裝說明參考
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** — 系統設計與技術決策
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — 貢獻指南
- **[SECURITY.md](SECURITY.md)** — 安全政策

## 隱私與遙測 (Telemetry)

rtk 每天可收集一次**匿名的彙整使用指標**。遙測功能**預設為停用**，並且在 `rtk init` 期間或透過 `rtk telemetry enable` 需要取得您的**明確同意（GDPR 第 6、7 條）**。這些數據能協助我們打造更好的產品：例如識別哪些指令需要過濾器、哪些過濾器需要改進，以及評估 rtk 所帶來的價值。如需完整的欄位列表、資料處理方式和貢獻者指南，請參閱 **[docs/TELEMETRY.md](docs/TELEMETRY.md)**。

**收集內容與目的：**

| 類別 | 數據內容 | 收集目的 |
|----------|------|-----|
| 身分識別 | 加鹽的裝置雜湊值 (SHA-256，不可逆) | 在不追蹤個人的情況下，統計不重複安裝次數 |
| 環境資訊 | rtk 版本、OS、系統架構、安裝方式 | 了解需要支援與測試哪些平台 |
| 使用量 | 24 小時內指令執行次數、總指令次數、已節省的 Token 數量（24小時/30天/總計） | 衡量採用率與帶來的價值 |
| 品質改善 | 前 5 名直接穿透的指令（0% 節省）、解析失敗次數、節省小於 30% 的指令 | 識別缺少的過濾器以及需要改進的薄弱部分 |
| 生態系統 | 指令類別分佈（例如 Git 45%, cargo 20%, js 15%） | 優先為熱門的生態系統開發過濾器 |
| 留存率 | 自首次使用後的天數、最近 30 天內的活躍天數 | 了解使用者參與度並偵測流失情況 |
| 採用情況 | AI Agent 的 hook 類型 (claude/gemini/codex)、自訂 TOML 過濾器數量 | 追蹤整合覆蓋率與 DSL 採用率 |
| 設定偏好 | 是否存在 config.toml、排除指令的數量、專案數量 | 了解使用者熟悉度與自訂模式 |
| 功能使用 | 元指令（gain, discover, proxy, verify）的使用次數 | 了解哪些 rtk 功能受重視，哪些未被使用 |
| 經濟價值 | 估算節省的美元金額（基於 API Token 的定價） | 量化 rtk 提供給使用者的價值 |

所有數據皆為**彙整計數或去識別化的指令名稱**（僅保留前 3 個單字，不包含參數）。最常執行的指令僅回報工具名稱（例如 "git"、"cargo"），絕不會回報完整的指令行內容。

**絕對不會收集的內容**：原始碼、檔案路徑、指令參數、機密資訊、環境變數、個人資料或儲存庫內容。

**管理遙測：**
```bash
rtk telemetry status     # 檢查目前的同意狀態
rtk telemetry enable     # 給予同意（互動式提示）
rtk telemetry disable    # 撤回同意 — 立即停止所有數據收集
rtk telemetry forget     # 撤回同意 + 刪除所有本機資料 + 請求伺服器端清除資料
```

**透過環境變數覆寫：**
```bash
export RTK_TELEMETRY_DISABLED=1   # 無論同意狀態為何，一律封鎖遙測功能
```

## Star 成長軌跡

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

## 核心團隊

- **Patrick Szymkowiak** — 創辦人
  [GitHub](https://github.com/pszymkowiak) · [LinkedIn](https://www.linkedin.com/in/patrick-szymkowiak/)
- **Florian Bruniaux** — 核心貢獻者
  [GitHub](https://github.com/FlorianBruniaux) · [LinkedIn](https://www.linkedin.com/in/florian-bruniaux-43408b83/)
- **Adrien Eppling** — 核心貢獻者
  [GitHub](https://github.com/aeppling) · [LinkedIn](https://www.linkedin.com/in/adrien-eppling/)
- **Nicolas Le Cam** — 核心貢獻者
  [GitHub](https://github.com/kush) · [LinkedIn](https://www.linkedin.com/in/nicolas-le-cam-386387160/)

## 參與貢獻

歡迎各界貢獻！請在 [GitHub](https://github.com/rtk-ai/rtk) 上提交 Issue 或 PR。

歡迎加入 [Discord](https://discord.gg/RySmvNF5kF) 互動社群。

## 授權條款

本專案採用 Apache License 2.0 授權 — 詳見 [LICENSE](LICENSE) 檔案。

## 免責聲明

請參閱 [DISCLAIMER.md](DISCLAIMER.md)。
