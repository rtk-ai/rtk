<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Proxy CLI haute performance qui elimine jusqu'a 90% de la sortie bash lue par votre agent</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Site web</a> &bull;
  <a href="#installation">Installer</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">Depannage</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">Architecture</a> &bull;
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

## Ce que fait RTK

RTK intercepte les commandes shell et compresse leur sortie avant que votre agent ne la lise.

| Operation | Ce que RTK fait de la sortie |
|-----------|------------------------------|
| `ls` / `tree` | Format arborescent avec compteurs de fichiers au lieu d'une ligne par entree |
| `cat` / `read` | Lecture intelligente : signatures et structure plutot que corps complets |
| `grep` / `rg` | Tronque les lignes longues, regroupe les correspondances par fichier |
| `git status` | Format stat compact, regroupe par etat |
| `git diff` | Contexte reduit, en-tetes supprimes |
| `git log` | Hash, auteur et sujet uniquement |
| `git add/commit/push` | Ligne de confirmation au lieu de la sortie de progression complete |
| `cargo test` / `npm test` | Echecs uniquement, tests reussis reduits a un compteur |
| `ruff check` | Regroupe par regle et par fichier |
| `pytest` | Echecs uniquement, traceback raccourci |
| `go test` | NDJSON parse, echecs uniquement |
| `docker ps` | Champs essentiels uniquement |

## Comment fonctionnent les economies

RTK elimine **jusqu'a 90% de la sortie bash** que votre agent lit. C'est cela que RTK mesure, et ce n'est pas la meme chose que reduire votre facture de 90%.

La sortie bash est **un contributeur parmi d'autres aux tokens d'entree**, aux cotes de votre prompt, du prompt systeme et de l'historique de conversation. Les tokens d'entree ne sont eux-memes **qu'une partie de la facture**, qui compte aussi les tokens de sortie. La reduction se dilue a chaque etape.

Les nombres de tokens rapportes par RTK sont estimes avec `octets / 4` : RTK n'embarque aucun tokenizer, donc les **pourcentages sont fiables mais les valeurs absolues en tokens restent approximatives**.

> Explication complete : [Comment fonctionnent les economies RTK](docs/guide/resources/savings-explained.md)

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

> Les pourcentages ci-dessous sont des **reductions d'octets de sortie bash**, mesurees avec l'estimateur `octets / 4` de RTK. Voir [Comment fonctionnent les economies](#comment-fonctionnent-les-economies).

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
rtk jest                        # Jest compact
rtk vitest                      # Vitest compact
rtk pytest                      # Tests Python (-90%)
rtk go test                     # Tests Go (-90%)
rtk cargo test                  # Tests Cargo (-90%)
rtk test <cmd>                  # Echecs uniquement (-90%)
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
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - Architecture technique

## Contribuer

Les contributions sont les bienvenues ! Ouvrez une issue ou une PR sur [GitHub](https://github.com/rtk-ai/rtk).

Rejoignez la communaute sur [Discord](https://discord.gg/RySmvNF5kF).

## Licence

Licence Apache 2.0 - voir [LICENSE](LICENSE) pour les details.

## Avertissement

Voir [DISCLAIMER.md](DISCLAIMER.md).
