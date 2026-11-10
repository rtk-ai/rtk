# Security Audit — 2026-07-22

## Summary
- Issues found: 3 categories (36 gitleaks hits, 9 bandit findings, 37 semgrep findings) | Auto-fixed: 1 (secrets:inherit, 2 call sites) | Unresolved: 2 categories (noted below, all low-severity/non-exploitable)
- Status: ISSUES FIXED — ⚠️ build step failed (no Rust toolchain in this environment; see Build Failure)

## Fixed Issues

| # | Component | Advisory | Change |
|---|-----------|----------|--------|
| 1 | `.github/workflows/cd.yml` (2 call sites: `build-prerelease`, `build-release`) | semgrep `github-actions-secrets-inherit` (least privilege) | Replaced `secrets: inherit` with an explicit map of only the 5 secrets `release.yml` actually consumes (`RTK_TELEMETRY_TOKEN`, `APP_CLIENT_ID`, `APP_PRIVATE_KEY`, `RTK_DISCORD_RELEASE`, `HOMEBREW_TAP_TOKEN`), verified by grepping `release.yml` for `secrets.*` usage first |

## Unresolved Issues

- **GitHub Actions mutable tag references (semgrep, ~25+ locations across `.github/workflows/ci.yml` and `release.yml`)**: steps reference `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `actions/upload-artifact@v4`, `actions/download-artifact@v4`, `actions/setup-go@v5`, `actions/create-github-app-token@v3`, `softprops/action-gh-release@v2` by mutable tag instead of pinned 40-char commit SHA. No safe automated fix: pinning correctly requires looking up and verifying the exact current commit SHA for each action version from its upstream repo — guessing a SHA blind risks either breaking CI or pinning to a stale/wrong commit. Recommend the maintainer either pin manually (`gh` + upstream repo verification) or adopt Dependabot/Renovate with SHA-pinning enabled.
- **Bandit subprocess findings (9, all in `hooks/hermes/tests/test_rtk_rewrite_plugin.py` and `scripts/benchmark-sessions/lib/runner.py`)**: B404/B603/B607 flags on `subprocess.run(...)`. Reviewed manually — all calls use fixed argv lists (no `shell=True`, no string concatenation of untrusted input), so not exploitable as command injection. No code change made; flagged low-severity/false-positive by design.

## Raw Scanner Output

### Gitleaks — secrets in git history + filesystem
**Summary:** 36 leaks found, all confirmed false positives.
```
9:09AM INF 1202 commits scanned.
9:09AM INF scanned ~8415289 bytes (8.42 MB) in 985ms
9:09AM WRN leaks found: 36
```
All 36 hits are synthetic placeholder values (Stripe/generic-api-key patterns of the form `[REDACTED-PLACEHOLDER]`, plus AWS-style fixture strings) living in: past `Security reports/security-audit-*.md` files (this task's own prior output, quoting example secret formats), `scripts/benchmark/cloud-init.yaml` (fixture), and `src/cmds/cloud/aws_cmd.rs` (test fixtures for the AWS credential-redaction filter itself). TruffleHog (below), which live-verifies against real provider APIs, found **0** verified and **0** unverified matches on the same repo — corroborating these are inert placeholders, not live credentials. No fix needed.

### Bandit — Python SAST
**Summary:** 9 issues, all Low severity / High confidence, all in test/benchmark scripts, none exploitable (see Unresolved above).
```
Location: ./hooks/hermes/tests/test_rtk_rewrite_plugin.py:318
  B603 subprocess call - check for execution of untrusted input (fixed argv, no shell=True)
Location: ./scripts/benchmark-sessions/lib/runner.py:5
  B404 import subprocess
Location: ./scripts/benchmark-sessions/lib/runner.py:28
  B607 starting process with partial path ("tar")
  B603 subprocess call - check for execution of untrusted input

Run metrics:
  Total issues: Low 9, Medium 0, High 0
  Confidence: High 9
```

### Semgrep — Multi-language SAST (OWASP Top 10 + Python)
**Summary:** 37 findings, 300 rules run across 346 files. All findings are GitHub Actions workflow hygiene (mutable tags, one `secrets: inherit`) — no application-code (Rust) findings.
```
Findings: 37 (37 blocking)
Rules run: 300
Targets scanned: 346

- yaml.github-actions.security.github-actions-mutable-action-tag.github-actions-mutable-action-tag
  (ci.yml, release.yml — actions/checkout@v4, dtolnay/rust-toolchain@stable,
   Swatinem/rust-cache@v2, actions/upload-artifact@v4, actions/download-artifact@v4,
   actions/setup-go@v5, actions/create-github-app-token@v3, softprops/action-gh-release@v2)
- yaml.github-actions.security.secrets-inherit (or equivalent) — cd.yml:94, cd.yml:131
  [FIXED — see Fixed Issues above]
```

### Trivy — Dependencies + secrets + IaC
**Summary:** 0 vulnerabilities, 0 secrets. Clean.
```
Target                                                  Type    Vulnerabilities  Secrets
Cargo.lock                                              cargo   0                -
tests/fixtures/multi-module-skeleton/child-a/pom.xml    pom     0                -
tests/fixtures/multi-module-skeleton/child-b/pom.xml    pom     0                -
tests/fixtures/multi-module-skeleton/pom.xml            pom     0                -
tests/fixtures/multifail-skeleton/pom.xml               pom     0                -
```

### TruffleHog — Secrets in git history (live-verified)
**CONFIDENTIAL: secrets tool** — no findings to redact.
**Summary:** 0 verified, 0 unverified secrets across full git history (1202 commits, 12659 chunks, 9.4MB scanned).
```
{"msg":"finished scanning","chunks":12659,"bytes":9410109,"verified_secrets":0,"unverified_secrets":0,"scan_duration":"2.66s"}
```

## Cross-Tool Observations
Gitleaks' 36 hits and TruffleHog's 0 hits overlap on the same commit range — TruffleHog's live-verification step is the higher-confidence signal here and confirms none of gitleaks' matches are real credentials. Semgrep and Bandit findings don't overlap (different languages/surfaces); no finding was independently corroborated by two tools as a real vulnerability.

## Coverage Gaps
- CodeQL: skipped (not run this cycle — would require a live GitHub Actions dispatch via `gh run list --workflow codeql.yml`, out of scope for this autonomous pass).
- mcps-audit: skipped — no `.skill`/`SKILL.md`/MCP config files in this repo (rtk is a standalone CLI, not a Claude plugin/skill package).
- Not covered by any tool that ran: business logic correctness, IDOR/authz (N/A — no server component), runtime/dynamic behavior, and the new `src/cmds/scala/sbt_cmd.rs` module merged from upstream today (would normally be covered by `cargo test`/`clippy` in the Step 5 build gate — see Build Failure below, that gate did not run this cycle).

## Build Failure

Step 5 (`cargo install --path . --force`) could not run: no Rust toolchain (`cargo`/`rustup`) is present anywhere on this machine/sandbox — `command -v cargo`, `~/.cargo/bin`, `~/.rustup`, and Homebrew all came up empty. This is an environment gap, not a code issue introduced by today's merge. The binary at `$HOME/.cargo/bin/rtk` and the `$HOME/.local/bin/rtk` wrapper were left unchanged from the last successful build. Cargo/clippy/test verification of the newly-merged `src/cmds/scala/sbt_cmd.rs` module did not run as a result — recommend re-running the full `cargo fmt && cargo clippy --all-targets && cargo test --all` gate manually once a Rust toolchain is available before relying on the new SBT filter in production.
