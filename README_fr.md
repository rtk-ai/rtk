<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Proxy CLI haute performance qui élimine jusqu'à 90% de la sortie bash lue par votre agent</strong>
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
  <a href="docs/TROUBLESHOOTING.md">Dépannage</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">Architecture</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Français</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a>
</p>

---

rtk filtre et compresse les sorties de commandes avant qu'elles n'atteignent le contexte de votre LLM. Binaire Rust unique, zéro dépendance, <10ms d'overhead.

## Ce que fait RTK

RTK intercepte les commandes shell et compresse leur sortie avant que votre agent ne la lise.

| Opération                 | Ce que RTK fait de la sortie |
|---------------------------|------------------------------|
| `ls` / `tree`             | Format arborescent avec compteurs de fichiers au lieu d'une ligne par entree |
| `cat` / `read`            | Lecture intelligente : signatures et structure plutot que corps complets |
| `grep` / `rg`             | Tronque les lignes longues, regroupe les correspondances par fichier |
| `git status`              | Format stat compact, regroupe par etat |
| `git diff`                | Contexte reduit, en-tetes supprimes |
| `git log`                 | Hash, auteur et sujet uniquement |
| `git add/commit/push`     | Ligne de confirmation au lieu de la sortie de progression complete |
| `cargo test` / `npm test` | Echecs uniquement, tests reussis reduits a un compteur |
| `ruff check`              | Regroupe par regle et par fichier |
| `pytest`                  | Echecs uniquement, traceback raccourci |
| `go test`                 | NDJSON parse, echecs uniquement |
| `docker ps`               | Champs essentiels uniquement |

## Comment fonctionnent les économies

RTK élimine **jusqu'à 90% de la sortie bash** que votre agent lit. C'est cela que RTK mesure, et ce n'est pas la même chose que réduire votre facture de 90%.

La sortie bash est **un contributeur parmi d'autres aux tokens d'entrée**, aux cotes de votre prompt, du prompt système et de l'historique de conversation. Les tokens d'entrée ne sont eux-mêmes **qu'une partie de la facture**, qui compte aussi les tokens de sortie. La réduction se dilue à chaque étape.

Les nombres de tokens rapportés par RTK sont estimés avec `octets / 4` : RTK n'embarque aucun tokenizer, donc les **pourcentages sont fiables mais les valeurs absolues en tokens restent approximatives**.

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

### Vérification

```bash
rtk --version   # Doit afficher "rtk 0.27.x"
rtk gain        # Doit afficher les statistiques d’économies
```

> **Attention** : Un autre projet "rtk" (Rust Type Kit) existe sur crates.io. Si `rtk gain` échoue, vous avez le mauvais package.

## Démarrage rapide

```bash
# 1. Installer le hook pour Claude Code (recommande)
rtk init --global
# Suivre les instructions pour enregistrer dans ~/.claude/settings.json

# 2. Redémarrer Claude Code, puis tester
git status  # Automatiquement réécrit en rtk git status
```

Le hook réécrit de manière transparente les commandes (ex : `git status` -> `rtk git status`) avant exécution.

## Comment ça marche

```
  Sans rtk :                                       Avec rtk :

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |        ~2 000 tokens (brut)       |             |   ~200 tokens        | filtre   |
    +-----------------------------------+             +------- (filtre) -----+----------+
```

Quatre stratégies appliquées par type de commande :

1. **Filtrage intelligent** - Supprime le bruit (commentaires, espaces, boilerplate)
2. **Regroupement** - Agrégat d'éléments similaires (fichiers par dossier, erreurs par type)
3. **Troncature** - Conserve le contexte pertinent, coupe la redondance
4. **Déduplication** - Fusionne les lignes de log répétées avec compteurs

## Commandes

> Les pourcentages ci-dessous sont des **réductions d'octets de sortie bash**, mesurées avec l'estimateur `octets / 4` de RTK. Voir [Comment fonctionnent les economies](#comment-fonctionnent-les-economies).

### Fichiers
```bash
rtk ls .                        # Arbre de répertoires optimisé
rtk read file.rs                # Lecture intelligente
rtk read file.rs -l aggressive  # Signatures uniquement
rtk find "*.rs" .               # Résultats compacts
rtk grep "pattern" .            # Résultats groupes par fichier
rtk diff file1 file2            # Diff condensé
```

### Git
```bash
rtk git status                  # Status compact
rtk git log -n 10               # Commits sur une ligne
rtk git diff                    # Diff condensé
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
rtk test <cmd>                  # Échecs uniquement (-90%)
```

### Build & Lint
```bash
rtk lint                        # ESLint groupe par règle
rtk tsc                         # Erreurs TypeScript groupées
rtk cargo build                 # Build Cargo (-80%)
rtk cargo clippy                # Clippy (-80%)
rtk ruff check                  # Linting Python (-80%)
```

### Conteneurs
```bash
rtk docker ps                   # Liste compacte
rtk docker logs <container>     # Logs dédupliqués
rtk kubectl pods                # Pods compacts
```

### Analytics
```bash
rtk gain                        # Statistiques d'économies
rtk gain --graph                # Graphique ASCII (30 jours)
rtk discover                    # Trouver les économies manquées
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

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - Résoudre les problèmes courants
- **[INSTALL.md](INSTALL.md)** - Guide d'installation détaillé
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - Architecture technique

## Contribuer

Les contributions sont les bienvenues ! Ouvrez une issue ou une PR sur [GitHub](https://github.com/rtk-ai/rtk).

Rejoignez la communauté sur [Discord](https://discord.gg/RySmvNF5kF).

## Licence

Licence Apache 2.0 - voir [LICENSE](LICENSE) pour les détails.

## Avertissement

Voir [DISCLAIMER.md](DISCLAIMER.md).
