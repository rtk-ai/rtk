---
title: Telemetry & Privacy
description: What RTK collects, how to opt out, and your GDPR rights
sidebar:
  order: 3
---

# Telemetry & Privacy

RTK collects anonymous, aggregate usage metrics once per day to help improve the product. Telemetry is **disabled by default** and requires explicit consent during `rtk init` or `rtk telemetry enable`.

## Data Collector

**Entity**: `RTK AI Labs`
**Contact**: contact@rtk-ai.app

## Why we collect telemetry

Without telemetry, we have no visibility into:

- Which commands are used most and need the best filters
- Which filters are underperforming and need improvement
- Which ecosystems to prioritize for new filter development
- How much value RTK delivers to users (token savings in $ terms)
- Whether users stay engaged over time or churn after trying RTK

This data directly drives our roadmap. For example, if telemetry shows that 40% of users run Python commands but only 10% of our filters cover Python, we know where to invest next.

## How it works

1. **Once per day** (23-hour interval), RTK sends a single HTTPS POST to our telemetry endpoint
2. The ping runs in a **background thread** and never blocks the CLI (2-second timeout)
3. A marker file prevents duplicate pings within the interval
4. If the server is unreachable, the ping is silently dropped — no retries, no queue

## What is collected

### Identity (anonymous)

| Field | Example | Purpose |
|-------|---------|---------|
| `device_hash` | `a3f8c9...` (64 hex chars) | Count unique installations. SHA-256 of a per-device random salt stored locally (`~/.local/share/rtk/.device_salt`). Not reversible. No hostname or username included. |

### Environment

| Field | Example | Purpose |
|-------|---------|---------|
| `version` | `0.34.1` | Track adoption of new versions |
| `os` | `macos` | Know which platforms to support and test |
| `arch` | `aarch64` | Prioritize ARM vs x86 builds |
| `install_method` | `homebrew` | Understand distribution channels (homebrew/cargo/script/nix) |

### Usage volume

| Field | Example | Purpose |
|-------|---------|---------|
| `commands_24h` | `142` | Daily activity level |
| `commands_total` | `32888` | Lifetime usage — segment light vs heavy users |
| `top_commands` | `["git", "cargo", "ls"]` | Most popular tools (names only, max 5) |
| `tokens_saved_24h` | `450000` | Daily value delivered |
| `tokens_saved_total` | `96500000` | Lifetime value delivered |
| `savings_pct` | `72.5` | Overall effectiveness |

### Quality (filter improvement)

| Field | Example | Purpose |
|-------|---------|---------|
| `passthrough_top` | `["git:15", "npm:8"]` | Top 5 commands with 0% savings — these need filters |
| `parse_failures_24h` | `3` | Filter fragility — high count means filters are breaking |
| `low_savings_commands` | `["rtk docker ps:25%"]` | Commands averaging <30% savings — filters to improve |
| `avg_savings_per_command` | `68.5` | Unweighted average (vs global which is volume-biased) |

### Ecosystem distribution

| Field | Example | Purpose |
|-------|---------|---------|
| `ecosystem_mix` | `{"git": 45, "cargo": 20, "js": 15}` | Category percentages — where to invest filter development |

### Retention (engagement)

| Field | Example | Purpose |
|-------|---------|---------|
| `first_seen_days` | `45` | Installation age in days |
| `active_days_30d` | `22` | Days with at least 1 command in last 30 days — measures stickiness |

### Economics

| Field | Example | Purpose |
|-------|---------|---------|
| `tokens_saved_30d` | `12000000` | 30-day token savings for trend analysis |
| `estimated_savings_usd_30d` | `36.0` | Estimated dollar value saved (at ~$3/Mtok input pricing, Claude Sonnet) |

### Adoption

| Field | Example | Purpose |
|-------|---------|---------|
| `hook_type` | `claude` | Which AI agent hook is installed (claude/gemini/codex/cursor/none) |
| `custom_toml_filters` | `3` | Number of user-created TOML filter files — DSL adoption |

### Configuration (user maturity)

| Field | Example | Purpose |
|-------|---------|---------|
| `has_config_toml` | `true` | Whether user has customized RTK config |
| `exclude_commands_count` | `2` | Commands excluded from rewriting — high count may indicate frustration |
| `projects_count` | `5` | Distinct project paths — multi-project = power user |

### Feature adoption

| Field | Example | Purpose |
|-------|---------|---------|
| `meta_usage` | `{"gain": 5, "discover": 2}` | Which RTK features are actually used |

## What is NOT collected

- Source code or file contents
- Full command lines or arguments (only tool names like "git", "cargo")
- File paths or directory structures
- Secrets, API keys, or environment variable values
- Repository names or URLs
- Personally identifiable information
- IP addresses (not stored in telemetry pings; stored temporarily in erasure audit log for accountability, anonymized after 6 months)

## Consent

Telemetry requires explicit opt-in consent (GDPR Art. 6, 7). Consent is requested during `rtk init` or via `rtk telemetry enable`. Without consent, no data is sent.

```bash
rtk telemetry status     # Check current consent state
rtk telemetry enable     # Give consent (interactive prompt)
rtk telemetry disable    # Withdraw consent
rtk telemetry forget     # Withdraw consent + delete local data + request server erasure
```

Environment variable override (blocks telemetry regardless of consent):
```bash
export RTK_TELEMETRY_DISABLED=1
```

## Retention Policy

- **Server-side**: telemetry records are retained for a maximum of **12 months**, then automatically purged.
- **Server-side (erasure log)**: IP addresses in the erasure audit log are **anonymized after 6 months** (GDPR — IP is personal data).
- **Client-side**: the local SQLite database (`~/.local/share/rtk/history.db`) retains data for **90 days** by default (configurable via `tracking.history_days` in `config.toml`). Deleted entirely by `rtk telemetry forget`.

## Your Rights (GDPR)

Under the EU General Data Protection Regulation, you have the right to:

- **Access** your data: `rtk telemetry status` shows your device hash; the telemetry payload is fully documented above.
- **Rectification**: since data is anonymous and aggregate, rectification is not applicable.
- **Erasure** (Art. 17): run `rtk telemetry forget` to delete local data and send an erasure request to the server. Alternatively, email contact@rtk-ai.app with your device hash.
- **Restriction of processing**: `rtk telemetry disable` stops all data collection immediately.
- **Portability**: the local SQLite database at `~/.local/share/rtk/history.db` contains all locally stored data.
- **Objection**: `rtk telemetry disable` or `export RTK_TELEMETRY_DISABLED=1`.

## Erasure Procedure

1. Run `rtk telemetry forget` — this disables telemetry, deletes your device salt, ping marker, and local tracking database (`history.db`), then sends an erasure request to the server.
2. If the server is unreachable, the CLI prints your full device hash and fallback instructions to email contact@rtk-ai.app for manual erasure.
3. You can also email contact@rtk-ai.app directly to request manual erasure.

## Data Handling

- All communications use HTTPS (TLS)
- Data is used exclusively for RTK product improvement
- No data is sold or shared with third parties
- Aggregate statistics may be published (e.g. "70% of RTK users are on macOS")
