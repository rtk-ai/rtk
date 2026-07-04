# OpenCode Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- TypeScript plugin using the zx library (not a shell hook)
- Installed to `~/.config/opencode/plugins/rtk.ts` by `rtk init -g --opencode`
- The file is embedded in the binary (`include_str!` in `src/hooks/init.rs`) and
  written on `init`; edit `hooks/opencode/rtk.ts` in the repo, never the installed copy.

The plugin has two independent responsibilities:

### 1. Command rewriting — `tool.execute.before`

- Intercepts `bash`/`shell` tool calls, runs `rtk rewrite <command>` as a subprocess
- Uses `.quiet().nothrow()` to silently ignore failures (Never Block)
- Mutates `args.command` in-place if the rewrite differs from the original
- All rewrite logic lives in `rtk rewrite` (`src/discover/registry.rs`) — the single
  source of truth. To change what gets rewritten, edit the Rust registry, not this file.

### 2. Output compression — `tool.execute.after`

Compresses heavy tool outputs that never pass through `rtk rewrite` — OpenCode
built-in tools (`read`, `grep`, `glob`, `task`, `webfetch`) and MCP tools
(codegraph, context7, engram, graphify, chrome-devtools, notebooks). These reach
the LLM directly, so the plugin trims them client-side.

- A per-tool registry (`HEAVY_TOOLS`) maps each known tool to a strategy;
  any unlisted tool falls back to `DEFAULT_STRATEGY` (`truncate-middle`).
- Strategies: `truncate-middle` (keep head+tail), `truncate-tail` (keep head),
  `json-compact` (prune deep nesting / long arrays), `rtk-minimal` (strip
  comments, preserving OpenCode's `N:` line prefixes — mirrors `rtk read --level minimal`).
- Two guards keep it conservative: it only compresses when the output exceeds the
  threshold **and** only applies the result when it saves >10% of tokens. This is
  the plugin-side analogue of the `never_worse` guard in the Rust core.
- The compressed output carries a visible `[rtk: compressed …]` header so the LLM
  knows content was trimmed.

#### Sinks

The hook mutates whichever field the tool populated:

| Tool kind    | Field mutated       | Notes                                                        |
|--------------|---------------------|--------------------------------------------------------------|
| Built-in     | `output.output`     | plain string                                                 |
| MCP          | `output.content[]`  | array of `{type, text}`; skipped when it holds non-text parts (images, resource blobs) so attachments are never dropped |

> MCP tools deliver the raw SDK `result` to the hook (since OpenCode commit
> `458ec7b37`); OpenCode rebuilds `output.output` from `content[].text` **after**
> the hook, so replacing `content` is what actually reaches the LLM.

#### Configuration (environment variables)

| Variable                | Default | Purpose                                                    |
|-------------------------|---------|------------------------------------------------------------|
| `RTK_TOKEN_THRESHOLD`   | `3000`  | Approx token count (×4 chars) an output must exceed to compress |
| `RTK_MAX_OUTPUT_CHARS`  | `32000` | Upper bound on post-compression size                        |
| `RTK_DEBUG`             | unset   | Set to `1` to log every tool call (size, sink, threshold, result) to `/tmp/rtk-plugin.log`. Off by default — no I/O on the hot path. |

> Note: OpenCode already truncates `bash` output with its own `Truncate` service
> **before** the hook runs, so shell command output is generally handled upstream;
> the plugin's compression adds value mainly on built-in and MCP tool outputs.
