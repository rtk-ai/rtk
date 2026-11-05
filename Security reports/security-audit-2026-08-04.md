# Security Audit — 2026-08-04

## Summary
- Issues found: 0 | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

No exploitable vulnerabilities, no verified secrets, no vulnerable dependencies found in the rtk codebase (Rust source, Cargo.toml, Cargo.lock, plus supporting Python hook scripts). All raw tool hits below are confirmed false positives / non-issues — no code changes required.

## Fixed Issues
None.

## Unresolved Issues
None.

## Raw Scanner Output

### Gitleaks (secrets, git history + filesystem)
1292 commits scanned, ~8.79 MB. **36 "leaks" flagged — all false positives**: every hit is an obviously-fake placeholder value (`stripe-access-token` and `generic-api-key` detector rules matching sequential-digit dummy strings, e.g. dummy Stripe-shaped tokens and `secret-api-key-12345`) embedded in prior audit report docs (`Security reports/security-audit-*.md`), `SECURITY.md` examples, `scripts/benchmark/cloud-init.yaml` fixtures, or synthetic AWS token strings in `src/cmds/cloud/aws_cmd.rs` test fixtures. No real credentials present.

### TruffleHog (secrets, live-verified)
13,800 chunks / 9.8 MB scanned. `verified_secrets: 0`, `unverified_secrets: 0`. Clean.

### OSV-Scanner (SCA)
Scanned `Cargo.lock` (202 packages) + test-fixture `pom.xml` files. **No issues found.**

### Trivy (deps, secrets, IaC)
`Cargo.lock`: 0 vulnerabilities. Fixture `pom.xml` files: 0 vulnerabilities. No secrets detected.

### Semgrep — secrets (`p/secrets`)
396 files, 52 rules. **0 findings.**

### Semgrep — OWASP Top Ten (`p/owasp-top-ten`, non-Rust files: py/js/ts/java/go/rb)
12 files (RTK's Python/JS/TS/Ruby helper scripts — Rust source isn't covered by this ruleset), 266 rules. **0 findings.**
Coverage note: `--max-target-bytes 300000` skips files >300KB; the only files over that threshold are build artifacts under `target/` (release/debug binaries, `.rlib` files) — not source, no coverage gap.

### Bandit (Python SAST)
Scanned `hooks/hermes/rtk-rewrite/__init__.py`, `hooks/hermes/tests/test_rtk_rewrite_plugin.py`, `scripts/benchmark-sessions/lib/runner.py`. 9 Low-severity/High-confidence hits, all `subprocess` advisories (B404/B603/B607 — "consider subprocess module implications", "partial executable path"). Reviewed `_pre_tool_call` in `hooks/hermes/rtk-rewrite/__init__.py`: uses `subprocess.run(["rtk", "rewrite", command], shell=False, ...)` — list-form args, `shell=False`, no shell interpolation. This is the textbook-safe pattern; Bandit flags it only because any subprocess use is advisory-level. No action needed.

### CodeQL
Skipped — no `codeql.yml` workflow on the `rtk-ai/rtk` default branch.

### Claude-config audit (config-audit.py)
This tool audits the *global* `~/.claude` environment (settings, all installed skills/plugins), not the rtk project specifically. It surfaced 7 CRITICAL / 16 HIGH / 56 MEDIUM / 12 LOW hits — all in **unrelated global Claude plugins** (e.g. `caveman`, `optimize`, `anysearch`, the security-scanner's own detection scripts matching their own exfil-pattern strings) or generic curl/pattern matches in this repo's `CLAUDE.md` (a documented benchmark command example). None reference rtk's Rust source, Cargo manifest, or lockfile. Out of scope for this audit's target (rtk codebase); no fix applied here.

### Skill/MCP-specific scanners (skill-audit, mcp-exfil-scan, mcps-audit, skillspector)
Not run — no `.skill`, `SKILL.md`, or MCP manifest files exist inside the rtk project tree (only in the user's global `~/.claude` environment, out of scope).

## Cross-Tool Observations
No cross-tool overlaps on real findings — all four SCA/secrets tools (Gitleaks, TruffleHog, Trivy, OSV-Scanner) independently confirm zero live secrets and zero vulnerable dependencies.

## Coverage Gaps
- Business logic / IDOR / runtime behavior not covered (static analysis only).
- CodeQL skipped (no workflow configured).
- Skill/MCP scanners N/A (no such artifacts in-repo).

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260804T020908Z.jsonl`
- **Tool runs recorded:** 5 (measured: 5, asserted: 0)
- **Standard:** OWASP APTS § Auditability
