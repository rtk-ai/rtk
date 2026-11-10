# Security Audit — 2026-06-02

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

All scanner findings were false positives:
- Gitleaks flagged 21 items — all test fixture strings, security-report archives, and documentation examples (no real credentials)
- OSV confirmed `rustls-webpki` is already at 0.103.13 (fixed version) in the main Cargo.lock
- Semgrep (OWASP + secrets), TruffleHog returned 0 findings
- Config-audit / mcp-exfil-scan findings are from global Claude Code settings (cc-beeper notification hooks) and security-scanner tool metadata, not the RTK codebase

## Upstream Sync
12 new commits merged from `origin/develop`:
- `4c278d1` Merge pull request #2002 (MSRV 1.91)
- `6873764` Merge pull request #2188 (go build pattern errors)
- `f69ad6e` fix(go): respect build failure exit status
- `4128572` fix(gh): show fallback note when PR/issue body is filtered to empty
- `f1474b4` fix(init): use fs::canonicalize for symlink resolution
- `508192d` fix(init): preserve settings.json symlink during atomic write
- `83cd93e` chore(args): introduce a more generic solution for restoring double dashes
- `653a209` Merge pull request #2172 (encode_project_path)
- `e9f42e5` Merge pull request #2173 (supported-agents-fixes)
- `67a5958` doc(init): fix documentation inconsistencies
- `5989ac9` fix(provider): sanitize more chars when encoding claude code project paths
- `6f4519a` chore(cargo): declare MSRV via rust-version = "1.91"

## Fixed Issues
None — no real vulnerabilities found.

## Unresolved Issues
None.

## Raw Scanner Output

### Gitleaks (21 findings — all false positives)
```
1055 commits scanned | 7.92 MB | leaks found: 21

All findings confirmed false positives:
- Security reports/security-audit-*.md — archived scan reports containing [REDACTED] example patterns
- src/cmds/cloud/aws_cmd.rs:1878-2035 — test assertion code (pagination token strings, not credentials)
- scripts/benchmark/cloud-init.yaml:282,613 — benchmark setup scripts with placeholder values
- SECURITY.md:151 — documentation example (stripe-access-token pattern reference)
```

### Semgrep OWASP
```
Rules run: 266 on 12 files — 0 findings
```

### Semgrep Secrets
```
Rules run: 45 on 343 files — 0 findings
```

### TruffleHog
```
Chunks: 12035 | Bytes: 8,577,733
Verified secrets: 0 | Unverified secrets: 0
```

### OSV-Scanner (main Cargo.lock)
```
203 packages scanned — No issues found
rustls-webpki: 0.103.13 (already at fixed version for RUSTSEC-2026-0104)
```

### Trivy
```
SKIPPED — Docker credential issue prevented vulnerability DB download
(docker-credential-desktop not in PATH)
```

### Config-audit
```
43 findings — all from global Claude Code settings (cc-beeper hooks, plugin metadata)
No findings in RTK source code
```

### MCP-Exfil-Scan
```
11 findings — all from security-scanner/skill-security-auditor tool metadata (self-referential false positives)
RTK codebase: clean
```

## APTS Audit Log
- **Standard:** OWASP APTS § Auditability
- **Git HEAD:** 7fa61ee (post-merge)
- **Tools run:** gitleaks, semgrep (owasp + secrets), trufflehog, osv-scanner, config-audit, mcp-exfil-scan
- **Trivy:** skipped (Docker credential issue)
- **Bandit/CodeQL:** skipped (no Python/GitHub Actions CodeQL workflow in project)
