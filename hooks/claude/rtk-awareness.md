# RTK - Rust Token Killer

**rtk is running.** It transparently rewrites your shell commands to a token-optimized `rtk` equivalent before they execute and compresses their output to save tokens — this is automatic and you don't need to add `rtk` yourself. If a command's output ever looks wrong, truncated, or is missing information you need, re-run it with `rtk proxy <cmd>` to bypass the filtering and get the raw, unfiltered output.
