<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Hochleistungs-CLI-Proxy, der den LLM-Tokenverbrauch um 60-90% reduziert</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Website</a> &bull;
  <a href="#installation">Installation</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">Fehlerbehebung</a> &bull;
  <a href="ARCHITECTURE.md">Architektur</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_de.md">Deutsch</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a>
</p>

---

rtk filtert und komprimiert Kommando-Ausgaben, bevor sie deinen LLM-Kontext erreichen. Eine einzelne Rust-Binary, keine Abhängigkeiten, <10ms Overhead.

## Token-Einsparung (30-minütige Claude-Code-Session)

| Operation | Häufigkeit | Standard | rtk | Ersparnis |
|-----------|------------|----------|-----|-----------|
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
| **Gesamt** | | **~118,000** | **~23,900** | **-80%** |

> Schätzungen basieren auf mittelgroßen TypeScript-/Rust-Projekten. Die tatsächliche Einsparung variiert je nach Projektgröße.

## Installation

### Homebrew (empfohlen)

```bash
brew install rtk
```

### Schnellinstallation (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

> Installiert nach `~/.local/bin`. Falls nötig zu PATH hinzufügen:
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # oder ~/.zshrc
> ```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### Vorgebaute Binaries

Download von [releases](https://github.com/rtk-ai/rtk/releases):
- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-musl.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

### Installation prüfen

```bash
rtk --version   # Sollte "rtk 0.28.2" anzeigen
rtk gain        # Sollte Token-Einsparungsstatistiken anzeigen
```

> **Warnung vor Namenskonflikt**: Ein anderes Projekt namens "rtk" (Rust Type Kit) existiert auf crates.io. Falls `rtk gain` fehlschlägt, hast du das falsche Paket installiert. Nutze stattdessen `cargo install --git` von oben.

## Schnellstart

```bash
# 1. Für dein AI-Tool installieren
rtk init -g                     # Claude Code / Copilot (Standard)
rtk init -g --gemini            # Gemini CLI
rtk init -g --codex             # Codex (OpenAI)
rtk init -g --agent cursor      # Cursor
rtk init --agent windsurf       # Windsurf
rtk init --agent cline          # Cline / Roo Code

# 2. AI-Tool neu starten, dann testen
git status  # Wird automatisch zu rtk git status umgeschrieben
```

Der Hook schreibt Bash-Befehle transparent vor der Ausführung um (z. B. `git status` -> `rtk git status`). Claude sieht das Umschreiben nicht, sondern nur die komprimierte Ausgabe.

**Wichtig:** Der Hook läuft nur bei Bash-Tool-Calls. Integrierte Claude-Code-Tools wie `Read`, `Grep` und `Glob` laufen nicht über den Bash-Hook und werden daher nicht automatisch umgeschrieben. Wenn du für diese Workflows RTKs kompakte Ausgabe willst, nutze Shell-Befehle (`cat`/`head`/`tail`, `rg`/`grep`, `find`) oder rufe `rtk read`, `rtk grep` bzw. `rtk find` direkt auf.

## So funktioniert es

```
  Ohne rtk:                                     Mit rtk:

  Claude  --git status-->  shell  -->  git      Claude  --git status-->  RTK  -->  git
    ^                                   |         ^                      |          |
    |       ~2,000 Tokens (roh)         |         |   ~200 Tokens       | filter   |
    +-----------------------------------+         +------- (gefiltert) --+----------+
