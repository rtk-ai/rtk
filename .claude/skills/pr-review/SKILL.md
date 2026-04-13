---
description: >
  Batch review des PRs RTK par ordre de complexité croissante (XS → S → M → L).
  Pour chaque PR : vérifie l'état (conflits, CLA, reviews), lit le diff complet,
  analyse le code en contexte, présente un résumé avec lien + taille + recommandation.
  Attend validation explicite avant tout merge. Poste des commentaires boldguy-adapt
  sur les PRs bloquées (conflit, CLA, CHANGES_REQUESTED).
  Args: "triage" pour lancer un triage complet avant la review. "from:<num>" pour
  reprendre à partir d'un numéro de PR spécifique.
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
  - Write
  - AskUserQuestion
---

# /pr-review

Batch review des PRs RTK — du plus simple au plus complexe, une par une, avec validation utilisateur avant chaque merge.

---

## Quand utiliser

- Après un `/rtk-triage` pour agir sur les résultats
- Régulièrement pour dégraisser le backlog
- Avant une release pour vider la file quick wins

---

## Workflow

### Phase 0 — Préconditions

```bash
git rev-parse --is-inside-work-tree
gh auth status
date +%Y-%m-%d
```

Si l'argument `triage` est passé, exécuter `/rtk-triage` d'abord et utiliser sa liste de quick wins comme séquence. Sinon, construire la liste soi-même.

---

### Phase 1 — Construire la liste de PRs (si pas de triage)

```bash
gh pr list --state open --limit 200 \
  --json number,title,author,additions,deletions,changedFiles,mergeable,mergeStateStatus,isDraft,statusCheckRollup,reviewDecision,body \
  | jq 'sort_by(.additions + .deletions)'
```

**Classement par taille** :

| Taille | Critère | Traitement |
|--------|---------|------------|
| XS | < 30 lignes, 1 fichier | En premier |
| S | 30-100 lignes, 1-3 fichiers | Ensuite |
| M | 100-200 lignes, logique non triviale | Après |
| L | > 200 lignes | Dernier ou skip |
| XL | > 500 lignes | Skip (session dédiée) |

**Filtrer d'emblée** :
- Exclure les PRs draft
- Exclure les PRs de nous (les nôtres ont une review flow différente)
- Si `from:<num>` passé en argument : commencer à ce numéro

---

### Phase 2 — Pour chaque PR (une par une, dans l'ordre)

#### Étape A — Vérification état (AVANT de lire le diff)

```bash
# 1. Etat mergeable + CLA
gh pr view <num> --json mergeable,mergeStateStatus,statusCheckRollup,reviewDecision

# 2. Reviews existantes (CHANGES_REQUESTED ?)
gh api repos/rtk-ai/rtk/pulls/<num>/reviews \
  --jq '.[] | {author: .user.login, state: .state, body: .body}'

# 3. Commentaires inline (si CHANGES_REQUESTED)
gh api repos/rtk-ai/rtk/pulls/<num>/comments \
  --jq '.[] | {author: .user.login, body: .body, path: .path, line: .line}'
```

**Décision rapide selon état** :

| État | Action |
|------|--------|
| MERGEABLE + CLA ok + pas de CHANGES_REQUESTED | → lire le diff |
| CONFLICTING | → préparer commentaire rebase, skip diff |
| CLA non signé | → préparer commentaire CLA, skip diff |
| CHANGES_REQUESTED par un maintainer | → skip (ne pas override), noter |
| Draft | → skip silencieusement |

#### Étape B — Lire le diff complet

```bash
gh pr diff <num>
```

Si le diff touche une logique complexe (filter functions, regex, routing) → lire le fichier source en contexte avec `Read` pour comprendre l'impact réel.

#### Étape C — Présenter à l'utilisateur

Format de présentation **obligatoire** pour chaque PR :

```
**PR #<num>** — https://github.com/rtk-ai/rtk/pull/<num>

**Author**: <login> | **Size**: <XS/S/M/L> (+<add> -<del>, <N> fichiers) | **CLA**: <ok/non signé> | **Mergeable**: <clean/conflit>

**Ce que ça fait** — [description en 2-4 phrases : le problème résolu, les fichiers touchés, la logique modifiée, les tests ajoutés]

**Qualité du diff** : [analyse honnête : propre/à vérifier/problème détecté]

Merge #<num> ?
```

