---
model: sonnet
description: RTK Code Review — Review locale pre-PR avec auto-fix
---

# RTK Code Review

Review locale de la branche courante avant création de PR. Applique les critères de qualité RTK.

**Principe**: Preview local → corriger → puis créer PR propre.

## Usage

```bash
/tech:codereview              # 🔴 + 🟡 uniquement (compact)
/tech:codereview --verbose    # + points positifs + 🟢 détaillées
/tech:codereview main         # Review vs main (défaut: master)
/tech:codereview --staged     # Seulement fichiers staged
/tech:codereview --auto       # Review + fix loop
/tech:codereview --auto --max 5
```

Arguments: $ARGUMENTS

## Étape 1: Récupérer le contexte

```bash
# Parse arguments
VERBOSE=false
AUTO_MODE=false
MAX_ITERATIONS=3
STAGED=false
BASE_BRANCH="master"

set -- "$ARGUMENTS"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --verbose) VERBOSE=true; shift ;;
    --auto) AUTO_MODE=true; shift ;;
    --max) MAX_ITERATIONS="$2"; shift 2 ;;
    --staged) STAGED=true; shift ;;
    *) BASE_BRANCH="$1"; shift ;;
  esac
done

# Fichiers modifiés
git diff "$BASE_BRANCH"...HEAD --name-only

# Diff complet
git diff "$BASE_BRANCH"...HEAD

# Stats
git diff "$BASE_BRANCH"...HEAD --stat
```

## Étape 2: Charger les guides pertinents (CONDITIONNEL)

| Si le diff contient...         | Vérifier                                   |
| ------------------------------ | ------------------------------------------ |
| `src/*.rs`                     | CLAUDE.md sections Error Handling + Tests  |
| `src/filter.rs` ou `*_cmd.rs`  | Filter Development Checklist (CLAUDE.md)   |
| `src/main.rs`                  | Command routing + Commands enum            |
| `src/tracking.rs`              | SQLite patterns + DB path config           |
| `src/config.rs`                | Configuration system + init patterns       |
| `.github/workflows/`           | CI/CD multi-platform build targets         |
| `tests/` ou `fixtures/`        | Testing Strategy (CLAUDE.md)               |
| `Cargo.toml`                   | Dependencies + build optimizations         |

### Règles clés RTK

**Error Handling**:
- `anyhow::Result` pour tout le CLI (jamais `std::io::Result` nu)
- TOUJOURS `.context("description")` avec `?` — jamais `?` seul
- JAMAIS `unwrap()` en production (tests: `expect("raison")`)
- Fallback gracieux : si filter échoue → exécuter la commande brute

**Performance**:
- JAMAIS `Regex::new()` dans une fonction → `lazy_static!` obligatoire
- JAMAIS dépendance async (tokio, async-std) → single-threaded by design
- Startup time cible: <10ms

**Tests**:
- `#[cfg(test)] mod tests` embarqué dans chaque module
- Fixtures réelles dans `tests/fixtures/<cmd>_raw.txt`
- `count_tokens()` pour vérifier savings ≥60%
- `assert_snapshot!` (insta) pour output format

**Module**:
- `lazy_static!` pour regex (compile once, reuse forever)
- `exit_code` propagé (0 = success, non-zero = failure)
- `strip_ansi()` depuis `utils.rs` — pas re-implémenté

**Filtres**:
- Token savings ≥60% obligatoire (release blocker)
- Fallback: si filter échoue → raw command exécutée
- Pas d'output ASCII art, pas de verbose metadata inutile

## Étape 3: Analyser selon critères

### 🔴 MUST FIX (bloquant)

- `unwrap()` en dehors des tests
- `Regex::new()` dans une fonction (pas de lazy_static)
- `?` sans `.context()` — erreur sans description
- Dépendance async ajoutée (tokio, async-std, futures)
- Token savings <60% pour un nouveau filtre
- Pas de fallback vers commande brute sur échec de filtre
- `panic!()` en production (hors tests)
- Exit code non propagé sur commande sous-jacente
- Secret ou credential hardcodé
- **Tests manquants pour NOUVEAU code** :
  - Nouveau `*_cmd.rs` sans `#[cfg(test)] mod tests`
  - Nouveau filtre sans fixture réelle dans `tests/fixtures/`
  - Nouveau filtre sans test de token savings (`count_tokens()`)

### 🟡 SHOULD FIX (important)

