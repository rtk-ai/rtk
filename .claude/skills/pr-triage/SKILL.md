---
name: pr-triage
description: >
  PR triage: audit open PRs, deep review selected ones, draft and post review comments.
  Args: "all" to review all, PR numbers to focus (e.g. "42 57"), "en"/"fr" for language, no arg = audit only in French.
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
effort: medium
tags: [triage, pr, github, review, code-review, rtk]
---

# PR Triage

## Quand utiliser

| Skill | Usage | Output |
|-------|-------|--------|
| `/pr-triage` | Trier, reviewer, commenter les PRs | Tableau d'action + reviews + commentaires postés |
| `/repo-recap` | Récap général pour partager avec l'équipe | Résumé Markdown (PRs + issues + releases) |

**Déclencheurs** :
- Manuellement : `/pr-triage` ou `/pr-triage all` ou `/pr-triage 42 57`
- Proactivement : quand >5 PRs ouvertes sans review, ou PR stale >14j détectée

---

## Langue

- Vérifier l'argument passé au skill
- Si `en` ou `english` → tableaux et résumé en anglais
- Si `fr`, `french`, ou pas d'argument → français (défaut)
- Note : les commentaires GitHub (Phase 3) restent TOUJOURS en anglais (audience internationale)

---

Workflow en 3 phases : audit automatique → deep review opt-in → commentaires avec validation obligatoire.

## Préconditions

```bash
git rev-parse --is-inside-work-tree
gh auth status
```

Si l'un échoue, stop et expliquer ce qui manque.

---

## Phase 1 — Audit (toujours exécutée)

### Data Gathering (commandes en parallèle)

```bash
# Identité du repo
gh repo view --json nameWithOwner -q .nameWithOwner

# PRs ouvertes avec métadonnées complètes (ajouter body pour cross-référence issues)
gh pr list --state open --limit 50 \
  --json number,title,author,createdAt,updatedAt,additions,deletions,changedFiles,isDraft,mergeable,reviewDecision,statusCheckRollup,body

# Collaborateurs (pour distinguer "nos PRs" des externes)
gh api "repos/{owner}/{repo}/collaborators" --jq '.[].login'
```

**Fallback collaborateurs** : si `gh api .../collaborators` échoue (403/404) :
```bash
# Extraire les auteurs des 10 derniers PRs mergés
gh pr list --state merged --limit 10 --json author --jq '.[].author.login' | sort -u
```
Si toujours ambigu, demander à l'utilisateur via `AskUserQuestion`.

Pour chaque PR, récupérer reviews existantes ET fichiers modifiés :

```bash
gh api "repos/{owner}/{repo}/pulls/{num}/reviews" \
  --jq '[.[] | .user.login + ":" + .state] | join(", ")'

# Fichiers modifiés (nécessaire pour overlap detection)
gh pr view {num} --json files --jq '[.files[].path] | join(",")'
```

**Note rate-limiting** : la récupération des fichiers est N appels API (1 par PR). Pour repos avec 20+ PRs, prioriser les PRs candidates à l'overlap (même domaine fonctionnel, même auteur).

**Note** : `author` est un objet `{login: "..."}` — toujours extraire `.author.login`.

### Analyse

**Classification taille** :
| Label | Additions |
|-------|-----------|
| XS | < 50 |
| S | 50–200 |
| M | 200–500 |
| L | 500–1000 |
| XL | > 1000 |

Format taille : `+{additions}/-{deletions}, {files} files ({label})`

**Détections** :
- **Overlaps** : comparer les listes de fichiers entre PRs — si >50% de fichiers en commun → cross-reference
- **Clusters** : auteur avec 3+ PRs ouvertes → suggérer ordre de review (plus petite en premier)
- **Staleness** : aucune activité depuis >14j → flag "stale"
- **CI status** : via `statusCheckRollup` → `clean` / `unstable` / `dirty`
- **Reviews** : approved / changes_requested / aucune

**Liens PR ↔ Issues** :
- Scanner le `body` de chaque PR pour `fixes #N`, `closes #N`, `resolves #N` (case-insensitive)
- Si trouvé, afficher dans le tableau : `Fixes #42` dans la colonne Action/Status

**Catégorisation** :

_Nos PRs_ : auteur dans la liste des collaborateurs

_Externes — Prêtes_ : additions ≤ 1000 ET files ≤ 10 ET `mergeable` ≠ `CONFLICTING` ET CI clean/unstable

_Externes — Problématiques_ : un des critères suivants :
- additions > 1000 OU files > 10
- OU `mergeable` == `CONFLICTING` (conflit de merge)
- OU CI dirty (statusCheckRollup contient des échecs)
- OU overlap avec une autre PR ouverte (>50% fichiers communs)

### Output — Tableau de triage

```
## PRs ouvertes ({count})

### Nos PRs
| PR | Titre | Taille | CI | Status |
| -- | ----- | ------ | -- | ------ |

### Externes — Prêtes pour review
| PR | Auteur | Titre | Taille | CI | Reviews | Action |
| -- | ------ | ----- | ------ | -- | ------- | ------ |

### Externes — Problématiques
| PR | Auteur | Titre | Taille | Problème | Action recommandée |
| -- | ------ | ----- | ------ | -------- | ------------------ |

### Résumé
- Quick wins : {PRs XS/S prêtes à merger}
- Risques : {overlaps, tailles XL, CI dirty}
- Clusters : {auteurs avec 3+ PRs}
- Stale : {PRs sans activité >14j}
- Overlaps : {PRs qui touchent les mêmes fichiers}
```

