---
title: How RTK Savings Work
description: What RTK actually reduces, how bash output savings translate into cost, and why the token counts are estimates
sidebar:
  order: 2
---

# How RTK Savings Work

RTK cuts **up to 90% of the bash output** your agent reads. This page explains what that number measures, what it does not measure, and how it reaches your bill.

## What RTK filters

RTK sits between your agent and the CLI. When the agent runs a shell command, RTK executes it, compresses the output, and returns the compressed version.

```
agent runs a shell command
        |
        v
   RTK filters the output
        |
        v
  agent reads the result
```

The only thing RTK changes is **the bytes a shell command sends back**. Everything RTK reports as "savings" is measured on those bytes.

## The savings chain

```
Cost
├─ Input tokens
│  ├─ Bash output           <- the only part RTK filters
│  ├─ Your prompt
│  ├─ System prompt
│  └─ Conversation history
└─ Output tokens            <- what the model writes
```

Those bytes are **one contributor to input tokens**, alongside your prompt, the system prompt, and conversation history. Input tokens are in turn **only part of the bill**, which also counts output tokens.

So the reduction dilutes at every step: a large cut in bash output produces a smaller cut in input tokens, and a smaller one again in cost. A command showing 90% fewer output bytes does not make your session 90% cheaper.

This is why RTK reports bash output reduction rather than a cost figure. Bash output is the part RTK controls; the rest depends on your prompt, your model, how much the agent writes back, and how much of the conversation is replayed on each call.

## Why the token counts are estimates

`rtk gain` estimates tokens as `bytes / 4`:

```rust
// src/core/tracking.rs
pub fn estimate_tokens(text: &str) -> usize {
    // ~4 chars per token on average
    (text.len() as f64 / 4.0).ceil() as usize
}
```

RTK ships **no real tokenizer** by design. Embedding one would cost startup time, and it would require a tokenizer per model, or a per-session model lookup, which RTK does not implement.

The consequence is worth understanding:

- **The percentage is reliable.** The same estimator is applied to the raw output and the filtered output, so the ratio between them holds regardless of the estimator's absolute accuracy.
- **The absolute token counts are approximate.** They will not match your provider's billing. Treat `Input tokens: 45,230` as an order of magnitude, not an invoice line.

If you need exact counts, run the raw and filtered output through your model's own tokenizer.

## How to read `rtk gain`

| Column | What it actually is |
|--------|---------------------|
| Input | Estimated tokens from raw command output, `bytes / 4` |
| Output | Estimated tokens after filtering, `bytes / 4` |
| Saved | Input minus Output, in estimated tokens |
| Save% | Reduction in bash output bytes |

`Save%` is the meaningful number. It is a byte ratio, and it is accurate as a ratio.

## What RTK does not reduce

- **Output tokens.** RTK never touches what the model writes.
- **Your prompt, the system prompt, or conversation history.** These are input tokens RTK has no visibility into.
- **Commands with no matching filter.** These pass through untouched and are tracked at 0% savings. See `rtk gain --history`.

## See also

- [What RTK Optimizes](what-rtk-covers.md) — per-command bash output reduction
- [Token Savings Analytics](../analytics/gain.md) — reading the `rtk gain` dashboard
