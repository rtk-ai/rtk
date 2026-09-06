# CI/CD Flows

## PR Quality Gates (ci.yml)

Trigger: pull_request to develop or master

```
                       ┌──────────────────┐
                       │     PR opened    │
                       └─────────┬────────┘
                                 │
          ┌──────────────────────┼──────────────────────┐
          │                      │                      │
 ┌────────▼─────────┐  ┌─────────▼────────┐  ┌──────────▼───────┐
 │ changed paths    │  │ test presence    │  │ doc review       │
 │ inert-paths.conf │  │ *_cmd.rs need    │  │ ai agent         │
 │ -> build=t/f     │  │ #[cfg(test)]     │  │ NEVER gated      │
 └────────┬─────────┘  └──────────────────┘  └──────────────────┘
          │ build == true
 ┌────────▼─────────┐
 │    fmt --all     │
 └────────┬─────────┘
          │
 ┌────────▼─────────────┐
 │ clippy --all-targets │
 └──┬─────┬─────┬─────┬─┘
    │     │     │     │
    ▼     ▼     ▼     ▼
 ┌────────┐ ┌──────────┐ ┌───────────┐ ┌──────────┐
 │ test   │ │ security │ │ semgrep   │ │ benchmark│
 │ ubuntu │ │ cargo    │ │ AST-aware │ │ >=80%    │
 │ windows│ │ audit    │ │ diff-only │ │ savings  │
 │ macos  │ │ patterns │ │           │ │          │
 └───┬────┘ └────┬─────┘ └─────┬─────┘ └────┬─────┘
     │           │             │            │
     └───────────┴──────┬──────┴────────────┘
                        │
             ┌──────────▼─────────┐
             │  All must pass     │
             │  to merge          │
             └────────────────────┘

     + Dependabot (weekly: Cargo deps + GitHub Actions)
```

On a docs-only PR the `changed paths` gate reports `build=false`, so `fmt` and
`test presence` skip and `clippy` plus its four dependents skip with them.

On a docs-only PR `doc review` still runs — it is the gate that matters there.
It is `develop`-only, so it does not run on a PR into `master`.

The PRs into `master` are the `develop` → `master` promotion and
release-please's version bump, and both build: the promotion carries the
release's own source, and the bump touches `Cargo.toml` and `Cargo.lock`. Each
commit was already tested on its way into `develop`, but the tree they add up
to was not, and `stable-release.yml` publishes the release before the binaries
are built — so anything that only breaks on the merged tree would ship first
and be found afterwards. A contributor PR that lands on `master` by mistake is
answered separately by `pr-target-check.yml`.

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
     │ changed-scope.sh since     │
     │   the last dev-*-rc tag    │
     └────────┬──────────────────┘
              │
         ┌────┴───────────────────┐
         │                        │
   build inputs changed     docs only
         │                        │
         ▼                        ▼
  ┌───────────────────┐   ┌──────────────────┐
  │ compute version   │   │ tag unset        │
  │ from conventional │   │ everything below │
  │ commits           │   │ skips, no RC     │
  │ tag =             │   └──────────────────┘
  │ dev-{next}-rc.{n} │
  └─────────┬─────────┘
            │
  ┌─────────▼─────────────────┐
  │ release.yml               │
  │ prerelease = true         │
  └─────────┬─────────────────┘
            │
  ┌─────────▼─────────────────┐
  │ Build                     │
  │ 5 platforms + DEB + RPM   │
  └─────────┬─────────────────┘
            │
  ┌─────────▼─────────────────┐
  │ GitHub Release            │
  │ (pre-release badge)       │
  │                           │
  │ Discord:  SKIPPED         │
  │ Homebrew: SKIPPED         │
  └───────────────────────────┘
```

## Merge to master — stable release (stable-release.yml)

Trigger: push to master (only) | Concurrency: never cancelled

This lives in its own file, not in `cd.yml`. The develop pre-release path is
path-gated and the release path is not, and keeping them in one workflow meant
one bad gate could take a release with it.

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
                     └────────────┬────────────┘
                                  │
                     ┌────────────▼────────────┐
                     │ verify-assets            │
                     │ all 8 present, and       │
                     │ checksums.txt agrees     │
                     └──┬─────────┬─────────┬──┘
                        │         │         │
                        ▼         ▼         ▼
                    Discord   Homebrew   latest
                    notify    tap update  tag
```

