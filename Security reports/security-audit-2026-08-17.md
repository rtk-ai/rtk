# Security Audit — 2026-08-17

## Summary
- Issues found: 0 real | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

All scanner hits this run were reviewed and confirmed **false positives** or **out of scope** for the rtk codebase (test fixtures with fake tokens, known detector noise, low-severity subprocess linting on already-safe list-form calls, and Claude Code harness/plugin config findings outside `/Users/mp3wizard/Public/Claude Proxy/rtk`). No dependency vulnerabilities (OSV, Trivy) and no real secrets. No code changes required.

## Fixed Issues
None — no real vulnerabilities found.

## Reviewed / False-Positive Findings

| # | Tool | Component | Finding | Disposition |
|---|------|-----------|---------|-------------|
| 1 | Gitleaks | `tests/guard_integration_test.rs`, `Security reports/*.md`, `SECURITY.md`, `scripts/benchmark/cloud-init.yaml`, `src/cmds/cloud/aws_cmd.rs` | 38 "leaks" (stripe-access-token, generic-api-key) | False positive — all are fake/example tokens in test fixtures (`sk_live_FAKE...`), historical audit-report docs, and AWS-filter test data (pagination tokens, redaction test cases). No live credentials. |
| 2 | TruffleHog | `src/integrity.rs`, `src/git.rs`, `src/toml_filter.rs`, `src/learn/detector.rs` | 120 "verified" secrets, `Detector Type: Lob` | False positive — known Lob-detector noise matching Rust test function names (e.g. `test_store_hash_creates_sha256sum_format`) as API keys. Not credentials. |
| 3 | Bandit | `hooks/hermes/tests/test_rtk_rewrite_plugin.py`, `scripts/benchmark-sessions/lib/runner.py` | 9 Low-severity B404/B603/B607 subprocess findings | Reviewed — all use list-form `subprocess.run([...])` with fixed argv, no `shell=True`, no untrusted input concatenation. Standard low-severity subprocess-usage linting, not exploitable as written. |
| 4 | config-audit | `CLAUDE.md` (project) | HIGH "curl to external URL", MEDIUM "skip verification" / "trust-all" instruction | False positive — matches documentation prose (an example `curl https://api.example.com/data` in the `rtk proxy` usage docs) and the repo's own "Avoiding Rabbit Holes" testing-scope guidance, not an injected/malicious instruction. Note: `claude.md` is the same file as `CLAUDE.md`, double-counted by the scanner on this case-insensitive filesystem. |
| 5 | Semgrep (secrets, p/secrets) | — | 0 findings | Clean. |
| 6 | OSV-Scanner | `Cargo.lock` (208 packages) | 0 findings | Clean — no known vulnerable Rust dependencies. |
| 7 | Trivy (fs) | `Cargo.lock`, `tests/fixtures/**/pom.xml` | 0 vulnerabilities | Clean. |

## Unresolved Issues
None.

## Coverage Disclosure

| Tool | Ran? | Version | Files covered | Skipped reason |
|------|------|---------|---------------|-----------------|
| Gitleaks | OK | 8.30.1 | full git history (1345 commits, ~9.1MB) | — |
| Bandit | OK | 1.9.4 | `hooks/hermes/`, `scripts/benchmark-sessions/` (4 `.py` files) | — |
| Semgrep (secrets) | OK | (metrics off) | 402 tracked files | OWASP-Top-Ten config skipped — no Python/JS/TS/Java/Go/Ruby app code in scope beyond the 4 Python scripts already covered by Bandit; rtk itself is Rust, which p/owasp-top-ten and p/python/p/typescript don't cover |
| Trivy | OK | 0.72.0 | `Cargo.lock`, fixture `pom.xml` files | version 0.72.0, not in the known-compromised 0.69.4–0.69.6 range |
| TruffleHog | OK | 3.95.9 | full git history (14285 chunks, ~10.2MB) | — |
| CodeQL | SKIPPED | — | — | no `.github/workflows/codeql.yml` in this repo |
| mcps-audit | N/A | — | — | no MCP manifest files (`*.skill`, `SKILL.md`, `mcp*.json`) inside the rtk repo itself |
| OSV-Scanner | OK | 2.4.0 | `Cargo.lock` (208 pkgs) + fixture poms | — |
| mcp-scan | SKIPPED (opt-in) | — | — | sends data to invariantlabs.ai; no user present to consent during this autonomous scheduled run |
| security-audit (config-audit.py) | OK | — | global `~/.claude` config + project `CLAUDE.md`/`claude.md` | scanner also covers global harness/plugin config outside the rtk repo scope by design — those findings are informational only and not part of this repo's fix surface |
| skill-security-auditor | N/A | — | — | no `SKILL.md`/`.skill` files in rtk repo |
| mcp-exfil-scan | N/A | — | — | no MCP artifacts in rtk repo |
| skillspector | SKIPPED (opt-in, LLM mode) | — | — | no AI-skill artifacts in rtk repo; not applicable |

