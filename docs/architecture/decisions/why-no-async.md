---
title: Why No Async
description: ADR — why RTK is single-threaded and does not use tokio or async runtimes
sidebar:
  order: 1
---

# ADR: Why No Async

## Decision

RTK is single-threaded. No `tokio`, `async-std`, or `futures`. All I/O is blocking.

## Context

RTK is a CLI proxy that runs for milliseconds and exits. The typical invocation:

1. Parse CLI arguments (~0.1ms)
2. Spawn one subprocess and capture its output (~2-5ms)
3. Filter the output (~0.1ms)
4. Print and exit

There is no concurrent I/O, no network server, no parallel request handling.

## Consequences

**Why async would hurt:**

- `tokio` adds 5-10ms to startup time from runtime initialization. RTK's target is `<10ms` total. Async would consume half the budget before the first useful line of code.
- The entire value proposition of RTK is zero-overhead transparency. If developers perceive any delay, they disable it.
- One subprocess. One output stream. No concurrency needed.

**Why blocking I/O is correct here:**

- `std::process::Command::output()` captures stdout + stderr in one blocking call. This is exactly what RTK needs.
- No event loop required. No `.await` noise in filter code.
- Binary stays under 5MB. No runtime dependencies.

## Tradeoffs

**What we give up:**
- Hypothetical future parallelism (e.g., running multiple filters in parallel). Not needed today.
- Async ecosystem crates (reqwest, sqlx). RTK uses `rusqlite` (sync) and `ureq` (sync) instead.

**What we gain:**
- `<10ms` startup, always.
- Simple, readable filter code with no `.await` punctuation.
- No runtime initialization path that can fail.

## Rule

If you add a dependency that pulls in `tokio` or any async runtime, the PR will be rejected. Check before adding: `cargo tree | grep tokio`.
