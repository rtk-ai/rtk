# Security Audit — 2026-07-21

## Summary
- Issues found: 6 | Auto-fixed: 0 | Unresolved: 6
- Status: PASSED (no new issues; all findings are prior known false-positives/manual-review items, unchanged since 2026-07-20)

## Fixed Issues

None. Both fixes from the 2026-07-20 audit (`dependabot.yml` cooldown, `ci.yml` shell-injection var) remain in place — verified present after today's upstream merge.

## Unresolved Issues

| # | Component | Advisory | Reason |
|---|-----------|----------|--------|
| 1 | `.github/workflows/cd.yml` (×4), `ci.yml`, `release.yml` | semgrep `github-actions-mutable-action-tag` | Actions referenced by mutable tag (`@v4`/`@v3`) instead of pinned 40-char commit SHA. Pinning to a wrong SHA silently breaks CI/CD — needs fetching+verifying the current SHA per action from each action's repo. Left for manual PR review, not an unattended edit. |
| 2 | `.github/workflows/cd.yml` (×2) | semgrep `secrets-inherit` | `secrets: inherit` passed to reusable workflow calls. Replacing with an explicit `secrets:` map requires knowing exactly which secrets the callee consumes — under-scoping would silently break the release pipeline. Needs manual verification against the callee's `on.workflow_call.secrets` block. |
| 3 | `src/cmds/dotnet/dotnet_cmd.rs`, `src/cmds/git/git.rs`, `src/core/tracking.rs`, `src/hooks/hook_cmd.rs` | semgrep `rust.lang.security.temp-dir` | `std::env::temp_dir()` used only for test fixtures / non-privileged scratch files (test DB paths, dotnet test-result staging). Not a security-decision path. Reviewed false positive, no code change. |
| 4 | `src/core/args_utils.rs`, `src/main.rs` | semgrep `rust.lang.security.args` | `std::env::args()` used for RTK's actual purpose — CLI argument parsing/routing — not for authorization decisions. Reviewed false positive. |
| 5 | `src/core/telemetry.rs`, `src/discover/registry.rs` | semgrep `rust.lang.security.current-exe` | `std::env::current_exe()` used for telemetry opt-out / hook self-install path resolution, not a security boundary. Reviewed false positive. |
| 6 | `src/main.rs:1506, 2514` | semgrep `rust.lang.security.unsafe-usage` | `unsafe { libc::signal(...) }` for SIGPIPE/SIGINT/SIGTERM handler registration — standard, narrowly-scoped `libc` FFI pattern for a single-threaded CLI, no memory-safety issue. Reviewed false positive, no code change. |

## Reviewed False Positives (no action needed)

| Tool | Finding | Location | Why it's benign |
|------|---------|----------|------------------|
| gitleaks | 36 `stripe-access-token`/`generic-api-key` matches | `Security reports/*.md` (historical audit docs) | Prior audit reports documenting past false-positive findings, not live keys |
| gitleaks | 5 `generic-api-key` matches | `src/cmds/cloud/aws_cmd.rs` (`#[test]` fns) | Synthetic CloudWatch pagination-token fixture strings (`f/abcdef123...`) used in unit tests |
| gitleaks | 2 `generic-api-key` matches | `scripts/benchmark/cloud-init.yaml` | Benchmark fixture, no live credentials |
| gitleaks | 1 `stripe-access-token` match | `SECURITY.md` | Documentation example of a bad pattern to avoid |
| gitleaks | 1 `stripe-access-token` match | `Security reports/2026-04-16-security-report.md` | Historical audit doc, same as above |
| trufflehog | 0 verified/unverified secrets | — | Confirms all gitleaks hits above are placeholder/fixture strings, not real credentials |

## Raw Scanner Output

**Scope**
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    36d36d0 (post-merge, pre-audit)
```

| Tool | Result |
|------|--------|
| gitleaks (git history, 1194 commits) | 36 leaks detected, all reviewed as false positives (see table above) |
| trufflehog (git, full) | 0 verified secrets, 0 unverified secrets |
| trivy fs | 0 vulnerabilities, 0 misconfigs, 0 secrets across all scanned lockfiles (Cargo.lock incl. worktree copies, fixture pom.xml files) |
| osv-scanner (Cargo.lock) | 0 issues, 203 packages scanned |
| semgrep (`p/rust`, `p/owasp-top-ten`, `p/secrets`) | 49 raw findings across 383 files / 341 rules — 6 unique advisories after de-duplication (all unresolved/false-positive, see table above); 0 secrets findings |
| bandit | Skipped — no `.py` source files (only test/CI fixture scripts) |
| CodeQL | Not applicable — no `.github/workflows/codeql.yml` in this repo |
| mcps-audit | No MCP manifest files found in scanned scope |

## Build Failure

`cargo install --path . --force` failed: no `cargo`/`rustup` toolchain present on `PATH` in this sandbox execution environment (`~/.cargo/bin` does not exist, `which rustup` empty). Environment gap, not a code regression — same as 2026-07-20. Binary not rebuilt/reinstalled this run; existing `$HOME/.local/bin/rtk` / `$HOME/.cargo/bin/rtk` left untouched.

```
rtk: Failed to run cargo install: Failed to spawn process: No such file or directory (os error 2)
```
