# Security Audit — 2026-08-29

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

## Fixed Issues
None.

## Unresolved Issues
None.

## Scan Detail

**Scope:** `/Users/mp3wizard/Public/Claude Proxy/rtk` (git HEAD `34787e5` pre-scan, post-merge of 2 upstream commits)

| Tool | Result | Notes |
|------|--------|-------|
| Gitleaks | 38 raw hits, 0 real | All in `Security reports/*.md` (past audit reports), `SECURITY.md`, test fixtures (`tests/guard_integration_test.rs`, `src/cmds/cloud/aws_cmd.rs`), and `scripts/benchmark/cloud-init.yaml` — confirmed fake/test tokens (`sk_live_FAKE...`, `hunter2`, pagination placeholders), not live credentials |
| TruffleHog (verified) | 128 raw hits, 0 real | All "Lob" detector false-positives matching Rust test function names (e.g. `test_match_output_invalid_regex_rejected`) |
| OSV-Scanner (Cargo.lock, 208 packages) | 0 | No known vulnerabilities |
| Trivy fs (deps + secrets) | 0 | Clean |
| Semgrep p/secrets (52 rules, 406 files) | 0 | Clean |
| Semgrep p/owasp-top-ten (266 rules, non-Rust files) | 0 | Clean — Rust isn't covered by this ruleset; no Python/JS/TS/Java/Go/Ruby vulnerable patterns found in the files that exist |
| config-audit (Claude config layer) | Informational only | Medium-severity hits are `CLAUDE.md`'s own documented "avoid rabbit holes" guidance (pattern-matched as "skip verification" / "trust-all" language) — not prompt injection, just this repo's existing dev-workflow rule. Low-severity hits are routine hook-config listings across installed plugins |
| mcps-audit / skill-audit / skillspector | N/A | No `.skill`, `SKILL.md`, or MCP manifest files in this repo |

**Raw scanner output:** archived at `/tmp/rtk-secscan/` (gitleaks.json, trufflehog.txt, osv.txt, trivy.txt, semgrep-secrets.txt, semgrep-owasp.txt, config-audit.txt) and APTS audit log `/tmp/css-scan-20260829T020847Z.jsonl` (8 tool runs, all measured).
