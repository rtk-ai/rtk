# Astraflow (UCloud) — RTK Integration

> Part of [`hooks/`](../README.md)

## What is Astraflow?

[Astraflow](https://astraflow.ucloud-global.com) by UCloud (优刻得) is an OpenAI-compatible AI model aggregation platform supporting 200+ models. Because it exposes the standard OpenAI Chat Completions API, you can use it as the backend for any RTK-supported agent that accepts a custom `base_url` — for example **OpenCode** or **Hermes**.

## Integration Approach

Astraflow is an **API provider**, not a standalone coding-agent IDE. There is no proprietary hook format to intercept. Instead:

1. Configure your OpenAI-compatible agent to point at Astraflow's endpoint.
2. Install RTK hooks for that agent as usual (e.g. `rtk init --opencode` or `rtk init --agent hermes`).
3. RTK transparently rewrites shell commands before they consume tokens — regardless of which model backend is in use.

## Endpoints & API Keys

| Region | Base URL | Environment Variable |
|--------|----------|----------------------|
| Global | `https://api-us-ca.umodelverse.ai/v1` | `ASTRAFLOW_API_KEY` |
| China  | `https://api.modelverse.cn/v1`        | `ASTRAFLOW_CN_API_KEY` |

Sign up and obtain an API key at:
- Global: <https://astraflow.ucloud-global.com>
- China:  <https://astraflow.ucloud.cn>

## Quick-Start with OpenCode

```bash
# 1. Export your Astraflow API key
export ASTRAFLOW_API_KEY=your_key_here
# (China users: export ASTRAFLOW_CN_API_KEY=your_key_here)

# 2. Point OpenCode at Astraflow's global endpoint
export OPENAI_API_KEY="$ASTRAFLOW_API_KEY"
export OPENAI_BASE_URL="https://api-us-ca.umodelverse.ai/v1"

# 3. Install the RTK plugin for OpenCode
rtk init -g --opencode

# 4. RTK now intercepts every shell command OpenCode runs,
#    delivering 60-90% token savings regardless of the model.
```

## Quick-Start with Hermes

```bash
export ASTRAFLOW_API_KEY=your_key_here
export OPENAI_API_KEY="$ASTRAFLOW_API_KEY"
export OPENAI_BASE_URL="https://api-us-ca.umodelverse.ai/v1"

# Install RTK Hermes plugin
rtk init --agent hermes
```

## Why RTK Matters for Astraflow

Astraflow charges per token. RTK reduces token consumption by 60-90% on common developer operations (builds, tests, git, grep, docker, kubectl). Combining RTK with Astraflow-backed agents directly lowers your Astraflow API costs.

## Description

- **Global**: Astraflow by UCloud — OpenAI-compatible platform supporting 200+ models (global endpoint)
- **China**: Astraflow by UCloud — OpenAI-compatible platform supporting 200+ models (China endpoint)
