# Security Audit — 2026-08-13

## Summary
- Issues found: 0 real vulnerabilities (166 raw tool hits, all triaged as false positives / out-of-scope)
- Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

Upstream sync merged 3 commits from `origin/develop` (PR #2952 — Windows drive-letter colon fix in `discover`) with no conflicts.

## Fixed Issues
None required — no real vulnerabilities found in this cycle.

## Triage Notes (no code changes needed)

| # | Tool | Finding | Disposition |
|---|------|---------|-------------|
| 1 | Gitleaks | 36 secret-pattern hits (`stripe-access-token`, `generic-api-key`) | False positive — all in `Security reports/*.md` (historical audit reports quoting redacted examples), `scripts/benchmark/cloud-init.yaml` (synthetic `.env` fixture for benchmark VM), and `src/cmds/cloud/aws_cmd.rs` test fixtures (synthetic AWS pagination tokens `f/abcdef123...`) |
| 2 | Bandit | 9 Low-severity `subprocess` findings | False positive — `scripts/benchmark-sessions/lib/runner.py` uses fixed-arglist `subprocess.run(["tar", "czf", ...])`, no shell, no untrusted input |
| 3 | Semgrep (OWASP/Python/secrets) | 0 findings | Clean |
| 4 | Trivy (fs, cargo+pom) | 0 vulnerabilities | Clean — `Cargo.lock` + all `tests/fixtures/**/pom.xml` |
| 5 | TruffleHog | 117 "verified" hits, single detector `Lob` | False positive — known noisy detector matching Rust `test_*` function names as if they were Lob API keys |
| 6 | OSV-Scanner | 0 issues | Clean |
| 7 | config-audit (Claude config) | 94 issues across machine-wide `~/.claude` config, other installed plugins/skills | Out of scope — global machine configuration, not part of the `rtk` repo. Only 2 hits land inside the repo (`CLAUDE.md`/`claude.md`): flagged for the word "curl" (a `Proxy Mode` doc example) and the "Avoiding Rabbit Holes" scope-discipline section (misclassified as a "skip verification" instruction) — both are legitimate documentation, not injected instructions |
| 8 | mcp-exfil-scan.sh | Script crashed (`MCP_CONFIGS[@]: unbound variable`, line 151/197) | Tool bug in the bundled scanner script, not an rtk finding — logged under Coverage Gaps |
| 9 | mcps-audit | Risk score 100/100, 15 findings ("Dangerous execution" in `hooks/pi/rtk.ts`, `hooks/opencode/rtk.ts`, `openclaw/index.ts`) | False positive — generic pattern-match on any `exec()`/`execFileSync()` call. All flagged calls use array-argument form (`pi.exec("rtk", ["rewrite", cmd], ...)`, `execFileSync("which", ["rtk"], ...)`), never shell-string concatenation, so there is no command-injection vector. This is rtk's documented hook-delegation design (`rtk rewrite` as single source of truth) |

## Unresolved Issues
None.

## Raw Scanner Output

### Gitleaks (SARIF summary — 36 results, all false positive per Triage Notes #1)
```
9:09AM INF 1326 commits scanned.
9:09AM INF scanned ~9074983 bytes (9.07 MB) in 1.02s
9:09AM WRN leaks found: 36
```
Sample locations: `Security reports/security-audit-2026-07-07.md:20`, `scripts/benchmark/cloud-init.yaml:613,282`, `src/cmds/cloud/aws_cmd.rs:1878,1879,1923,2035`, `SECURITY.md:151` (full 36-row list retained in `/tmp/gitleaks.sarif`).

### Bandit
```
Run metrics:
	Total issues (by severity):
		Undefined: 0
		Low: 9
		Medium: 0
		High: 0
	Total issues (by confidence):
		High: 9
Files skipped (0):
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
Ran 45 rules on 360 files: 0 findings.
```

### Trivy (fs)
```
Report Summary
Cargo.lock (root + all worktrees): cargo, 0 vulnerabilities
tests/fixtures/**/pom.xml: pom, 0 vulnerabilities (x8)
```

### TruffleHog
```
finished scanning {"chunks": 14145, "bytes": 10134863, "verified_secrets": 117,
"unverified_secrets": 0, "scan_duration": "1.571587833s"}
```
All 117 "verified" results: `Detector Type: Lob`, raw result = Rust test function names (e.g. `test_hook_already_present_different_path`, `test_registry_covers_all_git_subcommands`).

### OSV-Scanner
```
Scanned Cargo.lock (root + 3 worktrees): 202 packages each
Scanned tests/fixtures/**/pom.xml (root + 3 worktrees)
No issues found
```

### config-audit.py (machine-wide Claude config scan)
```
Found 94 issue(s):
  CRITICAL: 7   HIGH: 16   MEDIUM: 59   LOW: 12
```
Only in-repo hits: `CLAUDE.md` / `claude.md` — "curl to external URL" (doc example in Proxy Mode section), "instruction to skip verification" / "trust-all instruction" (Avoiding Rabbit Holes scope-discipline guidance, misclassified). All other 90 findings are global `~/.claude/settings.json` hooks (cc-beeper, AgentPeek, vibe-island bridges) and other installed plugins/skills — outside the `rtk` repo's scope.

### mcp-exfil-scan.sh
```
mcp-exfil-scan.sh: line 151: MCP_CONFIGS[@]: unbound variable
MCP configs found: 0
Skill files found: 24
mcp-exfil-scan.sh: line 197: MCP_CONFIGS[@]: unbound variable
```
Script exited early on an internal bug (unset array reference) before completing its 6-stage scan. Logged as a Coverage Gap — see below.

### mcps-audit
```
Coverage: 2/8 mitigated | MCPS SDK: not found
CRITICAL: 4  HIGH: 2  MEDIUM: 9  LOW: 0
Verdict: FAIL  Risk Score: 100/100
Files: 25 | Lines: 3063 | Findings: 15
```
All "Dangerous execution" (AS-001) / "Code execution without sandboxing" (AS-006) findings are generic exec-call pattern matches against `hooks/pi/rtk.ts`, `hooks/opencode/rtk.ts`, `hooks/hermes/rtk-rewrite/__init__.py`, and `openclaw/index.ts` — all use array-argument `exec`/`execFileSync`, no shell interpolation. See Triage Notes #9.

## Cross-Tool Observations
No cross-tool overlaps on real findings. Gitleaks and TruffleHog both flag test/fixture content but on disjoint file sets (no corroboration of an actual live secret). mcps-audit and config-audit both note the `hooks/*` delegation pattern but neither identifies unsafe interpolation — consistent with the codebase's documented array-arg execution rule (`.claude/rules/rust-patterns.md` § Error Handling, extended informally to the JS/TS/Python hook shims).

## Coverage Gaps
- `mcp-exfil-scan.sh` crashed on an internal bug (`MCP_CONFIGS[@]: unbound variable`) before completing its scan — MCP exfiltration analysis of this repo is incomplete this cycle.
- CodeQL not run (no `.github/workflows/codeql.yml` configured for this repo).
- mcp-scan (opt-in, sends data to invariantlabs.ai) and skillspector LLM-mode (opt-in) skipped — no human present to consent this run.
- Not covered: business logic correctness, IDOR, runtime behavior — static analysis only.

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260813T020844Z.jsonl`
- **Tool runs recorded:** 3 (measured: 3, asserted: 0) via wrapper; remaining tools (semgrep, trivy, trufflehog, osv-scanner, config-audit, mcps-audit, mcp-exfil-scan) run directly and logged manually in this report per Operational Rule 5 (tool crash/non-zero exit noted).
- **Standard:** OWASP APTS § Auditability