**Règles de présentation** :
- Toujours inclure le lien GitHub cliquable
- Toujours mentionner si des tests couvrent le changement
- Si une fonction complexe est touchée, expliquer l'impact
- Ne pas embellir — si le diff est moyen, le dire
- Langue : français pour l'analyse (comme ici)

#### Étape D — Attendre la validation

**NE JAMAIS MERGER SANS RÉPONSE EXPLICITE.** Les réponses attendues :

| Réponse | Action |
|---------|--------|
| "ok" / "go" / "merge" | Merger avec `gh pr merge --merge` |
| "skip" / "next" | Passer à la PR suivante sans merger |
| "comment" | Poster un commentaire (demander le texte si pas fourni) |
| "close" | Fermer la PR |
| Retour avec instructions | Appliquer puis redemander confirmation |

#### Étape E — Merger (si validé)

```bash
gh pr merge <num> --merge --squash
```

Confirmer immédiatement : `Merged #<num>. ✓`

Puis **vérifier que la PR suivante n'est pas passée en CONFLICTING** à cause du merge (surtout si les deux touchent `rules.rs`, `registry.rs`, `main.rs`, ou `CHANGELOG.md`).

---

### Phase 3 — PRs bloquées : commentaire boldguy-adapt

Pour les PRs avec conflit, CLA manquant, ou besoin de rebase, poster un commentaire en anglais, ton boldguy-adapt.

**Règles du commentaire** :
- **Anglais uniquement** (GitHub)
- Remercier la contribution en ouverture (sincèrement, pas de manière générique)
- Dire clairement ce qui bloque (1-2 points max)
- Donner les étapes exactes pour débloquer
- Pas d'em dash (`—`), pas de staccato, longueurs de phrases variées
- Ne pas sonner comme un bot

**Template conflit + CLA** :
```
Hey @<author>, thanks for the contribution! [mention spécifique de ce que la PR apporte]

Two things before we can merge:

1. The branch needs a rebase on `develop` — there's a conflict on [fichier]. A `git rebase origin/develop` should do it.

2. The CLA hasn't been signed yet. The CLAassistant bot left instructions in the PR — just follow the link, takes about a minute.

Once both are sorted, this will move quickly.
```

**Template conflit seul** :
```
Hey @<author>, good fix on [description spécifique]. One thing to address before merge: the branch has a conflict on [fichier] after recent changes to develop. A `git rebase origin/develop` should resolve it cleanly.
```

**Template CLA seul** :
```
Hey @<author>, thanks for [description spécifique]. The only thing blocking merge is the CLA signature — the CLAassistant bot left the link in the PR. Once that's done, we're good to go.
```

---

### Phase 4 — Récap de session

Après avoir traité toutes les PRs (ou à la demande) :

```
## Session recap — YYYY-MM-DD

| PR | Titre | Action | Raison |
|----|-------|--------|--------|
| #N | titre | Mergé ✓ | — |
| #N | titre | Skip | CHANGES_REQUESTED (KuSh) |
| #N | titre | Commenté | Conflit + CLA |
| #N | titre | Fermé | Doublon avec #M |

Mergées : N | Skippées : N | Commentées : N
```

---

## Règles

- **Une PR à la fois** — ne jamais présenter plusieurs PRs en attente de validation
- **Jamais merger sans "ok" explicite** — "ça a l'air bien" n'est pas un ok
- **Ne pas overrider un CHANGES_REQUESTED** d'un maintainer sans instructions explicites de l'utilisateur
- **Vérifier les conflits post-merge** sur la PR suivante si les deux touchent les mêmes fichiers
- **Langue** : analyse en français, commentaires GitHub en anglais
- **Ton boldguy** : factuel, direct, bienveillant, pas de marqueurs AI (em dash, staccato, punchline finale parfaite)

---

## Fichiers fréquemment en conflit (surveiller)

- `CHANGELOG.md` — toutes les PRs y touchent
- `src/discover/rules.rs` — ajouts fréquents de règles
- `src/discover/registry.rs` — tests de classify/rewrite
- `src/main.rs` — routing des commandes
- `src/hooks/rewrite_cmd.rs` — rewrites hooks
