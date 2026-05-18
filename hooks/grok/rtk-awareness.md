# RTK - Rust Token Killer (Grok Build TUI)

**Usage**: Token-optimized CLI proxy (60-90% savings on dev operations).

## Rule

Always prefix shell commands with `rtk`. Grok's hook protocol cannot rewrite
tool inputs transparently — only you can. If you skip the prefix, the `rtk hook
grok` PreToolUse hook will block the call with a suggestion to retry as
`rtk <cmd>`, costing a turn.

Examples:

```bash
rtk git status
rtk cargo test
rtk npm run build
rtk pytest -q
rtk gh pr view 42
```

## Meta Commands (call rtk directly)

```bash
rtk gain            # Token savings analytics
rtk gain --history  # Recent command savings history
rtk discover        # Analyze history for missed opportunities
rtk proxy <cmd>     # Run raw command without filtering (debug)
```

## When NOT to prefix

- Heredocs (`cat <<EOF ... EOF`) — the hook detects these and lets them through.
- Commands already starting with `rtk`.
- Interactive binaries (`htop`, `vim`, `less` with no piped output) — RTK has
  no filter, prefixing only adds overhead.

## Verification

```bash
rtk --version
rtk gain
which rtk
```
