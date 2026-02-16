---
description: Build, commit, push & version bump workflow - automates the complete release cycle
---

# Ship Release

Systematic release workflow for RTK: build verification, version bump, changelog update, git tag, and push to trigger CI/CD.

## When to Use

- **Manual invocation**: When ready to release a new version
- **After feature completion**: Before tagging and publishing
- **Before version bump**: To automate the release checklist

## Pre-Release Checklist (Auto-Verified)

Before running `/ship`, verify:

### 1. Quality Checks Pass
```bash
cargo fmt --all --check    # Code formatted
cargo clippy --all-targets # Zero warnings
cargo test --all           # All tests pass
```

### 2. Performance Benchmarks Pass
```bash
hyperfine 'target/release/rtk git status' --warmup 3
# Should show <10ms mean time

/usr/bin/time -l target/release/rtk git status
# Should show <5MB maximum resident set size
```

### 3. Integration Tests Pass
```bash
cargo install --path . --force  # Install locally
cargo test --ignored            # Run integration tests
```

### 4. Git Clean State
```bash
git status  # Should show "nothing to commit, working tree clean"
```

## Release Workflow

### Step 1: Determine Version Bump

**Semantic Versioning** (MAJOR.MINOR.PATCH):
- **MAJOR** (v1.0.0): Breaking changes (rare for RTK)
- **MINOR** (v0.X.0): New features, new filters, new commands
- **PATCH** (v0.0.X): Bug fixes, performance improvements

**Examples**:
- New filter added (`rtk pytest`) → **MINOR** bump (v0.16.0 → v0.17.0)
- Bug fix in `git log` filter → **PATCH** bump (v0.16.0 → v0.16.1)
- Breaking CLI arg change → **MAJOR** bump (v0.16.0 → v1.0.0)

### Step 2: Update Version

**Files to update**:
1. `Cargo.toml` (line 3): `version = "X.Y.Z"`
2. `CHANGELOG.md` (add new section)
3. `README.md` (if version mentioned)

**Example**:
```toml
# Cargo.toml (before)
[package]
name = "rtk"
version = "0.16.0"  # Current version

# Cargo.toml (after - MINOR bump)
[package]
name = "rtk"
version = "0.17.0"  # New version
```

**CHANGELOG.md template**:
```markdown
## [0.17.0] - 2026-02-15

### Added
- `rtk pytest` command for Python test filtering (90% token reduction)
- Support for `pytest` JSON output parsing
- Integration with `uv` package manager auto-detection

### Fixed
- Shell escaping for PowerShell on Windows
- Memory leak in regex pattern caching

### Changed
- Updated `cargo test` filter to show test names in failures
```

### Step 3: Build and Verify

```bash
# Clean build
cargo clean
cargo build --release

# Verify binary
target/release/rtk --version
# Should show new version

# Run full quality checks
cargo fmt --all --check
cargo clippy --all-targets
cargo test --all

# Benchmark performance
hyperfine 'target/release/rtk git status' --warmup 3
# Should still be <10ms
```

### Step 4: Commit Version Bump

```bash
# Stage version files
git add Cargo.toml Cargo.lock CHANGELOG.md README.md

# Commit with version tag
git commit -m "chore(release): bump version to v0.17.0

- Updated Cargo.toml version
- Updated CHANGELOG.md with release notes
- Verified all quality checks pass
- Benchmarked performance (<10ms startup)

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

### Step 5: Create Git Tag

```bash
# Create annotated tag with changelog excerpt
git tag -a v0.17.0 -m "Release v0.17.0

Added:
- rtk pytest command (90% token reduction)
- Support for uv package manager

Fixed:
- Shell escaping for PowerShell
- Memory leak in regex caching

Performance: <10ms startup, <5MB memory"
```

### Step 6: Push to Remote

```bash
# Push commit and tags
git push origin main
git push origin v0.17.0

# Trigger GitHub Actions release workflow
# (CI/CD will build binaries, create GitHub release, publish to crates.io if configured)
```

## Post-Release Verification

After pushing, verify:

### 1. GitHub Actions CI/CD Pass
```bash
# Check GitHub Actions workflow status
gh run list --limit 1

# Watch latest run
gh run watch
```

### 2. GitHub Release Created
```bash
# Check if release created
gh release view v0.17.0

# Should show:
# - Release notes from git tag
# - Binaries attached (macOS, Linux x86_64/ARM64, Windows)
# - Checksums for verification
```

### 3. Installation Verification
```bash
# Test installation from release
curl -sSL https://github.com/rtk-ai/rtk/releases/download/v0.17.0/rtk-macos-latest -o rtk
chmod +x rtk
./rtk --version
# Should show v0.17.0
```

## Rollback Plan

If release has critical issues:

### Option 1: Patch Release (Preferred)
```bash
# Fix issue in new branch
git checkout -b hotfix/v0.17.1
# Apply fix
cargo test --all
git commit -m "fix: critical issue in pytest filter"

