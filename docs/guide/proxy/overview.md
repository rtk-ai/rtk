---
title: RTK Proxy
description: Run the RTK Proxy daemon (rtk-proxy) and point your AI agent at it so the pipeline also runs on the requests it sends to the model
sidebar:
  order: 1
---

# RTK Proxy

`rtk gain` and the agent hooks work on **bash output** — what a command prints before your agent reads it. The RTK Proxy works one layer up, on the **request your agent sends to the model**: conversation history, tool results, file contents. It is a local HTTP daemon your agent talks to instead of talking to the model provider directly. The daemon binary is `rtk-proxy`; you drive it with `rtk llm-proxy`.

```
  your agent ──▶ rtk-proxy ──▶ the provider it was already talking to
                    │            (api.anthropic.com, api.openai.com,
                    │             chatgpt.com, api.mistral.ai, …)
                    ├─ normalize: strip ANSI / redraws / dupes
                    ├─ your proxy.request.tool_result rules
                    ├─ (opt-in) cache-align
                    └─ record usage ──▶ rtk gain
```

The daemon is agent-agnostic: it speaks the provider's dialect, not the agent's. Each upstream has its own namespace under the proxy's address (`/mistral/…`, `/chatgpt/…`, `/openai/…`), so an agent points its base URL at `http://127.0.0.1:8080/<upstream>` and keeps calling the paths it always called. Today Claude Code, Codex and Mistral Vibe are wired end to end; Cursor and Antigravity offer no base-URL override and keep the hooks alone (see [Agents the proxy cannot serve](#agents-the-proxy-cannot-serve)).

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

