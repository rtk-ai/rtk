# Security Audit — 2026-07-08

## Summary
- Issues found: 0 new | Auto-fixed: 0 new | Unresolved: 1 (carried over)
- Status: PASSED
- Note: 2 dependency vulnerabilities from 2026-07-07 (quick-xml RUSTSEC-2026-0194/0195, crossbeam-epoch RUSTSEC-2026-0204) were fixed in the working tree but had not yet been committed/pushed by the prior run. They are included in today's sync commit.

## Fixed Issues
None new this run. Carried over from 2026-07-07 (now committed):

| # | Component | Advisory | Change |
|---|-----------|----------|--------|
| 1 | quick-xml | RUSTSEC-2026-0194 / RUSTSEC-2026-0195 (CVSS 7.5) | Cargo.toml `0.37` → `0.41`; Cargo.lock `0.37.5` → `0.41.0`. Migrated call sites: `BytesText::unescape()` → `decode()` in [src/cmds/dotnet/dotnet_cmd.rs](../src/cmds/dotnet/dotnet_cmd.rs), `Attribute::decode_and_unescape_value()` → `decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)` in [src/cmds/dotnet/dotnet_trx.rs](../src/cmds/dotnet/dotnet_trx.rs) |
| 2 | crossbeam-epoch | RUSTSEC-2026-0204 | `cargo update -p crossbeam-epoch`: `0.9.18` → `0.9.20` (transitive via `ignore` crate) |

## Unresolved Issues
- **mcp-exfil-scan.sh checksum mismatch** — bundled script `SHA256SUMS` verification still FAILS for `mcp-exfil-scan.sh` (same as 2026-07-04 and 2026-07-07). Per skill policy, did not execute. Root cause is in the `claude-code-security-plugins` plugin cache, not the rtk codebase. Recommend reinstalling the plugin.
- **Pre-existing test failure `test_rewrite_uv_run`-family** (`src/discover/registry.rs`) — flagged unresolved in the 2026-07-07 report as introduced by an upstream merge, unrelated to security findings. Not re-verified this run (out of scope, avoiding rabbit-hole per CLAUDE.md guidance); still tracked as a known issue for a future non-security session.

## Raw Scanner Output

### Gitleaks (git history + filesystem)
1152 commits scanned, 35 leaks found — consistent with the 2026-07-07 run: all confirmed false positives (placeholder/example secrets in prior audit report docs, `SECURITY.md`, `scripts/benchmark/cloud-init.yaml`, `src/cmds/cloud/aws_cmd.rs`). No real credentials.

### OSV-Scanner (source scan, all Cargo.lock files under repo root)
15 vulnerabilities found across 10 packages — **all in sibling `.claude/worktrees/*/Cargo.lock` files** (stale lockfiles in other git worktrees, out of scope per 2026-07-07 finding). The root `Cargo.lock` (this repo's active dependency tree) reported **zero** vulnerabilities — confirms the quick-xml/crossbeam-epoch fixes are effective.

### Trivy (filesystem, root Cargo.lock)
```
Cargo.lock  cargo  0 vulnerabilities
```
Clean.

### TruffleHog (git, live verification)
12202 chunks / 9.1 MB scanned. 0 verified secrets, 0 unverified secrets. Clean.

### Semgrep (secrets, `p/secrets` config, `src/`)
201 files scanned, 36 rules. 0 findings.

### config-audit (Claude Code configuration)
No CRITICAL/HIGH findings. MEDIUM: broad `SessionStart`/`UserPromptSubmit` hook matchers in mode-activation plugins (caveman, pordee, openai-codex, addy-agent-skills, claude-plugins-official) — expected, not project code. `CLAUDE.md` "avoid rabbit holes" guidance flagged as a generic trust-all/skip-verification pattern — false positive, intentional scope-discipline instruction (consistent with prior audits). LOW: hook configuration present in several plugins (informational only).

### Skill-audit
N/A — no `*.skill` or `SKILL.md` files present in the rtk repository itself.

### Bundled-script integrity check
```
config-audit.py: OK
skill-audit.sh: OK
mcp-exfil-scan.sh: FAILED
apts-audit.sh: OK
aggregate-findings.py: OK
```

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260708T020945Z.jsonl`
- **Tool runs recorded:** 7 (measured: 7, asserted: 0)
- **Standard:** OWASP APTS § Auditability
