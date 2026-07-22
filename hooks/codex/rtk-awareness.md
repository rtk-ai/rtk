# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk`.

Examples:

```bash
rtk git status
rtk cargo test
rtk npm run build
rtk pytest -q
```

## Meta Commands

```bash
rtk gain            # Token savings analytics
rtk gain --history  # Recent command savings history
rtk proxy <cmd>     # Run raw command without filtering
```

## Verification

```bash
rtk --version
rtk gain
which rtk
```

## Reading the full output

RTK output is a filtered summary — for most commands it keeps what matters
(test failures, panics, and build errors are preserved). When you need the
complete, unfiltered output to confirm a claim (tests really passed, build is
really clean) rather than skim it, get the raw output instead of trusting the
summary:

- Run `rtk proxy <cmd>` to re-run the command with no filtering.
- Or, when the filtered output ends with `[full output: <path>]`, read that
  file directly — it is the complete, unfiltered log already on disk.
