# rtkx

**Context-compression CLI for the [AXON](https://github.com/sammyjdev/axon) stack — a fork of [rtk](https://github.com/rtk-ai/rtk) (Rust Token Killer).**

[![Release](https://img.shields.io/github/v/release/sammyjdev/rtkx)](https://github.com/sammyjdev/rtkx/releases)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

rtkx filters and compresses command output before it reaches an LLM's context — a single Rust binary, 100+ supported commands, <10ms overhead. It is the compression engine of the **AXON** context stack, extended with two features for agent workflows: a **reversible compression store** and a **deterministic, cache-friendly output mode**.

> **Credit:** rtkx is a fork of **[rtk-ai/rtk](https://github.com/rtk-ai/rtk)** by Patrick Szymkowiak and contributors (Apache-2.0). All of rtk's filtering is preserved; for the full command reference see the [upstream project](https://github.com/rtk-ai/rtk) and [rtk-ai.app](https://www.rtk-ai.app).

## What this fork adds

### `ccr` — reversible compression

Compress for the window, keep the original a tool-call away. `rtkx ccr store <file>` saves a content-addressed copy of the original (gzip on disk, SHA-256 handle) and prints a 16-char handle; `rtkx ccr restore <handle>` prints it back, byte for byte. AXON exposes this over MCP (`restore_context`), so an agent sees compressed context carrying a `[[ccr:<handle>]]` marker and can pull the full original on demand.

```bash
h=$(rtkx ccr store big_log.txt)   # -> 7e3cd69086f9bafc
rtkx ccr restore "$h"             # -> the original, unchanged
```

### `--stable` — cache-prefix alignment

`rtkx --stable <cmd>` (or `RTK_STABLE=1`, or `[cache] stable` in config) rewrites absolute cwd/home paths to `.`/`~`, so identical logical input produces byte-identical output across machines and runs. Stable bytes keep an LLM provider's prompt-cache prefix longer — rtk cuts token *count*, `--stable` cuts token *cost*.

## Install

```bash
# Homebrew
brew tap sammyjdev/rtkx && brew install rtkx

# Or the install script (Linux/macOS)
curl -fsSL https://raw.githubusercontent.com/sammyjdev/rtkx/develop/install.sh | sh

# Or as part of the AXON stack (downloads the prebuilt binary, no Rust toolchain)
axon rtk-install
```

Prebuilt binaries for Linux (x64/arm64), macOS (Intel/Apple Silicon), and Windows — plus `.deb`/`.rpm` — are published on every [release](https://github.com/sammyjdev/rtkx/releases).

## Part of the AXON stack

rtkx is the compression layer of a three-part, self-hosted context stack:

| Layer | Role |
|---|---|
| **[AXON](https://github.com/sammyjdev/axon)** | Cross-agent memory + MCP orchestration (the product front) |
| **[GLYPH](https://github.com/sammyjdev/glyph-kg)** | Graph-aware retrieval — decides *what* context to bring |
| **rtkx** (this repo) | Compression + reversible store — decides *how compact* |

## Commands

For the full command set (git, cargo, npm, docker, kubectl, aws, test runners, linters, and more), see the [upstream rtk guide](https://www.rtk-ai.app/guide). The fork keeps all of them; `ccr` and `--stable` are additive.

## License

Apache-2.0 — see [LICENSE](LICENSE). Inherited from upstream rtk.
