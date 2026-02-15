---
name: git-checkout-dot
patterns: ["git checkout ."]
action: rewrite
redirect: "git stash push -m 'RTK: checkout backup' && git checkout . {args}"
when: has_unstaged_changes
env_var: RTK_SAFE_COMMANDS
---

Safety: Stashing before checkout.