- `?` sans `.context()` dans code existant (tolerable si pattern établi)
- Regex non-lazy dans code existant migré vers lazy_static
- Fonction >50 lignes (split recommandé)
- Nesting >3 niveaux (early returns)
- `clone()` inutile (borrow possible)
- Output format inconsistant avec les autres filtres RTK
- Test avec données synthétiques au lieu de vraie fixture
- ANSI codes non strippés dans le filtre
- `println!` en production (debug artifact)
- **Tests manquants pour code legacy modifié** :
  - Fonction existante modifiée sans couverture test
  - Nouveau path de code sans test correspondant

### 🟢 CAN SKIP (suggestions)

- Optimisations non critiques
- Refactoring de style
- Renommage perfectible mais fonctionnel
- Améliorations de documentation mineures

## Étape 4: Générer le rapport

### Format compact (défaut)

```markdown
## 🔍 Review RTK

| 🔴  | 🟡  |
| :-: | :-: |
|  2  |  3  |

**[REQUEST CHANGES]** - unwrap() en production + regex non-lazy

---

### 🔴 Bloquant

• `git_cmd.rs:45` - `unwrap()` → `.context("...")?`

\```rust
// ❌ Avant
let hash = extract_hash(line).unwrap();
// ✅ Après
let hash = extract_hash(line).context("Failed to extract commit hash")?;
\```

• `grep_cmd.rs:12` - `Regex::new()` dans la fonction → `lazy_static!`

\```rust
// ❌ Avant (recompile à chaque appel)
let re = Regex::new(r"pattern").unwrap();
// ✅ Après
lazy_static! { static ref RE: Regex = Regex::new(r"pattern").unwrap(); }
\```

### 🟡 Important

• `filter.rs:78` - Fonction 67 lignes → split en 2
• `ls.rs:34` - clone() inutile, borrow suffit
• `new_cmd.rs` - Pas de fixture réelle dans tests/fixtures/

| Prio | Fichier     | L  | Action            |
| ---- | ----------- | -- | ----------------- |
| 🔴   | git_cmd.rs  | 45 | .context() manque |
| 🔴   | grep_cmd.rs | 12 | lazy_static!       |
| 🟡   | filter.rs   | 78 | split function    |
```

**Mode verbose (--verbose)** — ajoute points positifs + 🟢 détaillées.

## Règles anti-hallucination (CRITIQUE)

**OBLIGATOIRE avant de signaler un problème**:

1. **Vérifier existence** — Ne jamais recommander un pattern sans vérifier sa présence dans le codebase
2. **Lire le fichier COMPLET** — Pas juste le diff, lire le contexte entier
3. **Compter les occurrences** — Pattern existant (>10 occurrences) → "Suggestion", PAS "Bloquant"

```bash
# Vérifier si lazy_static est déjà utilisé dans le module
Grep "lazy_static" src/<module>.rs

# Compter unwrap() (si pattern établi dans tests = ok)
Grep "unwrap()" src/ --output_mode count

# Vérifier si fixture existe
Glob tests/fixtures/<cmd>_raw.txt
```

**NE PAS signaler**:
- `unwrap()` dans `#[cfg(test)] mod tests` → autorisé (avec `expect()` préféré)
- `lazy_static!` avec `unwrap()` pour initialisation → pattern établi RTK
- Variables `_unused` → peut être intentionnel (warn suppression)

## Mode Auto (--auto)

```
/tech:codereview --auto
    │
    ▼
┌─────────────────┐
│  1. Review      │  rapport 🔴🟡🟢
└────────┬────────┘
         │
    🔴 ou 🟡 ?
    ┌────┴────┐
    │ NON    │ OUI
    ▼         ▼
 ✅ DONE   ┌─────────────────┐
           │  2. Corriger    │
           └────────┬────────┘
                    │
                    ▼
           ┌──────────────────────┐
           │  3. Quality gate     │
           │  cargo fmt --all     │
           │  cargo clippy        │
           │  cargo test          │
           └────────┬─────────────┘
                    │
              Loop ←┘ (max N iterations)
```

**Safeguards mode auto**:
- Ne pas modifier : `Cargo.lock`, `.env*`, `*secret*`
- Si >5 fichiers modifiés → demander confirmation
- Quality gate : `cargo fmt --all && cargo clippy --all-targets && cargo test`
- Si quality gate fail → `git reset --hard HEAD` + reporter les erreurs
- Commit atomique par passage : `autofix(codereview): fix unwrap + lazy_static`

## Workflow recommandé

```
1. Développer sur feature branch
2. /tech:codereview → preview problèmes (compact)
3a. Corriger manuellement les 🔴 et 🟡
   OU
3b. /tech:codereview --auto → fix automatique
4. /tech:codereview → vérifier READY
5. gh pr create --base master
```
