# Security Audit — 2026-07-07

## Summary
- Issues found: 3 | Auto-fixed: 2 | Unresolved: 1
- Status: ISSUES FIXED

## Fixed Issues
| # | Component | Advisory | Change |
|---|-----------|----------|--------|
| 1 | quick-xml | RUSTSEC-2026-0194 / RUSTSEC-2026-0195 (CVSS 7.5) | Cargo.toml `0.37` → `0.41`; Cargo.lock `0.37.5` → `0.41.0`. Migrated call sites: `BytesText::unescape()` → `decode()` in [src/cmds/dotnet/dotnet_cmd.rs](../src/cmds/dotnet/dotnet_cmd.rs), `Attribute::decode_and_unescape_value()` → `decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)` in [src/cmds/dotnet/dotnet_trx.rs](../src/cmds/dotnet/dotnet_trx.rs) |
| 2 | crossbeam-epoch | RUSTSEC-2026-0204 | `cargo update -p crossbeam-epoch`: `0.9.18` → `0.9.20` (transitive via `ignore` crate) |

## Unresolved Issues
- **mcp-exfil-scan.sh checksum mismatch** — bundled script `SHA256SUMS` verification FAILED for `mcp-exfil-scan.sh`. Per skill policy, did not execute (possible corruption/tamper, not confirmed malicious). Skipped this scanner for the run; no fix applied since root cause is in the security-scanner plugin cache, not the rtk codebase. Recommend reinstalling the `claude-code-security-plugins` plugin.
- **Pre-existing test failure `test_rewrite_uv_run`** (`src/discover/registry.rs:2650`) — expects `"uv run rtk pytest"`, got `"rtk uv run pytest"`. Confirmed present before this session's changes (reproduced via `git stash`), introduced by upstream merge, unrelated to security findings. Out of scope for this security-focused run; left unfixed.

## Raw Scanner Output

### Gitleaks (git history + filesystem)
35 findings, all confirmed false positives — placeholder/example secrets in prior audit report docs, `SECURITY.md`, `scripts/benchmark/cloud-init.yaml`, and `src/cmds/cloud/aws_cmd.rs` (e.g. `sk-1234567890abcdef`, `sk_live_1234567890abcdef`, AWS CloudWatch example token format). No real credentials.

### TruffleHog (git, live verification)
0 verified secrets, 0 unverified secrets. Clean.

### Trivy (filesystem)
Root `Cargo.lock`: no findings (`rustls-webpki` already at fixed `0.103.13`).
26 findings across `.claude/worktrees/*/Cargo.lock` — stale lockfiles in sibling git worktrees (out of scope; not part of this repo's active dependency tree).

### OSV-Scanner (root Cargo.lock)
3 vulnerabilities found, all fixed this run (see Fixed Issues table above): `crossbeam-epoch` 0.9.18, `quick-xml` 0.37.5 (2 advisories).

### Semgrep (secrets, `p/secrets` config, `src/`)
201 files scanned, 36 rules, 0 findings.

### config-audit (Claude Code configuration)
No CRITICAL/HIGH findings. MEDIUM: broad `SessionStart`/`UserPromptSubmit` hook matchers in `caveman` and `pordee` plugins (expected — these are mode-activation hooks, not project code); CLAUDE.md "avoid rabbit holes" guidance flagged as a generic "trust-all"/"skip verification" pattern (false positive — intentional scope-discipline instruction, not a verification bypass for security-relevant work). LOW: hook configuration present in several plugins (informational only).

### Bundled-script integrity check
```
config-audit.py: OK
skill-audit.sh: OK
mcp-exfil-scan.sh: FAILED
apts-audit.sh: OK
aggregate-findings.py: OK
```

## Cross-Tool Observations
No cross-tool overlaps requiring escalation. Gitleaks/TruffleHog agree: no real secrets present.

## Coverage Gaps
- mcp-exfil-scan skipped (checksum mismatch, see Unresolved).
- skillspector and mcp-scan not run (opt-in, external LLM/network calls — no user present to consent in this autonomous run).
- Bandit not applicable (no `.py` files in scope).
- CodeQL not run (no `.github/workflows/codeql.yml` present).
- Business logic / IDOR / runtime behavior not covered by static tools.

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260707T020909Z.jsonl`
- **Tool runs recorded:** 6 (measured: 6, asserted: 0)
- **Standard:** OWASP APTS § Auditability
