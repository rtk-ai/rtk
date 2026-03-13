---
model: sonnet
description: RTK Codebase Health Audit — 7 catégories scorées 0-10
argument-hint: "[--category <cat>] [--fix] [--json]"
allowed-tools: [Read, Grep, Glob, Bash, Write]
---

# Audit Codebase — Santé du Projet RTK

Score global et par catégorie (0-10) avec plan d'action priorisé.

## Arguments

- `--category <cat>` — Auditer une seule catégorie : `secrets`, `security`, `deps`, `structure`, `tests`, `perf`, `ai`
- `--fix` — Après l'audit, proposer les fixes prioritaires
- `--json` — Output JSON pour CI/CD

## Usage

```bash
/tech:audit-codebase
/tech:audit-codebase --category security
/tech:audit-codebase --fix
/tech:audit-codebase --json
```

Arguments: $ARGUMENTS

## Seuils de Scoring

| Score | Tier      | Status               |
| ----- | --------- | -------------------- |
| 0-4   | 🔴 Tier 1 | Critique             |
| 5-7   | 🟡 Tier 2 | Amélioration requise |
| 8-10  | 🟢 Tier 3 | Production Ready     |

## Phase 1 : Audit Secrets (Poids: 2x)

```bash
# API keys hardcodées
Grep "sk-[a-zA-Z0-9]{20}" src/
Grep "Bearer [a-zA-Z0-9]" src/

# Credentials dans le code
Grep "password\s*=\s*\"" src/
Grep "token\s*=\s*\"[^$]" src/

# .env accidentellement commité
git ls-files | grep "\.env" | grep -v "\.env\.example"

# Chemins absolus hardcodés (home dir, etc.)
Grep "/home/[a-z]" src/
Grep "/Users/[A-Z]" src/
```

| Condition               | Score        |
| ----------------------- | ------------ |
| 0 secrets trouvés       | 10/10        |
| Chemin absolu hardcodé  | -1 par occ.  |
| Credential réel exposé  | 0/10 immédiat|

## Phase 2 : Audit Sécurité (Poids: 2x)

**Objectif** : Pas d'injection shell, pas de panic en prod, error handling complet.

```bash
# unwrap() en production (hors tests)
Grep "\.unwrap()" src/ --glob "*.rs"
# Filtrer les tests : compter ceux hors #[cfg(test)]

# panic! en production
Grep "panic!" src/ --glob "*.rs"

# expect() sans message explicite
Grep '\.expect("")' src/

# format! dans des chemins injection-possibles
Grep "Command::new.*format!" src/

# ? sans .context()
# (approximation - chercher les ? seuls)
Grep "[^;]\?" src/ --glob "*.rs"
```

| Condition                        | Score             |
| -------------------------------- | ----------------- |
| 0 unwrap() hors tests            | 10/10             |
| `unwrap()` en production         | -1.5 par fichier  |
| `panic!` hors tests              | -2 par occurrence |
| `?` sans `.context()`            | -0.5 par 10 occ.  |
| Injection shell potentielle      | -3 par occurrence |

## Phase 3 : Audit Dépendances (Poids: 1x)

```bash
# Vulnérabilités connues
cargo audit 2>&1 | tail -30

# Dépendances outdated
cargo outdated 2>&1 | head -30

# Dépendances async (interdit dans RTK)
Grep "tokio\|async-std\|futures" Cargo.toml

# Taille binaire post-strip
ls -lh target/release/rtk 2>/dev/null || echo "Build needed"
```

| Condition                        | Score         |
| -------------------------------- | ------------- |
| 0 CVE high/critical              | 10/10         |
| 1 CVE moderate                   | -1 par CVE    |
| 1+ CVE high                      | -2 par CVE    |
| 1+ CVE critical                  | 0/10 immédiat |
| Dépendance async présente        | -3 (perf killer) |
| Binaire >5MB stripped            | -1            |

## Phase 4 : Audit Structure (Poids: 1.5x)

**Objectif** : Architecture RTK respectée, conventions Rust appliquées.

```bash
# Regex non-lazy (compilées à chaque appel)
Grep "Regex::new" src/ --glob "*.rs"
# Compter celles hors lazy_static!

# Modules sans fallback vers commande brute
Grep "execute_raw\|passthrough\|raw_cmd" src/ --glob "*.rs"

# Modules sans module de tests intégré
Grep "#\[cfg(test)\]" src/ --glob "*.rs" --output_mode files_with_matches

# Fichiers source sans tests correspondants
Glob src/*_cmd.rs

# main.rs : vérifier que tous les modules sont enregistrés
Grep "mod " src/main.rs
```

| Condition                              | Score               |
| -------------------------------------- | ------------------- |
| 0 regex non-lazy                       | 10/10               |
| Regex dans fonction (pas lazy_static)  | -2 par occurrence   |
| Module sans fallback brute             | -1.5 par module     |
| Module sans #[cfg(test)]               | -1 par module       |

## Phase 5 : Audit Tests (Poids: 2x)

