---
title: Data Loss and Transformations
description: What RTK removes or rewrites for each supported command
sidebar:
  order: 2
---

# Data Loss and Transformations

RTK compresses **bash output**, not the command's behavior. This page lists what each filter may drop so you can decide when to use `rtk proxy <command>` for the unfiltered output.

The rule is: default output is compressed; an explicit verbose flag (`-v`, `--nocapture`, `--json`, `--jq`, `--template`, `-O`, `-@`, `-e`, …) is respected and usually passes through or skips the rewrite.

| Command | What RTK does | What can be lost |
|---|---|---|
| `rtk ls` | Tree view with file counts | Per-entry columns, permissions, timestamps, symlink targets |
| `rtk read` / `cat` | Signature/structure-oriented reading | Full bodies of large files, blank lines, some comments |
| `rtk grep` / `rg` | Groups matches by file, caps results | Long line tails, matches past the result cap |
| `rtk git status` | Compact grouped status | Untracked-file detail past the cap, some hints |
| `rtk git log` | Hash, author, subject | Full commit bodies, trailers, graph decorations |
| `rtk git diff` / `show` | Reduced context and headers | Some context lines, full hunk bodies past the cap |
| `rtk cargo test` / `npm test` | Failures and counts | Passing test names, per-test timings |
| `rtk pytest` | Failures and traceback excerpts | Passed tests, some captured output |
| `rtk ruff` / `eslint` / `tsc` | Violations grouped by file | Non-violation context, full rule documentation |
| `rtk docker ps` | Essential columns | Labels, mounts, full command lines |
| `rtk kubectl get` | Name, status, restarts | Full resource manifests, events |
| `rtk curl` | Body only, JSON compaction | Headers, some JSON values, responses past the cap |
| `rtk aws` | Condensed JSON fields | Fields outside the selected projection |
| `rtk psql` | Rows without decoration | Table borders, some notices |

## Recovering the full output

```bash
rtk proxy git log -p          # run the real command, no filtering
RTK_DISABLED=1 git log -p     # same, for one command
```

`rtk gain --history` shows the recent filtered commands and their savings. `rtk discover` lists commands RTK did not rewrite.

If a filter drops data you need, prefer the native verbose flag first; if the flag is not respected, report it with the command and the output shape.
