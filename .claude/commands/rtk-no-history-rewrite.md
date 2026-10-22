Refuse force-pushes / history rewrites unless the user explicitly asks.

# No history rewrite

Without an explicit user instruction, NEVER:

- run `git push --force` / `--force-with-lease`
- run `git rebase -i` on commits already pushed
- amend a commit that exists on a remote branch
- run `git filter-branch` / `git filter-repo`
- delete a remote branch tip (`git push origin :branch`)

If the user asks for "clean up the history" or similar, propose the
non-destructive alternatives FIRST (squash-on-merge, follow-up commit)
and only proceed with rewrite after explicit confirmation including
the branch name.
