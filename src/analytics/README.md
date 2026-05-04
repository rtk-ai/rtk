# Analytics

> See also [docs/contributing/TECHNICAL.md](../../docs/contributing/TECHNICAL.md) for the full architecture overview

## Scope

**Read-only dashboards** over the tracking database. Queries token savings, correlates with external spending data, and surfaces adoption metrics. Never modifies the tracking DB.

Owns: `rtk gain` (savings dashboard), `rtk cc-economics` (cost reduction), `rtk session` (adoption analysis), and Claude Code usage data parsing.

Does **not** own: recording token savings (that's `core/tracking` called by `cmds/`), or command filtering itself (that's `cmds/`).

Boundary rule: if a new module writes to the DB, it belongs in `core/` or `cmds/`, not here. Tool-specific analytics (like `cc_economics` reading Claude Code data) are fine — the boundary is "read-only presentation", not "tool-agnostic".

## Purpose
Token savings analytics, economic modeling, and adoption metrics.

These modules read from the SQLite tracking database to produce dashboards, spending estimates, and session-level adoption reports.

## Adding New Functionality
To add a new analytics view: (1) create a new `*_cmd.rs` file in this directory, (2) query `core/tracking` for the metrics you need using the existing `TrackingDb` API, (3) register the command in `main.rs` under the `Commands` enum, and (4) add `#[cfg(test)]` unit tests with sample tracking data. Analytics modules should be read-only against the tracking database and never modify it.