**Objectif** : Couverture croissante, savings claims vérifiés.

```bash
# Ratio modules avec tests embarqués
MODULES=$(Glob src/*_cmd.rs | wc -l)
TESTED=$(Grep "#\[cfg(test)\]" src/ --glob "*_cmd.rs" --output_mode files_with_matches | wc -l)
echo "Test coverage: $TESTED / $MODULES modules"

# Fixtures réelles présentes
Glob tests/fixtures/*.txt | wc -l

# Tests de token savings (count_tokens assertions)
Grep "count_tokens\|savings" src/ --glob "*.rs" --output_mode count

# Smoke tests OK
ls scripts/test-all.sh 2>/dev/null && echo "Smoke tests present" || echo "Missing"
```

| Coverage %         | Score | Tier |
| ------------------ | ----- | ---- |
| <30% modules       | 3/10  | 🔴 1 |
| 30-49%             | 5/10  | 🟡 2 |
| 50-69%             | 7/10  | 🟡 2 |
| 70-89%             | 8/10  | 🟢 3 |
| 90%+ modules       | 10/10 | 🟢 3 |

**Bonus** : Fixtures réelles pour chaque filtre = +0.5. Smoke tests présents = +0.5.

## Phase 6 : Audit Performance (Poids: 2x)

**Objectif** : Startup <10ms, mémoire <5MB, savings claims tenus.

```bash
# Benchmark startup (si hyperfine dispo)
which hyperfine && hyperfine 'rtk git status' --warmup 3 2>&1 | grep "Time"

# Mémoire binaire
ls -lh target/release/rtk 2>/dev/null

# Dépendances lourdes
Grep "serde_json\|regex\|rusqlite" Cargo.toml
# (ok mais vérifier qu'elles sont nécessaires)

# Regex compilées au runtime
Grep "Regex::new" src/ --glob "*.rs" --output_mode count

# Clone() excessifs (approx)
Grep "\.clone()" src/ --glob "*.rs" --output_mode count
```

| Condition                      | Score          |
| ------------------------------ | -------------- |
| Startup <10ms vérifié          | 10/10          |
| Startup 10-15ms                | 8/10           |
| Startup 15-25ms                | 6/10           |
| Startup >25ms                  | 3/10           |
| Regex runtime (non-lazy)       | -2 par occ.    |
| Dépendance async présente      | -4 (éliminatoire) |

## Phase 7 : Audit AI Patterns (Poids: 1x)

```bash
# Agents définis
ls .claude/agents/ | wc -l

# Commands/skills
ls .claude/commands/tech/ | wc -l

# Règles auto-loaded
ls .claude/rules/ | wc -l

# CLAUDE.md taille (trop gros = trop dense)
wc -l CLAUDE.md

# Filter development checklist présente
Grep "Filter Development Checklist" CLAUDE.md
```

| Condition                        | Score |
| -------------------------------- | ----- |
| >5 agents spécialisés            | +2    |
| >10 commands/skills              | +2    |
| >5 règles auto-loaded            | +2    |
| CLAUDE.md bien structuré         | +2    |
| Smoke tests + CI multi-platform  | +2    |
| Score max                        | 10/10 |

## Phase 8 : Score Global

```
Score global = (
  (secrets × 2) +
  (security × 2) +
  (structure × 1.5) +
  (tests × 2) +
  (perf × 2) +
  (deps × 1) +
  (ai × 1)
) / 11.5
```

## Format de Sortie

```
🔍 Audit RTK — {date}

┌──────────────┬───────┬────────┬──────────────────────────────┐
│ Catégorie    │ Score │ Tier   │ Top issue                    │
├──────────────┼───────┼────────┼──────────────────────────────┤
│ Secrets      │  9.5  │ 🟢 T3  │ 0 issues                     │
│ Sécurité     │  7.0  │ 🟡 T2  │ unwrap() ×8 hors tests       │
│ Structure    │  8.0  │ 🟢 T3  │ 2 modules sans fallback      │
│ Tests        │  6.5  │ 🟡 T2  │ 60% modules couverts         │
│ Performance  │  9.0  │ 🟢 T3  │ startup ~6ms ✅              │
│ Dépendances  │  8.0  │ 🟢 T3  │ 3 packages outdated          │
│ AI Patterns  │  8.5  │ 🟢 T3  │ 7 agents, 12 commands        │
└──────────────┴───────┴────────┴──────────────────────────────┘

Score global : 8.1 / 10  [🟢 Tier 3]
```

## Plan d'Action (--fix)

```
📋 Plan de progression vers Tier 3

Priorité 1 — Sécurité (7.0 → 8+) :
  1. Migrer unwrap() restants vers .context()? — ~2h
  2. Ajouter fallback brute aux 2 modules manquants — ~1h

Priorité 2 — Tests (6.5 → 8+) :
  1. Ajouter #[cfg(test)] aux 4 modules non testés — ~4h
  2. Créer fixtures réelles pour les nouveaux filtres — ~2h

Estimé : ~9h de travail
```
