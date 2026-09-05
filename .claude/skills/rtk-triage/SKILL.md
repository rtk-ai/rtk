---
name: rtk-triage
description: >
  Triage complet RTK : exécute issue-triage + pr-triage en parallèle,
  puis croise les données pour détecter doubles couvertures, trous sécurité,
  P0 sans PR, et conflits internes. Sauvegarde dans claudedocs/RTK-YYYY-MM-DD.md.
  Args: "en"/"fr" pour la langue (défaut: fr), "--focus recent/critical/stale/all", "save" pour forcer la sauvegarde.
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

## Modes de filtrage

| Mode | Issues | PRs | Quand utiliser |
|------|--------|-----|----------------|
| `--focus recent` (défaut si >200) | updatedAt < 60j | updatedAt < 60j | Triage hebdomadaire |
| `--focus critical` | risque rouge + jaune | CI dirty + CONFLICTING | Avant sprint urgent |
| `--focus stale` | >30j sans activité | >14j sans activité | Nettoyage backlog |
| `--all` | Toutes, paginé | Toutes, paginé | Audit mensuel exhaustif |

**Seuil automatique** : si >200 issues OU >200 PRs détectées, activer `--focus recent` et prévenir l'utilisateur avant de continuer.

---

## Workflow en 4 phases

### Phase 0 — Préconditions

```bash
git rev-parse --is-inside-work-tree
gh auth status
date +%Y-%m-%d
```

Si >200 issues ou >200 PRs, annoncer : "Repo à fort volume ({N} PRs, {M} issues). Mode `--focus recent` actif. Passer `--all` pour un audit exhaustif (plus lent)."

---

### Phase 1 — Data gathering (parallèle)

Lancer les deux collectes simultanément. Utiliser une approche deux passes (métadonnées d'abord, bodies ensuite sur le scope filtré).

**Issues — Passe 1 (métadonnées)**:
```bash
gh repo view --json nameWithOwner -q .nameWithOwner

gh issue list --state open --limit 500 \
  --json number,title,author,createdAt,updatedAt,labels,assignees

gh issue list --state closed --limit 20 \
  --json number,title,labels,closedAt

gh api "repos/{owner}/{repo}/collaborators" --jq '.[].login'
```

**Issues — Passe 2 (bodies sur le scope filtré uniquement)**:
```bash
# Pour chaque issue dans le scope :
gh issue view {num} --json body --jq '.body'
```

**PRs — Passe 1 (métadonnées)**:
```bash
gh pr list --state open --limit 500 \
  --json number,title,author,createdAt,updatedAt,additions,deletions,changedFiles,isDraft,mergeable,reviewDecision,statusCheckRollup,body
```

**PRs — Passe 2 (fichiers modifiés — échantillon ciblé)**:

Ne fetcher les fichiers QUE pour les PRs répondant à TOUS ces critères :
- `updatedAt` < 30 jours
- additions < 1000
- Pas en draft depuis >14j

```bash
gh pr view {num} --json files --jq '[.files[].path] | join(",")'
```

Si les PRs candidates dépassent 50, désactiver l'overlap detection et signaler.

**Pagination si >400 items** : utiliser `gh api` avec curseur :
```bash
# Issues au-delà de 400
gh api "repos/{owner}/{repo}/issues?state=open&per_page=100&page=2" \
  --jq '[.[] | {number: .number, title: .title, updatedAt: .updated_at}]'
# Répéter page=3, page=4... jusqu'à réponse vide
```

---

### Phase 2 — Triage individuel

Appliquer la logique de `/issue-triage` et `/pr-triage` sur le scope filtré (pas les 400 brutes).

**Issues** :
- Catégorisation (Bug/Feature/Enhancement/Question/Duplicate)
- Risque (Rouge/Jaune/Vert)
- Staleness (>30j / >90j)
- Map `issue_number → [PR numbers]` via scan `fixes #N`, `closes #N`, `resolves #N`
- Détection doublons : pré-filtre n-grams → Jaccard sur candidats seulement (cap 5K comparaisons max)

**PRs** :
- Taille (XS/S/M/L/XL)
- CI status (clean/dirty)
- Nos PRs vs externes
- Overlaps (>50% fichiers communs — sur l'échantillon ciblé uniquement)
- Clusters (auteur avec 3+ PRs)

Afficher les tableaux standards de chaque skill (voir SKILL.md de issue-triage et pr-triage pour le format exact).

Si le scope dépasse 100 items par catégorie, afficher les 50 les plus récents + "... et N autres".

---

### Phase 3 — Analyse croisée (cœur de ce skill)

**Fenêtre d'analyse** : pour limiter la combinatoire, travailler sur les 100 issues et 100 PRs les plus récemment actives du scope. Signaler si des items sont exclus : "Analyse croisée sur les 100 issues × 100 PRs les plus actives. {N} items exclus de la fenêtre."

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

Pour chaque issue rouge (security review) dans la fenêtre :
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
- Vérifier si une autre PR touche les mêmes fichiers (overlap)
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

Scope : {N} PRs, {M} issues ({mode} actif)

| Catégorie | Count |
|-----------|-------|
| PRs prêtes à merger (nos) | N |
| Quick wins externes | N |
| Double couverture (conflicts) | N paires |
| P0/P1 bugs sans PR | N |
| Security findings sans PR | N |
| Nos PRs dirty à rebaser | N |
| PRs à fermer (recommandé) | N |
| Items hors fenêtre d'analyse | N |
```

#### Sauvegarder dans claudedocs

Sauvegarder dans `claudedocs/RTK-YYYY-MM-DD.md` avec :
- Les tableaux de triage issues + PRs (Phase 2)
- L'analyse croisée complète (Phase 3)
- Le résumé chiffré
- Le mode utilisé (`--focus recent` / `--all` / etc.)

Confirmer : `Sauvegardé dans claudedocs/RTK-YYYY-MM-DD.md`

---

## Format du fichier sauvegardé

```markdown
# RTK Triage — YYYY-MM-DD

Scope : {N} PRs, {M} issues. Mode : {--focus recent / --all}.

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
- Ne jamais poster de commentaires GitHub sans validation utilisateur.
- Si >200 issues ou >200 PRs : activer `--focus recent` automatiquement, prévenir l'utilisateur.
- L'analyse croisée (Phase 3) est toujours exécutée sur la fenêtre d'analyse.
- Le fichier claudedocs est sauvegardé automatiquement sauf si l'utilisateur dit "no save".
- `--limit 500` couvre jusqu'à 500 items. Au-delà : pagination `gh api` page par page.
- La fenêtre d'analyse croisée est plafonnée à 100 issues × 100 PRs pour éviter la combinatoire.
