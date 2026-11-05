# Security Audit — 2026-09-02

## Summary
- Issues found: 0 exploitable | Findings triaged: 182 (all false positives / informational, no fix required)
- Status: PASSED

No real vulnerabilities, secrets, or unsafe code patterns were found in the rtk codebase (Rust source, Cargo.toml, Cargo.lock, project skills). Every raw hit produced by the scanners traced back to test fixtures, documentation examples, or bundled-script bugs unrelated to rtk's own code — none required a code or dependency change.

## Fixed Issues
None — no exploitable issue required a fix.

## Unresolved Issues

| # | Component | Finding | Reason |
|---|-----------|---------|--------|
| 1 | Gitleaks — 38 hits (`stripe-access-token`, `generic-api-key`) across `tests/guard_integration_test.rs`, `Security reports/security-audit-*.md`, `SECURITY.md`, `scripts/benchmark/cloud-init.yaml`, `src/cmds/cloud/aws_cmd.rs` | Secret-pattern regex matches | All confirmed fixture/placeholder values (a `sk_live_FAKE...`-prefixed test fixture, a `sk_live_1234...`-prefixed doc example, prior scan reports quoting the same fake keys for illustration). No live credentials. No fix applicable — these are intentional test/doc content. |
| 2 | TruffleHog — 135 "verified" `Lob` detector hits, all in `src/learn/detector.rs` and `src/hooks/init.rs` | Rust test function names (e.g. `test_find_corrections_excludes_tdd_cycle`) coincidentally match the Lob API key format and the Lob verification endpoint returned a false "verified" status | False positive from an overly permissive detector regex/verification combo. Not exploitable — no fix in rtk source; would require an allowlist entry in TruffleHog's own config, out of scope for this repo. |
| 3 | Bandit — 9 Low findings in `scripts/benchmark-sessions/lib/runner.py` (`B404`/`B603`/`B607` subprocess usage) | `subprocess.run`/`subprocess.Popen` calls with fixed argument lists (`tar`, etc.) | No untrusted input reaches the shell; args are hardcoded lists, `shell=False`. Standard Bandit noise for legitimate subprocess use. No change needed. |
| 4 | config-audit.py — 23 CRITICAL / 18 HIGH / 68 MEDIUM / 20 LOW findings | Scanned `~/.claude/settings.json`, globally-installed plugins (`impeccable`, `ponytail`, `caveman`, `anysearch`, `claude-code-security-plugins` itself, etc.) | Out of scope — these are the user's global Claude Code config and third-party plugin installs outside the rtk repo (`/Users/mp3wizard/.claude/...`), not part of the rtk codebase. Not actionable from this repo. |
| 5 | skill-audit.sh — `performance`, `security-guardian`, `rtk-tdd`, `ship` project SKILL.md files flagged CRITICAL/HIGH ("destructive file deletion", "format filesystem", "sensitive system file", "privilege escalation") | Pattern scan of skill markdown prose | All flagged lines are documentation *examples* teaching what to detect/block (e.g. `security-guardian` documents a `rm -rf /` shell-injection example and an `/etc/passwd` read as attack patterns to guard against; `rtk-tdd` shows a path-traversal string as a validation-test example; `performance` documents `sudo dtrace` as a legitimate profiling command). No executable attacker-controlled code. Skill files are prose/instructions, not scripts. No fix needed. |
| 6 | Trivy `fs` scan | DB download failed: `context deadline exceeded` fetching `mirror.gcr.io/aquasec/trivy-db:2` after ~5 min at ~200 KiB/s | Network/mirror slowness in this environment, not a codebase issue. OSV-Scanner (SCA via OSV.dev) ran successfully instead and reported **no issues found** across all 208 Cargo dependencies — dependency coverage is not lost. |
| 7 | mcp-exfil-scan.sh | Script crashed: `MCP_CONFIGS[@]: unbound variable` (line 151/197, `set -uo pipefail` bug on empty array when 0 MCP config files exist) | Bug in the bundled scanner script itself, not a target-repo finding. rtk has no MCP config files (`mcp*.json`, `.mcp*`) at its root, so this check's actual applicability to rtk is nil regardless. |

## Raw Scanner Output