```

Vier Strategien werden je nach Befehlstyp angewendet:

1. **Intelligente Filterung** - Entfernt Rauschen (Kommentare, Whitespace, Boilerplate)
2. **Gruppierung** - Fasst ähnliche Elemente zusammen (Dateien nach Verzeichnis, Fehler nach Typ)
3. **Trunkierung** - Behält relevanten Kontext und entfernt Redundanz
4. **Deduplizierung** - Verdichtet wiederholte Logzeilen mit Zählung

## Befehle

### Dateien
```bash
rtk ls .                        # Token-optimierter Verzeichnisbaum
rtk read file.rs                # Intelligentes Dateilesen
rtk read file.rs -l aggressive  # Nur Signaturen (entfernt Bodys)
rtk smart file.rs               # 2-zeilige heuristische Code-Zusammenfassung
rtk find "*.rs" .               # Kompakte find-Ergebnisse
rtk grep "pattern" .            # Gruppierte Suchergebnisse
rtk diff file1 file2            # Verdichteter Diff
```

### Git
```bash
rtk git status                  # Kompakter Status
rtk git log -n 10               # One-line-Commits
rtk git diff                    # Verdichteter Diff
rtk git add                     # -> "ok"
rtk git commit -m "msg"         # -> "ok abc1234"
rtk git push                    # -> "ok main"
rtk git pull                    # -> "ok 3 files +10 -2"
```

### GitHub CLI
```bash
rtk gh pr list                  # Kompakte PR-Liste
rtk gh pr view 42               # PR-Details + Checks
rtk gh issue list               # Kompakte Issue-Liste
rtk gh run list                 # Workflow-Run-Status
```

### Test-Runner
```bash
rtk test cargo test             # Nur Fehler anzeigen (-90%)
rtk err npm run build           # Nur Fehler/Warnungen
rtk vitest run                  # Vitest kompakt (nur Fehler)
rtk playwright test             # E2E-Ergebnisse (nur Fehler)
rtk pytest                      # Python-Tests (-90%)
rtk go test                     # Go-Tests (NDJSON, -90%)
rtk cargo test                  # Cargo-Tests (-90%)
rtk rake test                   # Ruby minitest (-90%)
rtk rspec                       # RSpec-Tests (JSON, -60%+)
```

### Build & Lint
```bash
rtk lint                        # ESLint nach Regel/Datei gruppiert
rtk lint biome                  # Unterstützt weitere Linter
rtk tsc                         # TypeScript-Fehler nach Datei gruppiert
rtk next build                  # Next.js-Build kompakt
rtk prettier --check .          # Dateien mit Formatierungsbedarf
rtk cargo build                 # Cargo-Build (-80%)
rtk cargo clippy                # Cargo clippy (-80%)
rtk ruff check                  # Python-Linting (JSON, -80%)
rtk golangci-lint run           # Go-Linting (JSON, -85%)
rtk rubocop                     # Ruby-Linting (JSON, -60%+)
```

### Paketmanager
```bash
rtk pnpm list                   # Kompakter Dependency-Baum
rtk pip list                    # Python-Pakete (uv auto-detect)
rtk pip outdated                # Veraltete Pakete
rtk bundle install              # Ruby-Gems (entfernt Using-Zeilen)
rtk prisma generate             # Schema-Generierung (ohne ASCII-Art)
```

### Container
```bash
rtk docker ps                   # Kompakte Container-Liste
rtk docker images               # Kompakte Image-Liste
rtk docker logs <container>     # Deduplizierte Logs
rtk docker compose ps           # Compose-Services
rtk kubectl pods                # Kompakte Pod-Liste
rtk kubectl logs <pod>          # Deduplizierte Logs
rtk kubectl services            # Kompakte Service-Liste
```

### Daten & Analytics
```bash
rtk json config.json            # Struktur ohne Werte
rtk deps                        # Dependency-Zusammenfassung
rtk env -f AWS                  # Gefilterte Umgebungsvariablen
rtk log app.log                 # Deduplizierte Logs
rtk curl <url>                  # JSON auto-detect + Schema
rtk wget <url>                  # Download, entfernt Progress-Bars
rtk summary <long command>      # Heuristische Zusammenfassung
rtk proxy <command>             # Rohes Passthrough + Tracking
```

### Token-Einsparungsanalyse
```bash
rtk gain                        # Zusammenfassungsstatistik
rtk gain --graph                # ASCII-Graph (letzte 30 Tage)
rtk gain --history              # Letzter Kommandoverlauf
rtk gain --daily                # Tagesweise Aufschlüsselung
rtk gain --all --format json    # JSON-Export für Dashboards

rtk discover                    # Verpasste Einsparpotenziale finden
rtk discover --all --since 7    # Alle Projekte, letzte 7 Tage

rtk session                     # RTK-Adoption über letzte Sessions anzeigen
```

## Globale Flags

```bash
-u, --ultra-compact    # ASCII-Icons, Inline-Format (zusätzliche Token-Einsparung)
-v, --verbose          # Ausführlichkeit erhöhen (-v, -vv, -vvv)
```

## Beispiele

**Verzeichnisauflistung:**
```
# ls -la (45 Zeilen, ~800 Tokens)        # rtk ls (12 Zeilen, ~150 Tokens)
drwxr-xr-x  15 user staff 480 ...        my-project/
-rw-r--r--   1 user staff 1234 ...        +-- src/ (8 files)
...                                       |   +-- main.rs
                                          +-- Cargo.toml
