---
name: pr-triage
description: >
  PR triage: audit open PRs, deep review selected ones, draft and post review comments.
  Args: "all" to review all, PR numbers to focus (e.g. "42 57"), "en"/"fr" for language,
  "--focus recent" (default, last 60d), "--focus critical" (bugs/security), "--all" (everything).
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

## Modes de filtrage (argument `--focus`)

| Mode | Comportement | Quand utiliser |
|------|-------------|----------------|
| `--focus recent` | PRs updatedAt < 60j (défaut si >200 PRs) | Triage hebdomadaire normal |
| `--focus critical` | CI dirty + CONFLICTING + overlaps détectés | Sprint urgent, avant merge |
| `--focus stale` | Aucune activité >14j | Nettoyage backlog |
| `--all` | Toutes les PRs ouvertes, paginé | Audit exhaustif mensuel |

**Seuil automatique** : si le repo a >200 PRs ouvertes, appliquer `--focus recent` par défaut et prévenir l'utilisateur : "400+ PRs détectées — mode `--focus recent` activé (60 derniers jours). Passer `--all` pour tout voir."

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

### Data Gathering — deux passes

**Passe 1 : métadonnées uniquement (rapide, sans body)**

```bash
# Identité du repo
gh repo view --json nameWithOwner -q .nameWithOwner

# PRs ouvertes — métadonnées sans body (léger)
gh pr list --state open --limit 500 \
  --json number,title,author,createdAt,updatedAt,additions,deletions,changedFiles,isDraft,mergeable,reviewDecision,statusCheckRollup

# Collaborateurs (pour distinguer "nos PRs" des externes)
gh api "repos/{owner}/{repo}/collaborators" --jq '.[].login'
```

**Fallback collaborateurs** : si `gh api .../collaborators` échoue (403/404) :
```bash
gh pr list --state merged --limit 10 --json author --jq '.[].author.login' | sort -u
```
Si toujours ambigu, demander à l'utilisateur via `AskUserQuestion`.

**Passe 2 : bodies + reviews (seulement pour les PRs dans le scope)**

Après application du filtre `--focus`, fetcher uniquement les PRs retenues :

```bash
# Body (pour cross-ref issues fixes #N)
gh pr view {num} --json body --jq '.body'

# Reviews existantes
gh api "repos/{owner}/{repo}/pulls/{num}/reviews" \
  --jq '[.[] | .user.login + ":" + .state] | join(", ")'
```

**Fichiers modifiés (overlap detection — échantillon ciblé)**

Ne fetcher les fichiers QUE pour les PRs répondant à TOUS ces critères :
- `updatedAt` < 30 jours
- additions < 1000 (skip les XL stales)
- Pas en draft depuis >14j

```bash
gh pr view {num} --json files --jq '[.files[].path] | join(",")'
```

**Limite API** : si les PRs dans le scope dépassent 50 après filtrage, désactiver l'overlap detection et signaler : "Overlap detection désactivée (>50 PRs dans le scope). Passer des numéros explicites pour l'activer."

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
- **Overlaps** : comparer les listes de fichiers entre PRs — si >50% de fichiers en commun → cross-reference (seulement sur l'échantillon ciblé, voir ci-dessus)
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

Si le scope dépasse 100 PRs après filtrage, afficher les 50 les plus récentes par catégorie et indiquer "... et N autres (passer `--all` pour voir toutes)".

```
## PRs ouvertes ({total} total, {scope} dans le scope {mode})

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
- Hors scope (filtre actif) : {N PRs non affichées}
```

0 PRs → afficher `Aucune PR ouverte.` et terminer.

### Copie automatique

Après affichage du tableau de triage, copier dans le presse-papier :
```bash
clip() {
  if command -v pbcopy &>/dev/null; then pbcopy
  elif command -v xclip &>/dev/null; then xclip -selection clipboard
  elif command -v wl-copy &>/dev/null; then wl-copy
  else cat
  fi
}

clip <<'EOF'
{tableau de triage complet}
EOF
```
Confirmer : `Tableau copié dans le presse-papier.` (FR) / `Triage table copied to clipboard.` (EN)

---

## Phase 2 — Deep Review (opt-in)

### Sélection des PRs

**Si argument passé** :
- `"all"` → toutes les PRs externes du scope actif (pas les 400 brutes)
- Numéros (`"42 57"`) → uniquement ces PRs
- Pas d'argument → proposer via `AskUserQuestion`

**Si pas d'argument**, afficher :

```
question: "Quelles PRs voulez-vous reviewer en profondeur ?"
header: "Deep Review"
multiSelect: true
options:
  - label: "Toutes les externes ({N} dans le scope)"
    description: "Review avec agents code-reviewer en parallèle — max 15 agents simultanés"
  - label: "Problématiques uniquement"
    description: "Focus sur les {M} PRs à risque (CI dirty, trop large, overlaps)"
  - label: "Prêtes uniquement"
    description: "Review {K} PRs prêtes à merger"
  - label: "Passer"
    description: "Terminer ici — juste l'audit"
```

**Note sur les drafts** :
- Les PRs en draft sont EXCLUES des options "Toutes les externes" et "Prêtes uniquement"
- Les PRs en draft sont INCLUSES dans "Problématiques uniquement"
- Pour reviewer un draft : taper son numéro explicitement (ex: `42`)

**Plafond agents** : lancer au maximum 15 agents en parallèle. Si la sélection dépasse 15 PRs, traiter en batches de 15 et afficher la progression entre chaque batch.

Si "Passer" → fin du workflow.

### Exécution des Reviews

Pour chaque PR sélectionnée, lancer un agent `code-reviewer` via **Task tool en parallèle** (max 15 simultanés) :

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
  - LazyLock<Regex> for fixed patterns reused across calls
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

Confirmer chaque post : `Commentaire posté sur PR #{num}: {title}`

Si "Aucun" → `Aucun commentaire posté. Workflow terminé.`

---

## Gestion des cas limites

| Situation | Comportement |
|-----------|--------------|
| 0 PRs ouvertes | `Aucune PR ouverte.` + terminer |
| >200 PRs ouvertes | Activer `--focus recent` auto, prévenir l'utilisateur |
| PR en draft | Indiquer dans tableau, skip pour review sauf si sélectionnée explicitement |
| CI inconnu | Afficher `?` dans colonne CI |
| Review agent timeout | Afficher erreur partielle, continuer avec les autres |
| `gh pr diff` vide | Skip cette PR, notifier l'utilisateur |
| PR très large (>5000 additions) | Avertir : "Review partielle, diff tronqué" |
| Collaborateurs API 403/404 | Fallback sur auteurs des 10 derniers PRs mergés |
| >50 PRs dans scope pour overlap | Désactiver overlap detection, signaler |
| Sélection deep review >15 PRs | Traiter en batches de 15, afficher progression |

---

## Notes

- Toujours dériver owner/repo via `gh repo view`, jamais hardcoder
- Utiliser `gh` CLI (pas `curl` GitHub API) sauf pour la liste des collaborateurs
- `statusCheckRollup` peut être null → traiter comme `?`
- `mergeable` peut être `MERGEABLE`, `CONFLICTING`, ou `UNKNOWN` → traiter `UNKNOWN` comme `?`
- Ne jamais poster sans validation explicite de l'utilisateur dans le chat
- Les commentaires draftés doivent être visibles AVANT tout `gh pr comment`
- `--limit 500` couvre les backlogs jusqu'à 500 PRs ; au-delà utiliser `gh api` avec pagination curseur