`verify-assets` asserts all 8 assets uploaded non-empty and that
`checksums.txt` names nothing that failed to upload, and runs before Discord
and the Homebrew tap so an incomplete release is never announced.

It **detects and demotes; it does not prevent.** release-please publishes the
release before this workflow starts, so `/releases/latest` already points at it
while the build runs — a ~6-7 minute window on every stable release, tracked in
[#3759](https://github.com/rtk-ai/rtk/issues/3759). If the check fails, the
release is marked a prerelease, which takes it out of `/releases/latest` so
fresh installs fall back to the last complete tag rather than 404-ing
indefinitely.

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

## Path-based gating

`.github/inert-paths.conf` holds **git pathspecs** for the paths that cannot
change the binary or CI. `.github/scripts/changed-scope.sh` subtracts them from
a diff range and answers through its exit status — `0` build, `3` nothing
survived. `ci.yml` passes `HEAD^1 HEAD` (on `pull_request` the checked-out ref
is the merge commit, so that is exactly the PR); `cd.yml` passes the last
`dev-*-rc.*` tag and `HEAD`.

Git does the matching, so there is no glob engine of ours to get wrong.
`.github/scripts/load-pathspecs.sh` is the one reader both scripts share.

The skip verdict is `3` rather than `1` because bash hands out the low statuses
itself: `1` for a `set -u` violation or a failed command, `2` for a syntax
error, `127` for a missing one. Callers skip only on `3`, so no accidental
failure can be mistaken for a skip.

`.github/security-paths.conf` is the opposite sense: positive pathspecs that
must reach the `Security Scan` job even when the build gate skips them.
`.claude/hooks/` holds shell scripts Claude Code runs on contributors' machines
— not build inputs, so the Rust pipeline has nothing to say about them, and
semgrep is Rust-only, which leaves the security job's diff scan as their only
automated review. That job's pattern scan was Rust-only too, so it also carries
a `Shell script scan` step over changed `*.sh`/`*.bash`; without it the routing
would report "no dangerous patterns" for anything a shell script can do. The
job's Rust-only steps still run for a shell-only change, tracked in
[#3761](https://github.com/rtk-ai/rtk/issues/3761).

The job keeps fail-fast: it runs when `clippy` succeeded, or when the build gate
skipped the Rust pipeline outright. A failing `fmt` skips `clippy` too, and that
case must not reach the scan.

Gating is per job, not under `on:`. A trigger-level `paths-ignore` skips the
whole workflow, which would take `doc review` with it on exactly the PRs it
exists to review, and would leave nothing to report should a required status
check ever be added. `stable-release.yml` is not gated at all, which is why it
is a separate file from `cd.yml`.

`cd.yml` diffs from the last RC rather than from `github.event.before` because
develop runs are `cancel-in-progress`. A docs push landing on top of a
cancelled source build would otherwise see only its own commits, skip, and
strand that source change with no RC.

### The `,glob` trap

The allowlist is inverted: a push builds unless *everything* it touched is
excluded, so an unrecognised path always builds.

`hooks/**` is deliberately not excluded. `src/hooks/init.rs` `include_str!`s
ten files out of `hooks/`, six of them `.md`. A bare `:(exclude)*.md` pathspec
would swallow them, because without `,glob` a pathspec `*` also matches `/` —
and a docs-only commit would then ship a stale binary.

`.github/scripts/check-embed-scope.sh` enforces this. It resolves every
`include_str!`/`include_bytes!` target under `src/` (collapsing newlines first,
since rustfmt wraps long paths) and fails if any is excluded, missing, or if it
finds nothing at all to check. It runs on every PR, including the docs-only
ones that skip the rest of CI.

Run both locally before changing the list:

```bash
bash .github/scripts/changed-scope.sh --self-test
bash .github/scripts/check-embed-scope.sh
bash .github/scripts/changed-scope.sh HEAD~1 HEAD
echo $?   # 0 = would build, 3 = would skip
```

Both scripts are pure POSIX-ish bash with no GNU-only tools, and every pathspec
carries `,top`, so the verdict does not depend on which directory you run them
from.

**The master path is never gated.** `install.sh` resolves the version by
following the redirect on `/releases/latest` and downloading that tag's
tarballs, so a stable release published without assets would break every fresh
install. Prereleases are excluded from `/releases/latest`, which is why gating
the develop path is safe and gating master is not.
