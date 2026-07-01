# Native Ecosystem

> Part of [`src/cmds/`](../README.md) — C/C++ and other native build tooling.

## Specifics

- `cmake_cmd.rs` filters `cmake`/`cmake --build` output: per-translation-unit
  `[ NN%] Building ...` progress and `make[N]` directory chatter are tallied and
  dropped; warnings, errors and the configure verdict are kept verbatim.
- Uses `RunOptions::with_tee("cmake")` so the full raw log is recoverable on failure.
