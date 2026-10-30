# Contributing to `rtk-proxy`

**Status: issues for `rtk-engine` and `rtk-proxy` are open. Pull requests open on 18 October 2026.**

The shared rules, the process, and what is accepted from 18 October are in
[`engine/CONTRIBUTING.md`](../engine/CONTRIBUTING.md) — it covers both crates.
This page holds only what is specific to the daemon.

Pull requests against `proxy/` opened before 18 October 2026 will be closed with a link
to this page. Not because they are unwelcome: the code underneath them is still
moving.

## Adding an agent to `rtk <agent>`

`rtk claude`, `rtk codex` and `rtk vibe` start the daemon if it is not running,
point the agent at it through the one channel that agent reads, and hand over
the process — on Unix by `exec`, so signals, stdio and the exit code behave
exactly as if the agent had been launched directly.

The table that drives it is `PROXY_AGENTS` in `src/llm_proxy.rs`. Each row
names the subcommand (which is also the binary it execs), a display label, and
a `Redirect`, which is how the agent is pointed elsewhere:

| `Redirect` | The agent reads | Launcher | Guided setup |
|---|---|---|---|
| `Env { var }` | an environment variable (Claude Code: `ANTHROPIC_BASE_URL`) | sets it for the process | writes it into the agent's settings |
| `CliOverride { file, key, read, path }` | a config key that a CLI flag can override per run (Codex: `openai_base_url`, `-c`) | passes the override | writes the key |
| `ConfigFile { file, path, read }` | a config file only (Vibe: the `mistral` provider's `api_base`) | verifies the file and refuses to launch at a dead daemon; never writes | writes the entry |

Agents with no channel at all are listed in `UNPROXYABLE` with the vendor's
own statement and the date it was checked. `rtk llm-proxy setup` and
`rtk init` print that reason when such an agent is detected, so the absence is
visible rather than silent.

**Membership is the contract.** A subcommand exists only where redirection has
been verified end to end, so `rtk <agent>` resolving is itself the signal that
the traffic really reaches the daemon. An agent that is absent falls through to
plain passthrough — it runs, unproxied, and nothing claims otherwise. Adding a
row you have not tested is worse than leaving it out, because it turns a visible
absence into a silent no-op.

To add one:

1. **Find the agent's channel in its own documentation or source**, and
   confirm it is sufficient on its own. Record it on the agent's harness
   profile too: `base_url_config` in `engine/src/harness.rs` takes the same
   three shapes (`Env`, `ConfigKey`, `ProviderTable`), and a test in
   `src/llm_proxy.rs` fails when the launcher and the profile disagree. An
   agent with no channel gets `None` there and a row in `UNPROXYABLE`.
2. **Confirm the dialect is served.** The daemon answers `/v1/messages`
   (Anthropic), `/v1/chat/completions` (OpenAI chat) and `/v1/responses`
   (OpenAI Responses), each also under a named upstream's namespace. An agent
   speaking anything else needs a route before it needs a table row.
3. **Confirm the upstream exists.** A provider other than Anthropic or OpenAI
   is a named upstream: add it to `DEFAULT_UPSTREAMS` in `src/config.rs` and
   its model families to `models.toml`.
4. **Add the row**, and the on-ramp for `rtk init` when the agent has a
   persistent channel: implement `ProxyOnramp` in
   `src/hooks/init/proxy_onramp.rs` for it, and have `--uninstall` remove only
   what the on-ramp wrote.
5. **Extend `only_verified_agents_get_a_subcommand`** in `src/llm_proxy.rs`,
   moving the agent out of the must-not-resolve list.
6. **Check the traffic actually arrives**: run the agent through it and confirm
   the request appears.

```bash
rtk llm-proxy start
rtk <agent> "say hello"
rtk llm-proxy stats            # one request, under the agent's client name
```

If step 6 shows nothing, the agent ignored the channel. That is a finding worth
an issue, not a row in the table.

## Adding a provider dialect

Serving a dialect and compressing it are separate steps.
`/v1/chat/completions` and `/v1/responses` are served and measured today;
every transform still skips them.

1. **`Provider`** (`src/model.rs`) — the variant, and how a request maps to it.
2. **Route** (`src/server.rs`) — the inbound path, bare and under a namespace.
3. **`src/extract.rs`** — content blocks, `usage_from_response`,
   `tool_calls_from_response`, and the streamed shape in `forward.rs`'s
   `SseAccumulator`. Measurement works as soon as these do, and that alone is
   a useful contribution.
4. **Open each transform's gate.** Every transform checks the provider itself —
   `normalize.rs` and `cache_align.rs` return early for anything but Anthropic.
   Open one only with a measured result behind it: reshaping an unfamiliar
   dialect risks the prompt cache, and one invalidation costs more than the
   compression saves.
5. **Tests** — round-trip a real captured body, and assert that a dialect you
   did not open is forwarded byte-identical.

## Reporting a compression result

The numbers decide what ships enabled. A result is useful when it says what was
run, against what baseline, over how many requests, and what happened to the
cache hit rate. `/stats/summary` and `/stats/by-compression` report the daemon's
figures; `rtk gain` shows both sites together.

## Scope

`rtk-proxy` is a single-operator local daemon driven by the `rtk` CLI. There is
no browser and no dashboard, so a change that only makes sense with a web UI, a
scraper, or more than one operator does not belong here.

The practical test: if a route or setting has no caller in this repository, it is
not a feature, it is surface.

## Licensing

`rtk-proxy` is BUSL-1.1 (Business Source License) — source-available, not
OSI-open-source. A contribution is licensed under BUSL-1.1. Whether a sign-off
(DCO) is required will be announced when pull requests open.
