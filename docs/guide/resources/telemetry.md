---
title: Telemetry & Privacy
description: What RTK collects, how to opt out, and your GDPR rights
sidebar:
  order: 4
---

# Telemetry & Privacy

Telemetry is **off by default**. RTK sends nothing until you give explicit consent, either at the `rtk init` prompt (`[y/N]`, default no) or with `rtk telemetry enable`. Once enabled, RTK sends anonymous, aggregate usage metrics at most once per day.

## Data Collector

**Entity**: `RTK AI Labs`
**Contact**: contact@rtk-ai.app

## How it works

1. **Once per day** (23-hour interval), RTK sends a single HTTPS POST to the telemetry endpoint
2. The ping runs in a **background thread** and never blocks the CLI (2-second timeout)
3. A marker file prevents duplicate pings within the interval
4. If the server is unreachable, the ping is silently dropped — no retries, no queue

## What is collected

Example values below are illustrative.

### Identity (anonymous)

| Field | Example | Description |
|-------|---------|-------------|
| `device_hash` | `a3f8c9...` (64 hex chars) | SHA-256 of a per-device random salt stored locally (`~/.local/share/rtk/.device_salt`). Not reversible. No hostname or username included. |

### Environment

| Field | Example | Description |
|-------|---------|-------------|
| `version` | `1.0.0` | RTK version |
| `os` | `macos` | Operating system |
| `arch` | `aarch64` | CPU architecture |
| `install_method` | `homebrew` | How RTK was installed (homebrew/cargo/script/nix) |

### Usage volume

| Field | Example | Description |
|-------|---------|-------------|
| `commands_24h` | `100` | Commands run in the last 24 hours |
| `commands_total` | `10000` | Commands run since install |
| `top_commands` | `["git", "cargo", "ls"]` | Most used tools (names only, max 5) |
| `tokens_saved_24h` | `100000` | Estimated tokens saved in the last 24 hours |
| `tokens_saved_total` | `10000000` | Estimated tokens saved since install |
| `savings_pct` | `70.0` | Overall bash output reduction |

### Filter quality

| Field | Example | Description |
|-------|---------|-------------|
| `passthrough_top` | `["git:10", "npm:5"]` | Top 5 commands with 0% bash output reduction |
| `parse_failures_24h` | `0` | Filter parse failures in the last 24 hours |
| `low_savings_commands` | `["rtk <cmd>:25%"]` | Commands with a weighted bash output reduction below 30% (net-negative ones included). The rate is `SUM(saved)/SUM(input)` over every call of the command, the same figure as the `rtk gain` By Command table. Exact 0% is reported in `passthrough_top` instead |
| `avg_savings_per_command` | `70.0` | Unweighted average of per-command reduction rates across distinct command names; commands that never had any input are skipped |

### Ecosystem distribution

| Field | Example | Description |
|-------|---------|-------------|
| `ecosystem_mix` | `{"git": 50, "cargo": 20, "js": 10}` | Share of commands per tool category, in percent |

### Activity

| Field | Example | Description |
|-------|---------|-------------|
| `first_seen_days` | `30` | Installation age in days |
| `active_days_30d` | `20` | Days with at least 1 command in the last 30 days |

### Economics

| Field | Example | Description |
|-------|---------|-------------|
| `tokens_saved_30d` | `1000000` | Estimated tokens saved in the last 30 days |
| `estimated_savings_usd_30d` | — | A USD value derived from the estimated tokens saved and a fixed per-token rate. It is not a measured cost and does not reflect any provider's pricing |

### Integration

| Field | Example | Description |
|-------|---------|-------------|
| `hook_type` | `claude` | Which AI agent hook is installed (claude/gemini/codex/cursor/none) |
| `custom_toml_filters` | `1` | Number of user-created TOML filter files |

### Configuration

| Field | Example | Description |
|-------|---------|-------------|
| `has_config_toml` | `true` | Whether a `config.toml` exists |
| `exclude_commands_count` | `2` | Number of entries in `hooks.exclude_commands` |
| `projects_count` | `5` | Number of distinct project paths |

### Feature usage

| Field | Example | Description |
|-------|---------|-------------|
| `meta_usage` | `{"gain": 5, "discover": 2}` | How often RTK's own commands are run |

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

1. Run `rtk telemetry forget` — this disables telemetry, deletes your device salt, ping marker, local tracking database (`history.db`), recall database (including SQLite sidecars), and legacy/posthook tee logs, then sends an erasure request to the server.
2. If the server is unreachable, the CLI prints your full device hash and fallback instructions to email contact@rtk-ai.app for manual erasure.
3. You can also email contact@rtk-ai.app directly to request manual erasure.

Recovery cleanup uses `RTK_RECALL_DB` / `[retriever].database_path` and `RTK_TEE_DIR` / `[retriever].tee_directory` (including migrated `[tee].directory`), falling back to the standard recall database and tee directory. Both backends are cleared regardless of the current recovery mode or disable switches. Missing files are harmless. Cleanup removes top-level tee files named `<epoch>_<sanitized-slug>.log` and files matching `<epoch-ms>_<pid>-<sequence>_<sanitized-slug>.log` directly inside the `posthook` subdirectory. Unrelated files and other subdirectories are preserved. File symlinks and symlinked tee or `posthook` directories are refused and cleanup failures return a nonzero exit status.

Stop other RTK commands before erasing recovery data. An exclusive SQLite lock check refuses a busy recall database; removal is not an atomic barrier against another process starting afterward. This removes currently stored output; subsequent commands can store new output unless recovery is also disabled with `rtk config recall disabled`. Configuration and unrelated local files are retained.

## Data Handling

- All communications use HTTPS (TLS)
- Data is used exclusively for RTK product improvement
- No data is sold or shared with third parties
- Aggregate statistics may be published