```

**Git-Operationen:**
```
# git push (15 Zeilen, ~200 Tokens)       # rtk git push (1 Zeile, ~10 Tokens)
Enumerating objects: 5, done.              ok main
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...
```

**Test-Ausgabe:**
```
# cargo test (200+ Zeilen bei Fehlern)    # rtk test cargo test (~20 Zeilen)
running 15 tests                           FAILED: 2/15 tests
test utils::test_parse ... ok                test_edge_case: assertion failed
test utils::test_format ... ok               test_overflow: panic at utils.rs:18
...
```

## Auto-Rewrite-Hook

Die effektivste Art, rtk zu nutzen. Der Hook fängt Bash-Befehle transparent ab und schreibt sie vor der Ausführung auf rtk-Äquivalente um.

**Ergebnis**: 100% rtk-Adoption über alle Konversationen und Subagents hinweg, ohne zusätzlichen Token-Overhead.

**Hinweis zum Geltungsbereich:** Das gilt nur für Bash-Tool-Calls. Claude-Code-Built-in-Tools wie `Read`, `Grep` und `Glob` umgehen den Hook. Nutze dort Shell-Befehle oder explizite `rtk`-Commands, wenn du RTK-Filterung möchtest.

### Setup

```bash
rtk init -g                 # Install hook + RTK.md (empfohlen)
rtk init -g --opencode      # OpenCode plugin (anstatt Claude Code)
rtk init -g --auto-patch    # Nicht-interaktiv (CI/CD)
rtk init -g --hook-only     # Nur Hook, ohne RTK.md
rtk init --show             # Installation prüfen
```

Nach der Installation **Claude Code neu starten**.

## Unterstützte AI-Tools

RTK unterstützt 9 AI-Coding-Tools. Jede Integration schreibt Shell-Befehle transparent auf `rtk`-Äquivalente um und spart so 60-90% Tokens.

| Tool | Install | Methode |
|------|---------|---------|
| **Claude Code** | `rtk init -g` | PreToolUse hook (bash) |
| **GitHub Copilot** | `rtk init -g` | PreToolUse hook (`rtk hook copilot`) |
| **Cursor** | `rtk init -g --agent cursor` | preToolUse hook (hooks.json) |
| **Gemini CLI** | `rtk init -g --gemini` | BeforeTool hook (`rtk hook gemini`) |
| **Codex** | `rtk init -g --codex` | AGENTS.md + RTK.md instructions |
| **Windsurf** | `rtk init --agent windsurf` | .windsurfrules (projektbezogen) |
| **Cline / Roo Code** | `rtk init --agent cline` | .clinerules (projektbezogen) |
| **OpenCode** | `rtk init -g --opencode` | Plugin TS (tool.execute.before) |
| **OpenClaw** | `openclaw plugins install ./openclaw` | Plugin TS (before_tool_call) |

### Claude Code (Standard)

```bash
rtk init -g                 # Install hook + RTK.md
rtk init -g --auto-patch    # Nicht-interaktiv (CI/CD)
rtk init --show             # Installation prüfen
rtk init -g --uninstall     # Entfernen
```

### GitHub Copilot (VS Code + CLI)

```bash
rtk init -g                 # Gleicher Hook wie bei Claude Code
```

Der Hook erkennt Copilot-Formate automatisch (VS Code `runTerminalCommand` oder CLI `toolName: bash`) und schreibt Kommandos um. Funktioniert sowohl mit Copilot Chat in VS Code als auch mit der `copilot` CLI.

### Cursor

```bash
rtk init -g --agent cursor
```

Erstellt `~/.cursor/hooks/rtk-rewrite.sh` und patcht `~/.cursor/hooks.json` mit einem preToolUse-Matcher. Funktioniert sowohl im Cursor-Editor als auch in der `cursor-agent` CLI.

### Gemini CLI

```bash
rtk init -g --gemini
rtk init -g --gemini --uninstall
```

Erstellt `~/.gemini/hooks/rtk-hook-gemini.sh` und patcht `~/.gemini/settings.json` mit einem BeforeTool-Hook.

### Codex (OpenAI)

```bash
rtk init -g --codex
```

Erstellt `~/.codex/RTK.md` und `~/.codex/AGENTS.md` mit `@RTK.md`-Referenz. Codex liest diese als globale Anweisungen.

### Windsurf

```bash
rtk init --agent windsurf
```

Erstellt `.windsurfrules` im aktuellen Projekt. Cascade liest diese Regeln und präfixiert Kommandos mit `rtk`.

### Cline / Roo Code

```bash
rtk init --agent cline
```

Erstellt `.clinerules` im aktuellen Projekt. Cline liest diese Regeln und präfixiert Kommandos mit `rtk`.

### OpenCode

```bash
rtk init -g --opencode
```

Erstellt `~/.config/opencode/plugins/rtk.ts`. Verwendet den `tool.execute.before`-Hook.

### OpenClaw

```bash
openclaw plugins install ./openclaw
```

Plugin liegt im Verzeichnis `openclaw/`. Verwendet den `before_tool_call`-Hook und delegiert an `rtk rewrite`.

### Umgeschriebene Kommandos

| Rohbefehl | Umgeschrieben zu |
|-----------|------------------|
| `git status/diff/log/add/commit/push/pull` | `rtk git ...` |
| `gh pr/issue/run` | `rtk gh ...` |
| `cargo test/build/clippy` | `rtk cargo ...` |
| `cat/head/tail <file>` | `rtk read <file>` |
| `rg/grep <pattern>` | `rtk grep <pattern>` |
| `ls` | `rtk ls` |
| `vitest/jest` | `rtk vitest run` |
| `tsc` | `rtk tsc` |
| `eslint/biome` | `rtk lint` |
| `prettier` | `rtk prettier` |
| `playwright` | `rtk playwright` |
| `prisma` | `rtk prisma` |
| `ruff check/format` | `rtk ruff ...` |
| `pytest` | `rtk pytest` |
| `pip list/install` | `rtk pip ...` |
| `go test/build/vet` | `rtk go ...` |
| `golangci-lint` | `rtk golangci-lint` |
| `rake test` / `rails test` | `rtk rake test` |
| `rspec` / `bundle exec rspec` | `rtk rspec` |
| `rubocop` / `bundle exec rubocop` | `rtk rubocop` |
| `bundle install/update` | `rtk bundle ...` |
| `docker ps/images/logs` | `rtk docker ...` |
| `kubectl get/logs` | `rtk kubectl ...` |
| `curl` | `rtk curl` |
| `pnpm list/outdated` | `rtk pnpm ...` |

Kommandos, die bereits `rtk` nutzen, Heredocs (`<<`) und unbekannte Befehle werden unverändert durchgereicht.

## Konfiguration

### Konfigurationsdatei

`~/.config/rtk/config.toml` (macOS: `~/Library/Application Support/rtk/config.toml`):

```toml
[tracking]
database_path = "/path/to/custom.db"  # default: ~/.local/share/rtk/history.db

