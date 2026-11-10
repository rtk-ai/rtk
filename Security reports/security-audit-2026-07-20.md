# Security Audit — 2026-07-20

## Summary
- Issues found: 8 | Auto-fixed: 2 | Unresolved: 6
- Status: ISSUES FIXED

## Fixed Issues

| # | Component | Advisory | Change |
|---|-----------|----------|--------|
| 1 | `.github/dependabot.yml` | semgrep `dependabot-missing-cooldown` | Added `cooldown: { default-days: 7 }` to both `cargo` and `github-actions` ecosystem entries — waits 7 days before proposing updates to newly published package versions, reducing exposure to malicious/unstable just-published packages |
| 2 | `.github/workflows/ci.yml` | semgrep `yaml.github-actions.security.run-shell-injection` | `check-test-presence` step interpolated `${{ github.base_ref }}` directly into a `run:` shell block; moved it into an `env: BASE_REF` var and referenced it as `"$BASE_REF"` in the script instead, per GitHub's script-injection hardening guidance |

## Unresolved Issues

| # | Component | Advisory | Reason |
|---|-----------|----------|--------|
| 3 | `.github/workflows/cd.yml` (×4), `ci.yml` (×1) | semgrep `github-actions-mutable-action-tag` | `actions/checkout@v4`, `actions/create-github-app-token@v3`, `googleapis/release-please-action@v4` referenced by mutable tag instead of a pinned 40-char commit SHA. Fixable, but pinning to the wrong SHA silently breaks CI/CD (a hard-to-reverse, shared-state change) and requires fetching+verifying the current SHA for each tag from each action's repo. Left for manual PR review rather than an unattended overnight edit. |
| 4 | `.github/workflows/cd.yml` (×2) | semgrep `secrets-inherit` | `secrets: inherit` passed to reusable workflow calls (lines 94, 131). Replacing with an explicit `secrets:` map requires knowing exactly which secrets the called reusable workflow consumes — under-scoping would silently break the release pipeline. Needs manual verification against the callee workflow's `on.workflow_call.secrets` block. |
| 5 | `src/main.rs:1810`, `src/hooks/hook_cmd.rs:1300` | semgrep `rust.lang.security.temp-dir` | `std::env::temp_dir()` used inside `#[cfg(test)]` fixtures only (`rtk-test-audit`, mtime probe) — not a runtime security-decision path. No live risk; reviewed false positive, no code change. |
| 6 | `src/main.rs:1252, 1252` | semgrep `rust.lang.security.args` | `std::env::args()` used for normal CLI argument parsing (RTK's actual purpose as a CLI proxy), not for any security/authorization decision. Reviewed false positive. |
| 7 | `src/hooks/*.rs` (2 sites) | semgrep `rust.lang.security.current-exe` | `std::env::current_exe()` used for hook self-install path resolution, not a security boundary. Reviewed false positive. |
| 8 | `src/main.rs:1506, 2514` | semgrep `rust.lang.security.unsafe-usage` | `unsafe { libc::signal(...) }` for SIGPIPE/SIGINT/SIGTERM handler registration — standard, narrowly-scoped `libc` FFI pattern for a single-threaded CLI, no memory-safety issue. Reviewed false positive, no code change. |

## Reviewed False Positives (no action needed)

| Tool | Finding | Location | Why it's benign |
|------|---------|----------|------------------|
| gitleaks | 33 `stripe-access-token`/`generic-api-key` matches | `Security reports/*.md` (historical audit docs) | Prior audit reports documenting past false-positive findings, not live keys |
| gitleaks | 5 `generic-api-key` matches | `src/cmds/cloud/aws_cmd.rs` (`#[test]` fns) | Synthetic CloudWatch pagination-token fixture strings (`f/abcdef123...`) used in unit tests |
| gitleaks | 2 `generic-api-key` matches | `scripts/benchmark/cloud-init.yaml` | Benchmark fixture, no live credentials |
| gitleaks | 1 `stripe-access-token` match | `SECURITY.md` | Documentation example of a bad pattern to avoid |
| gitleaks | 1 `stripe-access-token` match | `Security reports/2026-04-16-security-report.md` | Historical audit doc, same as above |

## Raw Scanner Output

**Scope**
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    b34cb0a (post-merge, pre-audit-fix)
```

| Tool | Result |
|------|--------|
| gitleaks (git history + fs) | 36 leaks detected, all reviewed as false positives (see table above) |
| trufflehog (git, `--only-verified`) | 0 verified secrets |
| trivy fs | 0 vulnerabilities, 0 misconfigs, 0 secrets across all scanned lockfiles (Cargo.lock incl. worktree copies, fixture pom.xml files) |
| osv-scanner (Cargo.lock) | 0 issues, 203 packages scanned |
| semgrep (`p/rust`, `p/owasp-top-ten`, `p/secrets`) | 52 findings across 382 files / 606 rules — 8 unique advisories after de-duplication (2 fixed, 6 unresolved/false-positive, see tables above); 0 secrets findings |
| bandit | Skipped — no `.py` source files (only test/CI fixture scripts) |
| CodeQL | Not applicable — no `.github/workflows/codeql.yml` in this repo |
| mcps-audit | No MCP manifest files found in scanned scope |

## Build Failure

`cargo install --path . --force` failed: no `cargo`/`rustup` toolchain present on `PATH` in this sandbox execution environment (`~/.cargo/bin` does not exist, `which rustup` empty). This is an environment gap, not a code regression — binary was not rebuilt or reinstalled this run. `$HOME/.local/bin/rtk` and `$HOME/.cargo/bin/rtk` were left untouched from the prior successful install.

```
rtk: Failed to run cargo install: Failed to spawn process: No such file or directory (os error 2)
```

### Note on scope
This run added `p/rust` and `p/secrets` Semgrep rulesets (previous days ran `p/owasp-top-ten` only, which doesn't meaningfully cover Rust or GitHub Actions YAML). This surfaced 8 new advisory classes not seen in prior daily reports — mostly CI/CD workflow hygiene (mutable action pins, `secrets: inherit`, one shell-injection-shaped interpolation) plus expected Rust FFI/CLI patterns.
