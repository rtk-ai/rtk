<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Proxy CLI haute performance qui reduit la consommation de tokens LLM de 60-90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Site web</a> &bull;
  <a href="#installation">Installer</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">Depannage</a> &bull;
  <a href="ARCHITECTURE.md">Architecture</a> &bull;
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

rtk filtre et compresse les sorties de commandes avant qu'elles n'atteignent le contexte de votre LLM. Binaire Rust unique, zero dependance, <10ms d'overhead.

## Economies de tokens (session Claude Code de 30 min)

| Operation | Frequence | Standard | rtk | Economies |
|-----------|-----------|----------|-----|-----------|
| `ls` / `tree` | 10x | 2 000 | 400 | -80% |
| `cat` / `read` | 20x | 40 000 | 12 000 | -70% |
| `grep` / `rg` | 8x | 16 000 | 3 200 | -80% |
| `git status` | 10x | 3 000 | 600 | -80% |
| `git diff` | 5x | 10 000 | 2 500 | -75% |
| `git log` | 5x | 2 500 | 500 | -80% |
| `git add/commit/push` | 8x | 1 600 | 120 | -92% |
| `cargo test` / `npm test` | 5x | 25 000 | 2 500 | -90% |
| **Total** | | **~118 000** | **~23 900** | **-80%** |

> Estimations basees sur des projets TypeScript/Rust de taille moyenne.

## Installation

### Homebrew (recommande)

```bash
brew install rtk
```

### Installation rapide (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### Verification

```bash
rtk --version   # Doit afficher "rtk 0.27.x"
rtk gain        # Doit afficher les statistiques d'economies
```

> **Attention** : Un autre projet "rtk" (Rust Type Kit) existe sur crates.io. Si `rtk gain` echoue, vous avez le mauvais package.

## Demarrage rapide

```bash
# 1. Installer le hook pour Claude Code (recommande)
rtk init --global
# Suivre les instructions pour enregistrer dans ~/.claude/settings.json

# 2. Redemarrer Claude Code, puis tester
git status  # Automatiquement reecrit en rtk git status
```

Le hook reecrit de maniere transparente les commandes (ex: `git status` -> `rtk git status`) avant execution.

## Comment ca marche

```
  Sans rtk :                                       Avec rtk :

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |        ~2 000 tokens (brut)       |             |   ~200 tokens        | filtre   |
    +-----------------------------------+             +------- (filtre) -----+----------+
```

Quatre strategies appliquees par type de commande :

1. **Filtrage intelligent** - Supprime le bruit (commentaires, espaces, boilerplate)
2. **Regroupement** - Agregat d'elements similaires (fichiers par dossier, erreurs par type)
3. **Troncature** - Conserve le contexte pertinent, coupe la redondance
4. **Deduplication** - Fusionne les lignes de log repetees avec compteurs

## Commandes

### Fichiers
```bash
rtk ls .                        # Arbre de repertoires optimise
rtk read file.rs                # Lecture intelligente
rtk read file.rs -l aggressive  # Signatures uniquement
rtk find "*.rs" .               # Resultats compacts
rtk grep "pattern" .            # Resultats groupes par fichier
rtk diff file1 file2            # Diff condense
```

### Git
```bash
rtk git status                  # Status compact
rtk git log -n 10               # Commits sur une ligne
rtk git diff                    # Diff condense
rtk git add                     # -> "ok"
rtk git commit -m "msg"         # -> "ok abc1234"
rtk git push                    # -> "ok main"
```

### Tests
```bash
rtk test cargo test             # Echecs uniquement (-90%)
rtk vitest run                  # Vitest compact
rtk pytest                      # Tests Python (-90%)
rtk go test                     # Tests Go (-90%)
rtk cargo test                  # Tests Cargo (-90%)
```

### Build & Lint
```bash
rtk lint                        # ESLint groupe par regle
rtk tsc                         # Erreurs TypeScript groupees
rtk cargo build                 # Build Cargo (-80%)
rtk cargo clippy                # Clippy (-80%)
rtk ruff check                  # Linting Python (-80%)
```

### Conteneurs
```bash
rtk docker ps                   # Liste compacte
rtk docker logs <container>     # Logs dedupliques
rtk kubectl pods                # Pods compacts
```

### Analytics
```bash
rtk gain                        # Statistiques d'economies
rtk gain --graph                # Graphique ASCII (30 jours)
rtk discover                    # Trouver les economies manquees
```

## Configuration

```toml
# ~/.config/rtk/config.toml
[tracking]
database_path = "/chemin/custom.db"

[hooks]
exclude_commands = ["curl", "playwright"]

[tee]
enabled = true
mode = "failures"
```

## Documentation

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - Resoudre les problemes courants
- **[INSTALL.md](INSTALL.md)** - Guide d'installation detaille
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - Architecture technique

## Contribuer

Les contributions sont les bienvenues ! Ouvrez une issue ou une PR sur [GitHub](https://github.com/rtk-ai/rtk).

Rejoignez la communaute sur [Discord](https://discord.gg/RySmvNF5kF).

## Licence

Licence MIT - voir [LICENSE](LICENSE) pour les details.