## Cross-Tool Observations
Gitleaks and TruffleHog both flagged the same class of content (test fixtures / fake tokens) independently — consistent false-positive signal, not corroborating evidence of a real leak. No cross-tool overlap on any genuine finding.

## Coverage Gaps
- Business logic and IDOR-style flaws not covered (static tools only).
- Runtime/dynamic behavior not exercised — this was a static-only pass.
- Global Claude Code harness/plugin findings (CRITICAL/HIGH counts from config-audit against `~/.claude/**`) are outside rtk's fix surface and were not triaged item-by-item here; they belong to a harness-level security review, not this repo's audit.

## Raw Scanner Output

### Gitleaks
```
9:31AM INF 1345 commits scanned.
9:31AM INF scanned ~9137013 bytes (9.14 MB) in 1.13s
9:31AM WRN leaks found: 38
```
(38 leak records — file/line list above in "Reviewed / False-Positive Findings"; full JSON at `/tmp/gitleaks-report.json`, not embedded here to avoid reproducing fake-token strings verbatim.)

### OSV-Scanner
```
Scanning dir /Users/mp3wizard/Public/Claude Proxy/rtk
Scanned Cargo.lock file and found 208 packages
No issues found
```

### Trivy
```
Cargo.lock                                            cargo   0 vulnerabilities
tests/fixtures/multi-module-skeleton/child-a/pom.xml  pom     0
tests/fixtures/multi-module-skeleton/child-b/pom.xml  pom     0
tests/fixtures/multi-module-skeleton/pom.xml          pom     0
tests/fixtures/multifail-skeleton/pom.xml             pom     0
```

### TruffleHog
```
finished scanning: chunks=14285 bytes=10209921 verified_secrets=120 unverified_secrets=0
scan_duration=1.54s
```
(All 120 "verified" hits are `Detector Type: Lob` matching Rust test function name strings — see disposition above.)

### Semgrep (p/secrets)
```
Findings: 0 (0 blocking)
Rules run: 45
Targets scanned: 402
```

### Bandit
```
Total issues (by severity): Low: 9, Medium: 0, High: 0
Total issues (by confidence): High: 9
Locations: hooks/hermes/tests/test_rtk_rewrite_plugin.py:318, scripts/benchmark-sessions/lib/runner.py:5,28 (x2)
```

### config-audit (Claude Code config)
```
Found 99 issue(s) across global harness + project scope:
  CRITICAL: 7   (all outside rtk repo — global plugin/skill scripts)
  HIGH: 16      (14 outside rtk repo; 2 = rtk's own CLAUDE.md/claude.md, both false positive)
  MEDIUM: 63    (61 outside rtk repo; 4 = rtk's own CLAUDE.md/claude.md, both false positive)
  LOW: 13       (all outside rtk repo — global hooks-configured notices)
```

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260817T023145Z.jsonl`
- **Tool runs recorded:** 7 (measured: 7, asserted: 0)
- **Standard:** OWASP APTS § Auditability

### Build Failure
N/A — see main sync log for build status (STEP 5).
