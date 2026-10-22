Conventional Commits enforcer for AI-authored PRs.

# Commit message style

Follow Conventional Commits 1.0:

```
<type>(<scope>): <short summary, imperative, lowercase, no period>

<body — wrap at 72 cols, explain WHY the change, not WHAT (the diff
shows what)>

<optional footer: BREAKING CHANGE, Co-Authored-By>
```

Allowed types: feat, fix, perf, refactor, docs, test, chore, ci, build,
revert. Scope is the directory or module touched.

Banned phrasing in the summary line:
- "various changes", "miscellaneous fixes", "WIP", "stuff"
- emoji
- exclamation marks except where the spec mandates `!` for breaking
