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
| `anthropic_upstream` | `RTK_PROXY_ANTHROPIC_UPSTREAM` | `https://api.anthropic.com` | Where `/v1/messages` goes |
| `openai_upstream` | `RTK_PROXY_OPENAI_UPSTREAM` | `https://api.openai.com` | Where `/v1/chat/completions` goes |
| `openai_path` | `RTK_PROXY_OPENAI_PATH` | `/v1/chat/completions` | Forward path appended to `openai_upstream`. Override for providers whose OpenAI-compatible endpoint doesn't live under `/v1` (e.g. Z.AI's `/chat/completions`) |
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

| Dialect | Route | Measured | Compressed |
|---|---|---|---|
| Anthropic Messages | `/v1/messages` | yes | yes |
| OpenAI Chat Completions | `/v1/chat/completions` | yes | **no** |

Every shipped transform is Anthropic-only. An OpenAI-dialect client is forwarded **unchanged** — usage, cache-hit accounting and `rtk gain` still work, compression does not.

## HTTP endpoints

| Route | Purpose |
|---|---|
| `POST /v1/messages` | Anthropic Messages API — intercept and forward |
| `POST /v1/chat/completions` | OpenAI Chat Completions — forward and measure; not compressed |
| `GET /health` | Liveness (what `rtk llm-proxy status` polls) |
| `GET /stats/{summary,by-compression}` | Usage reads — aggregate counts only, no request content |
| `GET` / `PUT /admin/settings` | Runtime settings. `PUT` requires the `x-rtk-proxy` header and does **not** accept the upstream URLs |
| `POST /admin/shutdown` | Graceful shutdown (what `stop` calls). Requires the `x-rtk-proxy` header |
| `/v1/messages/*` (e.g. `count_tokens`) | Relayed verbatim to the Anthropic upstream |
| anything else | `404` with a JSON `routing_unknown` error — never relayed |

A client points its **whole** base URL at the proxy, so it also calls sibling endpoints. Paths under `/v1/messages/` go to the one upstream that dialect is bound to. Any other path gets a JSON `404` instead of being relayed: guessing an upstream would send your API key to whichever provider was guessed. A client that needs other endpoints should talk to its provider directly.

Request bodies are accepted up to **64 MB** — long conversations and big tool results routinely exceed the usual 2 MB default.

There is no PID file and no signal handling: `stop`, `status`, `stats` and `config` are HTTP clients, so they work however the daemon was launched.

## Attribution headers

Both optional, both consumed by the proxy and never forwarded upstream:

| Header | Effect |
|---|---|
| `x-rtk-session` | Tags the usage row with a session. Without it, rows share one `default` session. |
| `x-rtk-project` | Tags the usage row, so savings can be attributed per codebase. |
