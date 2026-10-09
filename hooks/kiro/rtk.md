---
inclusion: always
---
# RTK — Rust Token Killer (Kiro)

Prefix shell commands with `rtk` to condense their output.
In chains with `&&`, `||` or `;`, prefix each command.

## Examples

```bash
rtk git status
rtk cargo test
rtk ls src/
rtk grep "pattern" src/
rtk docker ps
rtk gh pr list
```

## Meta commands

```bash
rtk gain            # bash output savings
rtk discover        # missed rtk opportunities
rtk proxy <cmd>     # run without filtering (debug)
```

## Why

Kiro cannot rewrite commands transparently: the RTK hook blocks a raw command
and asks for its `rtk` form. Using `rtk <cmd>` directly avoids that round trip.
