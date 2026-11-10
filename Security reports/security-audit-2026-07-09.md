# Security Audit — 2026-07-09

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

## Fixed Issues
None — no vulnerabilities found requiring a fix this run.

## Unresolved Issues
None (security-relevant). For completeness, informational-only observations are noted in the Raw Scanner Output section below (heuristic false positives, no code change applicable).

## Raw Scanner Output

### Gitleaks (git history + filesystem)
1163 commits scanned, 36 leaks found — all confirmed false positives (placeholder/example secrets in prior audit report docs under `Security reports/`, `SECURITY.md`, `scripts/benchmark/cloud-init.yaml`, and test fixture data in `src/cmds/cloud/aws_cmd.rs`). Consistent with 2026-07-07/2026-07-08 findings. No real credentials.

### TruffleHog (git, live verification)
0 verified secrets, 0 unverified secrets. Clean.

### Trivy (filesystem, vuln + secret scanners)
Root `Cargo.lock`: 0 vulnerabilities, 0 secrets. (Sibling `.claude/worktrees/*/Cargo.lock` files also scanned clean this run — no stale-lockfile vulnerabilities present, unlike 2026-07-08.)

### OSV-Scanner (source scan, root Cargo.lock)
203 packages scanned. No issues found.

### Semgrep (secrets, `p/secrets` config)
346 files scanned, 45 rules run. 0 findings.

### Semgrep (OWASP Top Ten, non-Rust files: py/js/ts/jsx/tsx/java/go/rb)
12 files scanned, 266 rules run. 0 findings.

### config-audit (Claude Code configuration)
No CRITICAL/HIGH findings. MEDIUM: broad `SessionStart`/`UserPromptSubmit` hook matchers in mode-activation plugins (caveman, pordee, openai-codex, addy-agent-skills, claude-plugins-official) — expected, not project code. `CLAUDE.md`/`claude.md` "avoid rabbit holes" guidance flagged as a generic trust-all/skip-verification pattern — false positive, intentional scope-discipline instruction (consistent with prior audits). LOW: hook configuration present in several plugins (informational only).

### Skill-audit (`.claude/skills/*/SKILL.md`, project-local workflow skills)
13 skill files found and scanned (not flagged in prior runs). Heuristic verdicts: 6 LOW, 4 MEDIUM, 2 HIGH/CRITICAL (`security-guardian`, `ship`, `performance`) driven by generic pattern matches (file-write + network-URL + credential-reference keyword combinations) in these internal workflow-automation skill docs, not actual malicious code or exploitable vulnerabilities — these are first-party skills authored for this repo's own dev workflow, not third-party/untrusted content. No code change applicable; flagged for informational awareness only.

### Bundled-script integrity check
```
config-audit.py: OK
skill-audit.sh: OK
apts-audit.sh: OK
```

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260709T020848Z.jsonl`
- **Standard:** OWASP APTS § Auditability
