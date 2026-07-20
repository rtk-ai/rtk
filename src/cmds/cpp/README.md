# C++ Ecosystem (cmds/cpp/)

Command filters for build toolchains — cmake configure and ninja build.

## Files

| File | Type | Description |
|------|------|-------------|
| `cmake_cmd.rs` | Buffered (`run_filtered_with_exit`) | Filters cmake configure output: strips compiler probes, `Performing Test` lines, `-- Configuring done`; keeps errors, warnings, missing deps, user cache vars. |
| `ninja_cmd.rs` | Streaming (`BlockStreamFilter`) | Filters ninja build output: strips `[N/M]` progress lines, keeps `FAILED:` blocks verbatim. Supports GCC/Clang (`file:line:col:`) and MSVC (`file(line):`) diagnostic formats. |

## Filtering Strategy

### cmake (Buffered)
- **Input**: stdout + stderr merged (errors go to stderr)
- **Output**: `ok cmake: configured (generator, build/)` or `cmake: configuration failed`
- **Drops**: Compiler identification, ABI detection, `Performing Test`, `Found PkgConfig`, `Configuring done`, blank lines
- **Keeps**: `CMake Error/Warning at` blocks, `-- Could NOT find`, user cache vars (`VAR:TYPE=VALUE`), `Build files written to`
- **Token savings**: 55-97% (depends on probe-to-signal ratio)

### ninja (Streaming)
- **Input**: streaming stdout+stderr (interleaved by `run_streaming`)
- **Output**: `ok ninja: N edges, 0 failed` or `ninja: M/N edges failed` + FAILED blocks
- **Drops**: `[N/M] Building/Linking...` progress lines, `ninja: Entering directory`
- **Keeps**: `FAILED: <target>` + command line + compiler diagnostics
- **Dedup**: Template errors repeated across TUs shown first 3 times, collapsed after
- **Warning grouping**: By `-Wflag` type (e.g., `unused-parameter ×14`)
- **Token savings**: 80-99.7% (depends on error count)
