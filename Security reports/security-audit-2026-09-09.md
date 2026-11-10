# Security Audit — 2026-09-09

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

Merged 188 upstream commits (`origin/develop` → local `master`, merge commit `d11a3e6`) since the last audit (2026-09-02). One merge conflict in `CHANGELOG.md`, resolved by taking upstream's version per policy.

## Fixed Issues
(none — no vulnerabilities found)

## Unresolved Issues
(none)

## Raw Scanner Output

### gitleaks (secrets)
144 findings, all confirmed false positives — test fixtures and documentation examples of fake/placeholder credentials (a stub `sk_live_FAKE...` Stripe key, a stub `secret-api-key-...` value, AWS CLI output mocks) in `src/cmds/cloud/aws_cmd.rs` and `tests/guard_integration_test.rs`. No real secrets present.

### trufflehog (secrets, filesystem)
```
finished scanning: chunks=694623 bytes=8919521879 verified_secrets=0 unverified_secrets=281
```
All 281 unverified hits are inside `target/release/deps/*.rlib` (compiled `url` crate test fixtures containing RFC example credentials like `user:password@example.com`). 0 verified secrets. No action needed — these are build artifacts, not source.

### semgrep (SAST)
```
Ran 51 rules on 210 files: 0 findings.
```

### osv-scanner (dependency vulnerabilities, Cargo.lock)
```
Scanned Cargo.lock file and found 208 packages
No issues found
```

### trivy (filesystem: vuln/secret/misconfig)
```
Cargo.lock (cargo): 0 vulnerabilities
tests/fixtures/**/pom.xml (pom): 0 vulnerabilities [test fixtures, not real deps]
tests/fixtures/oc_pods.json (kubernetes): 0 misconfigurations [test fixture]
```

### cargo-audit
Not installed on this host (not in the core tool list); skipped. RUSTSEC advisory coverage for `Cargo.lock` was still obtained via `osv-scanner`, which found 0 issues.

### Skipped (opt-in privacy gates, no user present to confirm)
`mcp-scan` and `skillspector` LLM-mode checks require interactive opt-in per the security-scanner skill's privacy gate. This is a non-interactive scheduled run, so they were skipped rather than silently run — no security-relevant MCP/skill config changes were introduced by this merge.
