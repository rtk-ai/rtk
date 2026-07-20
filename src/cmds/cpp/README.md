# C++ Ecosystem (cmds/cpp/)

Command filters for build toolchains — cmake configure and ninja build.

## Files

| File | Type | Description |
|------|------|-------------|
| `cmake_cmd.rs` | Buffered (`run_filtered_with_exit`) | Filters cmake configure output: strips compiler probes, `Performing Test` lines, `-- Configuring done`; keeps errors, warnings, missing deps, user cache vars. |
| `ninja_cmd.rs` | Streaming (`BlockStreamFilter`) | Filters ninja build output: strips `[N/M]` progress lines, keeps `FAILED:` blocks verbatim. Supports GCC/Clang (`file:line:col:`) and MSVC (`file(line):`) diagnostic formats. |
| `xmake_cmd.rs` | Buffered (`run_filtered_with_exit`) | Filters xmake build/configure output: strips full compiler/linker command lines (~500-4000 chars each), progress lines, probe noise; counts compiles/archives/links per target; keeps errors, warnings, notes verbatim. Cross-platform: MSVC, GCC, Clang. |

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

### xmake (Buffered)
- **Input**: stdout + stderr merged
- **Output**: `ok xmake: build (mode, platform, compiler)` with target counts, or `xmake: build failed` with error lines
- **Drops**: Full compiler/linker command lines (structural detection via length > 200 + tool patterns + flag patterns), `checking for ...` probe lines, `[N%]: ...` progress lines, `generating.unityfile`, ANSI codes, blank lines
- **Keeps**: `error:`, `warning:`, `note:` diagnostic lines verbatim (MSVC `file(line): error CXXXX:` and GCC/Clang `file:line:col: error:`), section headers, exit codes
- **Platform extraction**: `checking for platform ... <os> (<arch>)` → summary line
- **Compiler detection**: MSVC / GCC / Clang from probe lines
- **Warning dedup**: Grouped by flag (`CXXXX` for MSVC, `-Wflag` for GCC/Clang)
- **Token savings**: 92-98% on build logs with full compiler command lines
