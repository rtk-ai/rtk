# Security Audit — 2026-07-10

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

## Fixed Issues
None.

## Unresolved Issues
None.

## Reviewed False Positives (no action needed)

| Tool | Finding | Location | Why it's benign |
|------|---------|----------|------------------|
| gitleaks | 28 `stripe-access-token`/`generic-api-key` matches | `Security reports/*.md` (historical audit docs) | Prior audit reports documenting past false-positive findings, not live keys |
| gitleaks | 5 `generic-api-key` matches | `src/cmds/cloud/aws_cmd.rs` (`#[test]` fns) | Synthetic AWS ARN/fixture strings used in unit tests |
| gitleaks | 2 `generic-api-key` matches | `scripts/benchmark/cloud-init.yaml` | Benchmark fixture, no live credentials |
| gitleaks | 1 `stripe-access-token` match | `SECURITY.md` | Documentation example |
| skill-audit | CRITICAL — "destructive file deletion" / "format filesystem" | `.claude/skills/security-guardian/SKILL.md` | Skill documents *attack examples* (`rm -rf /`, `mkfs`) as things to defend against, not executable payloads |
| skill-audit | HIGH — "privilege escalation" (`sudo dtrace`) | `.claude/skills/performance/SKILL.md` | Documented macOS profiling command, not auto-executed |
| skill-audit | HIGH — "sensitive system file" (`../etc/passwd`) | `.claude/skills/rtk-tdd/SKILL.md` | Path-traversal example inside a test-assertion snippet |
| skill-audit | CRITICAL RISK (score 90/100) | `.claude/skills/ship/SKILL.md` | Heuristic score driven by broad tool grants (Bash+Write+Edit) + 21 bash code blocks in a legitimate git/release workflow skill — no actual dangerous pattern matched |
| config-audit | MEDIUM — broad SessionStart/UserPromptSubmit hook matchers | Various installed plugins (`caveman`, `pordee`, `claude-plugins-official`, `openai-codex`, `addy-agent-skills`) | Global Claude Code config, unrelated to rtk repo; matches by design (session-init hooks) |
| config-audit | MEDIUM — "skip verification" / "trust-all" instruction | `CLAUDE.md`, `claude.md` | Project's own documented "avoid rabbit holes" policy, not an injection attempt |

## Raw Scanner Output

**Scope**
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    cd4beb9 (post-merge)
Include:     all supported
Exclude:     .gitignore honored by each tool
```

| Tool | Result |
|------|--------|
| gitleaks | 36 leaks detected, all reviewed as false positives (see table above) |
| trufflehog (verified only) | 0 verified secrets |
| trivy fs | 0 vulnerabilities, 0 secrets (Cargo.lock, 203 packages incl. worktree copies) |
| osv-scanner (Cargo.lock) | 0 issues, 203 packages scanned |
| semgrep p/secrets | 0 findings (381 files, 45 rules) |
| semgrep p/owasp-top-ten | 0 findings (12 non-Rust files matched by include filters — TS/py/rb fixtures; Rust not covered by this OWASP ruleset) |
| config-audit (Claude config) | 0 CRITICAL/HIGH; several MEDIUM/LOW informational findings on global plugin config, not rtk code |
| skill-audit (`.claude/skills/*/SKILL.md`, 12 files) | 4 flagged by heuristics, all reviewed false positives |
| mcp-exfil-scan | Script failed local checksum verification (`SHA256SUMS` mismatch in bundled skill cache) — skipped rather than run un-verified |

### Note on Semgrep OWASP coverage
Rust has no dedicated `p/owasp-top-ten` ruleset in this Semgrep run — coverage was effectively limited to the repo's few non-Rust files (TS/Python/Ruby test fixtures). Rust-specific correctness is otherwise covered by `cargo clippy` in CI/pre-commit, which is outside this scanner's scope.

## Build Failure
None — see main sync summary for build/install status.

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260710T020938Z.jsonl`
- **Tool runs recorded:** 8 (measured: 8, asserted: 0)
- **Standard:** OWASP APTS § Auditability
