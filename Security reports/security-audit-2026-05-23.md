# Security Audit — 2026-05-23

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

All scanner findings were false positives (placeholder values in test fixtures/docs) or affected the user's global Claude Code configuration — not the RTK codebase itself.

## Fixed Issues
None.

## Findings Detail

### Gitleaks — 13 findings (all false positives)
All 13 detections are placeholder/documentation values committed across historical commits:
- `msk-1234567890abcdef` — generic API key placeholder in benchmark scripts and prior audit reports
- `msk_live_1234567890abcdef` — Stripe placeholder in SECURITY.md code examples and prior audit reports
- `f/1234567890abcdef...` / `b/1234567890abcdef...` — AWS CloudWatch pagination tokens in test fixtures (`src/cmds/cloud/aws_cmd.rs`)
- `secret-api-key-12345` — clearly named placeholder in test data

None are real credentials. All are in documentation, test fixtures, or the Security reports directory.

### TruffleHog — 0 findings
0 verified secrets, 0 unverified secrets across 11,647 chunks scanned.

### Trivy — Main Cargo.lock: 0 issues
- Main `Cargo.lock` (203 packages): **No vulnerabilities**
- `rustls-webpki` is at **0.103.13** (fixed version — GHSA-82j2-j2ch-gfr8 affected 0.103.12)
- Note: Several stale worktree `Cargo.lock` files in `.claude/worktrees/` showed `rustls-webpki 0.103.12` (HIGH — GHSA-82j2-j2ch-gfr8). These are ephemeral development environments, not the main codebase. The vulnerability is already fixed upstream and in the main lock file.

### OSV-Scanner — 0 issues
Scanned 203 packages from `Cargo.lock`. No vulnerabilities found.

### Semgrep — 0 findings
- Secrets scan: 0 findings across 334 files (45 rules)
- OWASP Top-10: 0 findings (no Python/JS/TS files in scope; Rust SAST skipped as p/owasp-top-ten has no Rust rules)

### security-audit (config-audit.py) — 43 issues (all out of scope)
All findings affect the user's **global** Claude Code configuration (`~/.claude/settings.json`, installed skills, and plugins) — not the RTK codebase:
- CRITICAL ×5: Security-scanner and skill-security-auditor skills scanning themselves (false positives — the scanner detecting patterns in its own source code)
- HIGH ×7: cc-beeper hooks posting to `localhost` (legitimate local notification tool, not external exfiltration)
- HIGH ×2: `CLAUDE.md` referencing `curl` in code examples within the documentation
- MEDIUM/LOW: Broad hook matchers, `skipDangerousModePermissionPrompt: true`, and benign guidance text in `CLAUDE.md`

None of these findings represent vulnerabilities in the RTK Rust code or its dependencies.

### mcp-exfil-scan — 11 issues (all out of scope)
All findings affect global Claude skills (`impeccable`, `security-audit`, `skill-security-auditor`, `atlas-cloud`, `playwright-cli`, `pyright`, `vtsls`) — not the RTK project files.

## Upstream Changes Merged (4 commits)
- `8564ddc` Merge pull request #2040 from rtk-ai/fix/review-1879-git-status-doc-test
- `6e6efd4` docs(git): sync status README with --porcelain -b (drop -uall)
- `06476d1` Merge pull request #2035 from rtk-ai/fix/git-status-uall
- `7753e48` fix(git): drop -uall from compact status so output never exceeds raw

Changes: `src/cmds/git/README.md` and `src/cmds/git/git.rs` (2 files, 5 insertions/5 deletions)

## Raw Scanner Output

```
Gitleaks: 13 findings (all false positives — placeholder values)
TruffleHog: 0 verified/unverified secrets
Trivy (main Cargo.lock): 0 vulnerabilities (203 packages)
OSV-Scanner: 0 issues
Semgrep secrets: 0 findings (334 files, 45 rules)
security-audit: 43 issues (global config — not RTK codebase)
mcp-exfil-scan: 11 issues (global skills — not RTK codebase)
```

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260523T020930Z.jsonl`
- **Tool runs recorded:** 9
- **Standard:** OWASP APTS § Auditability
