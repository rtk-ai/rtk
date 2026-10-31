---
title: What the Proxy Compresses
description: The pipeline rules the RTK Proxy runs on each request, what ships on, and how to add your own methods on tool results
sidebar:
  order: 2
---

# What the Proxy Compresses

The proxy runs the `proxy.request.*` rules of your pipeline on every request your agent sends, before it is forwarded. Those rules are written in the same `pipeline.toml` as your hook rules — see [Writing a Pipeline](../pipelines/writing-pipelines.md#proxy-parts) for the parts a rule can target.

`proxy.request` rules belong in your **user** pipeline: one daemon serves every project, so it never reads a project's `.rtk/` files (warning W017). A pipeline change takes effect when the daemon restarts.

## Proxy rules must be cache-stable

A block already in the conversation is part of the provider's cached prefix. If RTK treated it differently on one turn than on the previous one, the whole prefix would be re-billed. So a proxy rule may only use **cache-stable** methods and conditions that are identical on every turn (error E010 otherwise). The condition classes are listed in [Conditions and Judges](../pipelines/conditions.md#proxy-conditions).

## What ships on

**Normalisation** (`rtk:default`, rule `normalize`): `ansi`, `progress`, `dedup` on every tool result. It removes bytes no reader ever sees, and each method is a pure function of the block's own content, so the prefix stays byte-identical across turns. It does nothing unless the daemon is running.

**Cache alignment** (`rtk:proxy`, rule `align`): `cache-align` places the cache breakpoint on the stable prefix. It does nothing for a client that already sets its own `cache_control` breakpoints — Claude Code does.

Both are Anthropic-dialect only; see [Provider dialects](./configuration.md#provider-dialects).

## Your own methods on tool results

The daemon evaluates your `proxy.request.tool_result` rules per tool result — rule order and `when` honoured, conditions limited to the cache-safe keys — and runs the selected chain. `web-md`, `md-slim`, `base64-elide`, `lockfile` and your own `[method.*]` chains of them work there.

```toml
[[proxy.request.tool_result]]
name = "slim-web-pages"
when = { tool = "web" }
use  = ["web-md", "md-slim"]
```

Two limits apply:

- **`Read` results are never rewritten** — a later `Edit` needs the bytes on disk.
- **Methods whose only recovery is a recall copy are skipped** (`cap`, `toon`, `minify-json`, `rle`, `grep-group`, `truncate`): the daemon cannot store that copy. Use them under `hook.after_tool` instead. `rtk pipeline validate` reports W016 for a method that will be skipped, and for an engine method placed at a part the daemon does not transform (`tool_use`, `assistant`).

## Large tool results: CCR

Off until a rule names it. `ccr` compresses a tool result that is already in the conversation: the block becomes a head-and-tail preview ending in a marker,

```text
[rtk-proxy: compressed N tokens — call rtk_retrieve("<id>") to get the full content]
```

and an `rtk_retrieve` tool is added to the request. When the model calls it, the daemon answers from its own store inside the same request, so your agent never sees the round trip. The marker is identical on every later turn, so the cached prefix is not disturbed.

```toml
[[proxy.request.tool_result]]
name = "ccr"
when = { tokens_gt = 1500 }
use  = ["ccr"]
```

Name the rule `ccr`: its `tokens_gt` is the floor below which a result is left alone (1500 tokens if the rule has none). `Read` results are never compressed. `RTK_PROXY_CCR=false` keeps the method off even when a rule names it.

**What it stores.** To answer `rtk_retrieve`, the daemon keeps the original text of every result it compressed in `rtk-proxy-memory.db`, next to the usage database and readable by your user only, for 7 days (`icm_ttl_secs`). This is the one case where the proxy keeps request content on disk. Delete the file to wipe it; the daemon recreates it empty.

`retire-pointer`, at `proxy.request.tool_use`, is the companion for a file body echoed back through a `Write` or `Edit` argument: the write already succeeded, so the argument becomes a pointer to the file and the agent's own `Read` is the recovery. Same store, same marker; a rule named `retire-writes` sets its floor (400 tokens if none).
