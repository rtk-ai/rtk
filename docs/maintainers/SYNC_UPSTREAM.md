# Syncing the Homeserve mirror with upstream rtk-ai/rtk

This fork (`HomeserveFR/rtk`) is a macOS-only mirror of the upstream `rtk-ai/rtk`,
maintained for Homeserve security compliance. The branch `homeserve/main` carries
upstream `master` plus Homeserve-specific commits (e.g. SonarQube workflow).

Sync is **manual** — there is no scheduled automation. Pull upstream changes when
needed by following the procedure below.

## One-time setup

Add the upstream remote to your local clone:

```bash
git remote add upstream git@github.com:rtk-ai/rtk.git
git fetch upstream --tags
```

## Sync procedure

```bash
git checkout homeserve/main
git fetch upstream
git fetch origin

# Inspect what's new upstream since the last sync
git log --oneline homeserve/main..upstream/master

# Merge upstream master into homeserve/main (preferred over rebase
# to preserve the Homeserve commits without rewriting history)
git merge upstream/master

# Resolve conflicts if any (the SonarQube/release workflow files are
# the most likely conflict points). Keep the Homeserve versions for:
#   - .github/workflows/sonarqube.yml
#   - .github/workflows/cd.yml
#   - .github/workflows/release.yml
#   - install.sh
#   - Formula/rtk.rb
#   - README.md (Installation section)
#   - Cargo.toml (repository field)

# Push to origin (release-please will detect new commits and open
# a release PR automatically)
git push origin homeserve/main
```

## Releasing after sync

1. After the push, GitHub Actions runs `cd.yml` → `release-please` job
2. release-please opens (or updates) a PR titled `chore(homeserve/main): release X.Y.Z`
3. Review the PR — verify the version bump and CHANGELOG entries make sense
4. Merge the release PR → triggers `release.yml` → builds 2 macOS binaries
5. The `latest` tag is force-updated by `update-latest-tag` job
6. End users running `install.sh` get the new version automatically

## Verification

After release publication:

```bash
gh release view --repo HomeserveFR/rtk
gh release list --repo HomeserveFR/rtk --limit 3
```

Expected assets in each release:

- `rtk-x86_64-apple-darwin.tar.gz`
- `rtk-aarch64-apple-darwin.tar.gz`
- `checksums.txt`

## Skipping a release

If you need to sync upstream code without releasing, close the release-please
PR without merging. Release-please will reopen it on the next push. To suppress
a release entirely for a given commit, prefix the commit subject with `chore:`
(release-please ignores `chore` for version bumps under default config).
