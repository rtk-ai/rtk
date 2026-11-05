# Security Audit — 2026-08-03

## Summary
- Issues found: 8 (categories) across ~177 raw hits | Auto-fixed: 0 | Unresolved: 8 (all triaged as false positives / non-exploitable informational)
- Status: PASSED

All raw findings were manually triaged. No real vulnerability, verified secret, or vulnerable dependency was found. Trivy, OSV-Scanner, Semgrep (OWASP/Python/Secrets), and TruffleHog (with live verification) all report **zero** issues. Gitleaks, Bandit, config-audit, and skill-audit hits are all example/placeholder data, self-referential scanner-pattern matches, or standard low-severity subprocess usage — none require or admit a safe code change.

## Fixed Issues
None — no actionable vulnerability was found.

## Unresolved Issues

| # | Component | Advisory | Reason |
|---|-----------|----------|--------|
| 1 | Gitleaks — 36 hits (`stripe-access-token` ×19, `generic-api-key` ×17) in `Security reports/*.md` history, `SECURITY.md`, `scripts/benchmark/cloud-init.yaml`, `src/cmds/cloud/aws_cmd.rs` | n/a | All matched values are placeholder examples (redacted-format `sk_live_...` / `secret-api-key-...` / hex-digit test strings) used in prior audit reports and AWS-filter test fixtures. No live/verified secret. Not fixable — deleting example docs/fixtures would remove legitimate content. |
| 2 | Bandit — 45 Low/High-confidence hits (`B404`, `B603`, `B607`) in `hooks/hermes/rtk-rewrite/__init__.py`, its tests, `scripts/benchmark-sessions/lib/runner.py` | n/a | Standard `subprocess` usage inherent to hook scripts that must spawn `rtk`/git processes. `shell=False` already used (no `B602`); switching to `shell=True` would be a regression. Informational only. |
| 3 | config-audit — `CLAUDE.md` / `claude.md`: "curl to external URL" | n/a | The doc's own `rtk proxy curl https://api.example.com/data` usage example, matched by a heuristic string scan. Not an actual outbound call. |
| 4 | config-audit — `CLAUDE.md` / `claude.md`: "skip verification" / "trust-all instruction" | n/a | Matches the repo's legitimate "Avoiding Rabbit Holes" guidance section (trust snapshot tests, don't over-verify). Project convention, not a prompt-injection payload. |
| 5 | config-audit — CRITICAL: `anysearch/scripts/anysearch_cli.sh`, and the security-scanner plugin's own `mcp-exfil-scan.sh`/`skill-audit.sh`/`config-audit.py` | n/a | Global-scope (`~/.claude/...`) findings outside this project's tree; self-referential — the scanner's own source contains the exfiltration-pattern strings it searches for (base64/.env/ncat/SSH keywords in comments and detection regexes). Out of this project's scope. |
| 6 | skill-audit — `.claude/skills/security-guardian/SKILL.md` → 100/100 CRITICAL (destructive-delete / format-filesystem / sensitive-file patterns) | n/a | The skill's own documentation *of* injection-attack strings to detect/guard against (example payloads and a denylist array). Not executable, not exploitable. |
| 7 | skill-audit — `.claude/skills/ship/SKILL.md` → 90/100 CRITICAL (Bash+Write+Edit tools, `.env` regex match) | n/a | Expected tool footprint for a release-automation skill (build/commit/push/version-bump). `.env` match is a doc mention, not credential access. |
| 8 | skill-audit — `.claude/skills/performance/SKILL.md` → 65/100 HIGH ("privilege escalation": documented `sudo dtrace` command) | n/a | Documented macOS `dtrace` profiling command in a performance-debugging reference table — legitimate, standard technique, not privilege escalation by the skill itself. |