[hooks]
exclude_commands = ["curl", "playwright"]  # skip rewrite for these

[tee]
enabled = true          # save raw output on failure (default: true)
mode = "failures"       # "failures", "always", or "never"
max_files = 20          # rotation limit
```

### Tee: Vollständige Ausgabe-Wiederherstellung

Wenn ein Befehl fehlschlägt, speichert RTK die vollständige, ungefilterte Ausgabe, sodass das LLM sie lesen kann, ohne den Befehl erneut auszuführen:

```
FAILED: 2/15 tests
[full output: ~/.local/share/rtk/tee/1707753600_cargo_test.log]
```

### Deinstallation

```bash
rtk init -g --uninstall     # Remove hook, RTK.md, settings.json entry
cargo uninstall rtk          # Remove binary
brew uninstall rtk           # If installed via Homebrew
```

## Dokumentation

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - Häufige Probleme beheben
- **[INSTALL.md](INSTALL.md)** - Detaillierte Installationsanleitung
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - Technische Architektur
- **[SECURITY.md](SECURITY.md)** - Sicherheitsrichtlinie und PR-Review-Prozess
- **[AUDIT_GUIDE.md](docs/AUDIT_GUIDE.md)** - Leitfaden für Token-Einsparungsanalysen

## Datenschutz & Telemetrie

RTK erfasst einmal täglich **anonyme, aggregierte Nutzungsmetriken**, um die Entwicklung zu priorisieren. Das ist gängige Praxis bei Open-Source-CLI-Tools.

**Was erfasst wird:**
- Geräte-Hash (SHA-256 aus Hostname+Username, nicht rückrechenbar)
- RTK-Version, Betriebssystem, Architektur
- Kommandoanzahl (letzte 24h) und häufigste Kommandonamen (z. B. "git", "cargo" — keine Argumente, keine Dateipfade)
- Prozentuale Token-Einsparung

**Was NICHT erfasst wird:** Quellcode, Dateipfade, Kommandoargumente, Secrets, Umgebungsvariablen oder personenbezogene Informationen.

**Opt-out** (eine dieser Optionen):
```bash
# Environment variable
export RTK_TELEMETRY_DISABLED=1

# Or in config file (~/.config/rtk/config.toml)
[telemetry]
enabled = false
```

## Mitwirken

Beiträge sind willkommen! Bitte öffne ein Issue oder PR auf [GitHub](https://github.com/rtk-ai/rtk).

Tritt der Community auf [Discord](https://discord.gg/RySmvNF5kF) bei.

## Lizenz

MIT License - siehe [LICENSE](LICENSE) für Details.
