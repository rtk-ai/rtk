# Security Audit — 2026-05-11

## Summary
- Issues found: 17 | Auto-fixed: 0 | Unresolved: 17 (all pre-existing or non-actionable)
- Status: PASSED (no new exploitable findings introduced by upstream sync)
- Upstream sync: 11 commits merged from origin/develop (HEAD 7dbf50f)

## Fixed Issues
None this run — all real fixes were already in upstream (rustls-webpki 0.103.13 bump came in via the merge itself).

## Unresolved Issues

| # | Component | Advisory / Rule | Reason |
|---|-----------|-----------------|--------|
| 1 | `rustls-webpki 0.103.12` in 13 stale `.claude/worktrees/*/Cargo.lock` | GHSA-82j2-j2ch-gfr8 / RUSTSEC-2026-0104 (CVSS 7.5, HIGH) | Main `Cargo.lock` already at 0.103.13 (patched via upstream merge). Findings are in detached agent worktrees that are not part of the shipping build. Cleanup-only. |
| 2 | `.github/workflows/ci.yml:25` | yaml.github-actions.security.run-shell-injection | Pre-existing upstream pattern (`${{ github.base_ref }}` interpolated into `run:` block). Fixing would diverge from origin/develop and re-conflict on every sync. Risk is low — `base_ref` is a PR branch name controlled by GitHub. |
| 3-5 | `.github/workflows/release.yml:185,228,262` | yaml.github-actions.security.run-shell-injection | Same as above. Inputs are `${{ inputs.tag }}` / `${{ github.event.release.tag_name }}`, set by repo maintainers when cutting a release. Pre-existing upstream. |
| 6-17 | Gitleaks 12 hits | stripe-access-token / generic-api-key | All in known non-secret locations: prior audit reports under `Security reports/`, `SECURITY.md` example, AWS test fixtures in `src/cmds/cloud/aws_cmd.rs`, benchmark `cloud-init.yaml`. No live credentials. Same set as prior runs. |

## Scanner Results

| Tool | Findings | Notes |
|------|---------:|-------|
| Gitleaks | 12 | All pre-existing false positives in audit reports / test fixtures |
| Semgrep (owasp-top-ten + secrets) | 4 | GitHub Actions shell-injection, upstream-owned workflows |
| Trivy | 13 (deduped to 1 unique advisory) | Only stale worktree lockfiles; main Cargo.lock clean |
| OSV-Scanner | 13 (same advisory) | Same as Trivy |
| TruffleHog | 0 verified, 0 unverified | Clean — scanned 2.7 GB, 221k chunks |
| Bandit | N/A | No Python sources |

## Raw Scanner Output

### Gitleaks
12 leaks across known fixture/doc locations:
- `Security reports/security-audit-2026-04-19.md:17` (stripe-access-token in prior report)
- `Security reports/security-audit-2026-04-18.md:36,39`
- `Security reports/2026-04-16-security-report.md:64`
- `scripts/benchmark/cloud-init.yaml:613, 282`
- `src/cmds/cloud/aws_cmd.rs:1878, 1879, 1923 (x2), 2035` (AWS test fixtures)
- `SECURITY.md:151` (documentation example)

### Semgrep
```
.github/workflows/ci.yml:25       run-shell-injection
.github/workflows/release.yml:185 run-shell-injection
.github/workflows/release.yml:228 run-shell-injection
.github/workflows/release.yml:262 run-shell-injection
```

### Trivy + OSV
Single unique advisory `GHSA-82j2-j2ch-gfr8` (rustls-webpki DoS via malformed CRL BIT STRING).
- Installed in stale worktrees: 0.103.12
- Fixed: 0.103.13
- **Main repo `Cargo.lock`: 0.103.13 (already patched)**

### TruffleHog
`{"verified_secrets": 0, "unverified_secrets": 0, "scan_duration": "1m16s"}`

### APTS Audit Log
- Log: `/tmp/css-scan-20260511T020857Z.jsonl`
- Tool runs recorded: 5
- Standard: OWASP APTS § Auditability
