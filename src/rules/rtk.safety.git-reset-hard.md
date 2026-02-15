---
name: git-reset-hard
patterns: ["git reset --hard"]
action: rewrite
redirect: "git stash push -m 'RTK: reset backup' && git reset --hard {args}"
when: has_unstaged_changes
env_var: RTK_SAFE_COMMANDS
---

Safety: Stashing before reset.