## Tool Notes / Failures
- `mcp-exfil-scan.sh` crashed (`MCP_CONFIGS[@]: unbound variable`, `set -u` bug) after reporting "MCP configs found: 0" — no MCP config files exist in this project, so no findings were lost, but the script itself needs a bundled-tool fix (upstream skill issue, not this repo).
- `apts-audit.sh run` wrapper does not word-split a multi-flag `$SG` variable (`command not found`, exit 127) — worked around by invoking `semgrep` directly and logging results manually; noted as `asserted:1` (not wrapper-measured) in the APTS audit log.
- mcp-scan and skillspector LLM-mode were **not run** (opt-in, requires user consent; this is a fully autonomous unattended run — see security-scan skill privacy gate). skillspector `--no-llm` mode was not run this cycle (no new AI-skill-specific triggers beyond what config-audit/skill-audit already covered).

## Raw Scanner Output

### Gitleaks
```
1289 commits scanned. leaks found: 36 (exit=1)
Rules matched: stripe-access-token (19), generic-api-key (17)
Files: Security reports/security-audit-*.md, SECURITY.md, scripts/benchmark/cloud-init.yaml, src/cmds/cloud/aws_cmd.rs
All values are documented placeholder examples — see Unresolved #1.
```

### Bandit
```
Total issues (by severity): Low: 45, Medium: 0, High: 0
Total issues (by confidence): High: 45
Rules: B404 (subprocess import) x15, B603 (subprocess without shell=True) x15, B607 (partial exec path) x15
Files: hooks/hermes/rtk-rewrite/__init__.py, hooks/hermes/tests/test_rtk_rewrite_plugin.py, scripts/benchmark-sessions/lib/runner.py
```

### Semgrep — OWASP Top Ten
```
Ran 266 rules on 12 files: 0 findings.
```

### Semgrep — Python
```
Ran 151 rules on 2 files: 0 findings.
```

### Semgrep — Secrets
```
Ran 45 rules on 395 files: 0 findings.
```

### Trivy (fs scan — deps/IaC/secrets)
```
All scanned targets (Cargo.lock x5 across worktrees, pom.xml fixtures) — 0 findings each.
No vulnerabilities, misconfigurations, or secrets detected.
```

### TruffleHog (git, with live verification)
```
{"chunks": 13792, "bytes": 9818303, "verified_secrets": 0, "unverified_secrets": 0,
 "scan_duration": "2.906854125s"}
```

### OSV-Scanner
```
Scanned Cargo.lock (202 packages) x5 worktrees + pom.xml fixtures.
No issues found.
```

### config-audit (Claude config/skills/plugins/hooks)
```
Found 91 issue(s) total (global ~/.claude scope + project scope):
  CRITICAL: 7   HIGH: 16   MEDIUM: 56   LOW: 12
Project-scoped (rtk/CLAUDE.md, rtk/claude.md) subset: 2 HIGH + 4 MEDIUM — see Unresolved #3, #4.
Remainder is global-scope (~/.claude/settings.json, other installed plugins/skills) and
self-referential scanner-source matches — out of this project's scan target, see Unresolved #5.
```

### skill-audit (project `.claude/skills/*/SKILL.md`, 12 files scanned)
```
security-guardian: 100/100 CRITICAL   (false positive — see Unresolved #6)
ship:               90/100 CRITICAL   (false positive — see Unresolved #7)
performance:        65/100 HIGH       (false positive — see Unresolved #8)
rtk-triage:         35/100 MEDIUM     (sensitive-system-file string match, informational)
issue-triage:       30/100 MEDIUM     (bash access, expected for triage automation)
pr-triage:          25/100 MEDIUM     (credential-pattern string match, informational)
repo-recap:         25/100 MEDIUM     (bash access, expected)
design-patterns:      5/100 LOW
code-simplifier:      5/100 LOW
rtk-tdd:              5/100 LOW
tdd-rust:             5/100 LOW
pr-review:            5/100 LOW
```

### mcp-exfil-scan
```
MCP configs found: 0
Skill files found: 24
Script crashed on line 151/197 (unbound array under `set -u`) after confirming no MCP
config files exist in this project. See Tool Notes above.
```

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260803T020819Z.jsonl`
- **Tool runs recorded:** 9 (measured: 8, asserted: 1)
- **Standard:** OWASP APTS § Auditability
