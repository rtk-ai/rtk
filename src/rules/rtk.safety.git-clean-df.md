---
name: git-clean-df
patterns: ["git clean -df"]
action: rewrite
redirect: "git stash -u -m 'RTK: clean backup' && git clean -df {args}"
env_var: RTK_SAFE_COMMANDS
---

Safety: Stashing untracked before clean.
