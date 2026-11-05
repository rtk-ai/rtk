# Security Audit — 2026-09-30

## Summary
- Issues found: 0 actionable (48 raw hits triaged, all false positives) | Auto-fixed: 0 | Unresolved: 0
- Status: PASSED

## Fixed Issues
None — no genuine vulnerability, secret, or unsafe pattern found in the rtk codebase this run. `Cargo.lock` (213 packages) is clean per Trivy and OSV-Scanner; no RUSTSEC advisories.

## Unresolved Issues
None.

## Raw Scanner Output

### Gitleaks (git history + filesystem, 1908 commits, ~13.8MB)
38 findings, all confirmed false positives — placeholder/example secrets in prior `Security reports/*.md` audit docs, `SECURITY.md`, `scripts/benchmark/cloud-init.yaml`, `src/cmds/cloud/aws_cmd.rs`, and `tests/guard_integration_test.rs` (generic-api-key and Stripe-shaped placeholder tokens, all `1234567890abcdef`/`FAKE`-style test fixtures). No real credentials.

### TruffleHog (git, live verification)
0 verified secrets, 0 unverified secrets. Clean (21,192 chunks / 15.5MB scanned).

### Trivy (filesystem)
`Cargo.lock`: 0 findings. All other scanned manifests (test-fixture `pom.xml` skeletons under `tests/fixtures/`) also 0.

### OSV-Scanner (source scan, root `Cargo.lock`)
213 packages scanned. No issues found.

### Semgrep — secrets (`p/secrets`, full repo)
445 files, 52 rules. 0 findings.

### Semgrep — OWASP Top Ten (`p/owasp-top-ten`, py/js/ts/jsx/tsx/java/go/rb)
12 matching files, 266 rules. 0 findings. (rtk is Rust-only; this config's language filters don't cover `.rs`, so it mainly swept `scripts/`, test fixtures, and CI YAML — Rust memory/injection safety is instead covered by `cargo clippy` per repo's own gate, not part of this scan's scope.)

### mcp-exfil-scan (MCP servers, skills, plugins)
0 MCP configs in-repo, 24 skill files scanned (project `.claude/skills/*` + `.claude/agents/*`). No tool-description poisoning, outbound-flow risk, exfil chains, encoded payloads, or env-leak patterns. Risk score 0/100 — CLEAN.

### config-audit (Claude Code configuration)
126 total hits, but scope matters: the bulk (119) are in **globally-installed marketplace plugins/skills under `~/.claude/plugins/`** — out of scope for an rtk-codebase audit (can't be fixed here; not part of this repo). Restricted to files actually inside this worktree: 7 hits, all false positives —
- `CLAUDE.md` / `claude.md` (2 copies, same content) flagged "curl to external URL" (HIGH) and "instruction to skip verification" / "trust-all instruction" (MEDIUM) — these are the repo's own documented `rtk proxy curl ...` example and the "Avoiding Rabbit Holes" scope-discipline section (intentional, not a bypass).

### skill-audit (project `.claude/skills/*/SKILL.md`, 12 files)
3 files matched dangerous-pattern heuristics; all verified false positives on manual read (destructive command strings appear only as comments/test fixtures illustrating what the skill *detects*, never as executable code):
- `security-guardian/SKILL.md` — a destructive-delete injection example shown with its safe fix immediately after, plus the same literal in a Rust test's `malicious_inputs` fixture array; a naive command blacklist shown as a *criticized, unsafe* example.
- `ship/SKILL.md` — flagged on release/rollback terminology (`git tag -d`, `gh release delete`), which is legitimate rollback documentation.
- `performance/SKILL.md` — a `dtrace` profiling command (macOS, requires elevated privileges) documented in a "Detection" code block, not something the skill executes.

### Bundled-script integrity check
```
config-audit.py: OK
skill-audit.sh: OK
mcp-exfil-scan.sh: OK
apts-audit.sh: OK
```

### APTS Audit Log
- **Log:** `/tmp/css-scan-20260930T055405Z.jsonl`
- **Tool runs recorded:** 7 (measured: 7, asserted: 0)
- **Standard:** OWASP APTS § Auditability

## Coverage Gaps
- Bandit skipped — no `.py` files in the target tree.
- CodeQL skipped — not run this session.
- mcp-scan / skillspector LLM mode — opt-in, not run (no user present to consent, autonomous run).
- Business logic / IDOR / runtime behavior not covered by static scan.
