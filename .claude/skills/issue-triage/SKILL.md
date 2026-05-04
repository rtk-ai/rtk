---
name: issue-triage
description: >
  Issue triage: audit open issues, categorize, detect duplicates, cross-ref PRs, risk assessment, post comments.
  Args: "all" for deep analysis of all, issue numbers to focus (e.g. "42 57"), "en"/"fr" for language, no arg = audit only in French.
allowed-tools:
  - Bash
  - Read
  - Grep
effort: medium
tags: [triage, issues, github, categorize, duplicates, risk]
---

# Issue Triage

## Quand utiliser

| Skill | Usage | Output |
|-------|-------|--------|
| `/issue-triage` | Trier, analyser, commenter les issues | Tableaux d'action + deep analysis + commentaires postés |
| `/repo-recap` | Récap général pour partager avec l'équipe | Résumé Markdown (PRs + issues + releases) |

**Déclencheurs** :
- Manuellement : `/issue-triage` ou `/issue-triage all` ou `/issue-triage 42 57`
- Proactivement : quand >10 issues ouvertes sans triage, ou issue stale >30j détectée

---

## Langue

- Vérifier l'argument passé au skill
- Si `en` ou `english` → tableaux et résumé en anglais
- Si `fr`, `french`, ou pas d'argument → français (défaut)
- Note : les commentaires GitHub (Phase 3) restent TOUJOURS en anglais (audience internationale)

---

Workflow en 3 phases : audit automatique → deep analysis opt-in → commentaires avec validation obligatoire.

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

# Issues ouvertes avec métadonnées complètes
gh issue list --state open --limit 100 \
  --json number,title,author,createdAt,updatedAt,labels,assignees,body,comments

# PRs ouvertes (pour cross-référence)
gh pr list --state open --limit 50 --json number,title,body

# Issues fermées récemment (pour détection doublons)
gh issue list --state closed --limit 20 \
  --json number,title,labels,closedAt

# Collaborateurs (pour protéger les issues des mainteneurs)
gh api "repos/{owner}/{repo}/collaborators" --jq '.[].login'
```

**Fallback collaborateurs** : si `gh api .../collaborators` échoue (403/404) :
```bash
gh pr list --state merged --limit 10 --json author --jq '.[].author.login' | sort -u
```
Si toujours ambigu, demander à l'utilisateur via `AskUserQuestion`.

**Note** : `author` est un objet `{login: "..."}` — toujours extraire `.author.login`.

### Analyse — 6 dimensions

**1. Catégorisation** (labels existants > inférence titre/body) :
- **Bug** : mots-clés `crash`, `error`, `fail`, `broken`, `regression`, `wrong`, `unexpected`
- **Feature** : `add`, `implement`, `support`, `new`, `feat:`
- **Enhancement** : `improve`, `optimize`, `better`, `enhance`, `refactor`
- **Question/Support** : `how`, `why`, `help`, `unclear`, `docs`, `documentation`
- **Duplicate Candidate** : voir dimension 3 ci-dessous

**2. Cross-ref PRs** :
- Scanner `body` de chaque PR ouverte pour `fixes #N`, `closes #N`, `resolves #N` (case-insensitive, regex)
- Construire un map : `issue_number -> [PR numbers]`
- Une issue liée à une PR mergée → recommander fermeture

**3. Détection doublons** :
- Normaliser les titres : lowercase, strip préfixes (`bug:`, `feat:`, `[bug]`, `[feature]`, etc.)
- **Jaccard sur mots des titres** : si score > 60% entre deux issues → candidat doublon
- **Keywords body overlap** > 50% → renforcement du signal
- Comparer aussi avec issues fermées récentes (20 dernières)
- Un faux positif peut être confirmé/écarté en Phase 2

**4. Classification risque** :
- **Rouge** : mots-clés `CVE`, `vulnerability`, `injection`, `auth bypass`, `security`, `exploit`, `unsafe`, `credentials`, `leak`, `RCE`, `XSS`
- **Jaune** : `breaking change`, `migration`, `deprecation`, `remove API`, `breaking`, `incompatible`
- **Vert** : tout le reste

**5. Staleness** :
- >30j sans activité (updatedAt) → **Stale**
- >90j sans activité → **Very Stale**
- Calculer depuis la date actuelle

