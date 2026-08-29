---
title: Resume
description: Persist and retrieve compact repository execution context for a new agent session
---

# Resume

`rtk resume` replaces repeated repository-orientation commands with one compact record. It reports the current worktree, branch, merge-base, cleanliness, and HEAD, alongside optional execution fields: active plan, completed steps, blockers, last reviewed commit, and next action.

It is read-only by default. RTK stores saved fields outside the repository in its normal local data directory, keyed by the canonical repository path, so it never creates or changes project files.

```bash
rtk resume
rtk resume --format json
rtk resume --save --plan docs/plans/issue-42.md \
  --completed "unit tests pass" \
  --blocker "awaiting security review" \
  --last-reviewed abc1234 \
  --next "open a draft PR"
```

Repeat `--completed` or `--blocker` to replace the corresponding list. Supplying context fields without `--save` fails rather than silently changing state.