You rarely need `start` by hand — `rtk claude`, `rtk codex` and `rtk vibe` start the daemon for you. See [Getting your agent onto the proxy](#getting-your-agent-onto-the-proxy).

## Getting your agent onto the proxy

Two independent ways in. `rtk init` configures your agent once and persists it; `rtk <agent>` needs no configuration at all. Each agent has exactly one channel through which it can be pointed at another base URL, and RTK uses that channel and no other:

| Agent | Channel | What RTK sets it to |
|---|---|---|
| Claude Code | `ANTHROPIC_BASE_URL` (env, or `env` in `~/.claude/settings.json`) | `http://127.0.0.1:8080` |
| Codex | `openai_base_url` in `~/.codex/config.toml`, or `-c openai_base_url=…` for one run | `http://127.0.0.1:8080/openai/v1` with an API-key login, `http://127.0.0.1:8080/chatgpt/backend-api/codex` with a ChatGPT login |
| Mistral Vibe | the `[[providers]]` entry named `mistral` in `~/.vibe/config.toml` | `api_base = "http://127.0.0.1:8080/mistral/v1"` |

Codex has no environment variable for its base URL, and Vibe has none for its provider table, so neither can be routed by `--env`; the launcher and the guided setup use the channel each agent actually reads.

### `rtk init -g` — the guided setup

On a global init for Claude Code (`rtk init -g`), Codex (`rtk init -g --codex`) or Mistral Vibe (`rtk init -g --agent vibe`), RTK asks:

```
Set up the RTK proxy (routes Claude Code's API traffic through a local daemon)? [y/N]
```

Say yes and it writes the agent's channel (above) and adds `extends = "rtk:proxy"` to your pipeline. Then it asks how the daemon should come back after a reboot:

| Choice | What it does |
|---|---|
| **1. SessionStart hook** (default; Claude Code and Codex) | Registers a hook that starts the daemon when a session begins |
| **2. Background service** | `rtk llm-proxy service install` — a per-user service, independent of any agent |
| **3. Neither** | Prints what to set by hand and leaves the agent's config untouched |

Mistral Vibe has no session-start hook, so its menu offers the service and the manual line only; its keep-alive is described below.

Declining leaves everything alone: *"Skipped — enable later with `rtk init -g` or `rtk llm-proxy start --env`."*

If you already have the channel set but nothing restarting the daemon, re-running the same init asks only the persistence question.

The question is only asked when the daemon can actually run: the `rtk-proxy` binary is installed (next to `rtk` or on `PATH`) or an rtk-proxy already answers on `127.0.0.1:8080`. Otherwise init says the proxy was not offered and leaves the config alone. `--uninstall` removes what this flow wrote, when it still points at the RTK daemon: Claude Code's `env.ANTHROPIC_BASE_URL`, Codex's `openai_base_url`, Vibe's `mistral` provider entry (a customised entry gets its default `api_base` back instead). A base URL aimed anywhere else is kept.

Codex's namespace follows its login mode at the time of the init. `rtk codex` picks the right namespace at every launch, so if you switch between an API key and a ChatGPT login, either launch with `rtk codex` or re-run `rtk init -g --codex`.

#### What the keep-alive actually does

It does **not** do the redirection — that is the config entry above. The keep-alive checks whether the agent's channel points at a loopback address with nothing listening there, and if so runs `rtk llm-proxy start`:

| Agent | When it runs | How it reports |
|---|---|---|
| Claude Code | `SessionStart` hook (`rtk hook claude`) | `systemMessage`, user-visible, never in model context |
| Codex | `SessionStart` hook (`rtk hook codex`) | stderr, and `additionalContext` when the daemon could not be started so the failure is visible |
| Mistral Vibe | the `pre_tool` hook it already has (`rtk hook vibe`), at most once every five minutes | stderr |

Every failure path stays quiet rather than disrupting the session.

### `rtk <agent>` — the launcher

```bash
rtk claude
rtk codex
rtk vibe
```

One command, nothing persisted: it starts the daemon if it is not already up, points the agent at it through the agent's own channel, and execs the real binary. On Unix this is a genuine `exec`, so signals, stdio and the exit code behave exactly as if you had launched the agent directly.

| Subcommand | How it redirects |
|---|---|
| `rtk claude` | sets `ANTHROPIC_BASE_URL` for that process |
| `rtk codex` | passes `-c openai_base_url="…"` for that run, choosing the namespace from your login mode |
| `rtk vibe` | writes nothing: it checks that `rtk init -g --agent vibe` has pointed the `mistral` provider at the daemon, and tells you to run it otherwise |

A subcommand exists only where the redirection has been verified end to end, so `rtk <agent>` resolving is itself the signal that traffic really goes through the daemon. An agent absent from the list is never *silently* unproxied — it falls through to plain passthrough. `rtk llm-proxy setup` reports what can be launched on this machine.

The launcher never clobbers a route you chose: a base URL already aimed at another gateway is left alone and named. And it never launches an agent at a dead port: when the daemon cannot start, Claude Code and Codex launch unproxied with a warning, and Vibe, whose config file cannot be overridden per run, is refused with the command that fixes it.

:::note[Codex and WebSockets]
Codex opens the Responses API over a WebSocket first. The proxy answers the upgrade with a plain HTTP error rather than relaying it, and Codex then falls back to HTTP for the rest of the session, which is the transport the proxy measures. The fallback is Codex's own behaviour on an exhausted retry budget, so the first turn of a proxied session can start a little later than usual.
:::

### Agents the proxy cannot serve

| Agent | Why | What RTK does instead |
|---|---|---|
| Cursor | every model request is routed through Cursor's servers, and the CLI has no base-URL override | hooks only; `rtk llm-proxy setup` says so when `cursor-agent` is detected |
| Antigravity | sign-in is OAuth through `agy login` only, and no base-URL or proxy setting exists | hooks only; `rtk llm-proxy setup` says so when `agy` is detected |

Both facts come from the vendors' own documentation and staff answers, checked on 2026-10-23. `rtk init -g --agent cursor` and `rtk init -g --agent antigravity` print the same line after installing the hooks, so the absence is visible rather than silent.

### `--env` — one shell, by hand

```bash
eval "$(rtk llm-proxy start --env)"
```

Any agent that reads `ANTHROPIC_BASE_URL` and is launched from that shell routes through the proxy. This is choice 3 above, done manually. Codex and Vibe do not read an environment variable for this; use `rtk codex`, `rtk vibe` or the guided setup.

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
