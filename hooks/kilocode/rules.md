# RTK - Rust Token Killer (Kilo Code)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `rtk` to minimize token consumption.

Examples:

```bash
rtk git status
rtk cargo test
rtk ls src/
rtk grep "pattern" src/
rtk find "*.rs" .
rtk docker ps
rtk gh pr list
```

## Meta Commands

```bash
rtk gain              # Show token savings
rtk gain --history    # Command history with savings
rtk discover          # Find missed RTK opportunities
rtk proxy <cmd>       # Run raw (no filtering, for debugging)
```

## Why

RTK filters and compresses command output before it reaches the LLM context, saving 60-90% tokens on common operations. Always use `rtk <cmd>` instead of raw commands.

## Reading the full output

RTK output is a filtered summary — for most commands it keeps what matters
(test failures, panics, and build errors are preserved). When you need the
complete, unfiltered output to confirm a claim (tests really passed, build is
really clean) rather than skim it, get the raw output instead of trusting the
summary:

- Run `rtk proxy <cmd>` to re-run the command with no filtering.
- Or, when the filtered output ends with `[full output: <path>]`, read that
  file directly — it is the complete, unfiltered log already on disk.
