# Analytics

## Scope

**Read-only dashboards** over the tracking database. Analytics presents the value that `cmds/` creates — it queries token savings, correlates with external spending data, and surfaces adoption opportunities. It never modifies the tracking DB.

Owns: `rtk gain` (savings dashboard), `rtk cc-economics` (cost reduction), `rtk session` (adoption analysis), and Claude Code usage data parsing.

Does **not** own: recording token savings (that's `core/tracking` called by `cmds/`), or command filtering itself (that's `cmds/`).

Boundary rule: if a new module writes to the DB, it belongs in `core/` or `cmds/`, not here. Tool-specific analytics (like `cc_economics` reading Claude Code data) are fine — the boundary is "read-only presentation", not "tool-agnostic".

## Purpose
Token savings analytics, economic modeling, and adoption metrics.

These modules read from the SQLite tracking database to produce dashboards, spending estimates, and session-level adoption reports that help users understand the value RTK provides.



## Files
| File | Responsibility |
|------|---------------|
| gain.rs | `rtk gain` command -- token savings dashboard with ASCII graphs, per-command history, quota consumption estimates, and cumulative savings over time; the primary user-facing analytics view |
| cc_economics.rs | `rtk cc-economics` command -- combines Claude Code API spending data with RTK savings to show net cost reduction; correlates token savings with dollar amounts |
| ccusage.rs | Claude Code usage data parser; defines `CcusagePeriod` and `Granularity` types; reads Claude Code session spending records to feed into `cc_economics.rs` |
| session_cmd.rs | `rtk session` command -- per-session RTK adoption analysis; shows which commands in a coding session were routed through RTK vs executed raw, highlighting missed savings opportunities |

## Adding New Functionality
To add a new analytics view: (1) create a new `*_cmd.rs` file in this directory, (2) query `core/tracking` for the metrics you need using the existing `TrackingDb` API, (3) register the command in `main.rs` under the `Commands` enum, and (4) add `#[cfg(test)]` unit tests with sample tracking data. Analytics modules should be read-only against the tracking database and never modify it.
