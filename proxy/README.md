<h1 align="center">rtk-proxy</h1>

<p align="center">
  The local daemon that extends the RTK engine to your agent's provider-API
  traffic — <b>compress, relay routed work, and more</b>.
</p>

<p align="center">
  <a href="./LICENSE"><img alt="License: BUSL-1.1" src="https://img.shields.io/badge/license-BUSL--1.1-blue"></a>
</p>

---

`rtk` optimizes **command output** before it reaches your assistant's context.
`rtk-proxy` brings the same engine to the **API traffic** itself: it sits
between your AI coding tool and the real provider endpoint, runs the pipeline
on the traffic — compression, and the relay of calls your pipeline routes to
cheaper models — then relays the response back. It is agent-agnostic by
design: it speaks the provider's dialect, not the agent's. Claude Code
(`rtk claude`), Codex (`rtk codex`) and Mistral Vibe (`rtk vibe`) are wired
end to end; Cursor and Antigravity offer no base-URL override, so they keep
RTK's hooks and never reach the daemon.

It is a **local daemon**: one background process on your machine, driven by the
`rtk` CLI. It does four things: **intercept, compress, forward, record**.

```
  your agent ──▶ rtk-proxy ──▶ its provider (api.anthropic.com, api.openai.com,
                    │                        chatgpt.com, api.mistral.ai, …)
                    ├─ normalize: strip ANSI / redraws / dupes
                    ├─ your proxy.request.tool_result method rules
                    ├─ (opt-in) cache-align
                    └─ record usage ──▶ rtk gain
```

## User documentation

Setup, what the daemon compresses, its settings and endpoints live in the user guide, [`docs/guide/proxy/`](../docs/guide/proxy/overview.md), published at [rtk-ai.app/guide](https://www.rtk-ai.app/guide/proxy/overview):

| Page | Covers |
|---|---|
| [RTK Proxy](../docs/guide/proxy/overview.md) | install, `rtk <agent>`, `rtk init -g`, `--env`, which agents can be routed, binding, state, savings |
| [What the Proxy Compresses](../docs/guide/proxy/compression.md) | normalisation, cache alignment, your `proxy.request` rules |
| [Proxy Configuration](../docs/guide/proxy/configuration.md) | settings, provider dialects, HTTP endpoints, attribution headers |

```bash
rtk claude              # start the daemon if needed, then launch Claude Code through it
rtk codex               # same for Codex (API-key or ChatGPT login)
rtk vibe                # same for Mistral Vibe, once `rtk init -g --agent vibe` pointed it here
rtk llm-proxy status    # is it up
rtk gain                # savings, RTK and proxy side by side
```

## Building

The crate lives in the `rtk` repository as its own workspace, and is **not**
linked into the `rtk` CLI — `rtk` locates and spawns this binary.

```bash
cargo build --release --manifest-path proxy/Cargo.toml   # the daemon
cargo test --manifest-path proxy/Cargo.toml              # unit + functional + real-binary smoke
```

Contributing to the daemon: see [`docs/contributing/`](../docs/contributing/TECHNICAL.md).

## License

[Business Source License 1.1](./LICENSE) — source-available, **not** an
OSI-approved open-source license. Its Additional Use Grant permits **free
production use**, including running the daemon. What it does not permit is
offering `rtk-proxy`, or a derivative, to third parties as a paid or hosted
product or service — for that, contact **contact@rtk-ai.app**. On
**2030-09-09** this version converts automatically to **Apache-2.0**.

This is a different license from the `rtk` CLI in the parent directory, which
is Apache-2.0. The two are separate crates and separate binaries; `rtk` never
links this code.
