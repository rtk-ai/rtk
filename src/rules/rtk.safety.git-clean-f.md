---
name: git-clean-f
patterns: ["git clean -f"]
action: rewrite
redirect: "git stash -u -m 'RTK: clean backup' && git clean -f {args}"
env_var: RTK_SAFE_COMMANDS
---

Safety: Stashing untracked before clean.
