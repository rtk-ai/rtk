# Telemetry

RTK collects anonymous, aggregate usage metrics once per day to help improve the product. Telemetry is **enabled by default** and can be disabled at any time.

## Why we collect telemetry

RTK supports 100+ commands across 15+ ecosystems. Without telemetry, we have no way to know:

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

**Source code**: [`src/core/telemetry.rs`](../src/core/telemetry.rs)

## What is collected

### Identity (anonymous)

| Field | Example | Purpose |
|-------|---------|---------|
| `device_hash` | `a3f8c9...` (64 hex chars) | Count unique installations. Salted SHA-256 of hostname + username with a per-device random salt stored locally (`~/.local/share/rtk/.device_salt`). Not reversible. |

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
- IP addresses (not logged server-side)

## Opt-out

Telemetry can be disabled instantly with either method:

```bash
# Environment variable (per-session or in shell profile)
export RTK_TELEMETRY_DISABLED=1

# Or permanently in config file
# ~/.config/rtk/config.toml
[telemetry]
enabled = false
```

When disabled, `rtk init` shows `[info] Anonymous telemetry is disabled`. No data is sent, no background thread is spawned, no network requests are made.

## Data handling

- Telemetry endpoint URL and auth token are injected at **compile time** via `option_env!()` — they are not in the source code
- The server is hosted on GCP Cloud Run with TLS
- Data is used exclusively for RTK product improvement
- No data is sold or shared with third parties
- Aggregate statistics may be published (e.g. "70% of RTK users are on macOS")

## For contributors

The telemetry implementation lives in `src/core/telemetry.rs`. Key design decisions:

- **Fire-and-forget**: errors are silently ignored, never shown to users
- **Non-blocking**: runs in a `std::thread::spawn`, 2-second timeout
- **No async**: consistent with RTK's single-threaded design
- **Compile-time gating**: if `RTK_TELEMETRY_URL` is not set at build time, all telemetry code is dead — the binary makes zero network calls
- **23-hour interval**: prevents clock-drift accumulation that a strict 24h interval would cause

When adding new fields:
1. Add the query method to `src/core/tracking.rs`
2. Add the field to `EnrichedStats` in `src/core/telemetry.rs`
3. Populate it in `get_enriched_stats()`
4. Add it to the JSON payload in `send_ping()`
5. Update this document and the README.md privacy table
6. Ensure the field contains only **aggregate counts or anonymized names** — no raw paths, arguments, or user data