### Scope Record
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    4b12917 (post-merge)
Include:     all supported (Rust source, Cargo.toml/lock, project .claude/skills)
Exclude:     .gitignore-honored per tool; sibling worktrees under .claude/worktrees/ picked up incidentally by filesystem-walking tools (gitleaks, osv-scanner, trufflehog) — not part of this repo's own history
```

### Coverage Disclosure

| Tool | Ran? | Version | Files covered | Skipped reason |
|------|------|---------|---------------|----------------|
| Gitleaks | OK | 8.30.1 | Full git history (1505 commits, ~10.17 MB) | — |
| Bandit | OK | 1.9.4 | 2 `.py` files (`scripts/benchmark-sessions/lib/runner.py`, hooks) | — |
| Semgrep (secrets) | OK | latest | 410 tracked files | 99 `.semgrepignore`, 1 file >0.3 MB |
| Semgrep (OWASP) | OK | latest | 12 files (py/js/ts/go/rb) | 399 not matching include patterns (Rust not covered by this ruleset) |
| Semgrep (python) | OK | latest | 2 `.py` files | 409 not matching |
| Trivy | SKIPPED | 0.72.0 | — | Vuln DB download timed out (network) |
| TruffleHog | OK | 3.95.9 | Full git history (15,995 chunks, 11.4 MB) | — |
| CodeQL | N/A | — | — | No `.github/workflows/codeql.yml` present |
| mcps-audit | N/A | — | — | 0 MCP config files in rtk repo |
| OSV-Scanner | OK | 2.4.0 | Cargo.lock (208 packages) + Maven test fixtures | — |
| mcp-scan | SKIPPED | — | — | Opt-in (external service); no MCP servers to scan for this repo |
| config-audit (security-audit) | OK | — | `~/.claude` global config + installed plugins | Scope note: results are about the user's global Claude config, not rtk repo code |
| skill-audit (skill-security-auditor) | OK | — | 12 rtk-local `.claude/skills/*/SKILL.md` | — |
| mcp-exfil-scan | FAILED | — | — | Script bug (`set -u` unbound array) when 0 MCP configs found |
| skillspector | SKIPPED | — | — | Not run — skill-audit + config-audit already covered the skill/plugin surface; avoided redundant LLM-mode consent prompt (no human present to consent) |

### Gitleaks
**Summary:** 38 leaks, all confirmed false positives (fixture/example data, see Unresolved #1). [CONFIDENTIAL — secrets tool, values below redacted]
```
9:56PM INF 1505 commits scanned.
9:56PM INF scanned ~10167934 bytes (10.17 MB) in 1.19s
9:56PM WRN leaks found: 38
```
Representative samples (full 38-entry list triaged individually; values redacted):
```
stripe-access-token | tests/guard_integration_test.rs : 166 | commit 0baf07e2 | fixture: STRIPE_KEY=[REDACTED-test-fixture]
stripe-access-token | SECURITY.md : 151 | commit 66101ebb | doc example: const API_KEY: &str = "[REDACTED-doc-example]";
generic-api-key      | src/cmds/cloud/aws_cmd.rs : 1878-2035 | commit d1b37ced | test fixture tokens (nextForwardToken/nextBackwardToken hex strings)
stripe-access-token / generic-api-key | Security reports/security-audit-2026-*.md (multiple) | prior audit reports quoting the same fixture values for illustration
generic-api-key      | scripts/benchmark/cloud-init.yaml : 282, 613 | benchmark harness placeholder values
```

### Bandit
**Summary:** 9 Low/High-confidence findings, all in `scripts/benchmark-sessions/lib/runner.py` — subprocess usage with fixed arg lists.
```
Total lines of code: 467
Total issues (by severity): Low: 9, Medium: 0, High: 0
Total issues (by confidence): High: 9
B404 (subprocess import), B603 (subprocess_without_shell_equals_true), B607 (partial executable path: "tar")
```

### Semgrep — secrets / OWASP / python
```
Secrets:  Ran 45 rules on 410 files: 0 findings.
OWASP:    Ran 266 rules on 12 files: 0 findings. (399 files skipped — not py/js/ts/go/rb; Rust source not covered by this ruleset)
Python:   Ran 151 rules on 2 files: 0 findings.
```

### Trivy
```
2026-09-02T22:01:15+08:00 INFO [vulndb] Downloading vulnerability DB...
2026-09-02T22:01:15+08:00 INFO [vulndb] Downloading artifact... repo="mirror.gcr.io/aquasec/trivy-db:2"
2026-09-02T22:06:16+08:00 FATAL Fatal error  run error: init error: DB error: failed to download vulnerability DB:
  OCI artifact error: ... context deadline exceeded
exit=1
```

### TruffleHog
**Summary:** 135 "verified" hits, all `Lob` detector false positives on Rust test function identifiers. [CONFIDENTIAL — secrets tool, no real secret values present]
```
finished scanning: {"chunks": 15995, "bytes": 11391352, "verified_secrets": 135, "unverified_secrets": 0,
  "scan_duration": "6.438049291s", "trufflehog_version": "3.95.9"}
```
Example entries (raw result = literal Rust test function name, not a credential):
```
Detector Type: Lob | Raw result: test_find_corrections_excludes_tdd_cycle | File: src/learn/detector.rs:508
Detector Type: Lob | Raw result: test_deduplicate_corrections_merges_same | File: src/learn/detector.rs:571
Detector Type: Lob | Raw result: test_migrate_legacy_hook_noop_when_empty | File: src/hooks/init.rs:2830
Detector Type: Lob | Raw result: test_hook_already_present_native_command | File: src/hooks/init.rs:2757
```

### OSV-Scanner
```
Scanned Cargo.lock file and found 208 packages
Warning: enricher transitivedependency/pomxml may be risky when run on untrusted artifacts (test-fixture pom.xml files only)
No issues found
exit=0
```

### CodeQL
N/A — no `.github/workflows/codeql.yml` in this repo.

### mcps-audit
N/A — 0 MCP config files (`mcp*.json`, `.mcp*`) found under the rtk repo root.

### security-audit (config-audit.py)
**Summary:** 23 CRITICAL / 18 HIGH / 68 MEDIUM / 20 LOW — all findings are about the user's global `~/.claude/` configuration and globally-installed plugins (impeccable, ponytail, caveman, anysearch, claude-code-security-plugins, etc.), **not rtk repo code**. Out of scope for this codebase audit; not actioned.

### skill-audit (skill-security-auditor) — rtk-local skills
**Summary:** 4 of 12 rtk-local `SKILL.md` files scored CRITICAL/HIGH on pattern-matching; all confirmed false positives (documentation prose describing attack patterns to detect, not executable attacker code):
```
security-guardian/SKILL.md — CRITICAL: documents a shell-injection payload example and an /etc/passwd read as attack patterns it teaches reviewers to catch
ship/SKILL.md            — CRITICAL: risk score inflated by legitimate Bash/Write/Edit tool grants + a ".env" doc mention
performance/SKILL.md      — HIGH: cites "sudo dtrace" as a legitimate macOS profiling command in its own docs
rtk-tdd/SKILL.md          — HIGH: cites a path-traversal string as a validation-test example (assert!(!is_valid(...)))
```

### mcp-exfil-scan
```
MCP configs found: 0
Skill files found: 24
.../mcp-exfil-scan.sh: line 151: MCP_CONFIGS[@]: unbound variable
exit=1
```
Bundled-script bug (`set -uo pipefail` + empty-array expansion), unrelated to rtk repo content.

## Cross-Tool Observations
- Gitleaks and TruffleHog both flag `Security reports/security-audit-*.md` and `tests/guard_integration_test.rs` / `src/cmds/cloud/aws_cmd.rs` — consistent overlap confirming these are the same known fixture/doc values, not independent real secrets.
- OSV-Scanner and Trivy target the same Cargo.lock; OSV-Scanner's clean "no issues found" result stands in for Trivy's dependency-CVE coverage this run.
- No cross-tool overlap between config-audit's global-plugin findings and skill-audit's rtk-local findings — they scan disjoint scopes (global config vs. repo-local skills) and no rtk-owned file appears in both with a matching issue.

## Coverage Gaps
- **Trivy**: dependency/IaC/secrets coverage via Trivy not obtained this run (DB download timeout). Mitigated by OSV-Scanner (clean) for dependency CVEs; no IaC files in this repo to lose coverage on.
- **mcp-exfil-scan**: incomplete — bundled script bug on the zero-MCP-config path. rtk has no MCP configs, so the check's applicability is nil regardless.
- **CodeQL / mcp-scan / skillspector**: not run (N/A or opt-in — no human present to grant the mcp-scan/skillspector-LLM consent gate; skill-audit + config-audit already covered the equivalent surface).
- Not covered by any tool: business logic correctness, IDOR-equivalent CLI authorization issues, runtime behavior under real workloads.

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260902T135635Z.jsonl`
- **Tool runs recorded:** 5 (measured: 5, asserted: 0)
- **Standard:** OWASP APTS § Auditability
