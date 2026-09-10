Defensive idioms when generating Rust code.

# Rust safety net

When writing Rust:

- Avoid `.unwrap()` and `.expect()` outside of tests, build scripts,
  and at-startup code where a panic IS the failure mode. In every
  other path return `Result<T, E>` (custom error or `anyhow::Error`).
- `clone()` is acceptable in glue code; in hot paths, prefer `&str`
  over `String` and slice references over `Vec` clones.
- `unsafe` requires a SAFETY comment immediately above explaining why
  the invariants hold. No `unsafe` for "performance" without a
  benchmark.
- `mem::transmute`, `from_raw_parts` on user-provided lengths, and
  uninitialized memory are forbidden in business code.
- For async: avoid blocking calls (std::fs, std::sync::Mutex held
  across .await) inside an async fn. Use tokio::fs / tokio::sync.
