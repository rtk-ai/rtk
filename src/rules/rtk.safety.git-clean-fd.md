---
name: git-clean-fd
patterns: ["git clean -fd"]
action: rewrite
redirect: "git stash -u -m 'RTK: clean backup' && git clean -fd {args}"
env_var: RTK_SAFE_COMMANDS
---

Safety: Stashing untracked before clean.
