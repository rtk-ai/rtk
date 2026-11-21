<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Proxy CLI ad alte prestazioni che riduce il consumo di token LLM del 60-90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Sito web</a> &bull;
  <a href="#installazione">Installa</a> &bull;
  <a href="https://www.rtk-ai.app/guide/troubleshooting">Risoluzione dei problemi</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">Architettura</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a> &bull;
  <a href="README_pt.md">Português</a> &bull;
  <a href="README_it.md">Italiano</a>
</p>

---

rtk filtra e comprime l'output dei comandi prima che raggiunga il contesto del tuo LLM. Binario Rust unico, zero dipendenze, overhead inferiore a 10ms.

## Risparmio di token (sessione di 30 min in Claude Code)

| Operazione                | Frequenza | Standard     | rtk         | Risparmio |
| ------------------------- | --------- | ------------ | ----------- | --------- |
| `ls` / `tree`             | 10x       | 2,000        | 400         | -80%      |
| `cat` / `read`            | 20x       | 40,000       | 12,000      | -70%      |
| `grep` / `rg`             | 8x        | 16,000       | 3,200       | -80%      |
| `git status`              | 10x       | 3,000        | 600         | -80%      |
| `cargo test` / `npm test` | 5x        | 25,000       | 2,500       | -90%      |
| **Totale**                |           | **~118,000** | **~23,900** | **-80%**  |

## Installazione

### Homebrew (consigliato)

```bash
brew install rtk
```

### Installazione rapida (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### Verifica

```bash
rtk --version   # Dovrebbe mostrare "rtk 0.28.2"
rtk gain        # Dovrebbe mostrare le statistiche di risparmio
```

## Avvio rapido

```bash
# 1. Installa l'hook per Claude Code (consigliato)
rtk init --global

# 2. Riavvia Claude Code, poi prova
git status  # Riscritto automaticamente in rtk git status
```

## Come funziona

```
  Senza rtk:                                      Con rtk:

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |        ~2,000 token (grezzo)      |            |   ~200 token         | filtro   |
    +-----------------------------------+            +------- (filtrato) ---+----------+
```

Quattro strategie:

1. **Filtraggio intelligente** - Elimina il rumore (commenti, spazi, boilerplate)
2. **Raggruppamento** - Aggrega elementi simili (file per directory, errori per tipo)
3. **Troncamento** - Mantiene il contesto rilevante, elimina la ridondanza
4. **Deduplicazione** - Comprime le righe di log ripetute con contatori

## Comandi

### File

```bash
rtk ls .                        # Albero di directory ottimizzato
rtk read file.rs                # Lettura intelligente
rtk find "*.rs" .               # Risultati compatti
rtk grep "pattern" .            # Ricerca raggruppata per file
```

### Git

```bash
rtk git status                  # Status compatto
rtk git log -n 10               # Commit su una riga
rtk git diff                    # Diff condensato
rtk git push                    # -> "ok main"
```

### Test

```bash
rtk jest                        # Jest compatto
rtk vitest                      # Vitest compatto
rtk pytest                      # Test Python (-90%)
rtk go test                     # Test Go (-90%)
rtk cargo test                  # Test Rust (-90%)
rtk test <cmd>                  # Solo fallimenti (-90%)
```

### Build & Lint

```bash
rtk lint                        # ESLint raggruppato per regola
rtk tsc                         # Errori TypeScript raggruppati
rtk cargo build                 # Build Cargo (-80%)
rtk ruff check                  # Lint Python (-80%)
```

### Analisi

```bash
rtk gain                        # Statistiche di risparmio
rtk gain --graph                # Grafico ASCII (30 giorni)
rtk discover                    # Scopri i risparmi mancati
```

## Documentazione

- **[INSTALL.md](INSTALL.md)** - Guida di installazione dettagliata
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - Architettura tecnica
- **[CONTRIBUTING.md](CONTRIBUTING.md)** - Guida ai contributi

## Contribuire

I contributi sono benvenuti. Apri una issue o una PR su [GitHub](https://github.com/rtk-ai/rtk).

Unisciti alla community su [Discord](https://discord.gg/RySmvNF5kF).

## Licenza

Licenza Apache 2.0 - vedi [LICENSE](LICENSE) per i dettagli.

## Avviso legale

Vedi [DISCLAIMER.md](DISCLAIMER.md).
