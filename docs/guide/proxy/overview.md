---
title: RTK Proxy
description: Run the RTK Proxy daemon (rtk-proxy) and point your AI agent at it so the pipeline also runs on the requests it sends to the model
sidebar:
  order: 1
---

# RTK Proxy

`rtk gain` and the agent hooks work on **bash output** — what a command prints before your agent reads it. The RTK Proxy works one layer up, on the **request your agent sends to the model**: conversation history, tool results, file contents. It is a local HTTP daemon your agent talks to instead of talking to the model provider directly. The daemon binary is `rtk-proxy`; you drive it with `rtk llm-proxy`.

```
  Claude Code ──▶ rtk-proxy ──▶ api.anthropic.com
                     │
                     ├─ normalize: strip ANSI / redraws / dupes
                     ├─ your proxy.request.tool_result rules
                     ├─ (opt-in) cache-align
                     └─ record usage ──▶ rtk gain
```

It is local: one background process on your machine. It does four things — **intercept, compress, forward, record** — and does not store prompt or completion text (the opt-in [CCR](./compression.md#large-tool-results-ccr) keeps the originals it compresses). What it compresses is in [What the Proxy Compresses](./compression.md); its settings and endpoints are in [Proxy Configuration](./configuration.md).

Both layers are optional and independent. You can run the hooks without the proxy, the proxy without the hooks, or both — `rtk gain` reports them side by side.

:::note[Separate download, separate licence]
The daemon is a second binary, `rtk-proxy`, shipped as its own release archive (`rtk-proxy-<target>`). It is **not** inside the `rtk` archive, because it is licensed separately — the `rtk` CLI is Apache-2.0, `rtk-proxy` is BUSL-1.1. See [Editions and licensing](../resources/editions.md).
:::

## Install the daemon

Download the `rtk-proxy-<target>` archive for your platform from the [releases page](https://github.com/rtk-ai/rtk/releases) and put the binary next to `rtk` (anywhere on your `PATH` also works).

From a source checkout:

```bash
cargo build --release --manifest-path proxy/Cargo.toml
```

`rtk` looks for the daemon next to its own binary first, then on `PATH`. If it cannot find it, `rtk llm-proxy` tells you exactly where it looked.

## Quick start

```bash
rtk llm-proxy setup     # check the daemon is present, list agents it can serve
rtk llm-proxy start     # launch it in the background (no-op if already up)
rtk llm-proxy status    # is it running and reachable?
rtk llm-proxy stats     # usage and savings from the running daemon
rtk llm-proxy stop      # ask it to shut down
```

`setup` is the one to run first: it reports which AI agents on this machine can be pointed at the proxy, and how.

You rarely need `start` by hand — `rtk claude` starts the daemon for you. See [Getting your agent onto the proxy](#getting-your-agent-onto-the-proxy).

## Getting your agent onto the proxy

Two independent ways in. `rtk init` configures your agent once and persists it; `rtk <harness>` needs no configuration at all.

### `rtk init -g` — the guided setup

On a Claude Code init, RTK asks:

```
Set up the RTK proxy (routes Claude Code's API traffic through a local daemon)? [y/N]
```

Say yes and it writes `env.ANTHROPIC_BASE_URL` into `settings.json` and adds `extends = "rtk:proxy"` to your pipeline. Then it asks how the daemon should come back after a reboot:

| Choice | What it does |
|---|---|
| **1. SessionStart hook** (default) | Registers a hook that starts the daemon when a Claude session begins |
| **2. Background service** | `rtk llm-proxy service install` — a per-user service, independent of any agent |
| **3. Neither** | Prints the `export` line and leaves `settings.json` untouched |

Declining leaves everything alone: *"Skipped — enable later with `rtk init -g` or `rtk llm-proxy start --env`."*

If you already have the base URL set but nothing restarting the daemon, re-running `rtk init -g` asks only the persistence question.

The question is only asked when the daemon can actually run: the `rtk-proxy` binary is installed (next to `rtk` or on `PATH`) or an rtk-proxy already answers on `127.0.0.1:8080`. Otherwise init says the proxy was not offered and leaves `settings.json` alone. `rtk init -g --uninstall` removes the `env.ANTHROPIC_BASE_URL` this flow wrote, when it still points at the RTK daemon; a base URL aimed anywhere else is kept.

#### What the SessionStart hook actually does

It does **not** do the redirection — that is the `settings.json` entry above. The hook is a *keep-alive*: when a session whose `ANTHROPIC_BASE_URL` points at a loopback address finds nothing listening there, it runs `rtk llm-proxy start`.

It reports through `systemMessage`, which is user-visible and never enters model context — RTK spends no tokens configuring itself. Every failure path stays silent rather than disrupting the session.

This route only exists on harnesses that *have* a SessionStart hook, which is why the next one matters.

### `rtk <harness>` — the launcher

```bash
rtk claude
```

One command, no configuration, nothing persisted: it starts the daemon if it is not already up, points the harness at it, and execs the real binary. On Unix this is a genuine `exec`, so signals, stdio and the exit code behave exactly as if you had launched the harness directly.

**This route needs no hook system.** Not every harness has a SessionStart hook, but most offer a base-URL override — so this is how an agent without hooks still gets onto the proxy.

The daemon is agent-agnostic by design: it speaks the provider's dialect, not the agent's. A subcommand exists only where that redirection has been verified end to end, so `rtk <harness>` resolving is itself the signal that traffic really goes through the daemon. Today the list is:

| Subcommand | Base-URL variable |
|---|---|
| `rtk claude` | `ANTHROPIC_BASE_URL` |

A harness absent from the list is never *silently* unproxied — it falls through to plain passthrough. `rtk llm-proxy setup` reports what can be launched on this machine.

The launcher never blocks you: if the daemon cannot start, the harness launches unproxied with a warning rather than failing.

### `--env` — one shell, by hand

```bash
eval "$(rtk llm-proxy start --env)"
```

Any agent launched from that shell routes through the proxy. This is choice 3 above, done manually.

## Binding to another address

Start it on a different address with `--bind`:

```bash
rtk llm-proxy start --bind 127.0.0.1:9090
```

Every subcommand that talks to a running daemon (`stop`, `status`, `stats`) takes the same `--bind` flag, so you can manage a proxy that is not on the default `127.0.0.1:8080`.

## Runtime settings and background service

Two subcommands forward straight through to the daemon's own CLI:

```bash
rtk llm-proxy config <args...>    # get/set runtime settings on a running proxy
rtk llm-proxy service <args...>   # manage a per-user background service
```

Run either with `--help` to see the daemon's own options; the settings themselves are listed in [Proxy Configuration](./configuration.md). `service` is choice 2 of the [guided setup](#rtk-init--g--the-guided-setup) — a per-user background service that survives logins independently of any agent.

## Where it keeps its state

One fixed location, so the daemon's state does not depend on the shell's working directory when it was started:

| Path | Contents |
|---|---|
| `~/.config/rtk/llm-proxy/` | daemon log and databases — on macOS `~/Library/Application Support/rtk/llm-proxy/`, on Windows `%APPDATA%\rtk\llm-proxy\` |
| `rtk-proxy.log` | daemon log, started fresh on every `start` |
| `rtk-proxy.db` | usage records — the file `rtk gain` reads |
| `rtk-proxy-memory.db` | originals kept by [CCR](./compression.md#large-tool-results-ccr) (opt-in), 7 days |

Because the usage database lives at a known path, `rtk gain` can report proxy savings whether or not the daemon is currently running.

## Seeing the savings

With the proxy running, a bare `rtk gain` switches to a live dashboard covering both layers. See [Live dashboard](../analytics/gain.md#live-dashboard) and [Attribution](../analytics/gain.md#attribution).

```bash
rtk gain              # live dashboard (proxy running, interactive terminal)
rtk gain --text       # force the static view
rtk gain --text --by-method  # which compression method saved what
```
