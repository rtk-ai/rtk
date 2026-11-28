# Contributing to `rtk-proxy`

**Status: issues for `rtk-engine` and `rtk-proxy` are open. Pull requests open on 18 October 2026.**

The shared rules, the process, and what is accepted from 18 October are in
[`engine/CONTRIBUTING.md`](../engine/CONTRIBUTING.md) — it covers both crates.
This page holds only what is specific to the daemon.

Pull requests against `proxy/` opened before 18 October 2026 will be closed with a link
to this page. Not because they are unwelcome: the code underneath them is still
moving.

## Adding an agent to `rtk <agent>`

`rtk claude` starts the daemon if it is not running, points Claude Code at it,
and hands over the process — on Unix by `exec`, so signals, stdio and the exit
code behave exactly as if `claude` had been launched directly.

The table that drives it is `PROXY_AGENTS` in `src/llm_proxy.rs`:

```rust
pub const PROXY_AGENTS: &[ProxyAgent] = &[ProxyAgent {
    name: "claude",
    base_url_var: "ANTHROPIC_BASE_URL",
}];
```

**Membership is the contract.** A subcommand exists only where redirection has
been verified end to end, so `rtk <agent>` resolving is itself the signal that
the traffic really reaches the daemon. An agent that is absent falls through to
plain passthrough — it runs, unproxied, and nothing claims otherwise. Adding a
row you have not tested is worse than leaving it out, because it turns a visible
absence into a silent no-op.

To add one:

1. **Confirm the agent honours a base-URL environment variable**, and that
   setting it is sufficient — some agents also require a config-file entry, and
   some offer no override at all. Both cases are known and neither belongs in
   the table until the whole path works.
2. **Confirm the dialect is served.** The daemon answers `/v1/messages`
   (Anthropic) and `/v1/chat/completions` (OpenAI). An agent speaking anything
   else needs a route before it needs a table row.
3. **Add the row** — `name` is both the subcommand and the binary `exec`s.
4. **Extend `only_verified_agents_get_a_subcommand`** in `src/llm_proxy.rs`,
   moving the agent out of the must-not-resolve list.
5. **Check the traffic actually arrives**: run the agent through it and confirm
   the requests appear.

```bash
rtk llm-proxy start
rtk <agent> "say hello"
curl -s localhost:8080/stats/requests | head   # the request should be here
```

If step 5 shows nothing, the agent ignored the variable. That is a finding worth
an issue, not a row in the table.

## Adding a provider dialect

Serving a dialect and compressing it are separate steps.
`/v1/chat/completions` is served and measured today; every transform still
skips it.

1. **`Provider`** (`src/model.rs`) — the variant, and how a request maps to it.
2. **Route** (`src/server.rs`) — the inbound path.
3. **`src/extract.rs`** — content blocks, `usage_from_response`,
   `tool_calls_from_response`. Measurement works as soon as these do, and that
   alone is a useful contribution.
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
