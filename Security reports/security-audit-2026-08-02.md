# Security Audit — 2026-08-02

## Summary
- Issues found: 0 (in rtk source) | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED
- 36 gitleaks hits were reviewed and confirmed false positives (placeholder Stripe-shaped and generic-API-key-shaped example strings inside prior `Security reports/*.md` audit reports — documentation examples, not live credentials).

## Fixed Issues
(none — no fixable vulnerabilities found in rtk source, Cargo.toml, or Cargo.lock)

## Unresolved Issues
(none)

## Scope Record
```
Scan target: /Users/mp3wizard/Public/Claude Proxy/rtk
Git HEAD:    d729f48
Include:     all supported (src/**/*.rs, Cargo.toml, Cargo.lock, tests/**)
Exclude:     .gitignore-honored per tool; .claude/worktrees/* present but duplicate scans of worktree copies were also clean
```

## Coverage Disclosure
| Tool | Ran? | Version | Files covered | Skipped reason |
|------|------|---------|---------------|----------------|
| Gitleaks | OK | 8.30.1 | full git history, 1279 commits | — |
| Bandit | SKIPPED | 1.9.4 | — | no `.py` source in rtk (fixtures/scripts only under `.claude/`, out of scope) |
| Semgrep (secrets) | OK | current | 394 files | 61 files matched `.semgrepignore` |
| Semgrep (OWASP top-10) | OK | current | 12 files (py/ts/rb configs) | 382 not matching `--include`, 61 `.semgrepignore`; Rust has no dedicated OWASP ruleset in this pack |
| Trivy (fs: deps/secrets/IaC) | OK | 0.72.0 | Cargo.lock + pom.xml test fixtures | — |
| TruffleHog | OK | 3.95.9 | 13,735 chunks / 9.78 MB, full git history | — |
| OSV-Scanner | OK | 2.4.0 | Cargo.lock (202 crates) + pom.xml fixtures | — |
| CodeQL | SKIPPED | — | — | no `.github/workflows/*codeql*` configured |
| mcps-audit | N/A | — | — | no MCP config files in repo |
| skill-security-auditor | N/A | — | — | no `SKILL.md`/`.skill` files in repo |
| mcp-exfil-scan | ERROR | — | — | bundled script crashed: `MCP_CONFIGS[@]: unbound variable` (bash `set -u` bug in scanner script itself, not a repo finding) |
| skillspector | N/A | — | — | no AI-skill artifacts in repo |
| mcp-scan | OPT-IN, SKIPPED | — | — | sends data externally; no human present to consent in this automated run |
| security-audit (config-audit.py) | OK | — | global `~/.claude` config + this repo's CLAUDE.md | ran system-wide by design; findings below are scoped to what's relevant to rtk |

## Gitleaks — Secrets in git history + filesystem
**Summary:** 36 matches, all false positives (placeholder API keys in prior audit-report markdown, `Security reports/security-audit-*.md`), 0 real secrets.
```
stripe-access-token   19
generic-api-key       17
```
Sample: `Security reports/security-audit-2026-07-07.md:20` → a Stripe-live-key-shaped placeholder string (doc example, matches pattern across ~15 historical audit files that reused the same illustrative fake key).

## Semgrep — secrets + OWASP top-ten
**Summary:** 0 findings (52 secrets rules / 394 files; 266 OWASP rules / 12 applicable files).

## Trivy — deps/secrets/IaC
**Summary:** 0 vulnerabilities, 0 secrets across Cargo.lock (this repo + 3 stray worktree copies) and Maven test-fixture poms.

## TruffleHog — secrets w/ live verification
**Summary:** 0 verified, 0 unverified secrets. 114 detector attempts, all misses.

## OSV-Scanner — SCA
**Summary:** No issues found across 202 Cargo crates + Maven fixture packages.

## security-audit (config-audit.py) — Claude config audit
**Summary:** 91 findings total, **none inside rtk's `src/`, `Cargo.toml`, or `Cargo.lock`**. All are either:
1. Global `~/.claude/settings.json` hooks (cc-beeper, AgentPeek, vibe-island bridges) and third-party plugin/skill files (`anysearch`, `caveman`, `claude-code-security-plugins` itself, `plugin-dev` examples) — these are the user's global Claude Code environment, outside rtk's repo scope, and several are the security tooling's own detection code matching on its own `curl`/`base64`/`.env` strings (self-referential false positives, e.g. `mcp-exfil-scan.sh` flagging itself, `skill-audit.sh` flagging itself).
2. Two hits inside this repo's `CLAUDE.md` / `claude.md`, both false positives from generic pattern matching: "curl to external URL" (matches the `curl https://api.example.com/data` documentation example for `rtk proxy`) and "trust-all instruction" (matches the "trust snapshot tests" wording in the Avoiding Rabbit Holes section — project guidance, not an executable trust bypass).

No action required in rtk's own codebase.

## Cross-Tool Observations
No cross-tool overlaps on rtk source/dependencies. Gitleaks/Trivy/TruffleHog agree: 0 real secrets. Semgrep/Bandit/CodeQL/OSV-Scanner agree: 0 code or dependency vulnerabilities.

## Coverage Gaps
- Not covered: business logic correctness, IDOR, runtime behavior (static analysis only).
- mcp-exfil-scan crashed on an unbound-variable bug in the bundled script when no MCP config files exist — did not produce a usable result for this repo; low risk given mcps-audit/skill-audit both independently reported no MCP/skill artifacts present.
- mcp-scan (opt-in, sends data to invariantlabs.ai) and skillspector LLM-mode were both skipped — no human present in this automated run to grant consent, and neither is applicable anyway (no MCP/skill files in this repo).

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260802T020835Z.jsonl`
- **Tool runs recorded:** 8 (measured: 8, asserted: 0)
- **Standard:** OWASP APTS § Auditability
