---
name: rtk-triage
description: >
  Triage complet RTK : exécute issue-triage + pr-triage en parallèle,
  puis croise les données pour détecter doubles couvertures, trous sécurité,
  P0 sans PR, et conflits internes. Sauvegarde dans claudedocs/RTK-YYYY-MM-DD.md.
  Args: "en"/"fr" pour la langue (défaut: fr), "save" pour forcer la sauvegarde.
allowed-tools:
  - Bash
  - Write
  - Read
  - AskUserQuestion
effort: high
tags: [triage, orchestration, issues, pr, security, cross-analysis, rtk]
---

# /rtk-triage

Orchestrateur de triage RTK. Fusionne issue-triage + pr-triage et produit une analyse croisée.

---

## Quand utiliser

- Hebdomadaire ou avant chaque sprint
- Quand le backlog PR/issues grossit rapidement
- Pour identifier les doublons avant de reviewer

---

## Workflow en 4 phases

### Phase 0 — Préconditions

```bash
git rev-parse --is-inside-work-tree
gh auth status
```

Vérifier que la date actuelle est connue (utiliser `date +%Y-%m-%d`).

---

### Phase 1 — Data gathering (parallèle)

Lancer les deux collectes simultanément :

**Issues** :
```bash
gh repo view --json nameWithOwner -q .nameWithOwner

gh issue list --state open --limit 150 \
  --json number,title,author,createdAt,updatedAt,labels,assignees,body

gh issue list --state closed --limit 20 \
  --json number,title,labels,closedAt

gh api "repos/{owner}/{repo}/collaborators" --jq '.[].login'
```

**PRs** :
```bash
# Fetcher toutes les PRs ouvertes — paginer si nécessaire (gh limite à 200 par appel)
gh pr list --state open --limit 200 \
  --json number,title,author,createdAt,updatedAt,additions,deletions,changedFiles,isDraft,mergeable,reviewDecision,statusCheckRollup,body

# Si le repo a >200 PRs ouvertes, relancer avec --search pour paginer :
# gh pr list --state open --limit 200 --search "is:pr is:open sort:updated-desc" ...

# Pour chaque PR, récupérer les fichiers modifiés (nécessaire pour overlap detection)
# Prioriser les PRs candidates (même domaine, même auteur)
gh pr view {num} --json files --jq '[.files[].path] | join(",")'
```

---

### Phase 2 — Triage individuel

Exécuter les analyses de `/issue-triage` et `/pr-triage` séparément (même logique que les skills individuels) pour produire :

**Issues** :
- Catégorisation (Bug/Feature/Enhancement/Question/Duplicate)
- Risque (Rouge/Jaune/Vert)
- Staleness (>30j)
- Map `issue_number → [PR numbers]` via scan `fixes #N`, `closes #N`, `resolves #N`

**PRs** :
- Taille (XS/S/M/L/XL)
- CI status (clean/dirty)
- Nos PRs vs externes
- Overlaps (>50% fichiers communs entre 2 PRs)
- Clusters (auteur avec 3+ PRs)

Afficher les tableaux standards de chaque skill (voir SKILL.md de issue-triage et pr-triage pour le format exact).

---

### Phase 3 — Analyse croisée (cœur de ce skill)

C'est ici que ce skill apporte de la valeur au-delà des deux skills individuels.

#### 3.1 Double couverture — 2 PRs pour 1 issue

Pour chaque issue liée à ≥2 PRs (via scan des bodies + overlap fichiers) :

| Issue | PR1 (infos) | PR2 (infos) | Verdict recommandé |
|-------|-------------|-------------|-------------------|
| #N (titre) | PR#X — auteur, taille, CI | PR#Y — auteur, taille, CI | Garder la plus ciblée. Fermer/coordonner l'autre |

Règle de verdict :
- Préférer la plus petite (XS < S < M) si même scope
- Préférer CI clean sur CI dirty
- Préférer "nos PRs" si l'une est interne
- Si overlap de fichiers >80% → conflit quasi-certain, signaler

#### 3.2 Trous de couverture sécurité

Pour chaque issue rouge (#640-type security review) :
- Lister les sous-findings mentionnés dans le body
- Croiser avec les PRs existantes (mots-clés dans titre/body)
- Identifier les findings sans PR

Format :
```
## Issue #N — security review (finding par finding)
| Finding | PR associée | Status |
|---------|-------------|--------|
| Description finding 1 | PR#X | En review |
| **Description finding critique** | **AUCUNE** | ⚠️ Trou |
```

#### 3.3 P0/P1 bugs sans PR

Issues labelisées P0 ou P1 (ou mots-clés : "crash", "truncat", "cap", "hardcoded") sans aucune PR liée.

Format :
```
## Bugs critiques sans PR
| Issue | Titre | Pattern commun | Effort estimé |
|-------|-------|----------------|---------------|
```

Chercher un pattern commun (ex: "cap hardcodé", "exit code perdu") — si 3+ bugs partagent un pattern, suggérer un sprint groupé.

#### 3.4 Nos PRs dirty — causes probables

Pour chaque PR interne avec CI dirty ou CONFLICTING :
- Vérifier si un autre PR touche les mêmes fichiers
- Vérifier si un merge récent sur develop peut expliquer le conflit
- Recommander : rebase, fermeture, ou attente

Format :
```
## Nos PRs dirty
| PR | Issue(s) | Cause probable | Action |
|----|----------|----------------|--------|
```

#### 3.5 PRs sans issue trackée

PRs internes sans `fixes #N` dans le body — signaler pour traçabilité.

---

### Phase 4 — Output final

#### Afficher l'analyse croisée complète (sections 3.1 → 3.5)

Puis afficher le résumé chiffré :

```
## Résumé chiffré — YYYY-MM-DD

| Catégorie | Count |
|-----------|-------|
| PRs prêtes à merger (nos) | N |
| Quick wins externes | N |
| Double couverture (conflicts) | N paires |
| P0/P1 bugs sans PR | N |
| Security findings sans PR | N |
| Nos PRs dirty à rebaser | N |
| PRs à fermer (recommandé) | N |
```

#### Sauvegarder dans claudedocs

```bash
date +%Y-%m-%d  # Pour construire le nom de fichier
```

Sauvegarder dans `claudedocs/RTK-YYYY-MM-DD.md` avec :
- Les tableaux de triage issues + PRs (Phase 2)
- L'analyse croisée complète (Phase 3)
- Le résumé chiffré

Confirmer : `Sauvegardé dans claudedocs/RTK-YYYY-MM-DD.md`

---

## Format du fichier sauvegardé

```markdown
# RTK Triage — YYYY-MM-DD

Croisement issues × PRs. {N} PRs ouvertes, {N} issues ouvertes.

---

## 1. Double couverture
...

## 2. Trous sécurité
...

## 3. P0/P1 sans PR
...

## 4. Nos PRs dirty
...

## 5. Nos PRs prêtes à merger
...

## 6. Quick wins externes
...

## 7. Actions prioritaires
(liste ordonnée par impact/urgence)

---

## Résumé chiffré
...
```

---

## Règles

- Langue : argument `en`/`fr`. Défaut : `fr`. Les commentaires GitHub restent toujours en anglais.
- Ne jamais poster de commentaires GitHub sans validation utilisateur (AskUserQuestion).
- Si >200 issues ou >200 PRs : prévenir l'utilisateur et paginer (relancer avec `--search` ou `gh api` avec pagination).
- L'analyse croisée (Phase 3) est toujours exécutée — c'est la valeur ajoutée de ce skill.
- Le fichier claudedocs est sauvegardé automatiquement sauf si l'utilisateur dit "no save".
