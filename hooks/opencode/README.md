# OpenCode Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- TypeScript plugin using OpenCode's Bun shell API (not a shell hook)
- Installed to `~/.config/opencode/plugins/rtk.ts` by `rtk init -g --opencode`
- The file is embedded in the binary (`include_str!` in `src/hooks/init.rs`) and
  written on `init`; edit `hooks/opencode/rtk.ts` in the repo, never the installed copy.

The plugin has two independent responsibilities:

### 1. Command rewriting — `tool.execute.before`

- Intercepts `bash`/`shell` tool calls, runs `rtk rewrite <command>` as a subprocess
- Uses `.quiet().nothrow()` to silently ignore failures (Never Block)
- Mutates `args.command` in-place if the rewrite differs from the original
- Uses Bun's cross-platform executable lookup at startup; if RTK is unavailable,
  only command rewriting is disabled and output compression remains active
- All rewrite logic lives in `rtk rewrite` (`src/discover/registry.rs`) — the single
  source of truth. To change what gets rewritten, edit the Rust registry, not this file.

### 2. Output compression — `tool.execute.after`

Compresses heavy tool outputs that never pass through `rtk rewrite`. These reach
the LLM directly, so the plugin trims them client-side.

- `HEAVY_TOOLS` holds hand-picked strategies for OpenCode's **built-in** tools
  (`read`, `grep`, `glob`, `task`, `webfetch`) — they ship with every install,
  so tuning them benefits everyone.
- **Every other tool** (MCP servers, third-party plugins, unknown tools) is not
  listed and falls back to `DEFAULT_STRATEGY` (`truncate-middle`), which keeps
  both ends and is safe when the output's shape is unknown. This keeps the
  plugin generic — no assumptions about which MCP servers you happen to run.
- Strategies: `truncate-middle` (keep head+tail), `truncate-tail` (keep head),
  and `rtk-minimal` (strip full-line comments while preserving OpenCode's `N:`
  line prefixes — mirrors `rtk read --level minimal`).
- Two guards keep it conservative: it only compresses when the output exceeds the
  trigger **and** only applies the complete result (header and recovery footer
  included) when it saves >10%. This is the plugin-side analogue of the
  `never_worse` guard in the Rust core.
- The compressed output carries a visible `[rtk: compressed …]` header so the LLM
  knows content was trimmed.
- ANSI terminal control sequences are removed before compression.
- Compression failures are isolated from tool execution; a plugin error leaves
  the successful tool result untouched.

#### Recovery

Compression is reversible:

- If OpenCode already persisted a truncated result, RTK preserves its
  `metadata.outputPath` in the model-visible footer.
- Otherwise, outputs up to 1 MiB are cached in a private temporary directory
  with mode-`0600` files, a 24-hour TTL, and a 16 MiB total cap. When the cap is
  full, new results stay uncompressed instead of evicting referenced entries.
- Cache admission is serialized across OpenCode processes, writes are atomic,
  stale temporary files are cleaned, and symlink entries are never followed.
- The plugin registers `rtk_retrieve`, which can read a bounded `offset`/`limit`
  slice or search the cached original by substring.
- Raw MCP text over 1 MiB is left untouched so OpenCode can apply its own bounded
  preview and durable full-output storage without RTK duplicating the payload in memory.
- Standard tool output over 1 MiB is still bounded because OpenCode has no later
  truncation stage on that path; its footer explicitly says recovery is unavailable.

#### Sinks

The hook mutates whichever field the tool populated:

| Tool kind | Field mutated      | Notes                                                                                                       |
| --------- | ------------------ | ----------------------------------------------------------------------------------------------------------- |
| Built-in  | `output.output`    | plain string                                                                                                |
| MCP       | `output.content[]` | model-visible text and resource text are compressed in place; binary fields and part order remain unchanged |

> MCP tools deliver the raw SDK `result` to the hook (since OpenCode commit
> `458ec7b37`); OpenCode rebuilds `output.output` from text and textual resource
> parts **after** the hook, so updating `content` is what actually reaches the LLM.

#### Configuration (environment variables)

| Variable               | Default | Purpose                                                                                           |
| ---------------------- | ------- | ------------------------------------------------------------------------------------------------- |
| `RTK_TRIGGER_TOKENS`   | `3000`  | Approx token count (×4 chars) an output must exceed to compress                                   |
| `RTK_TARGET_TOKENS`    | `2500`  | Approx budget for the complete compressed result                                                  |
| `RTK_MAX_OUTPUT_CHARS` | `32000` | Hard character ceiling for the complete compressed result                                         |
| `RTK_TOKEN_THRESHOLD`  | `3000`  | Legacy fallback for `RTK_TRIGGER_TOKENS`                                                          |
| `RTK_CACHE_DIR`        | OS temp | Override with a trusted reversible-output cache directory                                         |
| `RTK_DEBUG`            | unset   | Set to `1` to log tool size, strategy, and result to the platform temp directory. Off by default. |

> Note: OpenCode currently truncates normal tool output at 50 KiB or 2,000 lines
> **before** the hook runs. RTK can reduce that preview further while retaining
> OpenCode's full-output path. Raw MCP results reach the hook before that limit.

## Tests

```bash
bun test hooks/opencode/rtk.test.ts
```

The focused suite covers binary-independent compression, Never Block behavior,
final budgets, recovery, mixed MCP content, safe comment stripping, bounded raw
MCP handling, ANSI removal, metadata, and command rewriting.
