## Problem: "Does RTK break Claude's prompt cache?"

### Short Answer

No. RTK filters command output once at execution time. The filtered result is stored in conversation history and never changes between API calls. Prompt caching works on prefix matching, and RTK does not alter the prefix.

### Details

RTK actually improves cache economics: smaller tool results mean cheaper cache writes (1.25x on fewer tokens) and reads (0.1x on fewer tokens).

Run `rtk gain` to see your actual cache metrics — cache writes and reads are tracked alongside standard token savings.

---

## Problem: "rtk gain" command not found

### Symptom

Running `rtk gain` returns `command not found` or an unexpected error about "type kit".

### Cause

Two different packages named `rtk` exist on crates.io:

- **rtk-ai/rtk** (this project): Rust Token Killer
- **reachingforthejack/rtk**: Rust Type Kit (generates Rust types from schemas)

If you installed via `cargo install rtk`, you may have installed the wrong package.

### Fix

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

Or via Homebrew:

```bash
brew install rtk-ai/tap/rtk
```

Verify with `rtk gain` — it should show token savings stats, not a "command not found" error.