**6. Recommandations d'action** :
- `Accept & Prioritize` : issue claire, reproducible, dans scope
- `Label needed` : issue sans label
- `Comment needed` : info manquante, body insuffisant
- `Linked to PR` : une PR ouverte référence cette issue
- `Duplicate candidate` : candidat doublon identifié (préciser avec `#N`)
- `Close candidate` : stale + aucune activité récente, ou hors scope (jamais si auteur est collaborateur)
- `PR merged → close` : PR liée est mergée, issue encore ouverte

### Output — 5 tableaux

```
## Issues ouvertes ({count})

### Critiques (risque rouge)
| # | Titre | Auteur | Âge | Labels | Action |
| - | ----- | ------ | --- | ------ | ------ |

### Liées à une PR
| # | Titre | Auteur | PR(s) liée(s) | Status PR | Action |
| - | ----- | ------ | ------------- | --------- | ------ |

### Actives
| # | Titre | Auteur | Catégorie | Âge | Labels | Action |
| - | ----- | ------ | --------- | --- | ------ | ------ |

### Doublons candidats
| # | Titre | Doublon de | Similarité | Action |
| - | ----- | ---------- | ---------- | ------ |

### Stale
| # | Titre | Auteur | Dernière activité | Action |
| - | ----- | ------ | ----------------- | ------ |

### Résumé
- Total : {N} issues ouvertes
- Critiques : {N} (risque sécurité ou breaking)
- Liées à PR : {N}
- Doublons candidats : {N}
- Stale (>30j) : {N} | Very Stale (>90j) : {N}
- Sans labels : {N}
- Quick wins (à fermer ou labeler rapidement) : {liste}
```

0 issues → afficher `Aucune issue ouverte.` et terminer.

**Note** : `Âge` = jours depuis `createdAt`, format `{N}j`. Si >30j, afficher en **gras**.

### Copie automatique

Après affichage du tableau de triage, copier dans le presse-papier :
```bash
# Cross-platform clipboard
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

## Phase 2 — Deep Analysis (opt-in)

### Sélection des issues

**Si argument passé** :
- `"all"` → toutes les issues ouvertes
- Numéros (`"42 57"`) → uniquement ces issues
- Pas d'argument → proposer via `AskUserQuestion`

**Si pas d'argument**, afficher :

```
question: "Quelles issues voulez-vous analyser en profondeur ?"
header: "Deep Analysis"
multiSelect: true
options:
  - label: "Toutes ({N} issues)"
    description: "Analyse approfondie de toutes les issues avec agents en parallèle"
  - label: "Critiques uniquement"
    description: "Focus sur les {M} issues à risque rouge/jaune"
  - label: "Doublons candidats"
    description: "Confirmer ou écarter les {K} doublons détectés"
  - label: "Stale uniquement"
    description: "Décision close/keep sur les {J} issues stale"
  - label: "Passer"
    description: "Terminer ici — juste l'audit"
