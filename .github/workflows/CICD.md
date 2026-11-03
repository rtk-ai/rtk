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
     │ tag = v{next}-rc.{run}    │
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

## Claude Desktop plugin (release.yml `desktop-plugin`, ci.yml `desktop-plugin`)

The plugin in `desktop-plugin/` ships as `rtk-desktop-plugin-<version>.zip` on every release, built from the Linux musl `rtk` of that release:

```
release ──► desktop-plugin (continue-on-error)
              ├─ test the service (node --test) and the hooks against the musl rtk
              ├─ build.sh → rtk-desktop-plugin-<version>.zip
              ├─ gh release upload (versioned zip + rtk-desktop-plugin.zip) + checksums.txt lines
              ├─ stable release: push the plugin into rtk-ai/claude-plugins (token from the GitHub App)
              └─ on failure: error annotation, issue "Claude Desktop plugin build failed"
                 (opened, or commented when one is open), release Discord channel
```

It runs after `release` and never blocks it: a red plugin job leaves the rtk release complete and `update-latest-tag` unaffected, but the alerts mean it has to be fixed before the next release. On pull requests, ci.yml runs the same tests and a build (with a gnu `rtk`) only when the PR touches `desktop-plugin/` or these workflows.
