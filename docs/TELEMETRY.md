# Telemetry (local-only build)

**This repository build does not upload usage data.** There is no outbound HTTPS ping, no compiled telemetry endpoint, and no device hash sent to a server.

## What is still recorded

- **Local SQLite** (`history.db` under the platform RTK data directory) stores command summaries, token savings, and history for `rtk gain` / `rtk gain --history`.
- Configure retention with `[tracking]` in `config.toml`; see [docs/usage/TRACKING.md](usage/TRACKING.md).

## CLI (legacy subcommands)

These remain for clearing old on-disk artifacts and the local database; they do **not** contact a network:

```bash
rtk telemetry status   # Explains local-only behavior + lists legacy files/config
rtk telemetry enable   # Informs that remote uploads are unavailable
rtk telemetry disable  # Clears legacy [telemetry] flags in config.toml
rtk telemetry forget   # Removes legacy salt/marker files and deletes local history.db
```

## Source

Implementation: [`src/core/telemetry.rs`](../src/core/telemetry.rs) (paths for legacy cleanup only) and [`src/core/telemetry_cmd.rs`](../src/core/telemetry_cmd.rs).
