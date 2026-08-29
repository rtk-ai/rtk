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
 │                  │  │ skips: build=f   │  │                  │
 └────────┬─────────┘  └──────────────────┘  └──────────────────┘
          │ build != false
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
 │ ubuntu │ │ cargo    │ │ AST-aware │ │ no empty │
 │ windows│ │ audit    │ │ diff-only │ │ or grown │
 │ macos  │ │ patterns │ │           │ │ output   │
 └───┬────┘ └────┬─────┘ └─────┬─────┘ └────┬─────┘
     │           │             │            │
     └───────────┴──────┬──────┴────────────┘
                        │
             ┌──────────▼─────────┐
             │  All must pass     │
             │  to merge          │
             └────────────────────┘

     + release (build, after clippy: must also pass to merge)
     + Dependabot (weekly: Cargo deps + GitHub Actions)
```

On a docs-only PR the `changed paths` gate reports `build=false`, so `fmt` and
`test presence` skip and every job downstream of `fmt` skips with them.
`doc review` still runs — it is the gate that matters there. It is
`develop`-only, so it does not run on a PR into `master`.

If the `changed paths` job itself fails, `fmt` and `test presence` still run, but
`clippy` and everything below it skip through the failure, so the run ends red.

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
  │ 6 targets + DEB + RPM     │
  └─────────┬─────────────────┘
            │
  ┌─────────▼─────────────────┐
  │ GitHub Release            │
  │ (pre-release badge)       │
  │ draft, published last     │
  │                           │
  │ Discord:  SKIPPED         │
  │ Homebrew: SKIPPED         │
  └───────────────────────────┘
```

## Merge to master — stable release (stable-release.yml)

Trigger: push to master | workflow_dispatch | Concurrency: never cancelled

This lives in its own file, not in `cd.yml`, and is never path-gated: keeping
both paths in one workflow meant one bad gate could take a release with it.
`install.sh` resolves the version by following the redirect on
`/releases/latest` and downloading that tag's tarballs, so a stable release
published without assets would break every fresh install. Prereleases are
excluded from `/releases/latest`, which is why gating the develop path is safe
and gating master is not.

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
                     │ 6 targets + DEB + RPM    │
                     └────────────┬────────────┘
                                  │
                     ┌────────────▼────────────┐
                     │ GitHub Release           │
                     │ (stable, "Latest" badge) │
                     └────────────┬────────────┘
                                  │
                     ┌────────────▼────────────┐
                     │ verify-assets            │
                     │ every asset present, and │
                     │ checksums.txt agrees     │
                     └──┬─────────┬─────────┬──┘
                        │         │         │
                        ▼         ▼         ▼
                    Discord   Homebrew   latest
                    notify    tap update  tag
```

`verify-assets` asserts every listed asset uploaded non-empty and that
`checksums.txt` names nothing that failed to upload, and runs before Discord
and the Homebrew tap so an incomplete release is never announced.

It **detects and demotes; it does not prevent.** release-please publishes the
release before this workflow starts, so `/releases/latest` already points at it
while the build runs — a ~6-7 minute window on every stable release, tracked in
[#3759](https://github.com/rtk-ai/rtk/issues/3759). If the check fails, the
release is marked a prerelease, which takes it out of `/releases/latest` so
fresh installs fall back to the last complete tag rather than 404-ing
indefinitely. A later pass does not undo that: after repairing the assets, run
`gh release edit <tag> --prerelease=false --latest`.

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
     │ 6 targets + DEB + RPM   │
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
`load_pathspecs` in `.github/scripts/scope-lib.sh` is the one reader both scripts
share.

Callers skip only on `3`, never on any other non-zero status, so no accidental
failure can be mistaken for a skip; the script's header says why it is not `1`.

`.claude/hooks/` builds although it looks like tooling:
`tests/hook_decision_protocol_test.rs` runs `.claude/hooks/rtk-rewrite.sh`. That
is a runtime read, which `check-embed-scope.sh` cannot see, so a test that starts
reading a listed path has to take it off the list by hand.

Gating is per job, not under `on:`. A trigger-level `paths-ignore` skips the
whole workflow, which would take `doc review` with it on exactly the PRs it
exists to review, and would leave nothing to report should a required status
check ever be added.

A manual dispatch of `cd.yml` always builds. `cd.yml` diffs from the last RC
rather than from `github.event.before` because develop runs are
`cancel-in-progress`. A docs push landing on top of a cancelled source build
would otherwise see only its own commits, skip, and strand that source change
with no RC. That makes an RC tag a claim that its tree has binaries, so
`release.yml` uploads an RC as a draft and publishes it last, which is what
creates the tag. A cancelled or failed upload leaves a draft to delete by hand
and no tag, so the next push still rebuilds.

### The `,glob` trap

The allowlist is inverted: a push builds unless *everything* it touched is
excluded, so an unrecognised path always builds.

`hooks/**` is deliberately not excluded. `src/hooks/init/` `include_str!`s files
out of `hooks/`, `.md` ones included. A bare `:(exclude)*.md` pathspec
would swallow them, because without `,glob` a pathspec `*` also matches `/` —
and a docs-only commit would then ship a stale binary.

`docs/` is listed piecemeal for the same reason: tests embed the pages under
`docs/guide/pipelines/`, `docs/guide/proxy/` and `docs/guide/resources/` to
check their examples, so those directories build.

`.github/scripts/check-embed-scope.sh` enforces this. It resolves every
`include_str!`/`include_bytes!` target in a tracked `.rs` file, literal or
`concat!(env!("CARGO_MANIFEST_DIR"), "/…")` (collapsing newlines first, since
rustfmt wraps long paths), and fails if any is excluded, missing, or in a form
it cannot resolve, or if it finds nothing at all to check. It runs on every PR, including the docs-only
ones that skip the rest of CI.

Run both locally before changing the list:

```bash
bash .github/scripts/changed-scope.sh --self-test
bash .github/scripts/check-embed-scope.sh --self-test
bash .github/scripts/check-embed-scope.sh
bash .github/scripts/changed-scope.sh HEAD~1 HEAD
echo $?   # 0 = would build, 3 = would skip
```

Both scripts are pure POSIX-ish bash with no GNU-only tools, and every pathspec
carries `,top`, so the verdict does not depend on which directory you run them
from.
