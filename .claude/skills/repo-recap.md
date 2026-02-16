---
description: Generate a comprehensive repo recap (PRs, issues, releases) for sharing with team. Pass "en" or "fr" as argument for language (default fr).
---

# Repo Recap

Generate a structured recap of the repository state: open PRs, open issues, recent releases, and executive summary. Output is formatted as Markdown with clickable GitHub links, ready to share.

## Language

- Check the argument passed to this skill
- If `en` or `english` → produce the recap in English
- If `fr`, `french`, or no argument → produce the recap in French (default)

## Preconditions

Before gathering data, verify:

```bash
# Must be inside a git repo
git rev-parse --is-inside-work-tree

# Must have gh CLI authenticated
gh auth status
```

If either fails, stop and tell the user what's missing.

## Steps

### 1. Gather Data

Run these commands in parallel via `gh` CLI:

```bash
# Repo identity (for links)
gh repo view --json nameWithOwner -q .nameWithOwner

# Open PRs with metadata
gh pr list --state open --limit 50 --json number,title,author,createdAt,changedFiles,additions,deletions,reviewDecision,isDraft

# Open issues with metadata
gh issue list --state open --limit 50 --json number,title,author,createdAt,labels,assignees

# Recent releases (for version history)
gh release list --limit 5

# Recently merged PRs (for contributor activity)
gh pr list --state merged --limit 10 --json number,title,author,mergedAt
```

Note: `author` in JSON results is an object `{login: "..."}` — always extract `.author.login` when processing.

### 2. Determine Maintainers

To distinguish "our PRs" from external contributions:

```bash
gh api repos/{owner}/{repo}/collaborators --jq '.[].login'
```

If this fails (permissions), fallback: authors with write/admin access are those who merged PRs recently. When in doubt, ask the user.

### 3. Analyze and Categorize

#### PRs — Categorize into 3 groups:

**Our PRs** (author is a repo collaborator):
- List with PR number (linked), title, size (+additions, files count), status

**External — Reviewable** (manageable size, no major blockers):
- Additions ≤ 1000 AND files ≤ 10
- No merge conflicts, CI not failing
- Include: PR link, author, title, size, review status, recommended action

**External — Problematic** (any of: too large, CI failing, overlapping, merge conflict):
- Additions > 1000 OR files > 10
- OR CI failing (reviewDecision = "CHANGES_REQUESTED" or checks failing)
- OR touches same files as another open PR (= overlap)
- Include: PR link, author, title, size, specific problem, action taken/needed

**Size labels** (use in "Taille" column for quick visual triage):

| Label | Additions |
| ----- | --------- |
| XS | < 50 |
| S | 50-200 |
| M | 200-500 |
| L | 500-1000 |
| XL | > 1000 |

Format: `+{additions}, {files} files ({label})` — e.g., `+245, 2 files (S)`

#### Detect overlaps:
Two PRs overlap if they modify the same files. Use `changedFiles` from the JSON data. If >50% file overlap between 2 PRs, flag both as overlapping and cross-reference them.

#### Flag clusters:
If one author has 3+ open PRs, note it as a "cluster" with suggested review order (smallest first, or by dependency chain).

#### Issues — Categorize by status:
- **In progress**: has an associated open PR (match by PR body containing `fixes #N`, `closes #N`, or same topic)
- **Quick fix**: small scope, actionable (bug reports, small enhancements)
- **Feature request**: larger scope, needs design discussion
- **Covered by PR**: an existing PR addresses this issue (link it)

### 4. Derive Recent Releases

From `gh release list` output, extract version, date, and name. List the 5 most recent.

If no releases found, check merged PRs for release-please pattern (title matching `chore(*): release *`) as fallback.

### 5. Executive Summary

Produce 5-6 bullet points:
- Total open PRs and issues count
- Active contributors (who has the most PRs/issues)
- Main risks (oversized PRs, CI failures, merge conflicts)
- Quick wins (small PRs ready to merge — XS/S size, no blockers)
- Bug fixes needed (hook bugs, regressions)
- Our own PRs status

### 6. Format Output

Structure the full recap as Markdown with:
- `# {Repo Name} — Récap au {date}` as title (FR) or `# {Repo Name} — Recap {date}` (EN)
- Sections separated by `---`
- All PR/issue numbers as clickable links: `[#123](https://github.com/{owner}/{repo}/pull/123)` for PRs, `.../issues/123` for issues
- Tables with Markdown pipe syntax for all listings
- Bold for emphasis on actions and risks
- Cross-references between related PRs and issues (e.g., "Covered by [#131](link)")

**Empty data handling**:
- 0 open PRs → display "Aucune PR ouverte." (FR) or "No open PRs." (EN) instead of empty table
- 0 open issues → display "Aucune issue ouverte." (FR) or "No open issues." (EN)
- 0 releases → display "Aucune release récente." (FR) or "No recent releases." (EN)

### 7. Copy to Clipboard

After displaying the recap, automatically copy it to clipboard:

```bash
cat << 'EOF' | pbcopy
{formatted recap content}
EOF
```

Confirm with: "Copié dans le presse-papier." (FR) or "Copied to clipboard." (EN)

## Output Template (FR)

```markdown
# {Repo Name} — Récap au {date}

## Releases récentes

| Version | Date | Highlights |
| ------- | ---- | ---------- |
| ...     | ...  | ...        |

---

## PRs ouvertes ({count} total)

### Nos PRs

| PR | Titre | Taille | Status |
| -- | ----- | ------ | ------ |

### Contributeurs externes — Reviewables

| PR | Auteur | Titre | Taille | Status | Action |
| -- | ------ | ----- | ------ | ------ | ------ |

### Contributeurs externes — Problématiques

| PR | Auteur | Titre | Taille | Problème | Action |
| -- | ------ | ----- | ------ | -------- | ------ |

---

## Issues ouvertes ({count} total)

| # | Auteur | Sujet | Priorité |
| - | ------ | ----- | -------- |

---

## Résumé exécutif

- **Point 1**: ...
- **Point 2**: ...
```

## Output Template (EN)

Same structure but with English headers:
- "Recent Releases", "Open PRs", "Our PRs", "External — Reviewable", "External — Problematic", "Open Issues", "Executive Summary"
- Action labels: "To review", "Rebase requested", "Split requested", "Trim requested", "CI broken", "Waiting on author", "Feature request", "Quick fix", "Covered by PR"

## Notes

- Always use `gh` CLI (not GitHub API directly, except for collaborators list)
- Derive repo owner/name from `gh repo view`, don't hardcode
- Keep tables compact — truncate long titles if needed (max ~60 chars)
- Cross-reference overlapping PRs/issues whenever possible
- `author` in gh JSON is an object — always use `.author.login`
