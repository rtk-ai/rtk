<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>高效能 CLI 代理，將 LLM token 消耗降低 60-90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">官網</a> &bull;
  <a href="#安裝">安裝</a> &bull;
  <a href="https://www.rtk-ai.app/guide/troubleshooting">疑難排解</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">架構</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh-Hans.md">简体中文</a> &bull;
  <a href="README_zh-Hant.md">繁體中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a>
</p>

---

rtk 在命令輸出到達 LLM 上下文之前進行過濾和壓縮。單一 Rust 二進位檔案，零依賴，<10ms 開銷。

## Token 節省（30 分鐘 Claude Code 工作階段）

| 操作 | 頻率 | 標準 | rtk | 節省 |
|------|------|------|-----|------|
| `ls` / `tree` | 10x | 2,000 | 400 | -80% |
| `cat` / `read` | 20x | 40,000 | 12,000 | -70% |
| `grep` / `rg` | 8x | 16,000 | 3,200 | -80% |
| `git status` | 10x | 3,000 | 600 | -80% |
| `git diff` | 5x | 10,000 | 2,500 | -75% |
| `cargo test` / `npm test` | 5x | 25,000 | 2,500 | -90% |
| **總計** | | **~118,000** | **~23,900** | **-80%** |

## 安裝

### Homebrew（推薦）

```bash
brew install rtk
```

### 快速安裝（Linux/macOS）

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### 驗證

```bash
rtk --version   # 應顯示 "rtk 0.27.x"
rtk gain        # 應顯示 token 節省統計
```

## 快速開始

```bash
# 1. 為 Claude Code 安裝 hook（推薦）
rtk init --global

# 2. 重新啟動 Claude Code，然後測試
git status  # 自動重寫為 rtk git status
```

## 運作原理

```
  沒有 rtk：                                      使用 rtk：

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |        ~2,000 tokens（原始）       |            |   ~200 tokens        | 過濾     |
    +-----------------------------------+            +------- （已過濾）-----+----------+
```

四種策略：

1. **智慧過濾** - 去除噪音（註解、空白、樣板程式碼）
2. **分組** - 聚合相似項目（按目錄分檔案，按類型分錯誤）
3. **截斷** - 保留��關上下文，刪除冗餘
4. **去重** - 合併重複日誌行並計數

## 命令

### 檔案
```bash
rtk ls .                        # 最佳化的目錄樹
rtk read file.rs                # 智慧檔案讀取
rtk find "*.rs" .               # 緊湊的搜尋結果
rtk grep "pattern" .            # 按檔案分組的搜尋結果
```

### Git
```bash
rtk git status                  # 緊湊狀態
rtk git log -n 10               # 單行提交
rtk git diff                    # 精簡 diff
rtk git push                    # -> "ok main"
```

### 測試
```bash
rtk jest                        # Jest 緊湊輸出
rtk vitest                      # Vitest 緊湊輸出
rtk pytest                      # Python 測試（-90%）
rtk go test                     # Go 測試（-90%）
rtk test <cmd>                  # 僅顯示失敗（-90%）
```

### 建置 & 檢查
```bash
rtk lint                        # ESLint 按規則分組
rtk tsc                         # TypeScript 錯誤分組
rtk cargo build                 # Cargo 建置（-80%）
rtk ruff check                  # Python lint（-80%）
```

### 容器
```bash
rtk docker ps                   # 緊湊容器列表
rtk docker logs <container>     # 去重日誌
rtk kubectl pods                # 緊湊 Pod 列表
```

### 分析
```bash
rtk gain                        # 節省統計
rtk gain --graph                # ASCII 圖表（30 天）
rtk discover                    # 發現遺漏的節省機會
```

## 文檔

- **[疑難排解](https://www.rtk-ai.app/guide/troubleshooting)** - 解決常見問題
- **[安裝指南](INSTALL.md)** - 詳細安裝指南
- **[架構](docs/contributing/ARCHITECTURE.md)** - 技術架構

## 貢獻

歡迎貢獻！請在 [GitHub](https://github.com/rtk-ai/rtk) 上提交 issue 或 PR。

加入 [Discord](https://discord.gg/RySmvNF5kF) 社群。

## 授權

Apache 2.0 授權 - 詳見 [LICENSE](LICENSE)。

## 免責聲明

詳見 [DISCLAIMER.md](DISCLAIMER.md)。
