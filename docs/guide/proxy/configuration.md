---
title: Proxy Configuration
description: RTK Proxy settings, supported provider dialects, the HTTP endpoints it answers, and its attribution headers
sidebar:
  order: 3
---

# Proxy Configuration

## Settings

Precedence: **CLI flag > environment variable > config file > default**. Every key mirrors its environment variable one to one.

| Key | Env | Default | What it does |
|---|---|---|---|
| `bind` | `RTK_PROXY_BIND` | `127.0.0.1:8080` | Listen address |
| `ccr` | `RTK_PROXY_CCR` | `true` | Permission only: CCR runs when a rule names it ([Large tool results](./compression.md#large-tool-results-ccr)); `false` keeps it off regardless |
| `anthropic_upstream` | `RTK_PROXY_ANTHROPIC_UPSTREAM` | `https://api.anthropic.com` | Where `/v1/messages` and `/anthropic/…` go |
| `openai_upstream` | `RTK_PROXY_OPENAI_UPSTREAM` | `https://api.openai.com` | Where `/v1/chat/completions`, `/v1/responses` and `/openai/…` go |
| `[upstreams]` table, one `name = "url"` per line | `RTK_PROXY_UPSTREAM_<NAME>` | `mistral = "https://api.mistral.ai"`, `chatgpt = "https://chatgpt.com"` | Named upstreams, each served under `/<name>/…`. A name is lowercase letters, digits, `_` or `-`; `anthropic`, `openai`, `v1`, `health`, `stats` and `admin` are reserved |
| `db` | `RTK_PROXY_DB` | `rtk-proxy.db` | Usage store — the database `rtk gain` reads |
| `relay_borrowing` | `RTK_PROXY_RELAY_BORROWING` | `true` | Lets a pipeline task with no key of its own borrow the credential your agent is already relaying ([Tasks](../pipelines/tasks.md)) |

```bash
rtk llm-proxy config get
```

The upstreams cannot be set over HTTP; `cache_align` and `normalize` are boot-only. A pipeline change takes effect on restart (`rtk llm-proxy stop && rtk llm-proxy start`) — the daemon does not watch the file.

There is no body-logging switch: the daemon does not store prompt or completion text. The one exception is opt-in — CCR keeps the originals it compresses, see [Large tool results](./compression.md#large-tool-results-ccr).

:::caution[Keep it on loopback]
The proxy forwards **your** provider credentials upstream. It is built to be a single-operator local process: `service install` refuses a non-loopback bind outright, and a foreground `start` warns. If you bind it to a network, put a boundary in front of it.
:::

## Provider dialects

| Dialect | Route | Who speaks it | Measured | Compressed |
|---|---|---|---|---|
| Anthropic Messages | `/v1/messages` | Claude Code | yes | yes |
| OpenAI Chat Completions | `/v1/chat/completions` | Mistral Vibe (through `/mistral/…`), OpenCode and others | yes | **no** |
| OpenAI Responses | `/v1/responses`, `/<upstream>/backend-api/codex/responses` | Codex | yes | **no** |

Every shipped transform is Anthropic-only. A client of the other two dialects is forwarded **unchanged** — usage, cache-hit accounting and `rtk gain` still work, compression does not. Mistral's `usage.prompt_tokens_details.cached_tokens` and the Responses API's `input_tokens_details.cached_tokens` are read the same way as the chat dialect, and the model registry prices Mistral families at their 0.1x cache rate.

## Upstream namespaces

A client points its **whole** base URL at the proxy. The first path segment names the upstream; the rest of the path is what the client would have sent to that upstream:

| The agent is pointed at | A request to | Reaches |
|---|---|---|
| `http://127.0.0.1:8080` | `/v1/messages` | `anthropic_upstream` + `/v1/messages` |
| `http://127.0.0.1:8080/openai/v1` | `/openai/v1/responses` | `openai_upstream` + `/v1/responses` |
| `http://127.0.0.1:8080/chatgpt/backend-api/codex` | `/chatgpt/backend-api/codex/responses` | `upstreams.chatgpt` + `/backend-api/codex/responses` |
| `http://127.0.0.1:8080/mistral/v1` | `/mistral/v1/chat/completions` | `upstreams.mistral` + `/v1/chat/completions` |

Inside a known namespace every sibling path (`/mistral/v1/models`, `/chatgpt/backend-api/codex/…`) is relayed verbatim to that one upstream, with no measurement. A credential relayed to one upstream is never lent to another.

## HTTP endpoints

| Route | Purpose |
|---|---|
| `POST /v1/messages` | Anthropic Messages API — intercept and forward |
| `POST /v1/chat/completions` | OpenAI Chat Completions — forward and measure; not compressed |
| `POST /v1/responses` | OpenAI Responses API — forward and measure; not compressed |
| `POST /<upstream>/v1/messages`, `/<upstream>/v1/chat/completions`, `/<upstream>/v1/responses`, `/<upstream>/backend-api/codex/responses` | The same three dialects, bound to a named upstream |
| `GET /health` | Liveness (what `rtk llm-proxy status` polls) |
| `GET /stats/{summary,by-compression}` | Usage reads — aggregate counts only, no request content |
| `GET` / `PUT /admin/settings` | Runtime settings. `PUT` requires the `x-rtk-proxy` header and does **not** accept the upstream URLs |
| `POST /admin/shutdown` | Graceful shutdown (what `stop` calls). Requires the `x-rtk-proxy` header |
| `/v1/messages/*` (e.g. `count_tokens`) | Relayed verbatim to the Anthropic upstream |
| `/<upstream>/*` | Relayed verbatim to that upstream |
| anything else | `404` with a JSON `routing_unknown` error — never relayed |

A client points its **whole** base URL at the proxy, so it also calls sibling endpoints. Paths under `/v1/messages/` go to the Anthropic upstream and paths under a known namespace go to that upstream. Any other path gets a JSON `404` instead of being relayed: guessing an upstream would send your API key to whichever provider was guessed. A WebSocket upgrade is answered the same way, never relayed.

Request bodies are accepted up to **64 MB** — long conversations and big tool results routinely exceed the usual 2 MB default.

There is no PID file and no signal handling: `stop`, `status`, `stats` and `config` are HTTP clients, so they work however the daemon was launched.

## Attribution headers

Both optional, both consumed by the proxy and never forwarded upstream:

| Header | Effect |
|---|---|
| `x-rtk-session` | Tags the usage row with a session. Without it, rows share one `default` session. |
| `x-rtk-project` | Tags the usage row, so savings can be attributed per codebase. |