# Release v0.17.1 (PATCH bump)
# Follow release workflow above
```

### Option 2: Yank Release (crates.io only)
```bash
# Yank broken version from crates.io
cargo yank --vers 0.17.0

# Users can't download yanked version, but existing installs work
```

### Option 3: Revert Tag (Last Resort)
```bash
# Delete tag locally
git tag -d v0.17.0

# Delete tag on remote
git push origin :refs/tags/v0.17.0

# Delete GitHub release
gh release delete v0.17.0 --yes

# Revert commit
git revert HEAD
git push origin main
```

## Automated Release Script (Optional)

Save as `scripts/ship.sh`:

```bash
#!/bin/bash
set -euo pipefail

# Parse version argument
if [ $# -ne 1 ]; then
    echo "Usage: $0 <version>"
    echo "Example: $0 0.17.0"
    exit 1
fi

NEW_VERSION=$1

echo "🚀 Starting release workflow for v$NEW_VERSION"

# 1. Quality checks
echo "📦 Running quality checks..."
cargo fmt --all --check
cargo clippy --all-targets
cargo test --all

# 2. Update version
echo "🔢 Updating version to $NEW_VERSION..."
sed -i '' "s/^version = .*/version = \"$NEW_VERSION\"/" Cargo.toml

# 3. Build
echo "🔨 Building release binary..."
cargo build --release

# 4. Verify version
echo "✅ Verifying version..."
target/release/rtk --version | grep "$NEW_VERSION"

# 5. Commit
echo "💾 Committing version bump..."
git add Cargo.toml Cargo.lock
git commit -m "chore(release): bump version to v$NEW_VERSION

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"

# 6. Tag
echo "🏷️  Creating git tag..."
git tag -a "v$NEW_VERSION" -m "Release v$NEW_VERSION"

# 7. Push
echo "🚢 Pushing to remote..."
git push origin main
git push origin "v$NEW_VERSION"

echo "✅ Release v$NEW_VERSION shipped!"
echo "Monitor CI/CD: gh run watch"
```

**Usage**:
```bash
chmod +x scripts/ship.sh
./scripts/ship.sh 0.17.0
```

## Release Frequency

**Recommended cadence**:
- **PATCH releases**: As needed for critical bugs (24h turnaround)
- **MINOR releases**: Weekly or bi-weekly for new features
- **MAJOR releases**: Quarterly or when breaking changes necessary

## Version History Reference

Check version history:
```bash
git tag -l "v*"  # List all version tags
git log --oneline --tags  # Show commits with tags
```

Example output:
```
v0.17.0 (HEAD -> main, tag: v0.17.0, origin/main)
v0.16.0
v0.15.1
v0.15.0
```

## Common Issues

### Issue: CI/CD Fails After Tag Push

**Symptom**: GitHub Actions workflow fails on release build

**Solution**:
```bash
# Fix issue locally
git checkout main
# Apply fix
cargo test --all
git commit -m "fix: CI/CD build issue"
git push origin main

# Delete old tag
git tag -d v0.17.0
git push origin :refs/tags/v0.17.0

# Create new tag
git tag -a v0.17.0 -m "Release v0.17.0 (rebuild)"
git push origin v0.17.0
```

### Issue: Version Mismatch

**Symptom**: `rtk --version` shows old version after bump

**Solution**:
```bash
# Cargo.lock might be out of sync
cargo update -p rtk
cargo build --release

# Verify
target/release/rtk --version
```

### Issue: Changelog Merge Conflict

**Symptom**: CHANGELOG.md has conflicts after rebase

**Solution**:
```bash
# Always add new entries at top
# Manual merge:
# 1. Keep all entries from both branches
# 2. Sort by version (newest first)
# 3. Ensure date format consistency
```

## Security Considerations

**Before releasing**:
- [ ] No secrets in code (API keys, tokens)
- [ ] No `.env` files committed
- [ ] Dependencies scanned (`cargo audit`)
- [ ] Shell injection vulnerabilities reviewed
- [ ] Cross-platform shell escaping tested

**Dependency audit**:
```bash
cargo install cargo-audit
cargo audit

# Example output:
# Crate: some-crate
# Version: 0.1.0
# Warning: vulnerability found
# Advisory: CVE-2024-XXXXX
```

If vulnerabilities found:
```bash
# Update vulnerable dependency
cargo update some-crate

# Verify fix
cargo audit

# Re-run quality checks
cargo test --all
```
