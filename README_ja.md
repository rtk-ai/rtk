<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>エージェントが読む bash 出力を最大 90% 削減する高性能 CLI プロキシ</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">ウェブサイト</a> &bull;
  <a href="#インストール">インストール</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">トラブルシューティング</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">アーキテクチャ</a> &bull;
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

rtk はコマンド出力を LLM コンテキストに届く前にフィルタリング・圧縮します。単一の Rust バイナリ、依存関係ゼロ、オーバーヘッド 10ms 未満。

## RTK がすること

RTK はシェルコマンドを横取りし、エージェントが読む前にその出力を圧縮します。

| 操作 | RTK が出力に対して行うこと |
|------|----------------------------|
| `ls` / `tree` | 1 エントリ 1 行ではなく、ファイル数付きのツリー形式 |
| `cat` / `read` | スマートなファイル読み取り：本文全体ではなくシグネチャと構造 |
| `grep` / `rg` | 長い行を切り詰め、マッチをファイル単位でグループ化 |
| `git status` | コンパクトな stat 形式、状態ごとにグループ化 |
| `git diff` | コンテキストを削減、ヘッダーを除去 |
| `git log` | ハッシュ、作者、件名のみ |
| `git add/commit/push` | 進捗出力全体の代わりに確認行 1 行 |
| `cargo test` / `npm test` | 失敗のみ、成功したテストは件数に集約 |
| `ruff check` | ルールとファイルごとにグループ化 |
| `pytest` | 失敗のみ、トレースバックを短縮 |
| `go test` | NDJSON をパースし、失敗のみ |
| `docker ps` | 必須フィールドのみ |

## 節約の仕組み

RTK はエージェントが読む **bash 出力を最大 90% 削減**します。これが RTK の測定対象であり、請求額が 90% 減ることと同じではありません。

bash 出力は、あなたのプロンプト、システムプロンプト、会話履歴と並ぶ**入力トークンの構成要素のひとつ**にすぎません。そして入力トークン自体も、出力トークンを含む**請求額の一部**でしかありません。削減効果は各段階で薄まります。

RTK が報告するトークン数は `バイト数 / 4` で見積もられています。RTK はトークナイザーを同梱していないため、**割合は信頼できますが、トークンの絶対値はあくまで概算**です。

> 詳しい解説：[RTK の節約の仕組み](docs/guide/resources/savings-explained.md)

## インストール

### Homebrew（推奨）

```bash
brew install rtk
```

### クイックインストール（Linux/macOS）

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### 確認

```bash
rtk --version   # "rtk 0.27.x" と表示されるはず
rtk gain        # トークン節約統計が表示されるはず
```

## クイックスタート

```bash
# 1. Claude Code 用フックをインストール（推奨）
rtk init --global

# 2. Claude Code を再起動してテスト
git status  # 自動的に rtk git status に書き換え
```

## 仕組み

```
  rtk なし：                                       rtk あり：

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |        ~2,000 tokens（生出力）     |             |   ~200 tokens        | フィルタ |
    +-----------------------------------+             +------- （圧縮済）----+----------+
```

4つの戦略：

1. **スマートフィルタリング** - ノイズを除去（コメント、空白、ボイラープレート）
2. **グルーピング** - 類似項目を集約（ディレクトリ別ファイル、タイプ別エラー）
3. **トランケーション** - 関連コンテキストを保持、冗長性をカット
4. **重複排除** - 繰り返しログ行をカウント付きで統合

## コマンド

> 以下の割合は **bash 出力バイト数の削減率**であり、RTK の `バイト数 / 4` 推定器で測定しています。[節約の仕組み](#節約の仕組み)を参照してください。

### ファイル
```bash
rtk ls .                        # 最適化されたディレクトリツリー
rtk read file.rs                # スマートファイル読み取り
rtk find "*.rs" .               # コンパクトな検索結果
rtk grep "pattern" .            # ファイル別グループ化検索
```

### Git
```bash
rtk git status                  # コンパクトなステータス
rtk git log -n 10               # 1行コミット
rtk git diff                    # 圧縮された diff
rtk git push                    # -> "ok main"
```

### テスト
```bash
rtk jest                        # Jest コンパクト
rtk vitest                      # Vitest コンパクト
rtk pytest                      # Python テスト（-90%）
rtk go test                     # Go テスト（-90%）
rtk test <cmd>                  # 失敗のみ表示（-90%）
```

### ビルド & リント
```bash
rtk lint                        # ESLint ルール別グループ化
rtk tsc                         # TypeScript エラーグループ化
rtk cargo build                 # Cargo ビルド（-80%）
rtk ruff check                  # Python リント（-80%）
```

### 分析
```bash
rtk gain                        # 節約統計
rtk gain --graph                # ASCII グラフ（30日間）
rtk discover                    # 見逃した節約機会を発見
```

## ドキュメント

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - よくある問題の解決
- **[INSTALL.md](INSTALL.md)** - 詳細インストールガイド
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - 技術アーキテクチャ

## コントリビュート

コントリビューション歓迎！[GitHub](https://github.com/rtk-ai/rtk) で issue または PR を作成してください。

[Discord](https://discord.gg/RySmvNF5kF) コミュニティに参加。

## ライセンス

Apache 2.0 ライセンス - 詳細は [LICENSE](LICENSE) を参照。

## 免責事項

詳細は [DISCLAIMER.md](DISCLAIMER.md) を参照。