0 PRs → afficher `Aucune PR ouverte.` et terminer.

### Copie automatique

Après affichage du tableau de triage, copier dans le presse-papier :
```bash
pbcopy <<'EOF'
{tableau de triage complet}
EOF
```
Confirmer : `Tableau copié dans le presse-papier.` (FR) / `Triage table copied to clipboard.` (EN)

---

## Phase 2 — Deep Review (opt-in)

### Sélection des PRs

**Si argument passé** :
- `"all"` → toutes les PRs externes
- Numéros (`"42 57"`) → uniquement ces PRs
- Pas d'argument → proposer via `AskUserQuestion`

**Si pas d'argument**, afficher :

```
question: "Quelles PRs voulez-vous reviewer en profondeur ?"
header: "Deep Review"
multiSelect: true
options:
  - label: "Toutes les externes"
    description: "Review {N} PRs externes avec agents code-reviewer en parallèle"
  - label: "Problématiques uniquement"
    description: "Focus sur les {M} PRs à risque (CI dirty, trop large, overlaps)"
  - label: "Prêtes uniquement"
    description: "Review {K} PRs prêtes à merger"
  - label: "Passer"
    description: "Terminer ici — juste l'audit"
```

**Note sur les drafts** :
- Les PRs en draft sont EXCLUES des options "Toutes les externes" et "Prêtes uniquement"
- Les PRs en draft sont INCLUSES dans "Problématiques uniquement" (car elles nécessitent attention)
- Pour reviewer un draft : taper son numéro explicitement (ex: `42`)

Si "Passer" → fin du workflow.

### Exécution des Reviews

Pour chaque PR sélectionnée, lancer un agent `code-reviewer` via **Task tool en parallèle** :

```
subagent_type: code-reviewer
model: sonnet
prompt: |
  Review PR #{num}: "{title}" by @{author}

  **Metadata**: +{additions}/-{deletions}, {changedFiles} files ({size_label})
  **CI**: {ci_status} | **Reviews**: {existing_reviews} | **Draft**: {isDraft}

  **PR Body**:
  {body}

  **Diff**:
  {gh pr diff {num} output}

  Apply your security-guardian and backend-architect skills for this review.
  Additionally, apply the RTK-specific checklist:
  - lazy_static! regex (no inline Regex::new())
  - anyhow::Result + .context() (no unwrap())
  - Fallback to raw command on filter failure
  - Exit code propagation
  - Token savings ≥60% in tests with real fixtures
  - No async/tokio dependencies

  Return structured review:
  ### Critical Issues 🔴
  ### Important Issues 🟡
  ### Suggestions 🟢
  ### What's Good ✅

  Be specific: quote the file:line, explain why it's an issue, suggest the fix.
```

Récupérer le diff via :
```bash
gh pr diff {num}
gh pr view {num} --json body,title,author -q '{body: .body, title: .title, author: .author.login}'
```

Agréger tous les rapports. Afficher un résumé après toutes les reviews.

---

## Phase 3 — Commentaires (validation obligatoire)

### Génération des drafts

Pour chaque PR reviewée, générer un commentaire GitHub en utilisant le template `templates/review-comment.md`.

**Règles** :
- Langue : **anglais** (audience internationale)
- Ton : professionnel, constructif, factuel
- Toujours inclure au moins 1 point positif
- Citer les lignes de code quand pertinent (format `file.rs:42`)

### Affichage et validation

**Afficher TOUS les commentaires draftés** au format :

```
---
### Draft — PR #{num}: {title}

{commentaire complet}

---
```

Puis demander validation via `AskUserQuestion` :

```
question: "Ces commentaires sont prêts. Lesquels voulez-vous poster ?"
header: "Poster"
multiSelect: true
options:
  - label: "Tous ({N} commentaires)"
    description: "Poster sur toutes les PRs reviewées"
  - label: "PR #{x} — {title_truncated}"
    description: "Poster uniquement sur cette PR"
  - label: "Aucun"
    description: "Annuler — ne rien poster"
```

(Générer une option par PR + "Tous" + "Aucun")

### Posting

Pour chaque commentaire validé :

```bash
gh pr comment {num} --body-file - <<'REVIEW_EOF'
{commentaire}
REVIEW_EOF
```

Confirmer chaque post : `✅ Commentaire posté sur PR #{num}: {title}`

Si "Aucun" → `Aucun commentaire posté. Workflow terminé.`

---

## Gestion des cas limites

| Situation | Comportement |
|-----------|--------------|
| 0 PRs ouvertes | `Aucune PR ouverte.` + terminer |
| PR en draft | Indiquer dans tableau, skip pour review sauf si sélectionnée explicitement |
| CI inconnu | Afficher `?` dans colonne CI |
| Review agent timeout | Afficher erreur partielle, continuer avec les autres |
| `gh pr diff` vide | Skip cette PR, notifier l'utilisateur |
| PR très large (>5000 additions) | Avertir : "Review partielle, diff tronqué" |
| Collaborateurs API 403/404 | Fallback sur auteurs des 10 derniers PRs mergés |

---

## Notes

- Toujours dériver owner/repo via `gh repo view`, jamais hardcoder
- Utiliser `gh` CLI (pas `curl` GitHub API) sauf pour la liste des collaborateurs
- `statusCheckRollup` peut être null → traiter comme `?`
- `mergeable` peut être `MERGEABLE`, `CONFLICTING`, ou `UNKNOWN` → traiter `UNKNOWN` comme `?`
- Ne jamais poster sans validation explicite de l'utilisateur dans le chat
- Les commentaires draftés doivent être visibles AVANT tout `gh pr comment`