```

Si "Passer" → fin du workflow.

### Exécution de l'analyse

Pour chaque issue sélectionnée, lancer un agent via **Task tool en parallèle** :

```
subagent_type: general-purpose
model: sonnet
prompt: |
  Analyze GitHub issue #{num}: "{title}" by @{author}

  **Metadata**: Created {createdAt}, last updated {updatedAt}, labels: {labels}

  **Body**:
  {body}

  **Existing comments** ({comments_count} total, showing last 5):
  {last_5_comments}

  **Context**:
  - Linked PRs: {linked_prs or "none"}
  - Duplicate candidate of: {duplicate_of or "none"}
  - Risk classification: {risk_color}

  Analyze this issue and return a structured report:
  ### Scope Assessment
  What is this issue actually asking for? Is it clearly defined?

  ### Missing Information
  What's needed to act on this? (reproduction steps, version, environment, etc.)

  ### Risk & Impact
  Security risk? Breaking change? Who's affected?

  ### Effort Estimate
  XS (<1h) / S (1-4h) / M (1-2d) / L (3-5d) / XL (>1 week)

  ### Priority
  P0 (critical, act now) / P1 (high, this sprint) / P2 (medium, backlog) / P3 (low, someday)

  ### Recommended Action
  One of: Accept & Prioritize, Request More Info, Mark Duplicate (#N), Close (Stale), Close (Out of Scope), Link to Existing PR

  ### Draft Comment
  Draft a GitHub comment in English using the appropriate template from templates/issue-comment.md.
  Be specific, helpful, and constructive.
```

Si issue a >50 commentaires, résumer les 5 derniers uniquement.

Agréger tous les rapports. Afficher un résumé après toutes les analyses.

---

## Phase 3 — Actions (validation obligatoire)

### Types d'actions possibles

- **Commenter** : `gh issue comment {num} --body-file -`
- **Labeler** : `gh issue edit {num} --add-label "{label}"` (skip si label déjà présent)
- **Fermer** : `gh issue close {num} --reason "not planned"` (jamais sans validation)

### Génération des drafts

Pour chaque issue analysée, générer les actions (commentaire + labels + fermeture si applicable) en utilisant `templates/issue-comment.md`.

**Règles** :
- Langue des commentaires : **anglais** (audience internationale)
- Ton : professionnel, constructif, factuel
- Ne jamais re-labeler une issue qui a déjà ce label
- Ne jamais proposer "close" pour une issue d'un collaborateur
- Toujours afficher le draft AVANT tout `gh issue comment`

### Affichage et validation

**Afficher TOUS les drafts** au format :

```
---
### Draft — Issue #{num}: {title}

**Actions proposées** : {Commentaire | Label: "bug" | Fermeture}

**Commentaire** :
{commentaire complet}

---
```

Puis demander validation via `AskUserQuestion` :

```
question: "Ces actions sont prêtes. Lesquelles voulez-vous exécuter ?"
header: "Exécuter"
multiSelect: true
options:
  - label: "Toutes ({N} actions)"
    description: "Commenter + labeler + fermer selon les drafts"
  - label: "Issue #{x} — {title_truncated}"
    description: "Exécuter uniquement les actions pour cette issue"
  - label: "Aucune"
    description: "Annuler — ne rien faire"
```

(Générer une option par issue + "Toutes" + "Aucune")

### Exécution

Pour chaque action validée, exécuter dans l'ordre : commenter → labeler → fermer.

```bash
# Commenter
gh issue comment {num} --body-file - <<'COMMENT_EOF'
{commentaire}
COMMENT_EOF

# Labeler (si applicable)
gh issue edit {num} --add-label "{label}"

# Fermer (si applicable)
gh issue close {num} --reason "not planned"
```

Confirmer chaque action : `Commentaire posté sur issue #{num}: {title}`

Si "Aucune" → `Aucune action exécutée. Workflow terminé.`

---

## Gestion des cas limites

| Situation | Comportement |
|-----------|--------------|
| 0 issues ouvertes | `Aucune issue ouverte.` + terminer |
| Issue sans body | Catégoriser par titre, recommander `Comment needed` |
| >50 commentaires | Résumer les 5 derniers uniquement |
| Faux positif doublon | Phase 2 confirme/écarte — ne pas agir sur suspicion seule |
| Labels déjà présents | Ne pas re-labeler, signaler "label déjà appliqué" |
| Issue d'un collaborateur | Jamais `close candidate` automatique |
| Rate limit GitHub API | Réduire `--limit`, notifier l'utilisateur |
| PR mergée liée à issue ouverte | Recommander fermeture de l'issue |
| Issue sans activité >90j | Very Stale — proposer fermeture avec message bienveillant |
| Duplicate confirmed in Phase 2 | Poster commentaire + fermer en faveur de l'issue originale |

---

## Notes

- Toujours dériver owner/repo via `gh repo view`, jamais hardcoder
- Utiliser `gh` CLI (pas `curl` GitHub API) sauf pour la liste des collaborateurs
- `updatedAt` peut être null sur certaines issues → traiter comme `createdAt`
- Ne jamais poster ou fermer sans validation explicite de l'utilisateur dans le chat
- Les commentaires draftés doivent être visibles AVANT tout `gh issue comment`
- Similarité Jaccard = |intersection mots| / |union mots| (exclure stop words : a, the, is, in, of, for, to, with, on, at, by)
