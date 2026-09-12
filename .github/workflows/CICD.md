# CI/CD Flows

## PR Quality Gates (ci.yml)

Trigger: pull_request to develop or master

```
                          ┌──────────────────┐
                          │    PR opened      │
                          └────────┬─────────┘
                                   │
                          ┌────────▼─────────┐
                          │    fmt --all     │
                          └────────┬─────────┘
                                   │
                       ┌───────────▼──────────┐
                       │ clippy --all-targets │
                       └───┬───┬───┬───┬───┬──┘
                           │   │   │   │   │
           ┌───────────────┘   │   │   │   └────────────────┐
           │       ┌───────────┘   │   └───────────┐        │
           ▼       ▼              ▼               ▼        ▼
     ┌──────────┐ ┌──────────┐ ┌───────────┐ ┌─────────┐ ┌──────────┐
     │ test     │ │ security │ │ semgrep   │ │benchmark│ │ doc      │
     │ ubuntu   │ │ cargo    │ │ AST-aware │ │ >=80%   │ │ review   │
     │ windows  │ │ audit    │ │ diff-only │ │ savings │ │ ai agent │
     │ macos    │ │ patterns │ │           │ │         │ │          │
     └────┬─────┘ └────┬─────┘ └─────┬─────┘ └────┬────┘ └────┬─────┘
          │            │             │             │            │
          └────────────┴─────────┬───┴─────────────┴────────────┘
                                 │
                      ┌──────────▼─────────┐
                      │  All must pass     │
                      │  to merge          │
                      └────────────────────┘

     + DCO check (independent, develop PRs only)
     + Dependabot (weekly: Cargo deps + GitHub Actions)
```

## Merge to develop — pre-release (cd.yml)

Trigger: push to develop | workflow_dispatch (not master) | Concurrency: cancel-in-progress

```
     ┌──────────────────┐
     │ push to develop   │
     │ OR dispatch       │
     └────────┬─────────┘
              │
     ┌────────▼──────────────────┐
     │ pre-release                │
      │ compute next version      │
      │ from conventional commits │
      │ tag = dev-{next}-rc.{run} │
     └────────┬──────────────────┘
              │
     ┌────────▼──────────────────┐
     │ release.yml               │
     │ prerelease = true         │
     └────────┬──────────────────┘
              │
     ┌────────▼──────────────────┐
     │ Build                     │
     │ 5 platforms + DEB + RPM   │
     └────────┬──────────────────┘
              │
     ┌────────▼──────────────────┐
     │ GitHub Release            │
     │ (pre-release badge)       │
     │                           │
     │ Discord:  SKIPPED         │
     │ Homebrew: SKIPPED         │
     └──────────────────────────┘
```

When no stable `vX.Y.Z` tag exists, the workflow uses the `Cargo.toml`
package version as the baseline and analyzes the full commit history. This
allows a fork to create its first pre-release before its first stable release.

## Merge to master — stable release (cd.yml)

Trigger: push to master (only) | Concurrency: never cancelled

```
     ┌──────────────────┐
     │ push to master    │
     └────────┬─────────┘
              │
     ┌────────▼──────────────────┐
     │ release-please            │
     │ analyze conventional      │
     │ commits                   │
     └────────┬──────────────────┘
              │
         ┌────┴────────────────┐
         │                     │
    no release           release created
         │                     │
         ▼                     ▼
  ┌──────────────┐    ┌───────────────────────┐
  │ create/update│    │ release.yml            │
  │ release PR   │    │ prerelease = false     │
  └──────────────┘    └───────────┬───────────┘
                                  │
                     ┌────────────▼────────────┐
                     │ Build                   │
                     │ 5 platforms + DEB + RPM  │
                     └────────────┬────────────┘
                                  │
                     ┌────────────▼────────────┐
                     │ GitHub Release           │
                     │ (stable, "Latest" badge) │
                     └──┬─────────┬─────────┬──┘
                        │         │         │
                        ▼         ▼         ▼
                    Discord   Homebrew   latest
                    notify    tap update  tag
```

## Manual release (release.yml)

Trigger: workflow_dispatch

```
     ┌────────────────────────┐
     │ workflow_dispatch       │
     │ inputs: tag, prerelease │
     └───────────┬────────────┘
                 │
     ┌───────────▼────────────┐
     │ Full build pipeline     │
     │ 5 platforms + DEB + RPM │
     └───────────┬────────────┘
                 │
          ┌──────┴──────┐
          │             │
   prerelease=false  prerelease=true
          │             │
          ▼             ▼
     Discord        pre-release
     Homebrew       badge only
     latest tag
```
