# RTK - Documentation fonctionnelle complete

> **rtk (Rust Token Killer)** -- Proxy CLI haute performance qui reduit la consommation de tokens LLM de 60 a 90%.

Binaire Rust unique, zero dependances externes, overhead < 10ms par commande.

---

## Table des matieres

1. [Vue d'ensemble](#vue-densemble)
2. [Drapeaux globaux](#drapeaux-globaux)
3. [Commandes Fichiers](#commandes-fichiers)
4. [Commandes Git](#commandes-git)
5. [Commandes GitHub CLI](#commandes-github-cli)
6. [Commandes Test](#commandes-test)
7. [Commandes Build et Lint](#commandes-build-et-lint)
8. [Commandes Formatage](#commandes-formatage)
9. [Gestionnaires de paquets](#gestionnaires-de-paquets)
10. [Conteneurs et orchestration](#conteneurs-et-orchestration)
11. [Donnees et reseau](#donnees-et-reseau)
12. [Cloud et bases de donnees](#cloud-et-bases-de-donnees)
13. [Stacked PRs (Graphite)](#stacked-prs-graphite)
14. [Analytique et suivi](#analytique-et-suivi)
15. [Systeme de hooks](#systeme-de-hooks)
16. [Configuration](#configuration)
17. [Systeme Tee (recuperation de sortie)](#systeme-tee)
18. [Telemetrie](#telemetrie)

---

## Vue d'ensemble

rtk agit comme un proxy entre un LLM (Claude Code, Gemini CLI, etc.) et les commandes systeme. Quatre strategies de filtrage sont appliquees selon le type de commande :

| Strategie | Description | Exemple |
|-----------|-------------|---------|
| **Filtrage intelligent** | Supprime le bruit (commentaires, espaces, boilerplate) | `ls -la` -> arbre compact |
| **Regroupement** | Agregation par repertoire, par type d'erreur, par regle | Tests groupes par fichier |
| **Troncature** | Conserve le contexte pertinent, supprime la redondance | Diff condense |
| **Deduplication** | Fusionne les lignes de log repetees avec compteurs | `error x42` |

### Mecanisme de fallback

Si rtk ne reconnait pas une sous-commande, il execute la commande brute (passthrough) et enregistre l'evenement dans la base de suivi. Cela garantit que rtk est **toujours sur** a utiliser -- aucune commande ne sera bloquee.

---

## Drapeaux globaux

Ces drapeaux s'appliquent a **toutes** les sous-commandes :

| Drapeau | Court | Description |
|---------|-------|-------------|
| `--verbose` | `-v` | Augmenter la verbosite (-v, -vv, -vvv). Montre les details de filtrage. |
| `--ultra-compact` | `-u` | Mode ultra-compact : icones ASCII, format inline. Economies supplementaires. |
| `--skip-env` | -- | Definit `SKIP_ENV_VALIDATION=1` pour les processus enfants (Next.js, tsc, lint, prisma). |

**Exemples :**

```bash
rtk -v git status          # Status compact + details de filtrage sur stderr
rtk -vvv cargo test        # Verbosite maximale (debug)
rtk -u git log             # Log ultra-compact, icones ASCII
rtk --skip-env next build  # Desactive la validation d'env de Next.js
```

---

## Commandes Fichiers

### `rtk ls` -- Listage de repertoire

**Objectif :** Remplace `ls` et `tree` avec une sortie optimisee en tokens.

**Syntaxe :**
```bash
rtk ls [args...]
```

Tous les drapeaux natifs de `ls` sont supportes (`-l`, `-a`, `-h`, `-R`, etc.).

**Economies :** ~80% de reduction de tokens

**Avant / Apres :**
```
# ls -la (45 lignes, ~800 tokens)          # rtk ls (12 lignes, ~150 tokens)
drwxr-xr-x  15 user staff 480 ...          my-project/
-rw-r--r--   1 user staff 1234 ...          +-- src/ (8 files)
-rw-r--r--   1 user staff 567 ...           |   +-- main.rs
...40 lignes de plus...                     +-- Cargo.toml
                                            +-- README.md
```

---

### `rtk tree` -- Arbre de repertoire

**Objectif :** Proxy vers `tree` natif avec sortie filtree.

**Syntaxe :**
```bash
rtk tree [args...]
```

Supporte tous les drapeaux natifs de `tree` (`-L`, `-d`, `-a`, etc.).

**Economies :** ~80%

---

### `rtk read` -- Lecture de fichier

**Objectif :** Remplace `cat`, `head`, `tail` avec un filtrage intelligent du contenu.

**Syntaxe :**
```bash
rtk read <fichier> [options]
rtk read - [options]          # Lecture depuis stdin
```

**Options :**

| Option | Court | Defaut | Description |
|--------|-------|--------|-------------|
| `--level` | `-l` | `minimal` | Niveau de filtrage : `none`, `minimal`, `aggressive` |
| `--max-lines` | `-m` | illimite | Nombre maximum de lignes |
| `--line-numbers` | `-n` | non | Afficher les numeros de ligne |

**Niveaux de filtrage :**

| Niveau | Description | Economies |
|--------|-------------|-----------|
| `none` | Aucun filtrage, sortie brute | 0% |
| `minimal` | Supprime commentaires et lignes vides excessives | ~30% |
| `aggressive` | Signatures uniquement (supprime les corps de fonctions) | ~74% |

**Avant / Apres (mode aggressive) :**
```
# cat main.rs (~200 lignes)                # rtk read main.rs -l aggressive (~50 lignes)
fn main() -> Result<()> {                   fn main() -> Result<()> { ... }
    let config = Config::load()?;           fn process_data(input: &str) -> Vec<u8> { ... }
    let data = process_data(&input);        struct Config { ... }
    for item in data {                      impl Config { fn load() -> Result<Self> { ... } }
        println!("{}", item);
    }
    Ok(())
}
...
```

**Langages supportes pour le filtrage :** Rust, Python, JavaScript, TypeScript, Go, C, C++, Java, Ruby, Shell.

---

### `rtk smart` -- Resume heuristique

**Objectif :** Genere un resume technique de 2 lignes pour un fichier source.

**Syntaxe :**
```bash
rtk smart <fichier> [--model heuristic] [--force-download]
```

**Economies :** ~95%

**Exemple :**
```
$ rtk smart src/tracking.rs
SQLite-based token tracking system for command executions.
Records input/output tokens, savings %, execution times with 90-day retention.
```

---

### `rtk find` -- Recherche de fichiers

**Objectif :** Remplace `find` et `fd` avec une sortie compacte groupee par repertoire.

**Syntaxe :**
```bash
rtk find [args...]
```

Supporte a la fois la syntaxe RTK et la syntaxe native `find` (`-name`, `-type`, etc.).

**Economies :** ~80%

**Avant / Apres :**
```
# find . -name "*.rs" (30 lignes)           # rtk find "*.rs" . (8 lignes)
./src/main.rs                                src/ (12 .rs)
./src/git.rs                                   main.rs, git.rs, config.rs
./src/config.rs                                tracking.rs, filter.rs, utils.rs
./src/tracking.rs                              ...6 more
./src/filter.rs                              tests/ (3 .rs)
./src/utils.rs                                 test_git.rs, test_ls.rs, test_filter.rs
...24 lignes de plus...
```

---

### `rtk grep` -- Recherche dans le contenu

**Objectif :** Remplace `grep` et `rg` avec une sortie groupee par fichier, tronquee.

**Syntaxe :**
```bash
rtk grep <pattern> [chemin] [options]
```

**Options :**

| Option | Court | Defaut | Description |
|--------|-------|--------|-------------|
| `--max-len` | `-l` | 80 | Longueur maximale de ligne |
| `--max` | `-m` | 50 | Nombre maximum de resultats |
| `--context-only` | `-c` | non | Afficher uniquement le contexte du match |
| `--file-type` | `-t` | tous | Filtrer par type (ts, py, rust, etc.) |
| `--line-numbers` | `-n` | oui | Numeros de ligne (toujours actif) |

Les arguments supplementaires sont transmis a `rg` (ripgrep).

**Economies :** ~80%

**Avant / Apres :**
```
# rg "fn run" (20 lignes)                   # rtk grep "fn run" (10 lignes)
src/git.rs:45:pub fn run(...)                src/git.rs
src/git.rs:120:fn run_status(...)              45: pub fn run(...)
src/ls.rs:12:pub fn run(...)                   120: fn run_status(...)
src/ls.rs:25:fn run_tree(...)                src/ls.rs
...                                            12: pub fn run(...)
                                               25: fn run_tree(...)
```

---

### `rtk diff` -- Diff condense

**Objectif :** Diff ultra-condense entre deux fichiers (uniquement les lignes modifiees).

**Syntaxe :**
```bash
rtk diff <fichier1> <fichier2>
rtk diff <fichier1>              # Stdin comme second fichier
```

**Economies :** ~60%

---

### `rtk wc` -- Comptage compact

**Objectif :** Remplace `wc` avec une sortie compacte (supprime les chemins et le padding).

**Syntaxe :**
```bash
rtk wc [args...]
```

Supporte tous les drapeaux natifs de `wc` (`-l`, `-w`, `-c`, etc.).

---

## Commandes Git

### Vue d'ensemble

Toutes les sous-commandes git sont supportees. Les commandes non reconnues sont transmises directement a git (passthrough).

**Options globales git :**

| Option | Description |
|--------|-------------|
| `-C <path>` | Changer de repertoire avant execution |
| `-c <key=value>` | Surcharger une config git |
| `--git-dir <path>` | Chemin vers le repertoire .git |
| `--work-tree <path>` | Chemin vers le working tree |
| `--no-pager` | Desactiver le pager |
| `--no-optional-locks` | Ignorer les locks optionnels |
| `--bare` | Traiter comme repo bare |
| `--literal-pathspecs` | Pathspecs literals |

---

### `rtk git status` -- Status compact

**Economies :** ~80%

```bash
rtk git status [args...]    # Supporte tous les drapeaux git status
```

**Avant / Apres :**
```
# git status (~20 lignes, ~400 tokens)      # rtk git status (~5 lignes, ~80 tokens)
On branch main                               main | 3M 1? 1A
Your branch is up to date with               M src/main.rs
  'origin/main'.                              M src/git.rs
                                              M tests/test_git.rs
Changes not staged for commit:                ? new_file.txt
  (use "git add <file>..." to update)        A staged_file.rs
  modified:   src/main.rs
  modified:   src/git.rs
  ...
```

---

### `rtk git log` -- Historique compact

**Economies :** ~80%

```bash
rtk git log [args...]    # Supporte --oneline, --graph, --all, -n, etc.
```

**Avant / Apres :**
```
# git log (50+ lignes)                      # rtk git log -n 5 (5 lignes)
commit abc123def... (HEAD -> main)           abc123 Fix token counting bug
Author: User <user@email.com>               def456 Add vitest support
Date:   Mon Jan 15 10:30:00 2024            789abc Refactor filter engine
                                             012def Update README
    Fix token counting bug                   345ghi Initial commit
...
```

---

### `rtk git diff` -- Diff compact

**Economies :** ~75%

```bash
rtk git diff [args...]    # Supporte --stat, --cached, --staged, etc.
```

**Avant / Apres :**
```
# git diff (~100 lignes)                    # rtk git diff (~25 lignes)
diff --git a/src/main.rs b/src/main.rs      src/main.rs (+5/-2)
index abc123..def456 100644                    +  let config = Config::load()?;
--- a/src/main.rs                              +  config.validate()?;
+++ b/src/main.rs                              -  // old code
@@ -10,6 +10,8 @@                              -  let x = 42;
   fn main() {                               src/git.rs (+1/-1)
+    let config = Config::load()?;              ~  format!("ok {}", branch)
...30 lignes de headers et contexte...
```

---

### `rtk git show` -- Show compact

**Economies :** ~80%

```bash
rtk git show [args...]
```

Affiche le resume du commit + stat + diff compact.

---

### `rtk git add` -- Add ultra-compact

**Economies :** ~92%

```bash
rtk git add [args...]    # Supporte -A, -p, --all, etc.
```

**Sortie :** `ok` (un seul mot)

---

### `rtk git commit` -- Commit ultra-compact

**Economies :** ~92%

```bash
rtk git commit -m "message" [args...]    # Supporte -a, --amend, --allow-empty, etc.
```

**Sortie :** `ok abc1234` (confirmation + hash court)

---

### `rtk git push` -- Push ultra-compact

**Economies :** ~92%

```bash
rtk git push [args...]    # Supporte -u, remote, branch, etc.
```

**Avant / Apres :**
```
# git push (15 lignes, ~200 tokens)         # rtk git push (1 ligne, ~10 tokens)
Enumerating objects: 5, done.                ok main
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...
```

---

### `rtk git pull` -- Pull ultra-compact

**Economies :** ~92%

```bash
rtk git pull [args...]
```

**Sortie :** `ok 3 files +10 -2`

---

### `rtk git branch` -- Branches compact

```bash
rtk git branch [args...]    # Supporte -d, -D, -m, etc.
```

Affiche branche courante, branches locales, branches distantes de facon compacte.

---

### `rtk git fetch` -- Fetch compact

```bash
rtk git fetch [args...]
```

**Sortie :** `ok fetched (N new refs)`

---

### `rtk git stash` -- Stash compact

```bash
rtk git stash [list|show|pop|apply|drop|push] [args...]
```

---

### `rtk git worktree` -- Worktree compact

```bash
rtk git worktree [add|remove|prune|list] [args...]
```

---

### Passthrough git

Toute sous-commande git non listee ci-dessus est executee directement :

```bash
rtk git rebase main        # Execute git rebase main
rtk git cherry-pick abc    # Execute git cherry-pick abc
rtk git tag v1.0.0         # Execute git tag v1.0.0
```

---

## Commandes GitHub CLI

### `rtk gh` -- GitHub CLI compact

**Objectif :** Remplace `gh` avec une sortie optimisee.

**Syntaxe :**
```bash
rtk gh <sous-commande> [args...]
```

**Sous-commandes supportees :**

| Commande | Description | Economies |
|----------|-------------|-----------|
| `rtk gh pr list` | Liste des PRs compacte | ~80% |
| `rtk gh pr view <num>` | Details d'une PR + checks | ~87% |
| `rtk gh pr checks` | Status des checks CI | ~79% |
| `rtk gh issue list` | Liste des issues compacte | ~80% |
| `rtk gh run list` | Status des workflow runs | ~82% |
| `rtk gh api <endpoint>` | Reponse API compacte | ~26% |

**Avant / Apres :**
```
# gh pr list (~30 lignes)                   # rtk gh pr list (~10 lignes)
Showing 10 of 15 pull requests in org/repo   #42 feat: add vitest (open, 2d)
                                              #41 fix: git diff crash (open, 3d)
#42  feat: add vitest support                 #40 chore: update deps (merged, 5d)
  user opened about 2 days ago                #39 docs: add guide (merged, 1w)
  ... labels: enhancement
...
```

---

## Commandes Test

### `rtk test` -- Wrapper de tests generique

**Objectif :** Execute n'importe quelle commande de test et affiche uniquement les echecs.

**Syntaxe :**
```bash
rtk test <commande...>
```

**Economies :** ~90%

**Exemple :**
```bash
rtk test cargo test
rtk test npm test
rtk test bun test
rtk test pytest
```

**Avant / Apres :**
```
# cargo test (200+ lignes en cas d'echec)   # rtk test cargo test (~20 lignes)
running 15 tests                             FAILED: 2/15 tests
test utils::test_parse ... ok                  test_edge_case: assertion failed
test utils::test_format ... ok                 test_overflow: panic at utils.rs:18
test utils::test_edge_case ... FAILED
...150 lignes de backtrace...
```

---

### `rtk err` -- Erreurs/avertissements uniquement

**Objectif :** Execute une commande et ne montre que les erreurs et avertissements.

**Syntaxe :**
```bash
rtk err <commande...>
```

**Economies :** ~80%

**Exemple :**
```bash
rtk err npm run build
rtk err cargo build
```

---

### `rtk cargo test` -- Tests Rust

**Economies :** ~90%

```bash
rtk cargo test [args...]
```

N'affiche que les echecs. Supporte tous les arguments de `cargo test`.

---

### `rtk cargo nextest` -- Tests Rust (nextest)

```bash
rtk cargo nextest [run|list|--lib] [args...]
```

Filtre la sortie de `cargo nextest` pour n'afficher que les echecs.

---

### `rtk vitest run` -- Tests Vitest

**Economies :** ~99.5%

```bash
rtk vitest run [args...]
```

---

### `rtk playwright test` -- Tests E2E Playwright

**Economies :** ~94%

```bash
rtk playwright [args...]
```

---

### `rtk pytest` -- Tests Python

**Economies :** ~90%

```bash
rtk pytest [args...]
```

---

### `rtk go test` -- Tests Go

**Economies :** ~90%

```bash
rtk go test [args...]
```

Utilise le streaming JSON NDJSON de Go pour un filtrage precis.

---

## Commandes Build et Lint

### `rtk cargo build` -- Build Rust

**Economies :** ~80%

```bash
rtk cargo build [args...]
```

Supprime les lignes "Compiling...", ne conserve que les erreurs et le resultat final.

---

### `rtk cargo check` -- Check Rust

**Economies :** ~80%

```bash
rtk cargo check [args...]
```

Supprime les lignes "Checking...", ne conserve que les erreurs.

---

### `rtk cargo clippy` -- Clippy Rust

**Economies :** ~80%

```bash
rtk cargo clippy [args...]
```

Regroupe les avertissements par regle de lint.

---

### `rtk cargo install` -- Install Rust

```bash
rtk cargo install [args...]
```

Supprime la compilation des dependances, ne conserve que le resultat d'installation et les erreurs.

---

### `rtk tsc` -- TypeScript Compiler

**Economies :** ~83%

```bash
rtk tsc [args...]
```

Regroupe les erreurs TypeScript par fichier et par code d'erreur.

**Avant / Apres :**
```
# tsc --noEmit (50 lignes)                  # rtk tsc (15 lignes)
src/api.ts(12,5): error TS2345: ...          src/api.ts (3 errors)
src/api.ts(15,10): error TS2345: ...           TS2345: Argument type mismatch (x2)
src/api.ts(20,3): error TS7006: ...            TS7006: Parameter implicitly has 'any'
src/utils.ts(5,1): error TS2304: ...         src/utils.ts (1 error)
...                                            TS2304: Cannot find name 'foo'
```

---

### `rtk lint` -- ESLint / Biome

**Economies :** ~84%

```bash
rtk lint [args...]
rtk lint biome [args...]
```

Regroupe les violations par regle et par fichier. Auto-detecte le linter.

---

### `rtk prettier` -- Verification du formatage

**Economies :** ~70%

```bash
rtk prettier [args...]    # ex: rtk prettier --check .
```

Affiche uniquement les fichiers necessitant un formatage.

---

### `rtk format` -- Formateur universel

```bash
rtk format [args...]
```

Auto-detecte le formateur du projet (prettier, black, ruff format) et applique un filtre compact.

---

### `rtk next build` -- Build Next.js

**Economies :** ~87%

```bash
rtk next [args...]
```

Sortie compacte avec metriques de routes.

---

### `rtk ruff` -- Linter/formateur Python

**Economies :** ~80%

```bash
rtk ruff check [args...]
rtk ruff format --check [args...]
```

Sortie JSON compressee.

---

### `rtk mypy` -- Type checker Python

```bash
rtk mypy [args...]
```

Regroupe les erreurs de type par fichier.

---

### `rtk golangci-lint` -- Linter Go

**Economies :** ~85%

```bash
rtk golangci-lint run [args...]
```

Sortie JSON compressee.

---

## Commandes Formatage

### `rtk prettier` -- Prettier

```bash
rtk prettier --check .
rtk prettier --write src/
```

---

### `rtk format` -- Detecteur universel

```bash
rtk format [args...]
```

Detecte automatiquement : prettier, black, ruff format, rustfmt. Applique un filtre compact unifie.

---

## Gestionnaires de paquets

### `rtk pnpm` -- pnpm

| Commande | Description | Economies |
|----------|-------------|-----------|
| `rtk pnpm list [-d N]` | Arbre de dependances compact | ~70% |
| `rtk pnpm outdated` | Paquets obsoletes : `pkg: old -> new` | ~80% |
| `rtk pnpm install [pkgs...]` | Filtre les barres de progression | ~60% |
| `rtk pnpm build` | Delegue au filtre Next.js | ~87% |
| `rtk pnpm typecheck` | Delegue au filtre tsc | ~83% |

Les sous-commandes non reconnues sont transmises directement a pnpm (passthrough).

---

### `rtk npm` -- npm

```bash
rtk npm [args...]    # ex: rtk npm run build
```

Filtre le boilerplate npm (barres de progression, en-tetes, etc.).

---

### `rtk npx` -- npx avec routage intelligent

```bash
rtk npx [args...]
```

Route intelligemment vers les filtres specialises :
- `rtk npx tsc` -> filtre tsc
- `rtk npx eslint` -> filtre lint
- `rtk npx prisma` -> filtre prisma
- Autres -> passthrough filtre

---

### `rtk pip` -- pip / uv

```bash
rtk pip list              # Liste des paquets (auto-detecte uv)
rtk pip outdated          # Paquets obsoletes
rtk pip install <pkg>     # Installation
```

Auto-detecte `uv` si disponible et l'utilise a la place de `pip`.

---

### `rtk deps` -- Resume des dependances

**Objectif :** Resume compact des dependances du projet.

```bash
rtk deps [chemin]    # Defaut: repertoire courant
```

Auto-detecte : `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, `Gemfile`, etc.

**Economies :** ~70%

---

### `rtk prisma` -- ORM Prisma

| Commande | Description |
|----------|-------------|
| `rtk prisma generate` | Generation du client (supprime l'ASCII art) |
| `rtk prisma migrate dev [--name N]` | Creer et appliquer une migration |
| `rtk prisma migrate status` | Status des migrations |
| `rtk prisma migrate deploy` | Deployer en production |
| `rtk prisma db-push` | Push du schema |

---

## Conteneurs et orchestration

### `rtk docker` -- Docker

| Commande | Description | Economies |
|----------|-------------|-----------|
| `rtk docker ps` | Liste compacte des conteneurs | ~80% |
| `rtk docker images` | Liste compacte des images | ~80% |
| `rtk docker logs <conteneur>` | Logs dedupliques | ~70% |
| `rtk docker compose ps` | Services Compose compacts | ~80% |
| `rtk docker compose logs [service]` | Logs Compose dedupliques | ~70% |
| `rtk docker compose build [service]` | Resume du build | ~60% |

Les sous-commandes non reconnues sont transmises directement (passthrough).

**Avant / Apres :**
```
# docker ps (lignes longues, ~30 tokens/ligne)    # rtk docker ps (~10 tokens/ligne)
CONTAINER ID   IMAGE          COMMAND     ...      web  nginx:1.25 Up 2d (healthy)
abc123def456   nginx:1.25     "/dock..."  ...      db   postgres:16 Up 2d (healthy)
789012345678   postgres:16    "docker..."           redis redis:7 Up 1d
```

---

### `rtk kubectl` -- Kubernetes

| Commande | Description | Options |
|----------|-------------|---------|
| `rtk kubectl pods [-n ns] [-A]` | Liste compacte des pods | Namespace ou tous |
| `rtk kubectl services [-n ns] [-A]` | Liste compacte des services | Namespace ou tous |
| `rtk kubectl logs <pod> [-c container]` | Logs dedupliques | Container specifique |

Les sous-commandes non reconnues sont transmises directement (passthrough).

---

## Donnees et reseau

### `rtk json` -- Structure JSON

**Objectif :** Affiche la structure d'un fichier JSON sans les valeurs.

```bash
rtk json <fichier> [--depth N]    # Defaut: profondeur 5
rtk json -                         # Depuis stdin
```

**Economies :** ~60%

**Avant / Apres :**
```
# cat package.json (50 lignes)              # rtk json package.json (10 lignes)
{                                            {
  "name": "my-app",                            name: string
  "version": "1.0.0",                         version: string
  "dependencies": {                            dependencies: { 15 keys }
    "react": "^18.2.0",                        devDependencies: { 8 keys }
    "next": "^14.0.0",                         scripts: { 6 keys }
    ...15 dependances...                     }
  },
  ...
}
```

---

### `rtk env` -- Variables d'environnement

```bash
rtk env                    # Toutes les variables (sensibles masquees)
rtk env -f AWS             # Filtrer par nom
rtk env --show-all         # Inclure les valeurs sensibles
```

Les variables sensibles (tokens, secrets, mots de passe) sont masquees par defaut : `AWS_SECRET_ACCESS_KEY=***`.

---

### `rtk log` -- Logs dedupliques

**Objectif :** Filtre et deduplique la sortie de logs.

```bash
rtk log <fichier>     # Depuis un fichier
rtk log               # Depuis stdin (pipe)
```

Les lignes repetees sont fusionnees : `[ERROR] Connection refused (x42)`.

**Economies :** ~60-80% (selon la repetitivite)

---

### `rtk curl` -- HTTP avec detection JSON

```bash
rtk curl [args...]
```

Auto-detecte les reponses JSON et affiche le schema au lieu du contenu complet.

---

### `rtk wget` -- Telechargement compact

```bash
rtk wget <url> [args...]
rtk wget -O - <url>           # Sortie vers stdout
```

Supprime les barres de progression et le bruit.

---

### `rtk summary` -- Resume heuristique

**Objectif :** Execute une commande et genere un resume heuristique de la sortie.

```bash
rtk summary <commande...>
```

Utile pour les commandes longues dont la sortie n'a pas de filtre dedie.

---

### `rtk proxy` -- Passthrough avec suivi

**Objectif :** Execute une commande **sans filtrage** mais enregistre l'utilisation pour le suivi.

```bash
rtk proxy <commande...>
```

Utile pour le debug : comparer la sortie brute avec la sortie filtree.

---

## Cloud et bases de donnees

### `rtk aws` -- AWS CLI

```bash
rtk aws <service> [args...]
```

Force la sortie JSON et compresse le resultat. Supporte tous les services AWS (sts, s3, ec2, ecs, rds, cloudformation, etc.).

---

### `rtk psql` -- PostgreSQL

```bash
rtk psql [args...]
```

Supprime les bordures de tableaux et compresse la sortie.

---

## Stacked PRs (Graphite)

### `rtk gt` -- Graphite

| Commande | Description |
|----------|-------------|
| `rtk gt log` | Stack log compact |
| `rtk gt submit` | Submit compact |
| `rtk gt sync` | Sync compact |
| `rtk gt restack` | Restack compact |
| `rtk gt create` | Create compact |
| `rtk gt branch` | Branch info compact |

Les sous-commandes non reconnues sont transmises directement ou detectees comme passthrough git.

---

## Analytique et suivi

### Systeme de tracking

RTK enregistre chaque execution de commande dans une base SQLite :

- **Emplacement :** `~/.local/share/rtk/tracking.db` (Linux), `~/Library/Application Support/rtk/tracking.db` (macOS)
- **Retention :** 90 jours automatique
- **Metriques :** tokens entree/sortie, pourcentage d'economies, temps d'execution, projet

---

### `rtk gain` -- Statistiques d'economies

```bash
rtk gain                        # Resume global
rtk gain -p                     # Filtre par projet courant
rtk gain --graph                # Graphe ASCII (30 derniers jours)
rtk gain --history              # Historique recent des commandes
rtk gain --daily                # Ventilation jour par jour
rtk gain --weekly               # Ventilation par semaine
rtk gain --monthly              # Ventilation par mois
rtk gain --all                  # Toutes les ventilations
rtk gain --quota -t pro         # Estimation d'economies sur le quota mensuel
rtk gain --failures             # Log des echecs de parsing (commandes en fallback)
rtk gain --format json          # Export JSON (pour dashboards)
rtk gain --format csv           # Export CSV
```

**Options :**

| Option | Court | Description |
|--------|-------|-------------|
| `--project` | `-p` | Filtrer par repertoire courant |
| `--graph` | `-g` | Graphe ASCII des 30 derniers jours |
| `--history` | `-H` | Historique recent des commandes |
| `--quota` | `-q` | Estimation d'economies sur le quota mensuel |
| `--tier` | `-t` | Tier d'abonnement : `pro`, `5x`, `20x` (defaut: `20x`) |
| `--daily` | `-d` | Ventilation quotidienne |
| `--weekly` | `-w` | Ventilation hebdomadaire |
| `--monthly` | `-m` | Ventilation mensuelle |
| `--all` | `-a` | Toutes les ventilations |
| `--format` | `-f` | Format de sortie : `text`, `json`, `csv` |
| `--failures` | `-F` | Affiche les commandes en fallback |

**Exemple de sortie :**
```
$ rtk gain
RTK Token Savings Summary
  Total commands:     1,247
  Total input:        2,341,000 tokens
  Total output:       468,200 tokens
  Total saved:        1,872,800 tokens (80%)
  Avg per command:    1,501 tokens saved

Top commands:
  git status    312x  -82%
  cargo test    156x  -91%
  git diff       98x  -76%
```

---

### `rtk discover` -- Opportunites manquees

**Objectif :** Analyse l'historique Claude Code pour trouver les commandes qui auraient pu etre optimisees par rtk.

```bash
rtk discover                          # Projet courant, 30 derniers jours
rtk discover --all --since 7          # Tous les projets, 7 derniers jours
rtk discover -p /chemin/projet        # Filtrer par projet
rtk discover --limit 20              # Max commandes par section
rtk discover --format json            # Export JSON
```

**Options :**

| Option | Court | Description |
|--------|-------|-------------|
| `--project` | `-p` | Filtrer par chemin de projet |
| `--limit` | `-l` | Max commandes par section (defaut: 15) |
| `--all` | `-a` | Scanner tous les projets |
| `--since` | `-s` | Derniers N jours (defaut: 30) |
| `--format` | `-f` | Format : `text`, `json` |

---

### `rtk learn` -- Apprendre des erreurs

**Objectif :** Analyse l'historique d'erreurs CLI de Claude Code pour detecter les corrections recurrentes.

```bash
rtk learn                             # Projet courant
rtk learn --all --since 7             # Tous les projets
rtk learn --write-rules               # Generer .claude/rules/cli-corrections.md
rtk learn --min-confidence 0.8        # Seuil de confiance (defaut: 0.6)
rtk learn --min-occurrences 3         # Occurrences minimales (defaut: 1)
rtk learn --format json               # Export JSON
```

---

### `rtk cc-economics` -- Analyse economique Claude Code

**Objectif :** Compare les depenses Claude Code (via ccusage) avec les economies RTK.

```bash
rtk cc-economics                      # Resume
rtk cc-economics --daily              # Ventilation quotidienne
rtk cc-economics --weekly             # Ventilation hebdomadaire
rtk cc-economics --monthly            # Ventilation mensuelle
rtk cc-economics --all                # Toutes les ventilations
rtk cc-economics --format json        # Export JSON
```

---

### `rtk hook-audit` -- Metriques du hook

**Prerequis :** Necessite `RTK_HOOK_AUDIT=1` dans l'environnement.

```bash
rtk hook-audit                        # 7 derniers jours (defaut)
rtk hook-audit --since 30             # 30 derniers jours
rtk hook-audit --since 0              # Tout l'historique
```

---

## Systeme de hooks

### Fonctionnement

Le hook RTK intercepte les commandes Bash dans Claude Code **avant leur execution** et les reecrit automatiquement en equivalent RTK.

**Flux :**
```
Claude Code "git status"
    |
    v
settings.json -> PreToolUse hook
    |
    v
rtk-rewrite.sh (bash)
    |
    v
rtk rewrite "git status"  ->  "rtk git status"
    |
    v
Claude Code execute "rtk git status"
    |
    v
Sortie filtree retournee a Claude (~10 tokens vs ~200)
```

**Points cles :**
- Claude ne voit jamais la recriture -- il recoit simplement une sortie optimisee
- Le hook est un delegateur leger (~50 lignes bash) qui appelle `rtk rewrite`
- Toute la logique de recriture est dans le registre Rust (`src/discover/registry.rs`)
- Les commandes deja prefixees par `rtk` passent sans modification
- Les heredocs (`<<`) ne sont pas modifies
- Les commandes non reconnues passent sans modification

### Installation

```bash
rtk init -g                     # Installation recommandee (hook + RTK.md)
rtk init -g --auto-patch        # Non-interactif (CI/CD)
rtk init -g --hook-only         # Hook seul, sans RTK.md
rtk init --show                 # Verifier l'installation
rtk init -g --uninstall         # Desinstaller
```

### Fichiers installes

| Fichier | Description |
|---------|-------------|
| `~/.claude/hooks/rtk-rewrite.sh` | Script hook (delegue a `rtk rewrite`) |
| `~/.claude/RTK.md` | Instructions minimales pour le LLM |
| `~/.claude/settings.json` | Enregistrement du hook PreToolUse |

### `rtk rewrite` -- Recriture de commande

Commande interne utilisee par le hook. Imprime la commande reecrite sur stdout (exit 0) ou sort avec exit 1 si aucun equivalent RTK n'existe.

```bash
rtk rewrite "git status"           # -> "rtk git status" (exit 0)
rtk rewrite "terraform plan"       # -> (exit 1, pas de recriture)
rtk rewrite "rtk git status"       # -> "rtk git status" (exit 0, inchange)
```

### `rtk verify` -- Verification d'integrite

Verifie l'integrite du hook installe via un controle SHA-256.

```bash
rtk verify
```

### Commandes reecrites automatiquement

| Commande brute | Reecrite en |
|----------------|-------------|
| `git status/diff/log/add/commit/push/pull` | `rtk git ...` |
| `gh pr/issue/run` | `rtk gh ...` |
| `cargo test/build/clippy/check` | `rtk cargo ...` |
| `cat/head/tail <fichier>` | `rtk read <fichier>` |
| `rg/grep <pattern>` | `rtk grep <pattern>` |
| `ls` | `rtk ls` |
| `tree` | `rtk tree` |
| `wc` | `rtk wc` |
| `vitest/jest` | `rtk vitest run` |
| `tsc` | `rtk tsc` |
| `eslint/biome` | `rtk lint` |
| `prettier` | `rtk prettier` |
| `playwright` | `rtk playwright` |
| `prisma` | `rtk prisma` |
| `ruff check/format` | `rtk ruff ...` |
| `pytest` | `rtk pytest` |
| `mypy` | `rtk mypy` |
| `pip list/install` | `rtk pip ...` |
| `go test/build/vet` | `rtk go ...` |
| `golangci-lint` | `rtk golangci-lint` |
| `docker ps/images/logs` | `rtk docker ...` |
| `kubectl get/logs` | `rtk kubectl ...` |
| `curl` | `rtk curl` |
| `pnpm list/outdated` | `rtk pnpm ...` |

### Exclusion de commandes

Pour empecher certaines commandes d'etre reecrites, ajoutez-les dans `config.toml` :

```toml
[hooks]
exclude_commands = ["curl", "playwright"]
```

---

## Configuration

### Fichier de configuration

**Emplacement :** `~/.config/rtk/config.toml` (Linux) ou `~/Library/Application Support/rtk/config.toml` (macOS)

**Commandes :**
```bash
rtk config                # Afficher la configuration actuelle
rtk config --create       # Creer le fichier avec les valeurs par defaut
```

### Structure complete

```toml
[tracking]
enabled = true              # Activer/desactiver le suivi
history_days = 90           # Jours de retention (nettoyage automatique)
database_path = "/custom/path/tracking.db"  # Chemin personnalise (optionnel)

[display]
colors = true               # Sortie coloree
emoji = true                # Utiliser les emojis
max_width = 120             # Largeur maximale de sortie

[filters]
ignore_dirs = [".git", "node_modules", "target", "__pycache__", ".venv", "vendor"]
ignore_files = ["*.lock", "*.min.js", "*.min.css"]

[tee]
enabled = true              # Activer la sauvegarde de sortie brute
mode = "failures"           # "failures" (defaut), "always", ou "never"
max_files = 20              # Rotation : garder les N derniers fichiers
# directory = "/custom/tee/path"  # Chemin personnalise (optionnel)

[telemetry]
enabled = false             # Telemetrie anonyme (1 ping/jour, requiert consentement)
# consent_given = true      # Defini automatiquement par `rtk init` ou `rtk telemetry enable`
# consent_date = "..."      # Date du consentement (RFC 3339)

[hooks]
exclude_commands = []       # Commandes a exclure de la recriture automatique
```

### Variables d'environnement

| Variable | Description |
|----------|-------------|
| `RTK_TEE_DIR` | Surcharge le repertoire tee |
| `RTK_TELEMETRY_DISABLED=1` | Desactiver la telemetrie |
| `RTK_HOOK_AUDIT=1` | Activer l'audit du hook |
| `SKIP_ENV_VALIDATION=1` | Desactiver la validation d'env (Next.js, etc.) |

---

## Systeme Tee

### Recuperation de sortie brute

Quand une commande echoue, RTK sauvegarde automatiquement la sortie brute complete dans un fichier log. Cela permet au LLM de lire la sortie sans re-executer la commande.

**Fonctionnement :**
1. La commande echoue (exit code != 0)
2. RTK sauvegarde la sortie brute dans `~/.local/share/rtk/tee/`
3. Le chemin du fichier est affiche dans la sortie filtree
4. Le LLM peut lire le fichier si besoin de plus de details

**Sortie :**
```
FAILED: 2/15 tests
[full output: ~/.local/share/rtk/tee/1707753600_cargo_test.log]
```

**Configuration :**

| Parametre | Defaut | Description |
|-----------|--------|-------------|
| `tee.enabled` | `true` | Activer/desactiver |
| `tee.mode` | `"failures"` | `"failures"`, `"always"`, `"never"` |
| `tee.max_files` | `20` | Rotation : garder les N derniers |
| Taille min | 500 octets | Les sorties trop courtes ne sont pas sauvegardees |
| Taille max fichier | 1 Mo | Troncature au-dela |

---

## Telemetrie

RTK peut envoyer un ping anonyme une fois par jour (23h d'intervalle) pour des statistiques d'utilisation. La telemetrie est **desactivee par defaut** et requiert un consentement explicite (RGPD Art. 6, 7).

**Donnees envoyees :** hash de device (SHA-256 d'un sel aleatoire), version, OS, architecture, nombre de commandes/24h, top commandes, pourcentage d'economies.

**Responsable du traitement :** `RTK AI Labs`, contact@rtk-ai.app

**Gerer la telemetrie :**
```bash
rtk telemetry status     # Voir l'etat du consentement
rtk telemetry enable     # Donner son consentement (prompt interactif)
rtk telemetry disable    # Retirer son consentement
rtk telemetry forget     # Retirer + supprimer donnees locales + demande d'effacement serveur
```

**Desactiver via variable d'environnement :**
```bash
export RTK_TELEMETRY_DISABLED=1
```

Aucune donnee personnelle, aucun contenu de commande, aucun chemin de fichier n'est transmis. Conservation serveur : 12 mois max. Details : [docs/TELEMETRY.md](../TELEMETRY.md)

---

## Resume des economies par categorie

| Categorie | Commandes | Economies typiques |
|-----------|-----------|-------------------|
| **Fichiers** | ls, tree, read, find, grep, diff | 60-80% |
| **Git** | status, log, diff, show, add, commit, push, pull | 75-92% |
| **GitHub** | pr, issue, run, api | 26-87% |
| **Tests** | cargo test, vitest, playwright, pytest, go test | 90-99% |
| **Build/Lint** | cargo build, tsc, eslint, prettier, next, ruff, clippy | 70-87% |
| **Paquets** | pnpm, npm, pip, deps, prisma | 60-80% |
| **Conteneurs** | docker, kubectl | 70-80% |
| **Donnees** | json, env, log, curl, wget | 60-80% |
| **Analytique** | gain, discover, learn, cc-economics | N/A (meta) |

---

## Nombre total de commandes

RTK supporte **45+ commandes** reparties en 9 categories, avec passthrough automatique pour les sous-commandes non reconnues. Cela en fait un proxy universel : il est toujours sur a utiliser en prefixe.
