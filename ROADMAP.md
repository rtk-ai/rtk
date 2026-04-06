# RTK Roadmap

## Completed

- **Easy Install**: Homebrew formula (`brew install rtk`) and pre-compiled binaries for macOS, Linux, and Windows.
- **Pro Tooling**: TOML-based configuration file (`~/.config/rtk/config.toml`) and structured logging.
- **Fork Strategy**: Established as the maintained fork with active development.

## In Progress

- **Early Adoption**: Prove token savings on real projects to onboard the first 5 teams.
- **Critical Fixes**: Resolve bugs and stabilize Vitest/pnpm support.

## Planned

- **Parser Infrastructure**: Migrate remaining command modules to structured three-tier parsing (LintResult, BuildOutput types).
- **Observability**: `rtk parse-health` command, degradation alerting, per-command parse tier tracking.
- **Per-Project Tracking**: Support multiple tracking databases scoped to individual projects.
- **Web Dashboard**: Localhost dashboard for visualizing token savings trends.

---
