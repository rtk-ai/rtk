---
name: git-stash-drop
patterns: ["git stash drop"]
action: rewrite
redirect: "git stash pop"
env_var: RTK_SAFE_COMMANDS
---

Safety: Using pop instead of drop (recoverable).
